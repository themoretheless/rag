//! Lean frozen manifests and captured project registry; never scans live files.
use super::*;
use duckdb::{Connection, OptionalExt};
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
fn revision(conn: &Connection, q: &CatalogQuery, cursor: Option<&Cursor>) -> Result<Option<i64>> {
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
        conn.query_row("SELECT MAX(id) FROM code_index_revisions", [], |r| r.get(0))?
    };
    if let Some(id) = selected {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM code_index_revisions WHERE id=?",
            [id],
            |r| r.get(0),
        )?;
        if exists != 1 {
            return Err(AppError::not_found(format!("unknown index revision {id}")));
        }
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
    conn: &Connection,
    q: &CatalogQuery,
    rev: Option<i64>,
    previous: Option<&Cursor>,
) -> Result<serde_json::Value> {
    let rev = rev.ok_or_else(|| AppError::not_found("run index_code or sync_sources first"))?;
    let filter="f.index_revision=? AND (CAST(? AS VARCHAR) IS NULL OR f.project=?) AND (CAST(? AS VARCHAR) IS NULL OR f.document_id=?) AND (CAST(? AS VARCHAR) IS NULL OR f.path=?)";
    let s = &q.selection;
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM code_index_revision_files f WHERE {filter}"),
        params![
            rev,
            s.project,
            s.project,
            s.document_id,
            s.document_id,
            s.path,
            s.path
        ],
        |r| r.get(0),
    )?;
    let after = previous.map(|c| c.after.as_str());
    let limit = s.limit.unwrap_or(100).clamp(1, 1000);
    let sql=format!("SELECT f.document_id,f.path,f.project,f.document_revision,f.content_hash,strlen(b.content),f.analysis_json IS NOT NULL,b.content IS NOT NULL FROM code_index_revision_files f LEFT JOIN code_index_source_blobs b ON b.content_hash=f.content_hash WHERE {filter} AND (CAST(? AS VARCHAR) IS NULL OR f.document_id>?) ORDER BY f.document_id LIMIT ?");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![
        rev,
        s.project,
        s.project,
        s.document_id,
        s.document_id,
        s.path,
        s.path,
        after,
        after,
        (limit + 1) as i64
    ])?;
    let mut items = Vec::new();
    while let Some(r) = rows.next()? {
        items.push(serde_json::json!({"document_id":r.get::<_,String>(0)?,"path":r.get::<_,String>(1)?,"project":r.get::<_,Option<String>>(2)?,"document_revision":r.get::<_,i64>(3)?,"content_hash":r.get::<_,String>(4)?,"bytes":r.get::<_,Option<i64>>(5)?,"syntax_indexed":r.get::<_,bool>(6)?,"source_available":r.get::<_,bool>(7)?}));
    }
    let more = items.len() > limit;
    items.truncate(limit);
    let next = if more {
        Some(cursor(
            q,
            Some(rev),
            items.last().unwrap()["document_id"]
                .as_str()
                .unwrap()
                .into(),
            None,
        )?)
    } else {
        None
    };
    Ok(
        serde_json::json!({"index_rev":rev,"items":items,"total":total,"truncated":more||previous.is_some(),"has_more":more,"next_cursor":next,"view":"immutable_revision_manifest","query_contract_version":1,"content_checksums_verified":false,"source_text_loaded_into_rust":false,"observation_consistency":"single_read_transaction"}),
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
    conn: &Connection,
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
    {
        let mut stmt=conn.prepare("SELECT id,COALESCE(source_file,uri),wing,content_hash,metadata_json FROM documents WHERE COALESCE(status,'active') NOT IN ('archived','tombstone','consolidated') AND (CAST(? AS VARCHAR) IS NULL OR wing=?) ORDER BY id")?;
        let mut rows = stmt.query(params![project, project])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let path: String = r.get(1)?;
            let wing: Option<String> = r.get(2)?;
            let declared: Option<String> = r.get(3)?;
            let meta: String = r.get(4)?;
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
        let mut stmt=conn.prepare("SELECT document_id,path,project,content_hash,analysis_json IS NOT NULL FROM code_index_revision_files WHERE index_revision=? AND (CAST(? AS VARCHAR) IS NULL OR project=?) ORDER BY document_id")?;
        let mut rows = stmt.query(params![rev, project, project])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let path: String = r.get(1)?;
            let wing: Option<String> = r.get(2)?;
            let hash: String = r.get(3)?;
            let g = group(&mut indexed, wing);
            add(g, &id, &path, Some(&hash))?;
            g.syntax += usize::from(r.get::<_, bool>(4)?);
        }
    }
    {
        let mut stmt=conn.prepare("SELECT d.wing,sm.canonical_root,COUNT(*),CAST(MAX(sm.last_seen) AS VARCHAR),CAST(MAX(sm.last_synced) AS VARCHAR) FROM source_manifest sm JOIN documents d ON d.id=sm.document_id WHERE COALESCE(d.status,'active') NOT IN ('archived','tombstone','consolidated') AND (CAST(? AS VARCHAR) IS NULL OR d.wing=?) GROUP BY d.wing,sm.canonical_root ORDER BY d.wing NULLS FIRST,sm.canonical_root")?;
        let mut rows = stmt.query(params![project, project])?;
        while let Some(r) = rows.next()? {
            let wing: Option<String> = r.get(0)?;
            group(&mut current,wing).roots.push(serde_json::json!({"canonical_root":r.get::<_,String>(1)?,"manifest_files":r.get::<_,i64>(2)?,"last_seen":r.get::<_,Option<String>>(3)?,"last_synced":r.get::<_,Option<String>>(4)?}));
        }
    }
    let published: Option<String> = if let Some(rev) = rev {
        conn.query_row(
            "SELECT CAST(created_at AS VARCHAR) FROM code_index_revisions WHERE id=?",
            [rev],
            |r| r.get(0),
        )
        .optional()?
        .flatten()
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
    conn: &Connection,
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
    if let Some(id) = revision {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM code_index_revisions WHERE id=?",
            [id],
            |r| r.get(0),
        )?;
        if exists != 1 {
            return Err(AppError::not_found("unknown index revision"));
        }
    }
    let anchor = if let Some(c) = previous {
        let a: RunAnchor = serde_json::from_str(c.observation.as_deref().unwrap_or(""))
            .map_err(|_| AppError::Invalid("invalid run cursor anchor".into()))?;
        let id: Option<String> = conn
            .query_row(
                "SELECT run_id FROM code_index_runs WHERE seq=?",
                [a.seq],
                |r| r.get(0),
            )
            .optional()?;
        if a.seq <= 0 || a.run_id.is_none() || id != a.run_id {
            return Err(AppError::Invalid(
                "run history anchor changed or is unavailable".into(),
            ));
        }
        a
    } else {
        let row: Option<(i64, String)> = conn
            .query_row(
                "SELECT seq,run_id FROM code_index_runs ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        RunAnchor {
            seq: row.as_ref().map_or(0, |r| r.0),
            run_id: row.map(|r| r.1),
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
        return Err(AppError::Invalid("invalid run cursor range".into()));
    }
    let total:i64=conn.query_row("SELECT COUNT(*) FROM code_index_runs WHERE seq<=? AND (CAST(? AS BIGINT) IS NULL OR index_revision=?)",params![anchor.seq,revision,revision],|r|r.get(0))?;
    let limit = s.limit.unwrap_or(50).clamp(1, 500);
    let mut stmt=conn.prepare("SELECT seq,run_id,CAST(recorded_at AS VARCHAR),index_revision,revision_created,CASE WHEN octet_length(encode(metrics_json))<=16384 THEN metrics_json ELSE NULL END FROM code_index_runs WHERE seq<=? AND (CAST(? AS BIGINT) IS NULL OR index_revision=?) AND (CAST(? AS BIGINT) IS NULL OR seq<?) ORDER BY seq DESC LIMIT ?")?;
    let rows = stmt.query_map(
        params![
            anchor.seq,
            revision,
            revision,
            after,
            after,
            (limit + 1) as i64
        ],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, bool>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        },
    )?;
    let mut rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    let more = rows.len() > limit;
    rows.truncate(limit);
    let items:Vec<_>=rows.iter().map(|(seq,id,at,rev,created,json)|{
        let decoded=json.as_deref().and_then(|j|timings::decode(*seq,id,at,*created,*rev,j).ok());
        decoded.map(|mut row|{row["metrics_available"]=serde_json::json!(true);row}).unwrap_or_else(||serde_json::json!({"seq":seq,"run_id":id,"recorded_at":at,"index_rev":rev,"revision_created":created,"metrics":null,"metrics_available":false,"reason":"invalid_or_oversized_run_metrics"}))
    }).collect();
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
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        if q.view == CatalogView::Runs {
            let result = runs(&transaction, q, previous.as_ref())?;
            transaction.commit()?;
            return Ok(result);
        }
        let rev = revision(&transaction, q, previous.as_ref())?;
        let result = match q.view {
            CatalogView::Files => files(&transaction, q, rev, previous.as_ref())?,
            CatalogView::Projects => projects(&transaction, q, rev, previous.as_ref())?,
            CatalogView::Runs => unreachable!(),
        };
        transaction.commit()?;
        Ok(result)
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
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_runs SET run_id='replaced' WHERE seq=2",
                [],
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
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_runs SET metrics_json=? WHERE seq=1",
                ["PRIVATE_BODY".repeat(2000)],
            )
            .unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_runs SET metrics_json='{}' WHERE seq=2",
                [],
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
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_revision_files SET analysis_json='broken' WHERE document_id='a'",
                [],
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
            .lock()
            .unwrap()
            .execute("UPDATE documents SET status='archived' WHERE id='a'", [])
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
