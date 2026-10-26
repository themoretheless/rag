//! Stored non-code resource identities, validated against the published corpus.
use super::*;
use duckdb::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    Fresh,
    Stale,
    Unpublished,
    Unavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceTarget {
    pub document_id: String,
    pub path: String,
    pub project: Option<String>,
    pub revision: i64,
    pub content_hash: Option<String>,
    pub published_revision: Option<i64>,
    pub published_hash: Option<String>,
    pub state: TargetState,
    pub reason: String,
}
impl ResourceTarget {
    pub(super) fn frozen(
        id: &str,
        path: &str,
        project: Option<String>,
        revision: i64,
        hash: String,
    ) -> Self {
        Self {
            document_id: id.into(),
            path: path.into(),
            project,
            revision,
            content_hash: Some(hash.clone()),
            published_revision: Some(revision),
            published_hash: Some(hash),
            state: TargetState::Fresh,
            reason: "published_stored_source_identity".into(),
        }
    }
    pub(super) fn fresh(&self) -> bool {
        self.state == TargetState::Fresh
    }
}
pub(super) fn inventory(
    conn: &Connection,
    revision: Option<i64>,
) -> Result<Option<HashMap<String, ResourceTarget>>> {
    let Some(revision) = revision else {
        return Ok(None);
    };
    // Old revisions did not record attachment identity for non-code documents.
    // Neither missing/invalid inventories nor duplicate IDs qualify freshness.
    let value:Option<String>=conn.query_row("SELECT CAST(json_extract(try_cast(graph_json AS JSON),'$.file_resource_targets') AS VARCHAR) FROM code_index_revisions WHERE id=?",[revision],|r|r.get(0))?;
    let Some(value) = value else { return Ok(None) };
    let Ok(values) = serde_json::from_str::<Vec<ResourceTarget>>(&value) else {
        return Ok(None);
    };
    let mut out = HashMap::new();
    for value in values {
        if !value.fresh()
            || value.content_hash.is_none()
            || value.content_hash != value.published_hash
            || value.published_revision != Some(value.revision)
            || out.contains_key(&value.document_id)
        {
            return Ok(None);
        }
        out.insert(value.document_id.clone(), value);
    }
    Ok(Some(out))
}
// Actual table provenance is checked in addition to graph inventory metadata.
pub(super) struct Published {
    pub path: String,
    pub project: Option<String>,
    pub revision: i64,
    pub hash: String,
}
pub(super) fn observe(
    current: ResourceTarget,
    inventory: Option<&HashMap<String, ResourceTarget>>,
    published: Option<Published>,
) -> ResourceTarget {
    let mut out = current;
    let Some(hash) = out.content_hash.as_deref() else {
        out.state = TargetState::Unavailable;
        out.reason = "stored_source_unavailable_or_corrupt".into();
        return out;
    };
    let Some(inventory) = inventory else {
        out.state = TargetState::Unpublished;
        out.reason = "published_resource_inventory_unavailable".into();
        return out;
    };
    let Some(captured) = inventory.get(&out.document_id) else {
        out.state = TargetState::Unpublished;
        out.reason = "source_attachment_not_in_published_inventory".into();
        return out;
    };
    out.published_revision = captured.published_revision;
    out.published_hash = captured.published_hash.clone();
    let table_matches = published.is_some_and(|p| {
        p.path == captured.path
            && p.project == captured.project
            && p.revision == captured.revision
            && Some(&p.hash) == captured.content_hash.as_ref()
    });
    if !table_matches
        || captured.path != out.path
        || captured.project != out.project
        || captured.revision != out.revision
        || captured.content_hash.as_deref() != Some(hash)
    {
        out.state = TargetState::Stale;
        out.reason = "stored_source_or_published_identity_changed".into();
    } else {
        out.state = TargetState::Fresh;
        out.reason = "published_stored_source_identity".into();
    }
    out
}

