//! Bounded live reads, checksum freshness and a persistent acknowledgement ledger.
use super::history::File;
use super::*;
use duckdb::Connection;
use rmcp::schemars;
use std::io::Read;
use std::path::PathBuf;

#[derive(Debug, Default, Clone, Deserialize, schemars::JsonSchema)]
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
    store.with_conn(|conn| revision_files_locked(conn, q))
}
fn revision_files_locked(conn: &duckdb::Connection, q: &SourceQuery) -> Result<(i64, Vec<File>)> {
    let revision = match q.index_revision {
        Some(id) => id,
        None => conn
            .query_row("SELECT MAX(id) FROM code_index_revisions", [], |r| {
                r.get::<_, Option<i64>>(0)
            })?
            .ok_or_else(|| AppError::not_found("run index_code or sync_sources first"))?,
    };
    let files = history::files(conn, revision, q.project.as_deref())?
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
    pub(super) fn evidence(&self) -> serde_json::Value {
        serde_json::json!({"encoding":self.encoding,"encoding_basis":self.basis,"bytes":self.bytes,"bytes_hash":self.bytes_hash})
    }
}
fn actual(store: &Store, file: &File, mode: &str, roots: &[PathBuf]) -> Result<Option<LiveSource>> {
    if mode == "stored" {
        let doc = store
            .documents()
            .get_document(&file.id)?
            .filter(|d| !matches!(d.status.as_str(), "archived" | "tombstone" | "consolidated"));
        return doc
            .map(|d| {
                store
                    .with_conn(|conn| {
                        source::read(conn, &d.content, &d.metadata_json)
                            .map(std::borrow::Cow::into_owned)
                    })
                    .map(|s| LiveSource {
                        bytes: s.len(),
                        bytes_hash: content_hash(&s),
                        text: s,
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
const DIFF_MAX_CELLS: usize = 1_000_000;
const DIFF_MAX_HUNKS: usize = 64;

/// Range-only edits; never exposes source text outside the requested live range.
fn line_diff(old: &str, new: &str) -> serde_json::Value {
    // Retain terminators: CRLF and missing final newline are actual source changes.
    let a: Vec<_> = old.split_inclusive('\n').collect();
    let b: Vec<_> = new.split_inclusive('\n').collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (left, right) = (&a[prefix..a.len() - tail], &b[prefix..b.len() - tail]);
    let cells = (left.len() + 1).checked_mul(right.len() + 1);
    let exact = left.is_empty() || right.is_empty() || cells.is_some_and(|n| n <= DIFF_MAX_CELLS);
    let mut hunks = Vec::new();
    let mut total = 0;
    let mut add = |i: usize, j: usize, x: usize, y: usize| {
        total += 1;
        if hunks.len() < DIFF_MAX_HUNKS {
            hunks.push(serde_json::json!({
                "kind":if i==x {"insert"} else if j==y {"delete"} else {"replace"},
                "indexed_range":{"start":prefix+i+1,"end_exclusive":prefix+x+1},
                "live_range":{"start":prefix+j+1,"end_exclusive":prefix+y+1}
            }));
        }
    };
    if left.is_empty() || right.is_empty() || !exact {
        if !left.is_empty() || !right.is_empty() {
            add(0, 0, left.len(), right.len());
        }
    } else {
        let width = right.len() + 1;
        let mut lcs = vec![0u32; cells.unwrap()];
        for i in (0..left.len()).rev() {
            for j in (0..right.len()).rev() {
                lcs[i * width + j] = if left[i] == right[j] {
                    lcs[(i + 1) * width + j + 1] + 1
                } else {
                    lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < left.len() || j < right.len() {
            if i < left.len() && j < right.len() && left[i] == right[j] {
                i += 1;
                j += 1;
                continue;
            }
            let (start_i, start_j) = (i, j);
            while i < left.len() || j < right.len() {
                if i < left.len() && j < right.len() && left[i] == right[j] {
                    break;
                }
                // Stable deletion-first tie break for repeated source lines.
                if i < left.len()
                    && (j == right.len() || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])
                {
                    i += 1;
                } else {
                    j += 1;
                }
            }
            add(start_i, start_j, i, j);
        }
    }
    serde_json::json!({"common_prefix_lines":prefix,"indexed_changed_range":{"start":prefix+1,"end_exclusive":a.len()-tail+1},"live_changed_range":{"start":prefix+1,"end_exclusive":b.len()-tail+1},"indexed_lines":a.len(),"live_lines":b.len(),"algorithm":"common_prefix_suffix_ranges",
        "edit_hunks":{"contract_version":1,"items":hunks,"total":total,"truncated":total>DIFF_MAX_HUNKS,
            "exact":exact,"algorithm":if exact {"bounded_line_lcs"} else {"coarse_changed_range"},
            "fallback_reason":if exact {None} else {Some("lcs_cell_budget_exceeded")},
            "max_cells":DIFF_MAX_CELLS,"max_hunks":DIFF_MAX_HUNKS,"line_terminators_significant":true,
            "coordinate_system":"one_based_half_open","text_included":false}})
}

impl<'a> CodeIndex<'a> {
    pub fn code_freshness(&self, q: &SourceQuery, roots: &[PathBuf]) -> Result<serde_json::Value> {
        let mode = source_mode(q)?;
        let (revision, files) = revision_files(self.store, q)?;
        let limit = q.limit.unwrap_or(500).clamp(1, 2000);
        let mut items = Vec::new();
        let mut changed = 0;
        let mut unavailable = 0;
        for file in files.iter().take(limit) {
            let entry = match actual(self.store, file, mode, roots) {
                Ok(content) => {
                    let (status, hash) =
                        comparison(&file.hash, content.as_ref().map(|s| s.text.as_str()));
                    let event = if status != "unchanged" {
                        changed += 1;
                        Some(self.store.with_conn(|conn| {
                            observe(conn, revision, file, status, hash.as_deref(), mode)
                        })?)
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
        let (revision, files) = revision_files(self.store, q)?;
        if files.len() != 1 {
            return Err(AppError::config(
                "live_read requires one unambiguous indexed source",
            ));
        }
        let file = &files[0];
        let content = actual(self.store, file, mode, roots)?;
        let (status, hash) = comparison(&file.hash, content.as_ref().map(|s| s.text.as_str()));
        let event =
            if status != "unchanged" {
                Some(self.store.with_conn(|conn| {
                    observe(conn, revision, file, status, hash.as_deref(), mode)
                })?)
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
        self.store.with_conn(|conn| {
            let limit = q.limit.unwrap_or(100).clamp(1, 500);
            let mut stmt=conn.prepare("SELECT event_id,seq,index_revision,document_id,path,project,kind,source_kind,indexed_hash,observed_hash,CAST(first_seen AS VARCHAR),CAST(last_seen AS VARCHAR),CAST(acknowledged_at AS VARCHAR),acknowledgement FROM code_index_drift WHERE (? IS NULL OR project=?) AND (? IS NULL OR index_revision=?) AND (? IS NULL OR seq<?) AND (? OR acknowledged_at IS NULL) ORDER BY seq DESC LIMIT ?")?;
            let rows=stmt.query_map(params![q.project,q.project,q.index_revision,q.index_revision,q.before_seq,q.before_seq,q.include_acknowledged,(limit+1)as i64],|r|Ok(serde_json::json!({"event_id":r.get::<_,String>(0)?,"seq":r.get::<_,i64>(1)?,"index_rev":r.get::<_,i64>(2)?,"document_id":r.get::<_,String>(3)?,"path":r.get::<_,String>(4)?,"project":r.get::<_,Option<String>>(5)?,"kind":r.get::<_,String>(6)?,"source":r.get::<_,String>(7)?,"indexed_hash":r.get::<_,Option<String>>(8)?,"observed_hash":r.get::<_,Option<String>>(9)?,"first_seen":r.get::<_,String>(10)?,"last_seen":r.get::<_,String>(11)?,"acknowledged_at":r.get::<_,Option<String>>(12)?,"acknowledgement":r.get::<_,Option<String>>(13)?})))?;
            let mut items = rows.collect::<std::result::Result<Vec<_>, _>>()?;
            let more = items.len() > limit;
            items.truncate(limit);
            Ok(
                serde_json::json!({"items":items,"has_more":more,"next_before_seq":if more{items.last().and_then(|v|v["seq"].as_i64())}else{None},"observation_log_only":true}),
            )
        })
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
        self.store.with_conn(|conn| {
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
        })
    }
}

#[cfg(test)]
mod tests;

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
        let store = crate::testkit::open_test_store(&db.path().join("source.duckdb")).unwrap();
        store
            .documents()
            .upsert_document(&crate::models::Document {
                id: "a".into(),
                uri: "memory://a".into(),
                source_file: Some(link.display().to_string()),
                content: "indexed bytes".into(),
                ..Default::default()
            })
            .unwrap();
        crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap();
        let q = SourceQuery {
            document_id: Some("a".into()),
            ..Default::default()
        };
        let roots = vec![root.path().to_path_buf()];
        assert!(crate::code_index::CodeIndex::new(&store)
            .code_live_read(&q, &roots)
            .is_err());
        assert_eq!(
            crate::code_index::CodeIndex::new(&store)
                .code_freshness(&q, &roots)
                .unwrap()["unavailable"],
            1
        );
        std::fs::remove_file(&link).unwrap();
        let file = std::fs::File::create(&link).unwrap();
        file.set_len(4 * 1024 * 1024 + 1).unwrap();
        assert!(crate::code_index::CodeIndex::new(&store)
            .code_live_read(&q, &roots)
            .unwrap_err()
            .to_string()
            .contains("4 MiB"));
        assert_eq!(
            crate::code_index::CodeIndex::new(&store)
                .code_freshness(&q, &roots)
                .unwrap()["all_checked_unchanged"],
            false
        );
    }
}
