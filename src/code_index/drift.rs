//! Bounded live reads, checksum freshness and a persistent acknowledgement ledger.
use super::history::File;
use super::*;
use duckdb::Connection;
use rmcp::schemars;
use std::io::Read;
use std::path::PathBuf;

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SourceQuery {
    pub index_revision: Option<i64>,
    pub project: Option<String>,
    pub document_id: Option<String>,
    pub path: Option<String>,
    pub limit: Option<usize>,
    /// filesystem (default) or stored. Filesystem reads require RAG_INGEST_ROOTS.
    pub source: Option<String>,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub max_chars: Option<usize>,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct DriftQuery {
    pub project: Option<String>,
    pub index_revision: Option<i64>,
    pub before_seq: Option<i64>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_acknowledged: bool,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DriftAck {
    pub event_ids: Vec<String>,
    pub note: String,
}
pub(super) fn observe(
    conn: &Connection,
    revision: i64,
    file: &File,
    kind: &str,
    observed: Option<&str>,
    source: &str,
) -> Result<String> {
    let id = content_hash(&serde_json::to_string(&(
        revision, &file.id, kind, observed, source,
    ))?);
    let seq: Option<i64> = conn
        .query_row(
            "SELECT seq FROM code_index_drift WHERE event_id=?",
            [&id],
            |r| r.get(0),
        )
        .optional()?;
    if seq.is_some() {
        conn.execute(
            "UPDATE code_index_drift SET last_seen=current_timestamp WHERE event_id=?",
            [&id],
        )?;
        return Ok(id);
    }
    let seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq),0)+1 FROM code_index_drift",
        [],
        |r| r.get(0),
    )?;
    conn.execute("INSERT INTO code_index_drift(event_id,seq,index_revision,document_id,path,project,kind,source_kind,indexed_hash,observed_hash) VALUES (?,?,?,?,?,?,?,?,?,?)",params![id,seq,revision,file.id,file.path,file.project,kind,source,if kind=="added_to_index"{None}else{Some(file.hash.as_str())},observed])?;
    Ok(id)
}
fn source_mode(q: &SourceQuery) -> Result<&str> {
    let mode = q.source.as_deref().unwrap_or("filesystem");
    if !matches!(mode, "filesystem" | "stored") {
        return Err(AppError::config("source must be filesystem or stored"));
    }
    Ok(mode)
}
fn revision_files(store: &Store, q: &SourceQuery) -> Result<(i64, Vec<File>)> {
    let conn = store.lock()?;
    let revision = match q.index_revision {
        Some(id) => id,
        None => conn
            .query_row("SELECT MAX(id) FROM code_index_revisions", [], |r| {
                r.get::<_, Option<i64>>(0)
            })?
            .ok_or_else(|| AppError::not_found("run index_code or sync_sources first"))?,
    };
    let files = history::files(&conn, revision, q.project.as_deref())?
        .into_iter()
        .filter(|f| {
            q.document_id.as_ref().is_none_or(|id| id == &f.id)
                && q.path.as_ref().is_none_or(|p| p == &f.path)
        })
        .collect();
    Ok((revision, files))
}
#[derive(Debug)]
pub(crate) struct LiveSource {
    pub(crate) text: String,
    pub(crate) bytes: usize,
    encoding: &'static str,
    basis: &'static str,
    bytes_hash: String,
}
impl LiveSource {
    fn evidence(&self) -> serde_json::Value {
        serde_json::json!({"encoding":self.encoding,"encoding_basis":self.basis,"bytes":self.bytes,"bytes_hash":self.bytes_hash})
    }
}
fn actual(store: &Store, file: &File, mode: &str, roots: &[PathBuf]) -> Result<Option<LiveSource>> {
    if mode == "stored" {
        let doc = store
            .get_document(&file.id)?
            .filter(|d| !matches!(d.status.as_str(), "archived" | "tombstone" | "consolidated"));
        return doc
            .map(|d| {
                source::read(&*store.lock()?, &d.content, &d.metadata_json).map(|s| LiveSource {
                    bytes: s.len(),
                    bytes_hash: content_hash(&s),
                    text: s.into_owned(),
                    encoding: "stored_utf8",
                    basis: "stored_representation",
                })
            })
            .transpose();
    }
    read_live_source(&file.path, roots, 4 * 1024 * 1024)
}
/// Shared bounded/allowlisted source-byte reader and versioned decoder.
pub(crate) fn read_live_source(
    source_path: &str,
    roots: &[PathBuf],
    max_bytes: usize,
) -> Result<Option<LiveSource>> {
    let path = if source_path.starts_with("file:") {
        reqwest::Url::parse(source_path)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .ok_or_else(|| AppError::config("source URI is not a local file"))?
    } else {
        PathBuf::from(source_path)
    };
    let path = match path.canonicalize() {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Resolve the nearest existing parent, including macOS /var -> /private/var,
            // before deciding that a missing path belongs to an allowed root.
            if path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(AppError::config(
                    "cannot verify missing source containing parent traversal",
                ));
            }
            for ancestor in path.ancestors().skip(1) {
                if let Ok(parent) = ancestor.canonicalize() {
                    let suffix = path
                        .strip_prefix(ancestor)
                        .map_err(|_| AppError::config("invalid source ancestor"))?;
                    crate::util::check_path_allowlist(&parent.join(suffix), roots)?;
                    return Ok(None);
                }
            }
            return Err(AppError::config("cannot verify missing source ancestry"));
        }
        Err(e) => return Err(e.into()),
    };
    crate::util::check_path_allowlist(&path, roots)?;
    if !path.metadata()?.is_file() {
        return Err(AppError::config("live source is not a regular file"));
    }
    let file = std::fs::File::open(&path)?;
    let before = file.metadata()?;
    if before.len() > max_bytes.min(4 * 1024 * 1024) as u64 {
        return Err(AppError::config(
            "live source exceeds byte budget (maximum 4 MiB); no partial checksum is reported",
        ));
    }
    let mut bytes = Vec::new();
    (&file)
        .take(max_bytes.min(4 * 1024 * 1024) as u64 + 1)
        .read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(AppError::Busy(
            "source changed during live read; retry".into(),
        ));
    }
    if bytes.len() > max_bytes.min(4 * 1024 * 1024) {
        return Err(AppError::config(
            "live source exceeds byte budget (maximum 4 MiB); no partial checksum is reported",
        ));
    }
    let (text, encoding, basis) = crate::file_ingest::decode_text(&bytes, &path)
        .map_err(|e| AppError::config(e.to_string()))?;
    Ok(Some(LiveSource {
        text,
        bytes: bytes.len(),
        encoding,
        basis,
        bytes_hash: blake3::hash(&bytes).to_hex().to_string(),
    }))
}
fn comparison(indexed: &str, live: Option<&str>) -> (&'static str, Option<String>) {
    match live {
        None => ("missing", None),
        Some(s) => {
            let hash = content_hash(s);
            (
                if hash == indexed {
                    "unchanged"
                } else {
                    "modified"
                },
                Some(hash),
            )
        }
    }
}
fn line_diff(old: &str, new: &str) -> serde_json::Value {
    let a: Vec<_> = old.lines().collect();
    let b: Vec<_> = new.lines().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    serde_json::json!({"common_prefix_lines":prefix,"indexed_changed_range":{"start":prefix+1,"end_exclusive":a.len()-tail+1},"live_changed_range":{"start":prefix+1,"end_exclusive":b.len()-tail+1},"indexed_lines":a.len(),"live_lines":b.len(),"algorithm":"common_prefix_suffix_ranges"})
}
impl Store {
    pub fn code_catalog(&self, q: &SourceQuery) -> Result<serde_json::Value> {
        let (revision, files) = revision_files(self, q)?;
        let limit = q.limit.unwrap_or(100).clamp(1, 1000);
        let items:Vec<_>=files.iter().take(limit).map(|f|serde_json::json!({"document_id":f.id,"path":f.path,"project":f.project,"document_revision":f.revision,"content_hash":f.hash,"bytes":f.source.len(),"syntax_indexed":f.analysis.is_some()})).collect();
        Ok(
            serde_json::json!({"index_rev":revision,"items":items,"total":files.len(),"truncated":files.len()>limit,"view":"immutable_revision_manifest"}),
        )
    }
    pub fn code_freshness(&self, q: &SourceQuery, roots: &[PathBuf]) -> Result<serde_json::Value> {
        let mode = source_mode(q)?;
        let (revision, files) = revision_files(self, q)?;
        let limit = q.limit.unwrap_or(500).clamp(1, 2000);
        let mut items = Vec::new();
        let mut changed = 0;
        let mut unavailable = 0;
        for file in files.iter().take(limit) {
            let entry = match actual(self, file, mode, roots) {
                Ok(content) => {
                    let (status, hash) =
                        comparison(&file.hash, content.as_ref().map(|s| s.text.as_str()));
                    let event = if status != "unchanged" {
                        changed += 1;
                        Some(observe(
                            &*self.lock()?,
                            revision,
                            file,
                            status,
                            hash.as_deref(),
                            mode,
                        )?)
                    } else {
                        None
                    };
                    serde_json::json!({"document_id":file.id,"path":file.path,"status":status,"indexed_hash":file.hash,"observed_hash":hash,"drift_event_id":event,"comparison_scope":"decoded_source_utf8","observed_source":content.as_ref().map(LiveSource::evidence)})
                }
                Err(error) => {
                    unavailable += 1;
                    serde_json::json!({"document_id":file.id,"path":file.path,"status":"unavailable","error":error.to_string()})
                }
            };
            items.push(entry);
        }
        Ok(
            serde_json::json!({"index_rev":revision,"source":mode,"items":items,"checked":items.len(),"total":files.len(),"truncated":files.len()>limit,"changed":changed,"unavailable":unavailable,"all_checked_unchanged":!items.is_empty()&&changed==0&&unavailable==0&&files.len()<=limit}),
        )
    }
    pub fn code_live_read(&self, q: &SourceQuery, roots: &[PathBuf]) -> Result<serde_json::Value> {
        if q.path.is_none() && q.document_id.is_none() {
            return Err(AppError::config("live_read requires path or document_id"));
        }
        let mode = source_mode(q)?;
        let (revision, files) = revision_files(self, q)?;
        if files.len() != 1 {
            return Err(AppError::config(
                "live_read requires one unambiguous indexed source",
            ));
        }
        let file = &files[0];
        let content = actual(self, file, mode, roots)?;
        let (status, hash) = comparison(&file.hash, content.as_ref().map(|s| s.text.as_str()));
        let event = if status != "unchanged" {
            Some(observe(
                &*self.lock()?,
                revision,
                file,
                status,
                hash.as_deref(),
                mode,
            )?)
        } else {
            None
        };
        let start = q.start_line.unwrap_or(1);
        let end = q.end_line.unwrap_or(start.saturating_add(199));
        if start == 0 || end < start {
            return Err(AppError::config("invalid live_read line range"));
        }
        let max = q.max_chars.unwrap_or(16000).clamp(1, 64000);
        let lines: Vec<_> = content
            .as_ref()
            .map(|s| s.text.as_str())
            .unwrap_or("")
            .lines()
            .collect();
        let selected = lines
            .iter()
            .skip(start - 1)
            .take(end - start + 1)
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        let truncated = selected.chars().count() > max || end < lines.len();
        let body: String = selected.chars().take(max).collect();
        Ok(
            serde_json::json!({"index_rev":revision,"document_id":file.id,"path":file.path,"project":file.project,"source":mode,"status":status,"indexed_hash":file.hash,"observed_hash":hash,"drift_event_id":event,"start_line":start,"requested_end_line":end,"total_lines":lines.len(),"content":body,"truncated":truncated,"diff":content.as_ref().map(|s|line_diff(&file.source,&s.text)),"historical_index_immutable":true,"comparison_scope":"decoded_source_utf8","observed_source":content.as_ref().map(LiveSource::evidence)}),
        )
    }
    pub fn code_drift_report(&self, q: &DriftQuery) -> Result<serde_json::Value> {
        let conn = self.lock()?;
        let limit = q.limit.unwrap_or(100).clamp(1, 500);
        let mut stmt=conn.prepare("SELECT event_id,seq,index_revision,document_id,path,project,kind,source_kind,indexed_hash,observed_hash,CAST(first_seen AS VARCHAR),CAST(last_seen AS VARCHAR),CAST(acknowledged_at AS VARCHAR),acknowledgement FROM code_index_drift WHERE (? IS NULL OR project=?) AND (? IS NULL OR index_revision=?) AND (? IS NULL OR seq<?) AND (? OR acknowledged_at IS NULL) ORDER BY seq DESC LIMIT ?")?;
        let rows=stmt.query_map(params![q.project,q.project,q.index_revision,q.index_revision,q.before_seq,q.before_seq,q.include_acknowledged,(limit+1)as i64],|r|Ok(serde_json::json!({"event_id":r.get::<_,String>(0)?,"seq":r.get::<_,i64>(1)?,"index_rev":r.get::<_,i64>(2)?,"document_id":r.get::<_,String>(3)?,"path":r.get::<_,String>(4)?,"project":r.get::<_,Option<String>>(5)?,"kind":r.get::<_,String>(6)?,"source":r.get::<_,String>(7)?,"indexed_hash":r.get::<_,Option<String>>(8)?,"observed_hash":r.get::<_,Option<String>>(9)?,"first_seen":r.get::<_,String>(10)?,"last_seen":r.get::<_,String>(11)?,"acknowledged_at":r.get::<_,Option<String>>(12)?,"acknowledgement":r.get::<_,Option<String>>(13)?})))?;
        let mut items = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        let more = items.len() > limit;
        items.truncate(limit);
        Ok(
            serde_json::json!({"items":items,"has_more":more,"next_before_seq":if more{items.last().and_then(|v|v["seq"].as_i64())}else{None},"observation_log_only":true}),
        )
    }
    pub fn code_drift_ack(&self, q: &DriftAck) -> Result<serde_json::Value> {
        if q.event_ids.is_empty()
            || q.event_ids.len() > 500
            || q.note.trim().is_empty()
            || q.note.len() > 4096
        {
            return Err(AppError::config(
                "drift_ack requires 1..500 event IDs and a nonblank note up to 4096 bytes",
            ));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let mut changed = 0;
        let mut seen = HashSet::new();
        for id in &q.event_ids {
            if !seen.insert(id) {
                continue;
            }
            let exists: i64 = tx.query_row(
                "SELECT COUNT(*) FROM code_index_drift WHERE event_id=?",
                [id],
                |r| r.get(0),
            )?;
            if exists != 1 {
                return Err(AppError::not_found(format!("unknown drift event {id}")));
            }
            changed+=tx.execute("UPDATE code_index_drift SET acknowledged_at=current_timestamp, acknowledgement=? WHERE event_id=? AND acknowledged_at IS NULL",params![q.note.trim(),id])?;
        }
        tx.commit()?;
        Ok(
            serde_json::json!({"acknowledged":changed,"requested_unique":seen.len(),"source_or_index_changed":false}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    fn seed(store: &Store, path: &str, content: &str) {
        store
            .upsert_document(&Document {
                id: "a".into(),
                uri: format!("file://{path}"),
                source_file: Some(path.into()),
                wing: Some("p".into()),
                content: content.into(),
                ..Default::default()
            })
            .unwrap();
        store.index_code().unwrap();
    }
    #[test]
    fn encoded_live_sources_compare_decoded_text_and_expose_raw_evidence() {
        let text = "// Привет\nfn work() {}\n";
        let cp = encoding_rs::WINDOWS_1251.encode(text).0.into_owned();
        let le = [
            vec![0xff, 0xfe],
            text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        ]
        .concat();
        let be = [
            vec![0xfe, 0xff],
            text.encode_utf16().flat_map(u16::to_be_bytes).collect(),
        ]
        .concat();
        let bom = [vec![0xef, 0xbb, 0xbf], text.as_bytes().to_vec()].concat();
        for (bytes, encoding) in [
            (cp, "windows-1251"),
            (le, "utf-16le"),
            (be, "utf-16be"),
            (bom, "utf-8-sig"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let db = tempfile::tempdir().unwrap();
            let path = root.path().join("a.rs");
            std::fs::write(&path, &bytes).unwrap();
            let store = Store::open(&db.path().join("live.duckdb")).unwrap();
            seed(&store, &path.display().to_string(), text);
            let roots = vec![root.path().to_path_buf()];
            let q = SourceQuery {
                document_id: Some("a".into()),
                ..Default::default()
            };
            let fresh = store.code_freshness(&q, &roots).unwrap();
            assert_eq!(fresh["all_checked_unchanged"], true, "{encoding}");
            assert_eq!(fresh["items"][0]["observed_source"]["encoding"], encoding);
            let live = store.code_live_read(&q, &roots).unwrap();
            assert!(live["content"].as_str().unwrap().contains("Привет"));
            assert_eq!(live["observed_source"]["bytes"], bytes.len());
            assert!(
                read_live_source(&path.display().to_string(), &roots, bytes.len() - 1).is_err()
            );
            std::fs::write(&path, text).unwrap();
            let recoded = store.code_live_read(&q, &roots).unwrap();
            assert_eq!(recoded["status"], "unchanged");
            assert_eq!(recoded["observed_hash"], live["observed_hash"]);
            assert_ne!(
                recoded["observed_source"]["bytes_hash"],
                live["observed_source"]["bytes_hash"]
            );
            std::fs::write(&path, text.replace("work", "changed")).unwrap();
            assert_eq!(store.code_freshness(&q, &roots).unwrap()["changed"], 1);
        }
    }

    #[test]
    fn live_files_are_bounded_and_drift_ack_is_atomic_persistent_and_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let path = root.path().join("a.rs");
        std::fs::write(&path, "first\nold\nlast\n").unwrap();
        let dbpath = db.path().join("drift.duckdb");
        let store = Store::open(&dbpath).unwrap();
        seed(&store, &path.display().to_string(), "first\nold\nlast\n");
        let q = SourceQuery {
            document_id: Some("a".into()),
            ..Default::default()
        };
        let roots = vec![root.path().to_path_buf()];
        assert_eq!(
            store.code_freshness(&q, &roots).unwrap()["all_checked_unchanged"],
            true
        );
        std::fs::write(&path, "first\nnew 🦀\nlast\n").unwrap();
        let freshness = store.code_freshness(&q, &roots).unwrap();
        assert_eq!(freshness["changed"], 1);
        let id = freshness["items"][0]["drift_event_id"]
            .as_str()
            .unwrap()
            .to_string();
        let read = store
            .code_live_read(
                &SourceQuery {
                    document_id: Some("a".into()),
                    start_line: Some(2),
                    end_line: Some(2),
                    max_chars: Some(3),
                    ..Default::default()
                },
                &roots,
            )
            .unwrap();
        assert_eq!(read["content"], "new");
        assert_eq!(read["truncated"], true);
        assert_eq!(read["drift_event_id"], id);
        assert_eq!(
            store.get_document("a").unwrap().unwrap().content,
            "first\nold\nlast\n"
        );
        assert!(store
            .code_drift_ack(&DriftAck {
                event_ids: vec![id.clone(), "unknown".into()],
                note: "reviewed".into()
            })
            .is_err());
        assert_eq!(
            store.code_drift_report(&DriftQuery::default()).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .code_drift_ack(&DriftAck {
                    event_ids: vec![id.clone()],
                    note: "reviewed".into()
                })
                .unwrap()["acknowledged"],
            1
        );
        assert_eq!(
            store
                .code_drift_ack(&DriftAck {
                    event_ids: vec![id.clone()],
                    note: "must not overwrite".into()
                })
                .unwrap()["acknowledged"],
            0
        );
        store.code_freshness(&q, &roots).unwrap();
        assert!(
            store.code_drift_report(&DriftQuery::default()).unwrap()["items"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        drop(store);
        let store = Store::open(&dbpath).unwrap();
        let report = store
            .code_drift_report(&DriftQuery {
                include_acknowledged: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(report["items"][0]["acknowledgement"], "reviewed");
        assert_eq!(
            store
                .code_index_history(&history::HistoryQuery::default())
                .unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn missing_disallowed_and_malformed_sources_never_report_fresh() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let path = root.path().join("a.rs");
        std::fs::write(&path, "old").unwrap();
        let store = Store::open(&db.path().join("drift.duckdb")).unwrap();
        seed(&store, &path.display().to_string(), "old");
        let q = SourceQuery {
            document_id: Some("a".into()),
            ..Default::default()
        };
        assert_eq!(store.code_freshness(&q, &[]).unwrap()["unavailable"], 1);
        assert!(store.code_live_read(&q, &[]).is_err());
        let roots = vec![root.path().to_path_buf()];
        std::fs::write(&path, [0xff, 0xfe, 0]).unwrap();
        assert_eq!(store.code_freshness(&q, &roots).unwrap()["unavailable"], 1);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            store.code_freshness(&q, &roots).unwrap()["items"][0]["status"],
            "missing"
        );
        assert_eq!(
            store.code_live_read(&q, &roots).unwrap()["status"],
            "missing"
        );
        assert_eq!(
            store
                .code_freshness(
                    &SourceQuery {
                        source: Some("stored".into()),
                        ..q
                    },
                    &[]
                )
                .unwrap()["all_checked_unchanged"],
            true
        );
    }
    #[test]
    fn publication_drift_has_stable_cursors_and_ack_requires_write_role() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("drift.duckdb")).unwrap();
        seed(&store, "/p/a.rs", "fn a() {}");
        let mut doc = store.get_document("a").unwrap().unwrap();
        doc.content = "fn changed() {}".into();
        store.upsert_document(&doc).unwrap();
        store.index_code().unwrap();
        store.delete_document("a").unwrap();
        store.index_code().unwrap();
        let first = store
            .code_drift_report(&DriftQuery {
                limit: Some(1),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(first["has_more"], true);
        assert_eq!(first["items"][0]["kind"], "removed_from_index");
        let second = store
            .code_drift_report(&DriftQuery {
                limit: Some(1),
                before_seq: first["next_before_seq"].as_i64(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(second["items"][0]["kind"], "modified");
        assert!(!crate::access::AccessRole::Read.allows_tool("drift_ack"));
        assert!(crate::access::AccessRole::Write.allows_tool("drift_ack"));
    }
}

#[cfg(all(test, unix))]
mod path_safety_tests {
    use super::*;
    #[test]
    fn indexed_symlink_cannot_bypass_root_and_oversize_reads_cannot_be_fresh() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let target = outside.path().join("secret.rs");
        std::fs::write(&target, "outside bytes").unwrap();
        let link = root.path().join("a.rs");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let store = Store::open(&db.path().join("source.duckdb")).unwrap();
        store
            .upsert_document(&crate::models::Document {
                id: "a".into(),
                uri: "memory://a".into(),
                source_file: Some(link.display().to_string()),
                content: "indexed bytes".into(),
                ..Default::default()
            })
            .unwrap();
        store.index_code().unwrap();
        let q = SourceQuery {
            document_id: Some("a".into()),
            ..Default::default()
        };
        let roots = vec![root.path().to_path_buf()];
        assert!(store.code_live_read(&q, &roots).is_err());
        assert_eq!(store.code_freshness(&q, &roots).unwrap()["unavailable"], 1);
        std::fs::remove_file(&link).unwrap();
        let file = std::fs::File::create(&link).unwrap();
        file.set_len(4 * 1024 * 1024 + 1).unwrap();
        assert!(store
            .code_live_read(&q, &roots)
            .unwrap_err()
            .to_string()
            .contains("4 MiB"));
        assert_eq!(
            store.code_freshness(&q, &roots).unwrap()["all_checked_unchanged"],
            false
        );
    }
}