pub(super) fn diagnostics(targets: &[ResourceTarget], limit: usize) -> serde_json::Value {
    let unavailable: Vec<_> = targets.iter().filter(|t| !t.fresh()).collect();
    serde_json::json!({"items":unavailable.iter().take(limit).collect::<Vec<_>>(),"total":unavailable.len(),"truncated":unavailable.len()>limit,"targets_total":targets.len(),"fresh_targets":targets.len()-unavailable.len(),"hash_semantics":"decoded_stored_source_text","live_filesystem_checked":false,"binary_decoding_proven":false})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    fn doc(id: &str, path: &str, project: &str, source: &str) -> Document {
        Document {
            id: id.into(),
            uri: format!("file://{project}/{id}"),
            source_file: Some(path.into()),
            wing: Some(project.into()),
            title: path.into(),
            content: source.into(),
            ..Default::default()
        }
    }
    fn corpus() -> (tempfile::TempDir, Store) {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("resources.duckdb")).unwrap();
        for d in [
            doc(
                "svg",
                "/p/icon.svg",
                "p",
                "<svg><!-- PRIVATE_ASSET --></svg>",
            ),
            doc("json", "/p/data.json", "p", "{\"ok\":true}"),
            doc("css", "/p/main.css", "p", ".a{background:url('icon.svg')}"),
            doc(
                "html",
                "/p/page.html",
                "p",
                "<link href='main.css'><object data='data.json'></object>",
            ),
            doc("unused", "/p/spare.py", "p", "def spare(): pass\n"),
        ] {
            store.upsert_document(&d).unwrap();
        }
        store.index_code().unwrap();
        (temp, store)
    }
    fn impact(store: &Store, path: &str) -> serde_json::Value {
        store.code_snapshot().unwrap().query(
            "impact",
            &CodeQuery {
                impact_file: Some(path.into()),
                impact_project: Some("p".into()),
                ..Default::default()
            },
        )
    }
    #[test]
    fn nonparsed_resources_join_file_graph_without_becoming_indexed_code() {
        let (_temp, store) = corpus();
        let snapshot = store.code_snapshot().unwrap();
        assert_eq!(snapshot.file_resource_targets.len(), 2);
        assert!(snapshot.file_resource_targets.iter().all(|t| t.fresh()));
        assert_eq!(snapshot.document_scopes.len(), 3);
        assert!(snapshot.unindexed_documents.is_empty() && snapshot.parse_errors.is_empty());
        assert!(snapshot
            .symbols
            .iter()
            .all(|s| s.document_id != "svg" && s.document_id != "json"));
        let svg = impact(&store, "/p/icon.svg");
        assert_eq!(svg["items"][0]["path"], "/p/main.css", "{svg}");
        assert_eq!(svg["items"][1]["path"], "/p/page.html");
        assert_eq!(svg["items"][1]["depth"], 2);
        assert_eq!(svg["target"]["captured_resource_target"]["state"], "fresh");
        assert!(svg["target"]["captured_resource_target"]["content_hash"].is_string());
        assert!(!svg.to_string().contains("PRIVATE_ASSET"));
        assert_eq!(
            impact(&store, "/p/data.json")["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let dead = store
            .code_dead_files(&CodeQuery {
                dead_code_project: Some("p".into()),
                ..Default::default()
            })
            .unwrap();
        for id in ["svg", "json"] {
            assert!(
                dead["protected"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["file"]["document_id"] == id),
                "{dead}"
            );
        }
    }
    #[test]
    fn changed_resource_is_unavailable_until_publication_and_blocks_negative_liveness() {
        let (_temp, store) = corpus();
        let mut svg = store.get_document("svg").unwrap().unwrap();
        svg.content = "<svg>changed</svg>".into();
        store.upsert_document(&svg).unwrap();
        let result = impact(&store, "/p/icon.svg");
        assert_eq!(result["target_found"], true);
        assert_eq!(result["target_available"], false);
        assert_eq!(result["traversal_complete"], false);
        assert!(result["items"].as_array().unwrap().is_empty());
        assert_eq!(
            result["target"]["captured_resource_target"]["state"],
            "stale"
        );
        assert_eq!(result["resource_target_diagnostics"]["total"], 1);
        // Code parser coverage retains its original semantics.
        assert!(result["stale_documents"].as_array().unwrap().is_empty());
        let dead = store
            .code_dead_files(&CodeQuery {
                dead_code_project: Some("p".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(
            dead["not_checked"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["file"]["document_id"] == "unused"),
            "{dead}"
        );
        store.index_code().unwrap();
        assert_eq!(
            impact(&store, "/p/icon.svg")["items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(!store.index_code().unwrap().revision_created);
    }
    #[test]
    fn source_attachment_identity_is_required_and_part_of_revision_fingerprint() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("attachment.duckdb")).unwrap();
        let mut data = doc("data", "/p/data.json", "p", "{}");
        data.uri = "/p/data.json".into();
        data.source_file = None;
        store.upsert_document(&data).unwrap();
        store
            .upsert_document(&doc(
                "html",
                "/p/page.html",
                "p",
                "<object data='data.json'></object>",
            ))
            .unwrap();
        store.index_code().unwrap();
        assert_eq!(impact(&store, "/p/data.json")["target_found"], false);
        // Simulate an attachment-only metadata update with unchanged revision/hash.
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE documents SET source_file='/p/data.json' WHERE id='data'",
                [],
            )
            .unwrap();
        assert_eq!(
            impact(&store, "/p/data.json")["target"]["captured_resource_target"]["state"],
            "unpublished"
        );
        assert!(store.index_code().unwrap().revision_created);
        assert_eq!(
            impact(&store, "/p/data.json")["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        store
            .lock()
            .unwrap()
            .execute("UPDATE documents SET status='archived' WHERE id='data'", [])
            .unwrap();
        assert_eq!(impact(&store, "/p/data.json")["target_found"], false);
    }
    #[test]
    fn corrupt_source_and_inventory_or_table_disagreement_never_qualify_freshness() {
        let (_temp, store) = corpus();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_revision_files SET content_hash='wrong' WHERE document_id='svg'",
                [],
            )
            .unwrap();
        assert_eq!(
            impact(&store, "/p/icon.svg")["target"]["captured_resource_target"]["state"],
            "stale"
        );
        let conn = store.lock().unwrap();
        let raw: String = conn
            .query_row(
                "SELECT graph_json FROM code_index_revisions WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut graph: serde_json::Value = serde_json::from_str(&raw).unwrap();
        graph
            .as_object_mut()
            .unwrap()
            .remove("file_resource_targets");
        conn.execute(
            "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
            [graph.to_string()],
        )
        .unwrap();
        drop(conn);
        let r = impact(&store, "/p/icon.svg");
        assert_eq!(r["resource_target_inventory_available"], false);
        assert_eq!(r["target_available"], false);
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_revision_files SET content_hash=? WHERE document_id='svg'",
                [content_hash("<svg><!-- PRIVATE_ASSET --></svg>")],
            )
            .unwrap();
        let mut data = doc("blob", "/p/blob.svg", "p", "readable");
        data.metadata_json =
            serde_json::json!({"code_source_text":"<svg>private</svg>"}).to_string();
        store.upsert_document(&data).unwrap();
        store.index_code().unwrap();
        let hash = content_hash("<svg>private</svg>");
        store
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM code_index_source_blobs WHERE content_hash=?",
                [hash],
            )
            .unwrap();
        let r = impact(&store, "/p/blob.svg");
        assert_eq!(
            r["target"]["captured_resource_target"]["state"],
            "unavailable"
        );
        assert_eq!(r["target_available"], false);
    }
    #[test]
    fn resource_identity_stays_project_scoped_and_legacy_diffs_skip_unqualified_family() {
        let (_temp, store) = corpus();
        store
            .upsert_document(&doc("foreign", "/p/icon.svg", "other", "<svg/>"))
            .unwrap();
        store.index_code().unwrap();
        assert_eq!(
            impact(&store, "/p/icon.svg")["items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let foreign = store.code_snapshot().unwrap().query(
            "impact",
            &CodeQuery {
                impact_file: Some("/p/icon.svg".into()),
                impact_project: Some("other".into()),
                ..Default::default()
            },
        );
        assert!(foreign["items"].as_array().unwrap().is_empty());
        let mut page = store.get_document("html").unwrap().unwrap();
        page.content = "<p>removed</p>".into();
        store.upsert_document(&page).unwrap();
        store.index_code().unwrap();
        let q = history::DiffQuery {
            before: 2,
            after: Some(3),
            project: Some("p".into()),
            limit: None,
        };
        let diff = store.code_index_diff(&q).unwrap();
        assert_eq!(diff["resource_target_relations_available"], true);
        let removed = diff["relations_removed"].as_array().unwrap();
        assert!(removed.iter().any(|e| e["target"] == "json"), "{diff}");
        assert!(removed.iter().any(|e| e["target"] == "css"));
        page.content = "<link href='main.css'>".into();
        store.upsert_document(&page).unwrap();
        store.index_code().unwrap();
        assert_eq!(store.code_index_diff(&q).unwrap(), diff);
        let c = store.lock().unwrap();
        let raw: String = c
            .query_row(
                "SELECT graph_json FROM code_index_revisions WHERE id=2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut graph: serde_json::Value = serde_json::from_str(&raw).unwrap();
        graph
            .as_object_mut()
            .unwrap()
            .remove("file_resource_targets");
        c.execute(
            "UPDATE code_index_revisions SET graph_json=? WHERE id=2",
            [graph.to_string()],
        )
        .unwrap();
        drop(c);
        let legacy = store.code_index_diff(&q).unwrap();
        assert_eq!(legacy["resource_target_relations_available"], false);
        let removed = legacy["relations_removed"].as_array().unwrap();
        assert!(!removed.iter().any(|e| e["target"] == "json"), "{legacy}");
        assert!(removed.iter().any(|e| e["target"] == "css"));
    }
}
