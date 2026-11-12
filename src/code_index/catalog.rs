//! Lean frozen manifests and captured project registry; never scans live files.
use super::*;
use crate::db::lin_engine::{cell_bool, cell_i64, cell_opt_string, cell_str, cell_string, LinReader};
use std::collections::BTreeMap;

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CatalogView {
    #[default]
    Files,
    Projects,
    Runs,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct CatalogQuery {
    #[serde(flatten)]
    pub selection: drift::SourceQuery,
    #[serde(default)]
    pub view: CatalogView,
    /// Opaque continuation from this view/filter set. Limit may change.
    pub cursor: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    version: u32,
    revision: Option<i64>,
    view: CatalogView,
    scope: String,
    after: String,
    observation: Option<String>,
}
fn scope(q: &CatalogQuery) -> Result<String> {
    Ok(content_hash(&serde_json::to_string(&(
        q.view,
        &q.selection.project,
        &q.selection.document_id,
        &q.selection.path,
    ))?))
}
fn revision(r: &impl LinReader, q: &CatalogQuery, cursor: Option<&Cursor>) -> Result<Option<i64>> {
    let selected = if let Some(c) = cursor {
        if q.selection.index_revision.is_some() && q.selection.index_revision != c.revision {
            return Err(AppError::Invalid(
                "catalog cursor revision does not match index_revision".into(),
            ));
        }
        c.revision
    } else if q.selection.index_revision.is_some() {
        q.selection.index_revision
    } else {
        crate::analysis_tasks::latest_code_index_rev(r)?
    };
    if let Some(id) = selected
        && r.get_by_id("code_index_revisions", &id.to_string())?
            .is_none()
    {
        return Err(AppError::not_found(format!("unknown index revision {id}")));
    }
    Ok(selected)
}
fn cursor(
    q: &CatalogQuery,
    rev: Option<i64>,
    after: String,
    observation: Option<String>,
) -> Result<String> {
    Ok(serde_json::to_string(&Cursor {
        version: 1,
        revision: rev,
        view: q.view,
        scope: scope(q)?,
        after,
        observation,
    })?)
}
fn files(
    r: &impl LinReader,
    q: &CatalogQuery,
    rev: Option<i64>,
    previous: Option<&Cursor>,
) -> Result<serde_json::Value> {
    let rev = rev.ok_or_else(|| AppError::not_found("run index_code or sync_sources first"))?;
    let s = &q.selection;
    let mut matched = Vec::new();
    for row in r.find_by_eq("code_index_revision_files", "index_revision", rev)? {
        let proj = cell_opt_string(&row, "project");
        if let Some(ref p) = s.project
            && proj.as_deref() != Some(p.as_str())
        {
            continue;
        }
        let doc_id = cell_string(&row, "document_id");
        if let Some(ref id) = s.document_id
            && &doc_id != id
        {
            continue;
        }
        let path = cell_string(&row, "path");
        if let Some(ref p) = s.path
            && &path != p
        {
            continue;
        }
        let doc_rev = cell_i64(&row, "document_revision", 1);
        let hash = cell_string(&row, "content_hash");
        let syntax_indexed = cell_str(&row, "analysis_json").is_some();
        matched.push((doc_id, path, proj, doc_rev, hash, syntax_indexed));
    }
    matched.sort_by(|a, b| a.0.cmp(&b.0));
    let total = matched.len() as i64;
    let after = previous.map(|c| c.after.as_str());
    let limit = s.limit.unwrap_or(100).clamp(1, 1000);
    let mut page = Vec::new();
    for (doc_id, path, proj, doc_rev, hash, syntax_indexed) in matched {
        if let Some(a) = after
            && doc_id.as_str() <= a
        {
            continue;
        }
        let blob_bytes = r
            .get_by_id("code_index_source_blobs", &hash)?
            .and_then(|b| cell_str(&b, "content").map(|c| c.len() as i64));
        page.push(serde_json::json!({
            "document_id": doc_id,
            "path": path,
            "project": proj,
            "document_revision": doc_rev,
            "content_hash": hash,
            "bytes": blob_bytes,
            "syntax_indexed": syntax_indexed,
            "source_available": blob_bytes.is_some(),
        }));
        if page.len() > limit {
            break;
        }
    }
    let more = page.len() > limit;
    page.truncate(limit);
    let next = if more {
        Some(cursor(
            q,
            Some(rev),
            page.last().unwrap()["document_id"]
                .as_str()
                .unwrap()
                .into(),
            None,
        )?)
    } else {
        None
    };
    Ok(
        serde_json::json!({"index_rev":rev,"items":page,"total":total,"truncated":more||previous.is_some(),"has_more":more,"next_cursor":next,"view":"immutable_revision_manifest","query_contract_version":1,"content_checksums_verified":false,"source_text_loaded_into_rust":false,"observation_consistency":"single_read_transaction"}),
    )
}
#[derive(Default)]
struct Group {
    hash: blake3::Hasher,
    count: usize,
    syntax: usize,
    available: bool,
    roots: Vec<serde_json::Value>,
}
fn add(group: &mut Group, id: &str, path: &str, hash: Option<&str>) -> Result<()> {
    let bytes = serde_json::to_vec(&(id, path, hash))?;
    group.hash.update(&(bytes.len() as u64).to_le_bytes());
    group.hash.update(&bytes);
    group.count += 1;
    if hash.is_none_or(str::is_empty) {
        group.available = false;
    }
    Ok(())
}
fn group(map: &mut BTreeMap<Option<String>, Group>, project: Option<String>) -> &mut Group {
    map.entry(project).or_insert_with(|| Group {
        available: true,
        ..Default::default()
    })
}
fn projects(
    r: &impl LinReader,
    q: &CatalogQuery,
    rev: Option<i64>,
    previous: Option<&Cursor>,
) -> Result<serde_json::Value> {
    if q.selection.document_id.is_some() || q.selection.path.is_some() {
        return Err(AppError::Invalid(
            "projects catalog supports project scope; document_id/path are files-view filters"
                .into(),
        ));
    }
    let mut current = BTreeMap::new();
    let mut indexed = BTreeMap::new();
    let project = q.selection.project.as_deref();
    let mut active_doc_wings: HashMap<String, Option<String>> = HashMap::new();
    {
        let mut doc_rows = Vec::new();
        r.scan_each("documents", |row| {
            let status = cell_str(row, "status").unwrap_or("active");
            if matches!(status, "archived" | "tombstone" | "consolidated") {
                return Ok(true);
            }
            let wing = cell_opt_string(row, "wing");
            if let Some(p) = project
                && wing.as_deref() != Some(p)
            {
                return Ok(true);
            }
            let id = cell_string(row, "id");
            let path = cell_str(row, "source_file")
                .or_else(|| cell_str(row, "uri"))
                .unwrap_or("")
                .to_string();
            let declared = cell_opt_string(row, "content_hash");
            let meta = cell_str(row, "metadata_json").unwrap_or("{}").to_string();
            doc_rows.push((id, path, wing, declared, meta));
            Ok(true)
        })?;
        doc_rows.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, path, wing, declared, meta) in doc_rows {
            active_doc_wings.insert(id.clone(), wing.clone());
            let hash = if meta.contains("code_source_hash") {
                match serde_json::from_str::<serde_json::Value>(&meta) {
                    Ok(value) => value
                        .get("code_source_hash")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .or(declared),
                    Err(_) => None,
                }
            } else {
                declared
            };
            add(group(&mut current, wing), &id, &path, hash.as_deref())?;
        }
    }
    if let Some(rev) = rev {
        let mut rf_rows = Vec::new();
        for row in r.find_by_eq("code_index_revision_files", "index_revision", rev)? {
            let wing = cell_opt_string(&row, "project");
            if let Some(p) = project
                && wing.as_deref() != Some(p)
            {
                continue;
            }
            rf_rows.push((
                cell_string(&row, "document_id"),
                cell_string(&row, "path"),
                wing,
                cell_string(&row, "content_hash"),
                cell_str(&row, "analysis_json").is_some(),
            ));
        }
        rf_rows.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, path, wing, hash, has_syntax) in rf_rows {
            let g = group(&mut indexed, wing);
            add(g, &id, &path, Some(&hash))?;
            g.syntax += usize::from(has_syntax);
        }
    }
    {
        type RootKey = (Option<String>, String);
        type RootStats = (i64, Option<String>, Option<String>);
        let mut root_groups: BTreeMap<RootKey, RootStats> = BTreeMap::new();
        r.scan_each("source_manifest", |row| {
            let Some(doc_id) = cell_str(row, "document_id") else {
                return Ok(true);
            };
            let Some(wing) = active_doc_wings.get(doc_id) else {
                return Ok(true);
            };
            let root = cell_string(row, "canonical_root");
            let seen = cell_opt_string(row, "last_seen");
            let synced = cell_opt_string(row, "last_synced");
            let entry = root_groups
                .entry((wing.clone(), root))
                .or_insert((0, None, None));
            entry.0 += 1;
            if let Some(s) = seen
                && entry.1.as_ref().is_none_or(|cur| &s > cur)
            {
                entry.1 = Some(s);
            }
            if let Some(s) = synced
                && entry.2.as_ref().is_none_or(|cur| &s > cur)
            {
                entry.2 = Some(s);
            }
            Ok(true)
        })?;
        for ((wing, canonical_root), (manifest_files, last_seen, last_synced)) in root_groups {
            group(&mut current, wing).roots.push(serde_json::json!({
                "canonical_root": canonical_root,
                "manifest_files": manifest_files,
                "last_seen": last_seen,
                "last_synced": last_synced,
            }));
        }
    }
    let published: Option<String> = if let Some(rev) = rev {
        r.get_by_id("code_index_revisions", &rev.to_string())?
            .and_then(|row| cell_opt_string(&row, "created_at"))
    } else {
        None
    };
    let keys: std::collections::BTreeSet<_> =
        current.keys().chain(indexed.keys()).cloned().collect();
    let mut all = Vec::new();
    for key in keys {
        let live = current.get(&key);
        let frozen = indexed.get(&key);
        let live_hash = live
            .filter(|g| g.available)
            .map(|g| g.hash.finalize().to_hex().to_string());
        let frozen_hash = frozen
            .filter(|g| g.available)
            .map(|g| g.hash.finalize().to_hex().to_string());
        let matches = live_hash
            .as_ref()
            .zip(frozen_hash.as_ref())
            .map(|(a, b)| a == b);
        let state = match (live, frozen, matches) {
            (Some(_), None, _) => "unindexed",
            (None, Some(_), _) => "absent_from_current_corpus",
            (_, _, Some(true)) => "captured_match_unverified",
            (_, _, Some(false)) => "captured_drift",
            _ => "unavailable_hash",
        };
        all.push((key.clone(),serde_json::json!({"project":key,"state":state,"index_rev":rev,"revision_published_at":published,"current_documents":live.map_or(0,|g|g.count),"indexed_documents":frozen.map_or(0,|g|g.count),"syntax_indexed_documents":frozen.map_or(0,|g|g.syntax),"current_corpus_hash":live_hash,"indexed_corpus_hash":frozen_hash,"captured_hashes_match":matches,"hash_matches":null,"filesystem_verified":false,"source_integrity_verified":false,"config_hash":null,"config_matches":null,"roots":live.map(|g|&g.roots),"roots_scope":"current_source_manifest","index_revision_scope":"shared_corpus","physical_storage_isolated":false})));
    }
    let observation = content_hash(&serde_json::to_string(&all)?);
    if previous.is_some_and(|c| c.observation.as_deref() != Some(&observation)) {
        return Err(AppError::Conflict(
            "CATALOG_OBSERVATION_CHANGED: restart projects catalog without cursor".into(),
        ));
    }
    let after: Option<Option<String>> = previous
        .map(|c| serde_json::from_str(&c.after))
        .transpose()
        .map_err(|_| AppError::Invalid("invalid catalog project cursor key".into()))?;
    let limit = q.selection.limit.unwrap_or(100).clamp(1, 1000);
    let mut page: Vec<_> = all
        .iter()
        .filter(|(key, _)| after.as_ref().is_none_or(|a| key > a))
        .take(limit + 1)
        .collect();
    let more = page.len() > limit;
    page.truncate(limit);
    let next = if more {
        Some(cursor(
            q,
            rev,
            serde_json::to_string(&page.last().unwrap().0)?,
            Some(observation),
        )?)
    } else {
        None
    };
    Ok(
        serde_json::json!({"index_rev":rev,"items":page.iter().map(|(_,v)|v).collect::<Vec<_>>(),"total":all.len(),"truncated":more||previous.is_some(),"has_more":more,"next_cursor":next,"view":"captured_project_registry","query_contract_version":1,"current_comparison_scope":"active_stored_documents_and_preserved_source_hash_metadata","corpus_hash_algorithm":"blake3_length_delimited_document_id_path_declared_source_hash","filesystem_verified":false,"content_checksums_verified":false,"config_identity_available":false,"independent_project_revisions":false,"observation_consistency":"single_read_transaction"}),
    )
}
#[derive(Serialize, Deserialize)]
struct RunAnchor {
    seq: i64,
    run_id: Option<String>,
}
fn runs(
    r: &impl LinReader,
    q: &CatalogQuery,
    previous: Option<&Cursor>,
) -> Result<serde_json::Value> {
    let s = &q.selection;
    if s.project.is_some()
        || s.path.is_some()
        || s.document_id.is_some()
        || s.source.is_some()
        || s.start_line.is_some()
        || s.end_line.is_some()
        || s.max_chars.is_some()
    {
        return Err(AppError::Invalid(
            "run history is corpus-global; only index_revision and limit filters are supported"
                .into(),
        ));
    }
    let revision = previous.map(|c| c.revision).unwrap_or(s.index_revision);
    if previous.is_some() && s.index_revision.is_some() && s.index_revision != revision {
        return Err(AppError::Invalid(
            "run cursor revision does not match index_revision".into(),
        ));
    }
    if let Some(id) = revision
        && r.get_by_id("code_index_revisions", &id.to_string())?
            .is_none()
    {
        return Err(AppError::not_found("unknown index revision"));
    }
    let anchor = if let Some(c) = previous {
        let a: RunAnchor = serde_json::from_str(c.observation.as_deref().unwrap_or(""))
            .map_err(|_| AppError::Invalid("invalid run cursor anchor".into()))?;
        let id = if a.seq > 0 && r.has_collection("code_index_runs") {
            r.get_by_id("code_index_runs", &a.seq.to_string())?
                .and_then(|row| cell_opt_string(&row, "run_id"))
        } else {
            None
        };
        if a.seq <= 0 || a.run_id.is_none() || id != a.run_id {
            return Err(AppError::Invalid(
                "run history anchor changed or is unavailable".into(),
            ));
        }
        a
    } else {
        let mut best: Option<(i64, String)> = None;
        if r.has_collection("code_index_runs") {
            r.scan_each("code_index_runs", |row| {
                let seq = cell_i64(row, "seq", 0);
                if best.as_ref().is_none_or(|b| seq > b.0) {
                    best = Some((seq, cell_string(row, "run_id")));
                }
                Ok(true)
            })?;
        }
        RunAnchor {
            seq: best.as_ref().map_or(0, |r| r.0),
            run_id: best.map(|r| r.1),
        }
    };
    let after = previous
        .map(|c| {
            c.after
                .parse::<i64>()
                .map_err(|_| AppError::Invalid("invalid run cursor sequence".into()))
        })
        .transpose()?;
    if after.is_some_and(|n| n <= 0 || n > anchor.seq) {
        return Err(AppError::Invalid("invalid run cursor sequence range".into()));
    }
    let mut total = 0_i64;
    let mut rows: Vec<(i64, String, String, i64, bool, Option<String>)> = Vec::new();
    if r.has_collection("code_index_runs") {
        r.scan_each("code_index_runs", |row| {
            let seq = cell_i64(row, "seq", 0);
            if seq > anchor.seq {
                return Ok(true);
            }
            let rev = cell_i64(row, "index_revision", 0);
            if let Some(target_rev) = revision
                && rev != target_rev
            {
                return Ok(true);
            }
            total += 1;
            if let Some(a) = after
                && seq >= a
            {
                return Ok(true);
            }
            let metrics = cell_str(row, "metrics_json")
                .filter(|m| m.len() <= 16384)
                .map(str::to_string);
            rows.push((
                seq,
                cell_string(row, "run_id"),
                cell_string(row, "recorded_at"),
                rev,
                cell_bool(row, "revision_created", false),
                metrics,
            ));
            Ok(true)
        })?;
    }
    rows.sort_by_key(|a| std::cmp::Reverse(a.0));
    let limit = s.limit.unwrap_or(50).clamp(1, 500);
    let more = rows.len() > limit;
    rows.truncate(limit);
    let items: Vec<_> = rows
        .iter()
        .map(|(seq, id, at, rev, created, json)| {
            let decoded = json
                .as_deref()
                .and_then(|j| timings::decode(*seq, id, at, *created, *rev, j).ok());
            decoded
                .map(|mut row| {
                    row["metrics_available"] = serde_json::json!(true);
                    row
                })
                .unwrap_or_else(|| {
                    serde_json::json!({"seq":seq,"run_id":id,"recorded_at":at,"index_rev":rev,"revision_created":created,"metrics":null,"metrics_available":false,"reason":"invalid_or_oversized_run_metrics"})
                })
        })
        .collect();
    let next = if more {
        rows.last()
            .map(|r| {
                cursor(
                    q,
                    revision,
                    r.0.to_string(),
                    Some(serde_json::to_string(&anchor).unwrap()),
                )
            })
            .transpose()?
    } else {
        None
    };
    Ok(
        serde_json::json!({"view":"committed_index_runs","query_contract_version":1,"scope":"corpus_global","index_rev":revision,"items":items,"total":total,"has_more":more,"truncated":more||previous.is_some(),"next_cursor":next,"pinned_max_seq":anchor.seq,"newer_runs_excluded":true,"successful_committed_runs_only":true,"failed_or_rolled_back_runs_available":false,"commit_measured":false,"embedding_fts_measured":false,"source_text_loaded_into_rust":false,"observation_consistency":"single_read_transaction"}),
    )
}
impl Store {
    pub fn code_catalog(&self, q: &drift::SourceQuery) -> Result<serde_json::Value> {
        self.code_catalog_query(&CatalogQuery {
            selection: q.clone(),
            ..Default::default()
        })
    }
    pub fn code_catalog_query(&self, q: &CatalogQuery) -> Result<serde_json::Value> {
        let previous: Option<Cursor> = q
            .cursor
            .as_ref()
            .map(|s| {
                serde_json::from_str(s)
                    .map_err(|_| AppError::Invalid("invalid catalog cursor".into()))
            })
            .transpose()?;
        let expected_scope = scope(q)?;
        if previous
            .as_ref()
            .is_some_and(|c| c.version != 1 || c.view != q.view || c.scope != expected_scope)
        {
            return Err(AppError::Invalid(
                "catalog cursor does not match view/filters".into(),
            ));
        }
        let r = self.read()?;
        if q.view == CatalogView::Runs {
            return runs(&*r, q, previous.as_ref());
        }
        let rev = revision(&*r, q, previous.as_ref())?;
        match q.view {
            CatalogView::Files => files(&*r, q, rev, previous.as_ref()),
            CatalogView::Projects => projects(&*r, q, rev, previous.as_ref()),
            CatalogView::Runs => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    fn setup() -> (tempfile::TempDir, Store) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("catalog.duckdb")).unwrap();
        (root, store)
    }
    fn put(store: &Store, id: &str, project: Option<&str>, path: &str, content: &str) {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("doc://{id}"),
                wing: project.map(str::to_string),
                source_file: Some(path.into()),
                content: content.into(),
                ..Default::default()
            })
            .unwrap();
    }
    fn q(project: Option<&str>) -> CatalogQuery {
        CatalogQuery {
            selection: drift::SourceQuery {
                project: project.map(str::to_string),
                limit: Some(1),
                ..Default::default()
            },
            ..Default::default()
        }
    }
    fn run_query(limit: usize) -> CatalogQuery {
        CatalogQuery {
            view: CatalogView::Runs,
            selection: drift::SourceQuery {
                limit: Some(limit),
                ..Default::default()
            },
            cursor: None,
        }
    }
    #[test]
    fn run_history_pins_append_only_pages_across_noops_and_new_revisions() {
        let (_root, store) = setup();
        put(&store, "a", Some("p"), "/p/a.py", "def a(): pass");
        store.index_code().unwrap();
        store.index_code().unwrap();
        store.index_code().unwrap();
        let mut q = run_query(1);
        let first = store.code_catalog_query(&q).unwrap();
        assert_eq!(first["total"], 3);
        assert_eq!(first["items"][0]["seq"], 3);
        put(&store, "a", Some("p"), "/p/a.py", "def changed(): pass");
        store.index_code().unwrap();
        q.cursor = first["next_cursor"].as_str().map(str::to_owned);
        q.selection.limit = Some(10);
        let second = store.code_catalog_query(&q).unwrap();
        assert_eq!(second["total"], 3);
        assert_eq!(second["items"].as_array().unwrap().len(), 2);
        assert_eq!(second["items"][0]["seq"], 2);
        assert_eq!(second["has_more"], false);
        let all = store.code_catalog_query(&run_query(10)).unwrap();
        assert_eq!(all["total"], 4);
        assert_eq!(all["items"][0]["index_rev"], 2);
        let mut filtered = run_query(10);
        filtered.selection.index_revision = Some(1);
        let r = store.code_catalog_query(&filtered).unwrap();
        assert_eq!(r["total"], 3);
        assert!(r["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["index_rev"] == 1));
    }
    #[test]
    fn run_history_rejects_wrong_filters_revision_and_replaced_anchors() {
        let (_root, store) = setup();
        put(&store, "a", None, "/p/a.py", "def a(): pass");
        store.index_code().unwrap();
        store.index_code().unwrap();
        let mut q = run_query(1);
        let first = store.code_catalog_query(&q).unwrap();
        q.cursor = first["next_cursor"].as_str().map(str::to_owned);
        q.selection.index_revision = Some(1);
        assert!(store.code_catalog_query(&q).is_err());
        q.selection.index_revision = None;
        q.selection.project = Some("p".into());
        assert!(store.code_catalog_query(&q).is_err());
        q.selection.project = None;
        store
            .write()
            .unwrap()
            .update_where(
                "code_index_runs",
                lin::query::pred::eq("seq", 2_i64),
                crate::db::lin_engine::RecBuilder::new()
                    .str("run_id", "replaced")
                    .build(),
            )
            .unwrap();
        assert!(store.code_catalog_query(&q).is_err());
    }
    #[test]
    fn run_history_reports_invalid_and_oversized_metrics_without_decoding_bodies() {
        let (_root, store) = setup();
        put(&store, "a", None, "/p/a.py", "def a(): pass");
        store.index_code().unwrap();
        store.index_code().unwrap();
        store
            .write()
            .unwrap()
            .update_where(
                "code_index_runs",
                lin::query::pred::eq("seq", 1_i64),
                crate::db::lin_engine::RecBuilder::new()
                    .str("metrics_json", "PRIVATE_BODY".repeat(2000))
                    .build(),
            )
            .unwrap();
        store
            .write()
            .unwrap()
            .update_where(
                "code_index_runs",
                lin::query::pred::eq("seq", 2_i64),
                crate::db::lin_engine::RecBuilder::new()
                    .str("metrics_json", "{}")
                    .build(),
            )
            .unwrap();
        let r = store.code_catalog_query(&run_query(10)).unwrap();
        assert_eq!(r["total"], 2);
        assert!(r["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["metrics_available"] == false && i["metrics"].is_null()));
        assert!(!r.to_string().contains("PRIVATE_BODY"));
    }
    #[test]
    fn run_history_empty_and_persisted_views_are_read_only() {
        let (root, store) = setup();
        assert_eq!(
            store.code_catalog_query(&run_query(10)).unwrap()["total"],
            0
        );
        put(&store, "a", None, "/p/a.py", "def a(): pass");
        store.index_code().unwrap();
        let before = store.code_catalog_query(&run_query(10)).unwrap();
        assert_eq!(before["items"][0]["metrics_available"], true);
        assert_eq!(store.code_catalog_query(&run_query(10)).unwrap(), before);
        drop(store);
        let reopened = Store::open(&root.path().join("catalog.duckdb")).unwrap();
        assert_eq!(reopened.code_catalog_query(&run_query(10)).unwrap(), before);
    }
    #[test]
    fn frozen_pages_pin_revision_and_read_metadata_without_decoding_payloads() {
        let (_root, store) = setup();
        for (id, p) in [("a", "alpha"), ("b", "alpha"), ("c", "beta")] {
            put(&store, id, Some(p), "/same.py", "def original(): pass\n");
        }
        store.index_code().unwrap();
        let first = store.code_catalog_query(&q(Some("alpha"))).unwrap();
        assert_eq!(first["total"], 2);
        assert_eq!(first["items"][0]["document_id"], "a");
        put(
            &store,
            "b",
            Some("alpha"),
            "/same.py",
            "def changed(): pass\n",
        );
        store.index_code().unwrap();
        let mut page = q(Some("alpha"));
        page.cursor = Some(first["next_cursor"].as_str().unwrap().into());
        let second = store.code_catalog_query(&page).unwrap();
        assert_eq!(second["index_rev"], first["index_rev"]);
        assert_eq!(second["items"][0]["document_id"], "b");
        assert_eq!(
            second["items"][0]["content_hash"],
            content_hash("def original(): pass\n")
        );
        assert_eq!(second["has_more"], false);
        page.selection.project = Some("beta".into());
        assert!(matches!(
            store.code_catalog_query(&page),
            Err(AppError::Invalid(_))
        ));
        page.selection.project = Some("alpha".into());
        page.selection.index_revision = Some(2);
        assert!(matches!(
            store.code_catalog_query(&page),
            Err(AppError::Invalid(_))
        ));
        store
            .write()
            .unwrap()
            .update_where(
                "code_index_revision_files",
                lin::query::pred::eq("document_id", "a"),
                crate::db::lin_engine::RecBuilder::new()
                    .str("analysis_json", "broken")
                    .build(),
            )
            .unwrap();
        assert_eq!(
            store.code_catalog_query(&q(Some("alpha"))).unwrap()["items"][0]["syntax_indexed"],
            true
        );
    }
    #[test]
    fn projects_report_registered_drift_removed_and_unassigned_without_false_live_checks() {
        let (_root, store) = setup();
        put(&store, "a", Some("alpha"), "/same.py", "def a(): pass\n");
        put(&store, "n", None, "/none.txt", "unassigned");
        let mut query = CatalogQuery {
            view: CatalogView::Projects,
            ..Default::default()
        };
        let before = store.code_catalog_query(&query).unwrap();
        assert!(before["index_rev"].is_null());
        assert!(before["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["state"] == "unindexed"));
        store.index_code().unwrap();
        query.selection.project = Some("alpha".into());
        let indexed = store.code_catalog_query(&query).unwrap();
        let item = &indexed["items"][0];
        assert_eq!(item["captured_hashes_match"], true);
        assert_eq!(item["state"], "captured_match_unverified");
        assert!(item["hash_matches"].is_null());
        assert_eq!(item["filesystem_verified"], false);
        put(
            &store,
            "a",
            Some("alpha"),
            "/same.py",
            "def changed(): pass\n",
        );
        let drift = store.code_catalog_query(&query).unwrap();
        assert_eq!(drift["items"][0]["state"], "captured_drift");
        assert_eq!(drift["items"][0]["captured_hashes_match"], false);
        store
            .write()
            .unwrap()
            .update_where(
                "documents",
                lin::query::pred::eq("id", "a"),
                crate::db::lin_engine::RecBuilder::new()
                    .str("status", "archived")
                    .build(),
            )
            .unwrap();
        assert_eq!(
            store.code_catalog_query(&query).unwrap()["items"][0]["state"],
            "absent_from_current_corpus"
        );
        query.selection.project = Some("alpha' OR 1=1 --".into());
        assert_eq!(store.code_catalog_query(&query).unwrap()["total"], 0);
    }
    #[test]
    fn project_pages_distinguish_null_names_and_guard_current_observations() {
        let (_root, store) = setup();
        put(&store, "0", None, "/p/0.txt", "none");
        put(&store, "1", Some("null"), "/p/1.txt", "named null");
        put(&store, "2", Some("alpha"), "/p/2.txt", "alpha");
        store.index_code().unwrap();
        let mut query = q(None);
        query.view = CatalogView::Projects;
        let first = store.code_catalog_query(&query).unwrap();
        assert!(first["items"][0]["project"].is_null());
        query.cursor = Some(first["next_cursor"].as_str().unwrap().into());
        let next = store.code_catalog_query(&query).unwrap();
        assert_eq!(next["items"][0]["project"], "alpha");
        put(&store, "1", Some("null"), "/p/1.txt", "changed");
        assert!(matches!(
            store.code_catalog_query(&query),
            Err(AppError::Conflict(_))
        ));
    }
    #[test]
    fn preserved_markup_hashes_match_original_source_not_readable_body() {
        let (_root, store) = setup();
        let source = "<p class='actual'>Visible</p>";
        store
            .upsert_document(&Document {
                id: "html".into(),
                uri: "doc://html".into(),
                wing: Some("html".into()),
                source_file: Some("/p/a.html".into()),
                content: "Visible".into(),
                metadata_json: serde_json::json!({"code_source_text":source}).to_string(),
                ..Default::default()
            })
            .unwrap();
        store.index_code().unwrap();
        let report = store
            .code_catalog_query(&CatalogQuery {
                view: CatalogView::Projects,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            report["items"][0]["captured_hashes_match"], true,
            "{report}"
        );
        assert_eq!(report["items"][0]["source_integrity_verified"], false);
        let files = store.code_catalog_query(&q(None)).unwrap();
        assert_eq!(files["items"][0]["bytes"], source.len());
        assert_eq!(files["content_checksums_verified"], false);
        assert_eq!(files["source_text_loaded_into_rust"], false);
    }
    #[test]
    fn metadata_mentions_and_nonstring_fields_follow_source_reader_fallback() {
        let (_root, store) = setup();
        for (id, metadata) in [
            (
                "note",
                serde_json::json!({"note":"code_source_hash is reserved"}),
            ),
            ("null", serde_json::json!({"code_source_hash":null})),
        ] {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("doc://{id}"),
                    wing: Some(id.into()),
                    source_file: Some(format!("/p/{id}.py")),
                    content: "def work(): pass\n".into(),
                    metadata_json: metadata.to_string(),
                    ..Default::default()
                })
                .unwrap();
        }
        store.index_code().unwrap();
        let report = store
            .code_catalog_query(&CatalogQuery {
                view: CatalogView::Projects,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(report["total"], 2);
        assert!(
            report["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["captured_hashes_match"] == true),
            "{report}"
        );
    }
}
