use super::*;
use crate::models::Document;
fn setup() -> (tempfile::TempDir, Store) {
    crate::code_index::source::install_document_write_hook();
    let root = tempfile::tempdir().unwrap();
    let store = crate::testkit::open_test_store(&root.path().join("catalog.duckdb")).unwrap();
    (root, store)
}
fn put(store: &Store, id: &str, project: Option<&str>, path: &str, content: &str) {
    store
        .documents()
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
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let mut q = run_query(1);
    let first = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&q)
        .unwrap();
    assert_eq!(first["total"], 3);
    assert_eq!(first["items"][0]["seq"], 3);
    put(&store, "a", Some("p"), "/p/a.py", "def changed(): pass");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    q.cursor = first["next_cursor"].as_str().map(str::to_owned);
    q.selection.limit = Some(10);
    let second = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&q)
        .unwrap();
    assert_eq!(second["total"], 3);
    assert_eq!(second["items"].as_array().unwrap().len(), 2);
    assert_eq!(second["items"][0]["seq"], 2);
    assert_eq!(second["has_more"], false);
    let all = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&run_query(10))
        .unwrap();
    assert_eq!(all["total"], 4);
    assert_eq!(all["items"][0]["index_rev"], 2);
    let mut filtered = run_query(10);
    filtered.selection.index_revision = Some(1);
    let r = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&filtered)
        .unwrap();
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
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let mut q = run_query(1);
    let first = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&q)
        .unwrap();
    q.cursor = first["next_cursor"].as_str().map(str::to_owned);
    q.selection.index_revision = Some(1);
    assert!(crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&q)
        .is_err());
    q.selection.index_revision = None;
    q.selection.project = Some("p".into());
    assert!(crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&q)
        .is_err());
    q.selection.project = None;
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE code_index_runs SET run_id='replaced' WHERE seq=2",
            [],
        )
        .unwrap();
    assert!(crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&q)
        .is_err());
}
#[test]
fn run_history_reports_invalid_and_oversized_metrics_without_decoding_bodies() {
    let (_root, store) = setup();
    put(&store, "a", None, "/p/a.py", "def a(): pass");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
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
    let r = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&run_query(10))
        .unwrap();
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
        crate::code_index::CodeIndex::new(&store)
            .code_catalog_query(&run_query(10))
            .unwrap()["total"],
        0
    );
    put(&store, "a", None, "/p/a.py", "def a(): pass");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let before = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&run_query(10))
        .unwrap();
    assert_eq!(before["items"][0]["metrics_available"], true);
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_catalog_query(&run_query(10))
            .unwrap(),
        before
    );
    drop(store);
    let reopened = crate::testkit::open_test_store(&root.path().join("catalog.duckdb")).unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&reopened)
            .code_catalog_query(&run_query(10))
            .unwrap(),
        before
    );
}
#[test]
fn frozen_pages_pin_revision_and_read_metadata_without_decoding_payloads() {
    let (_root, store) = setup();
    for (id, p) in [("a", "alpha"), ("b", "alpha"), ("c", "beta")] {
        put(&store, id, Some(p), "/same.py", "def original(): pass\n");
    }
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let first = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&q(Some("alpha")))
        .unwrap();
    assert_eq!(first["total"], 2);
    assert_eq!(first["items"][0]["document_id"], "a");
    put(
        &store,
        "b",
        Some("alpha"),
        "/same.py",
        "def changed(): pass\n",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let mut page = q(Some("alpha"));
    page.cursor = Some(first["next_cursor"].as_str().unwrap().into());
    let second = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&page)
        .unwrap();
    assert_eq!(second["index_rev"], first["index_rev"]);
    assert_eq!(second["items"][0]["document_id"], "b");
    assert_eq!(
        second["items"][0]["content_hash"],
        content_hash("def original(): pass\n")
    );
    assert_eq!(second["has_more"], false);
    page.selection.project = Some("beta".into());
    assert!(matches!(
        crate::code_index::CodeIndex::new(&store).code_catalog_query(&page),
        Err(AppError::Invalid(_))
    ));
    page.selection.project = Some("alpha".into());
    page.selection.index_revision = Some(2);
    assert!(matches!(
        crate::code_index::CodeIndex::new(&store).code_catalog_query(&page),
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
        crate::code_index::CodeIndex::new(&store)
            .code_catalog_query(&q(Some("alpha")))
            .unwrap()["items"][0]["syntax_indexed"],
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
    let before = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&query)
        .unwrap();
    assert!(before["index_rev"].is_null());
    assert!(before["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["state"] == "unindexed"));
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    query.selection.project = Some("alpha".into());
    let indexed = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&query)
        .unwrap();
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
    let drift = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&query)
        .unwrap();
    assert_eq!(drift["items"][0]["state"], "captured_drift");
    assert_eq!(drift["items"][0]["captured_hashes_match"], false);
    store
        .lock()
        .unwrap()
        .execute("UPDATE documents SET status='archived' WHERE id='a'", [])
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_catalog_query(&query)
            .unwrap()["items"][0]["state"],
        "absent_from_current_corpus"
    );
    query.selection.project = Some("alpha' OR 1=1 --".into());
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_catalog_query(&query)
            .unwrap()["total"],
        0
    );
}
#[test]
fn project_pages_distinguish_null_names_and_guard_current_observations() {
    let (_root, store) = setup();
    put(&store, "0", None, "/p/0.txt", "none");
    put(&store, "1", Some("null"), "/p/1.txt", "named null");
    put(&store, "2", Some("alpha"), "/p/2.txt", "alpha");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let mut query = q(None);
    query.view = CatalogView::Projects;
    let first = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&query)
        .unwrap();
    assert!(first["items"][0]["project"].is_null());
    query.cursor = Some(first["next_cursor"].as_str().unwrap().into());
    let next = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&query)
        .unwrap();
    assert_eq!(next["items"][0]["project"], "alpha");
    put(&store, "1", Some("null"), "/p/1.txt", "changed");
    assert!(matches!(
        crate::code_index::CodeIndex::new(&store).code_catalog_query(&query),
        Err(AppError::Conflict(_))
    ));
}
#[test]
fn preserved_markup_hashes_match_original_source_not_readable_body() {
    let (_root, store) = setup();
    let source = "<p class='actual'>Visible</p>";
    store
        .documents()
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
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let report = crate::code_index::CodeIndex::new(&store)
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
    let files = crate::code_index::CodeIndex::new(&store)
        .code_catalog_query(&q(None))
        .unwrap();
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
            .documents()
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
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let report = crate::code_index::CodeIndex::new(&store)
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
