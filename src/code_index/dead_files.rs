//! Stored-source file candidates and positive literal refutations. Never deletion proof.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const PER_SOURCE_BYTES: usize = 2 * 1024 * 1024;
pub(super) const SOURCES: &str = "FROM documents d LEFT JOIN code_index_source_blobs b ON b.content_hash=json_extract_string(try_cast(d.metadata_json AS JSON),'$.code_source_hash') WHERE d.source_file IS NOT NULL AND COALESCE(d.status,'active') NOT IN ('archived','tombstone','consolidated') AND (CAST(? AS VARCHAR) IS NULL OR d.wing=?)";
pub(super) const TEXT: &str = "CASE WHEN json_extract_string(try_cast(d.metadata_json AS JSON),'$.code_source_hash') IS NOT NULL THEN b.content ELSE d.content END";
#[derive(Serialize)]
struct File {
    document_id: String,
    path: String,
    project: Option<String>,
    revision: i64,
    content_hash: Option<String>,
    #[serde(skip)]
    preserved: Option<String>,
    #[serde(skip)]
    bytes: Option<usize>,
    #[serde(skip)]
    metadata_valid: bool,
    #[serde(skip)]
    shebang: bool,
}
fn path_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}
fn occurrence(source: &str, needle: &str) -> Option<usize> {
    source
        .match_indices(needle)
        .find(|(i, _)| {
            !source[..*i].chars().next_back().is_some_and(path_char)
                && !source[*i + needle.len()..]
                    .chars()
                    .next()
                    .is_some_and(path_char)
        })
        .map(|(i, _)| i)
}
impl Store {
    pub fn code_dead_files(&self, q: &CodeQuery) -> Result<serde_json::Value> {
        if q.symbol.is_some() {
            return Err(AppError::Invalid(
                "file dead-code view cannot select a symbol".into(),
            ));
        }
        let limit = q.limit.unwrap_or(50).clamp(1, 500);
        let checks = q.dead_code_max_candidates.unwrap_or(100).clamp(1, 500);
        let max_docs = q.dead_code_max_documents.unwrap_or(2000).clamp(1, 100000);
        let max_bytes = q
            .dead_code_max_bytes
            .unwrap_or(8 * 1024 * 1024)
            .clamp(1, 64 * 1024 * 1024);
        let mut connection = self.lock()?;
        let tx = connection.transaction()?;
        // This existing source/checksum validation phase is deliberately outside
        // the literal-scan budget. Do not present that budget as total query IO/RSS.
        let snapshot = snapshot_locked(&tx, q.dead_code_project.as_deref())?;
        let sql=format!("SELECT d.id,d.source_file,d.wing,COALESCE(d.revision,1),json_extract_string(try_cast(d.metadata_json AS JSON),'$.code_source_hash'),octet_length(encode({TEXT})),json_valid(d.metadata_json),COALESCE(starts_with({TEXT},'#!'),false) {SOURCES} ORDER BY d.source_file,d.id");
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(params![q.dead_code_project, q.dead_code_project], |r| {
            Ok(File {
                document_id: r.get(0)?,
                path: r.get(1)?,
                project: r.get(2)?,
                revision: r.get(3)?,
                preserved: r.get(4)?,
                bytes: r
                    .get::<_, Option<i64>>(5)?
                    .and_then(|n| usize::try_from(n).ok()),
                metadata_valid: r.get(6)?,
                shebang: r.get(7)?,
                content_hash: None,
            })
        })?;
        let mut files = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);
        let by_id: HashMap<_, _> = files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.document_id.clone(), i))
            .collect();
        let symbols: HashMap<_, _> = snapshot
            .symbols
            .iter()
            .map(|s| (s.id.as_str(), s))
            .collect();
        let mut incoming = HashSet::new();
        let mut add = |source: &str, target: &str| {
            if source == target {
                return;
            }
            if let (Some(a), Some(b)) = (by_id.get(source), by_id.get(target)) {
                if files[*a].project == files[*b].project {
                    incoming.insert(target.to_string());
                }
            }
        };
        for e in &snapshot.file_resources {
            for target in &e.candidates {
                add(&e.document_id, target);
            }
        }
        for e in &snapshot.file_imports {
            for target in &e.candidates {
                add(&e.document_id, target);
            }
        }
        for e in &snapshot.bound_references {
            for target in e
                .candidates
                .iter()
                .filter_map(|id| symbols.get(id.as_str()))
            {
                add(&e.reference.document_id, &target.document_id);
            }
        }
        for e in &snapshot.inheritance {
            if let Some(source) = symbols.get(e.derived.as_str()) {
                for target in e
                    .candidates
                    .iter()
                    .filter_map(|id| symbols.get(id.as_str()))
                {
                    add(&source.document_id, &target.document_id);
                }
            }
        }
        for e in &snapshot.overrides {
            if let Some(source) = symbols.get(e.derived.as_str()) {
                for target in e
                    .candidates
                    .iter()
                    .filter_map(|id| symbols.get(id.as_str()))
                {
                    add(&source.document_id, &target.document_id);
                }
            }
        }
        let roots: HashSet<_> = snapshot
            .entrypoints
            .iter()
            .filter_map(|e| e.symbol_id.as_deref())
            .filter_map(|id| symbols.get(id))
            .map(|s| s.document_id.as_str())
            .collect();
        let selected = |f: &File| q.path.as_ref().is_none_or(|p| p == &f.path);
        let candidates: Vec<_> = files
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                selected(f)
                    && !incoming.contains(&f.document_id)
                    && !roots.contains(f.document_id.as_str())
                    && !f.shebang
            })
            .map(|(i, _)| i)
            .collect();
        let checked: HashSet<_> = candidates.iter().take(checks).copied().collect();
        let mut basenames: BTreeMap<(Option<String>, String), usize> = BTreeMap::new();
        let mut common: BTreeMap<Option<String>, PathBuf> = BTreeMap::new();
        for f in &files {
            let p = Path::new(&f.path);
            let base = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            *basenames.entry((f.project.clone(), base)).or_default() += 1;
            let parent = p.parent().unwrap_or(Path::new(""));
            common
                .entry(f.project.clone())
                .and_modify(|root| {
                    while !parent.starts_with(root.as_path()) {
                        if !root.pop() {
                            break;
                        }
                    }
                })
                .or_insert_with(|| parent.to_path_buf());
        }
        let mut needles: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for &i in &checked {
            let f = &files[i];
            let path = Path::new(&f.path);
            let base = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let mut values = BTreeSet::from([f.path.clone()]);
            if let Some(relative) = common
                .get(&f.project)
                .and_then(|root| path.strip_prefix(root).ok())
            {
                values.insert(relative.to_string_lossy().to_string());
            }
            if basenames.get(&(f.project.clone(), base.clone())) == Some(&1) {
                values.insert(base);
            }
            for needle in values.into_iter().filter(|s| !s.is_empty()) {
                needles.entry(needle).or_default().push(i);
            }
        }
        let patterns: Vec<_> = needles.keys().cloned().collect();
        let matcher = regex::RegexSetBuilder::new(patterns.iter().map(|s| regex::escape(s)))
            .size_limit(2 * 1024 * 1024)
            .dfa_size_limit(2 * 1024 * 1024)
            .build()
            .map_err(|_| {
                AppError::Invalid(
                    "literal path pattern budget exceeded; reduce candidate checks".into(),
                )
            })?;
        let mut complete: BTreeMap<Option<String>, bool> =
            files.iter().map(|f| (f.project.clone(), true)).collect();
        if snapshot.index_revision.is_none() {
            for value in complete.values_mut() {
                *value = false;
            }
        }
        for id in snapshot
            .stale_documents
            .iter()
            .chain(&snapshot.unindexed_documents)
            .chain(&snapshot.parse_errors)
        {
            if let Some(scope) = snapshot.document_scopes.get(id) {
                complete.insert(scope.project.clone(), false);
            }
        }
        for target in snapshot.file_resource_targets.iter().filter(|t| !t.fresh()) {
            complete.insert(target.project.clone(), false);
        }
        let mut hits: HashMap<usize, serde_json::Value> = HashMap::new();
        let mut bytes = 0usize;
        let mut read = 0usize;
        let mut unavailable = 0usize;
        let source_sql=format!("SELECT CASE WHEN octet_length(encode({TEXT}))<=? THEN {TEXT} ELSE NULL END {SOURCES} AND d.id=?");
        for i in 0..files.len() {
            let f = &files[i];
            let size = f.bytes.unwrap_or(usize::MAX);
            if !f.metadata_valid
                || size > PER_SOURCE_BYTES
                || size > max_bytes.saturating_sub(bytes)
                || read >= max_docs
            {
                complete.insert(f.project.clone(), false);
                unavailable += 1;
                continue;
            }
            read += 1;
            let source: Option<String> = tx.query_row(
                &source_sql,
                params![
                    PER_SOURCE_BYTES as i64,
                    q.dead_code_project,
                    q.dead_code_project,
                    f.document_id
                ],
                |r| r.get(0),
            )?;
            let Some(source) = source else {
                complete.insert(f.project.clone(), false);
                unavailable += 1;
                continue;
            };
            bytes += source.len();
            let hash = content_hash(&source);
            if f.preserved.as_ref().is_some_and(|h| h != &hash) {
                complete.insert(f.project.clone(), false);
                unavailable += 1;
                continue;
            }
            if matches!(
                Path::new(&f.path).extension().and_then(|s| s.to_str()),
                Some("html" | "htm")
            ) && f.preserved.is_none()
            {
                complete.insert(f.project.clone(), false);
            }
            for pattern in matcher.matches(&source).iter() {
                let needle = &patterns[pattern];
                let Some(offset) = occurrence(&source, needle) else {
                    continue;
                };
                for &target in &needles[needle] {
                    if target != i && files[target].project == f.project {
                        hits.entry(target).or_insert_with(||serde_json::json!({"kind":"literal_text_path_mention","needle":needle,"document_id":f.document_id,"path":f.path,"project":f.project,"revision":f.revision,"content_hash":hash,"byte_start":offset,"byte_end":offset+needle.len(),"line":source[..offset].bytes().filter(|b|*b==b'\n').count()+1,"binding_proven":false}));
                    }
                }
            }
            files[i].content_hash = Some(hash);
        }
        let mut items = Vec::new();
        let mut refuted = Vec::new();
        let mut unknown = Vec::new();
        let mut fully_checked = 0;
        for i in candidates.iter().copied() {
            let f = &files[i];
            let full = checked.contains(&i)
                && complete.get(&f.project) == Some(&true)
                && f.content_hash.is_some();
            if full {
                fully_checked += 1;
            }
            if let Some(evidence) = hits.get(&i) {
                refuted
                    .push(serde_json::json!({"file":f,"evidence":evidence,"usage_proven":false}));
            } else if full {
                items.push(serde_json::json!({"file":f,"reason":"no_known_incoming_links_or_entrypoint_and_no_literal_path_mention","deletion_safe":false,"candidate_only":true}));
            } else {
                unknown.push(serde_json::json!({"file":f,"reason":if !checked.contains(&i){"candidate_check_budget"}else{"incomplete_graph_or_literal_sources"},"deletion_safe":false}));
            }
        }
        let protected: Vec<_> = files
            .iter()
            .filter(|f| {
                selected(f)
                    && (incoming.contains(&f.document_id)
                        || roots.contains(f.document_id.as_str())
                        || f.shebang)
            })
            .map(|f| {
                let mut reasons = Vec::new();
                if incoming.contains(&f.document_id) {
                    reasons.push("known_incoming_file_relation");
                }
                if roots.contains(f.document_id.as_str()) {
                    reasons.push("known_entrypoint_candidate");
                }
                if f.shebang {
                    reasons.push("shebang_entrypoint_candidate");
                }
                serde_json::json!({"file":f,"reasons":reasons,"runtime_usage_proven":false})
            })
            .collect();
        let incomplete = !unknown.is_empty()
            || snapshot
                .calls
                .iter()
                .any(|c| c.resolution != "resolved_static")
            || !snapshot.stale_documents.is_empty()
            || !snapshot.unindexed_documents.is_empty()
            || !snapshot.parse_errors.is_empty()
            || snapshot.file_resource_targets.iter().any(|t| !t.fresh());
        let result = serde_json::json!({"view":"files","query_contract_version":1,"index_rev":snapshot.index_revision,"items":items.iter().take(limit).collect::<Vec<_>>(),"total":items.len(),"truncated":items.len()>limit,"refuted":refuted.iter().take(limit).collect::<Vec<_>>(),"refuted_total":refuted.len(),"refuted_truncated":refuted.len()>limit,"not_checked":unknown.iter().take(limit).collect::<Vec<_>>(),"not_checked_total":unknown.len(),"not_checked_truncated":unknown.len()>limit,"files_candidates":candidates.len(),"files_fully_checked":fully_checked,"protected_files":protected.len(),"protected":protected.iter().take(limit).collect::<Vec<_>>(),"protected_truncated":protected.len()>limit,"static_analysis_only":true,"analysis_incomplete":incomplete,"literal_source_documents":files.len(),"literal_source_reads":read,"literal_bytes_loaded":bytes,"literal_sources_unavailable_or_skipped":unavailable,"literal_source_projects_complete":complete.iter().map(|(p,c)|serde_json::json!({"project":p,"complete":c})).collect::<Vec<_>>(),"budgets":{"candidate_checks":checks,"source_reads":max_docs,"literal_decoded_bytes":max_bytes,"per_source_bytes":PER_SOURCE_BYTES,"graph_source_validation_included":false},"source":"active_stored_source_file_documents","observation_consistency":"single_read_transaction","text_included":false,"runtime_usage_proven":false,"deletion_safe":false,"graph_extraction_exhaustive":false,"stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors,"resource_target_inventory_available":snapshot.resource_target_inventory_available,"resource_target_diagnostics":resource_targets::diagnostics(&snapshot.file_resource_targets,limit)});
        tx.commit()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    fn setup() -> (tempfile::TempDir, Store) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("files.duckdb")).unwrap();
        (root, store)
    }
    fn put(store: &Store, id: &str, path: &str, project: &str, body: &str) {
        let mut d = Document {
            id: id.into(),
            uri: format!("doc://{id}"),
            source_file: Some(path.into()),
            wing: Some(project.into()),
            content: body.into(),
            ..Default::default()
        };
        if path.ends_with(".html") {
            d.metadata_json = serde_json::json!({"code_source_text":body}).to_string();
            d.content = "Readable page".into();
        }
        store.upsert_document(&d).unwrap();
    }
    fn query(project: &str) -> CodeQuery {
        CodeQuery {
            dead_code_view: Some("files".into()),
            dead_code_project: Some(project.into()),
            ..Default::default()
        }
    }
    fn contains(r: &serde_json::Value, field: &str, path: &str) -> bool {
        r[field]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["file"]["path"] == path)
    }
    #[test]
    fn stored_markup_refutes_used_files_without_exposing_text_or_inventing_usage() {
        let (_root, store) = setup();
        put(&store, "used", "/p/used.py", "p", "def work(): pass");
        put(&store, "unused", "/p/unused.py", "p", "def spare(): pass");
        put(
            &store,
            "view",
            "/p/view.html",
            "p",
            "<script>const PRIVATE_BODY='used.py'; const old='unused.py.bak';</script>",
        );
        store.index_code().unwrap();
        let r = store.code_dead_files(&query("p")).unwrap();
        assert!(contains(&r, "refuted", "/p/used.py"), "{r}");
        assert!(contains(&r, "items", "/p/unused.py"), "{r}");
        assert!(!r.to_string().contains("PRIVATE_BODY"));
        assert_eq!(r["deletion_safe"], false);
        assert_eq!(r["runtime_usage_proven"], false);
        assert_eq!(r["refuted"][0]["evidence"]["binding_proven"], false);
    }
    #[test]
    fn graph_links_and_script_entrypoints_are_protected_and_stale_graphs_are_unknown() {
        let (_root, store) = setup();
        put(&store, "api", "/p/api.py", "p", "def work(): pass");
        put(
            &store,
            "main",
            "/p/main.py",
            "p",
            "from api import work\ndef main(): work()\n",
        );
        put(
            &store,
            "script",
            "/p/import.xyz",
            "p",
            "#!/usr/bin/env custom\nexecute\n",
        );
        put(&store, "unused", "/p/unused.py", "p", "def spare(): pass");
        store.index_code().unwrap();
        let r = store.code_dead_files(&query("p")).unwrap();
        assert!(r["protected_files"].as_u64().unwrap() >= 3, "{r}");
        assert!(!contains(&r, "items", "/p/api.py"));
        assert!(!contains(&r, "items", "/p/import.xyz"));
        put(&store, "api", "/p/api.py", "p", "def changed(): pass");
        let r = store.code_dead_files(&query("p")).unwrap();
        assert!(r["items"].as_array().unwrap().is_empty(), "{r}");
        assert!(contains(&r, "not_checked", "/p/unused.py"));
    }
    #[test]
    fn budgets_keep_positive_refutations_but_never_report_unchecked_absence() {
        let (_root, store) = setup();
        put(&store, "a", "/p/a.py", "p", "link='b.py'\ndef a(): pass");
        put(&store, "b", "/p/b.py", "p", "def b(): pass");
        store.index_code().unwrap();
        let mut q = query("p");
        q.dead_code_max_documents = Some(1);
        let r = store.code_dead_files(&q).unwrap();
        assert!(contains(&r, "refuted", "/p/b.py"), "{r}");
        assert!(r["items"].as_array().unwrap().is_empty());
        assert!(r["not_checked_total"].as_u64().unwrap() > 0);
        assert_eq!(r["literal_source_reads"], 1);
        q.dead_code_max_documents = None;
        q.dead_code_max_bytes = Some(1);
        let r = store.code_dead_files(&q).unwrap();
        assert_eq!(r["literal_bytes_loaded"], 0);
        assert_eq!(r["not_checked_total"], 2);
        assert_eq!(r["budgets"]["graph_source_validation_included"], false);
        q.dead_code_max_bytes = None;
        q.dead_code_max_candidates = Some(1);
        let r = store.code_dead_files(&q).unwrap();
        assert_eq!(r["files_fully_checked"], 1);
        assert_eq!(r["not_checked_total"], 1);
    }
    #[test]
    fn projects_and_ambiguous_basenames_do_not_create_false_literal_bindings() {
        let (_root, store) = setup();
        put(&store, "a", "/p/a/same.py", "p", "def a(): pass");
        put(&store, "b", "/p/b/same.py", "p", "def b(): pass");
        put(&store, "view", "/p/view.txt", "p", "same.py");
        put(&store, "foreign", "/q/other.txt", "q", "a/same.py");
        store.index_code().unwrap();
        let q = query("p");
        let first = store.code_dead_files(&q).unwrap();
        assert!(!contains(&first, "refuted", "/p/a/same.py"));
        assert!(!contains(&first, "refuted", "/p/b/same.py"));
        let all = store
            .code_dead_files(&CodeQuery {
                dead_code_view: Some("files".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(!contains(&all, "refuted", "/p/a/same.py"), "{all}");
        put(&store, "view", "/p/view.txt", "p", "a/same.py");
        let r = store.code_dead_files(&q).unwrap();
        assert!(contains(&r, "refuted", "/p/a/same.py"), "{r}");
        assert!(!contains(&r, "refuted", "/p/b/same.py"));
        assert_eq!(store.code_dead_files(&q).unwrap(), r);
    }
}
