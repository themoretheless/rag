//! Read-only Git object/index capture. Never checkout, run filters or invoke diff.
use super::*;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct Entry {
    pub mode: String,
    pub oid: String,
}
#[derive(Clone)]
pub(super) struct Source {
    pub text: String,
    pub evidence: Value,
}
pub(super) struct Capture {
    pub identity: Value,
    pub entries: BTreeMap<String, Entry>,
    pub sources: BTreeMap<String, Source>,
    pub unsupported: Vec<String>,
    pub unavailable: Vec<Value>,
    pub missing: Vec<String>,
}
pub(super) struct Git {
    pub root: PathBuf,
    pub bytes: usize,
    max_bytes: usize,
    max_files: usize,
    files: usize,
    pub commands: usize,
}
impl Git {
    pub fn open(q: &GitImpactQuery, roots: &[PathBuf]) -> Result<Self> {
        let root = PathBuf::from(&q.repository).canonicalize()?;
        crate::util::check_path_allowlist(&root, roots)?;
        let mut git = Self {
            root,
            bytes: 0,
            max_bytes: q
                .max_bytes
                .unwrap_or(8 * 1024 * 1024)
                .clamp(1024, 128 * 1024 * 1024),
            max_files: q.max_files.unwrap_or(2000).clamp(1, 10000),
            files: 0,
            commands: 0,
        };
        let top = git.run(&["rev-parse", "--show-toplevel"], None, 4096)?;
        let top = PathBuf::from(
            std::str::from_utf8(&top)
                .map_err(|_| AppError::config("non-UTF8 Git root"))?
                .trim_end_matches(['\n', '\r']),
        )
        .canonicalize()?;
        if top != git.root {
            return Err(AppError::config(
                "repository must be the exact Git working-tree root",
            ));
        }
        Ok(git)
    }
    pub fn run(&mut self, args: &[&str], input: Option<Vec<u8>>, cap: usize) -> Result<Vec<u8>> {
        self.commands += 1;
        let mut command = Command::new("git");
        // Git variables inherited from another repository must not redirect reads.
        for (key, _) in std::env::vars_os().filter(|(k, _)| k.to_string_lossy().starts_with("GIT_"))
        {
            command.env_remove(key);
        }
        command
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if std::env::consts::OS == "windows" {
                    "NUL"
                } else {
                    "/dev/null"
                },
            );
        command
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "-c",
                "core.pager=cat",
                "-C",
            ])
            .arg(&self.root)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let mut stdin = child.stdin.take().unwrap();
        let writer = std::thread::spawn(move || {
            if let Some(input) = input {
                let _ = stdin.write_all(&input);
            }
        });
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        // Drain excess output without retaining it; the process deadline bounds work.
        let reader = std::thread::spawn(move || -> std::io::Result<(Vec<u8>, bool)> {
            let mut out = Vec::new();
            let mut exceeded = false;
            let mut stream = stdout;
            let mut buf = [0u8; 16384];
            loop {
                let n = stream.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                let keep = n.min(cap.saturating_sub(out.len()));
                out.extend_from_slice(&buf[..keep]);
                exceeded |= keep < n;
            }
            Ok((out, exceeded))
        });
        let errors = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.take(4096).read_to_end(&mut bytes);
            bytes
        });
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if start.elapsed() > Duration::from_secs(30) {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let _ = writer.join();
        let (out, exceeded) = reader
            .join()
            .map_err(|_| AppError::config("Git output reader failed"))??;
        let error = errors.join().unwrap_or_default();
        let Some(status) = status else {
            return Err(AppError::Busy(
                "Git capture exceeded 30-second command deadline".into(),
            ));
        };
        if exceeded {
            return Err(AppError::config(
                "Git output exceeds capture budget; narrow repository or increase source limits",
            ));
        }
        if !status.success() {
            return Err(AppError::config(format!(
                "Git capture failed: {}",
                String::from_utf8_lossy(&error)
            )));
        }
        Ok(out)
    }
    pub fn revision(&mut self, revision: &str) -> Result<String> {
        if revision.is_empty()
            || revision.len() > 512
            || revision.starts_with('-')
            || revision.chars().any(char::is_control)
        {
            return Err(AppError::config("invalid Git revision"));
        }
        let target = format!("{revision}^{{commit}}");
        let output = self.run(
            &["rev-parse", "--verify", "--end-of-options", &target],
            None,
            256,
        )?;
        let oid = std::str::from_utf8(&output)
            .map_err(|_| AppError::config("invalid Git object ID"))?
            .trim()
            .to_string();
        if !valid_oid(&oid) {
            return Err(AppError::config("invalid resolved Git object ID"));
        }
        Ok(oid)
    }
    pub fn index(&mut self) -> Result<(Vec<u8>, BTreeMap<String, Entry>)> {
        let raw = self.run(&["ls-files", "--stage", "-z"], None, 8 * 1024 * 1024)?;
        let entries = parse_entries(&raw, true)?;
        Ok((raw, entries))
    }
    pub fn untracked(&mut self) -> Result<Vec<u8>> {
        self.run(
            &["ls-files", "--others", "--exclude-standard", "-z"],
            None,
            4 * 1024 * 1024,
        )
    }
    pub fn tree(&mut self, oid: &str) -> Result<Capture> {
        let raw = self.run(
            &["ls-tree", "-r", "-z", "--full-tree", oid],
            None,
            8 * 1024 * 1024,
        )?;
        let entries = parse_entries(&raw, false)?;
        self.objects(entries, serde_json::json!({"kind":"commit","commit":oid}))
    }
    pub fn objects(
        &mut self,
        entries: BTreeMap<String, Entry>,
        identity: Value,
    ) -> Result<Capture> {
        let mut result = Capture {
            identity,
            entries,
            sources: BTreeMap::new(),
            unsupported: vec![],
            unavailable: vec![],
            missing: vec![],
        };
        let mut selected = Vec::new();
        for (path, entry) in &result.entries {
            if !indexable(path) {
                result.unsupported.push(path.clone());
                continue;
            }
            if !matches!(entry.mode.as_str(), "100644" | "100755") {
                result.unavailable.push(serde_json::json!({"path":path,"reason":"non_regular_git_input","mode":entry.mode}));
                continue;
            }
            self.files += 1;
            if self.files > self.max_files {
                return Err(AppError::config(
                    "Git source-file budget exceeded; no partial graph published",
                ));
            }
            selected.push((path.clone(), entry.clone()));
        }
        if selected.is_empty() {
            return Ok(result);
        }
        let input = selected
            .iter()
            .map(|(_, e)| format!("{}\n", e.oid))
            .collect::<String>()
            .into_bytes();
        let remaining = self.max_bytes.saturating_sub(self.bytes);
        let raw = self.run(
            &["cat-file", "--batch"],
            Some(input),
            remaining.saturating_add(selected.len() * 256),
        )?;
        let mut cursor = 0;
        for (path, entry) in selected {
            let end = raw[cursor..]
                .iter()
                .position(|b| *b == b'\n')
                .map(|n| cursor + n)
                .ok_or_else(|| AppError::config("invalid Git batch header"))?;
            let header = std::str::from_utf8(&raw[cursor..end])
                .map_err(|_| AppError::config("invalid Git batch header"))?;
            let fields: Vec<_> = header.split(' ').collect();
            if fields.len() != 3 || fields[0] != entry.oid || fields[1] != "blob" {
                return Err(AppError::config("Git batch object mismatch"));
            }
            let size: usize = fields[2]
                .parse()
                .map_err(|_| AppError::config("invalid Git blob length"))?;
            cursor = end + 1;
            let end = cursor
                .checked_add(size)
                .filter(|n| *n < raw.len())
                .ok_or_else(|| AppError::config("truncated Git blob"))?;
            if raw[end] != b'\n' {
                return Err(AppError::config("invalid Git blob terminator"));
            }
            self.bytes = self
                .bytes
                .checked_add(size)
                .ok_or_else(|| AppError::config("Git byte budget overflow"))?;
            if self.bytes > self.max_bytes || size > 4 * 1024 * 1024 {
                return Err(AppError::config(
                    "Git source-byte budget exceeded; no partial graph published",
                ));
            }
            let bytes = &raw[cursor..end];
            let (text, encoding, basis) =
                crate::file_ingest::decode_text(bytes, &self.root.join(&path))
                    .map_err(|e| AppError::config(e.to_string()))?;
            result.sources.insert(path,Source{text,evidence:serde_json::json!({"blob_oid":entry.oid,"mode":entry.mode,"bytes":size,"bytes_hash":blake3::hash(bytes).to_hex().to_string(),"encoding":encoding,"encoding_basis":basis})});
            cursor = end + 1;
        }
        if cursor != raw.len() {
            return Err(AppError::config("unexpected trailing Git batch output"));
        }
        Ok(result)
    }
    pub fn working(
        &mut self,
        mut entries: BTreeMap<String, Entry>,
        untracked: &[u8],
        roots: &[PathBuf],
    ) -> Result<Capture> {
        let tagged = self.run(&["ls-files", "-t", "-z"], None, 8 * 1024 * 1024)?;
        let skipped: BTreeSet<String> = tagged
            .split(|b| *b == 0)
            .filter(|r| r.starts_with(b"S "))
            .map(|r| path(&r[2..]))
            .collect::<Result<_>>()?;
        for path in paths(untracked)? {
            entries.entry(path).or_insert(Entry {
                mode: "untracked".into(),
                oid: String::new(),
            });
        }
        let mut result = Capture {
            identity: serde_json::json!({"kind":"working_tree","consistency":"source_hashes_revalidated_before_return"}),
            entries,
            sources: BTreeMap::new(),
            unsupported: vec![],
            unavailable: vec![],
            missing: vec![],
        };
        let mut deleted = Vec::new();
        let mut virtual_entries = BTreeMap::new();
        for (path, entry) in &result.entries {
            if !indexable(path) {
                result.unsupported.push(path.clone());
                continue;
            }
            self.files += 1;
            if self.files > self.max_files {
                return Err(AppError::config(
                    "Git source-file budget exceeded; no partial graph published",
                ));
            }
            let absolute = self.root.join(path);
            match std::fs::symlink_metadata(&absolute) {
                Ok(m) if !m.is_file() => {
                    result.unavailable.push(
                        serde_json::json!({"path":path,"reason":"non_regular_working_input"}),
                    );
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if skipped.contains(path) {
                        virtual_entries.insert(path.clone(), entry.clone());
                        continue;
                    }
                    deleted.push(path.clone());
                    continue;
                }
                Err(e) => return Err(e.into()),
                _ => (),
            }
            let resolved = absolute.canonicalize()?;
            if !resolved.starts_with(&self.root) {
                return Err(AppError::config("working source escapes repository root"));
            }
            let remaining = self.max_bytes.saturating_sub(self.bytes);
            let source = drift::read_live_source(
                absolute
                    .to_str()
                    .ok_or_else(|| AppError::config("non-UTF8 source path"))?,
                roots,
                remaining,
            )?
            .ok_or_else(|| AppError::Busy("working source disappeared during capture".into()))?;
            self.bytes += source.bytes;
            if self.bytes > self.max_bytes {
                return Err(AppError::config(
                    "Git source-byte budget exceeded; no partial graph published",
                ));
            }
            result.sources.insert(
                path.clone(),
                Source {
                    evidence: serde_json::json!({"mode":entry.mode,"live":source.evidence()}),
                    text: source.text,
                },
            );
        }
        for path in deleted {
            result.entries.remove(&path);
            result.missing.push(path);
        }
        if !virtual_entries.is_empty() {
            let virtual_capture = self.objects(
                virtual_entries,
                json!({"kind":"absent_skip_worktree_projection"}),
            )?;
            for (path, mut source) in virtual_capture.sources {
                source.evidence["working_projection"] = "absent_skip_worktree_index_blob".into();
                result.missing.push(path.clone());
                result.sources.insert(path, source);
            }
            result.unavailable.extend(virtual_capture.unavailable);
        }
        result.identity["index_flags_hash"] = blake3::hash(&tagged).to_hex().to_string().into();
        Ok(result)
    }
    pub fn validate_working(&mut self, capture: &Capture, roots: &[PathBuf]) -> Result<()> {
        let tagged = self.run(&["ls-files", "-t", "-z"], None, 8 * 1024 * 1024)?;
        if capture.identity["index_flags_hash"] != blake3::hash(&tagged).to_hex().to_string() {
            return Err(AppError::Busy(
                "Git index flags changed during analysis".into(),
            ));
        }
        for path in &capture.missing {
            match std::fs::symlink_metadata(self.root.join(path)) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                _ => {
                    return Err(AppError::Busy(
                        "missing working source reappeared during analysis".into(),
                    ))
                }
            }
        }
        for (path, source) in &capture.sources {
            if source.evidence["working_projection"] == "absent_skip_worktree_index_blob" {
                continue;
            }
            let absolute = self.root.join(path);
            if !std::fs::symlink_metadata(&absolute)?.is_file()
                || !absolute.canonicalize()?.starts_with(&self.root)
            {
                return Err(AppError::Busy(
                    "working input changed identity during analysis".into(),
                ));
            }
            let live = drift::read_live_source(absolute.to_str().unwrap(), roots, 4 * 1024 * 1024)?
                .ok_or_else(|| {
                    AppError::Busy("working input disappeared during analysis".into())
                })?;
            if live.evidence()["bytes_hash"] != source.evidence["live"]["bytes_hash"] {
                return Err(AppError::Busy(
                    "working source changed during analysis; retry".into(),
                ));
            }
        }
        Ok(())
    }
}
fn valid_oid(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn path(bytes: &[u8]) -> Result<String> {
    let path = std::str::from_utf8(bytes)
        .map_err(|_| AppError::config("non-UTF8 Git path is unsupported"))?;
    if path.is_empty()
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(AppError::config("invalid repository-relative Git path"));
    }
    Ok(path.into())
}
fn paths(raw: &[u8]) -> Result<Vec<String>> {
    raw.split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(path)
        .collect()
}
fn parse_entries(raw: &[u8], index: bool) -> Result<BTreeMap<String, Entry>> {
    let mut out = BTreeMap::new();
    for record in raw.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let tab = record
            .iter()
            .position(|b| *b == b'\t')
            .ok_or_else(|| AppError::config("invalid Git entry"))?;
        let header = std::str::from_utf8(&record[..tab])
            .map_err(|_| AppError::config("invalid Git entry metadata"))?;
        let parts: Vec<_> = header.split(' ').collect();
        if parts.len() != 3 {
            return Err(AppError::config("invalid Git entry fields"));
        }
        let oid = if index {
            if parts[2] != "0" {
                return Err(AppError::Conflict(
                    "Git index contains unmerged entries".into(),
                ));
            }
            parts[1]
        } else {
            parts[2]
        };
        if !valid_oid(oid) {
            return Err(AppError::config("invalid Git entry object ID"));
        }
        let path = path(&record[tab + 1..])?;
        if out
            .insert(
                path,
                Entry {
                    mode: parts[0].into(),
                    oid: oid.into(),
                },
            )
            .is_some()
        {
            return Err(AppError::config("duplicate Git path"));
        }
    }
    Ok(out)
}
