use super::*;
use crate::models::Document;
fn doc(id: &str, path: &str, source: &str) -> Document {
    Document {
        id: id.into(),
        uri: format!("file://{path}"),
        source_file: Some(path.into()),
        content: source.into(),
        title: path.into(),
        ..Default::default()
    }
}
#[test]
fn frozen_method_relations_diff_and_legacy_missing_coverage_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("overrides.duckdb")).unwrap();
    let mut d = doc(
        "a",
        "/p/models.py",
        "class Base:\n    def work(self): pass\nclass Child(Base):\n    def work(self): pass\n",
    );
    store.documents().upsert_document(&d).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert_eq!(
        frozen_graph(&store.lock().unwrap(), 1).unwrap()["overrides"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    d.content = "class Base:\n    def work(self): pass\nclass Child(Base): pass\n".into();
    store.documents().upsert_document(&d).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let q = DiffQuery {
        before: 1,
        after: Some(2),
        project: None,
        limit: None,
    };
    let result = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&q)
        .unwrap();
    assert_eq!(result["method_relations_available"], true);
    assert!(result["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "method_override_candidate"));
    let mut graph = frozen_graph(&store.lock().unwrap(), 1).unwrap();
    graph.as_object_mut().unwrap().remove("overrides");
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
            [graph.to_string()],
        )
        .unwrap();
    let legacy = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&q)
        .unwrap();
    assert_eq!(legacy["method_relations_available"], false);
    assert!(!legacy["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "method_override_candidate"));
}
#[test]
fn translation_key_history_ignores_offsets_but_keeps_counts_and_legacy_gaps() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("key-history.duckdb")).unwrap();
    let mut view = doc("view", "/p/view.html", "%{key} %{key}");
    view.wing = Some("p".into());
    let mut code = doc(
        "code",
        "/p/render.php",
        "<?php function render(){Translate('key');}",
    );
    code.wing = Some("p".into());
    for d in [&view, &code] {
        store.documents().upsert_document(d).unwrap();
    }
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let before = frozen_graph(&store.lock().unwrap(), 1).unwrap();
    view.content = format!("\n\n{}", view.content);
    code.content = format!("\n{}", code.content);
    for d in [&view, &code] {
        store.documents().upsert_document(d).unwrap();
    }
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let diff = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&DiffQuery {
            before: 1,
            after: Some(2),
            project: Some("p".into()),
            limit: None,
        })
        .unwrap();
    assert_eq!(diff["translation_key_relations_available"], true);
    assert!(
        !diff["relations_added"]
            .as_array()
            .unwrap()
            .iter()
            .chain(diff["relations_removed"].as_array().unwrap())
            .any(|r| r["kind"] == "uses_key"),
        "{diff}"
    );
    view.content = "%{key}".into();
    store.documents().upsert_document(&view).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let q = DiffQuery {
        before: 2,
        after: Some(3),
        project: Some("p".into()),
        limit: None,
    };
    let changed = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&q)
        .unwrap();
    assert!(changed["relations_added"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["kind"] == "uses_key" && r["occurrences"] == 1));
    assert!(changed["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["kind"] == "uses_key" && r["occurrences"] == 2));
    let mut legacy = frozen_graph(&store.lock().unwrap(), 2).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("translation_keys_format");
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE code_index_revisions SET graph_json=? WHERE id=2",
            [legacy.to_string()],
        )
        .unwrap();
    let unknown = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&q)
        .unwrap();
    assert_eq!(unknown["translation_key_relations_available"], false);
    assert!(!unknown["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["kind"] == "uses_key"));
    assert_eq!(frozen_graph(&store.lock().unwrap(), 1).unwrap(), before);
}
#[test]
fn limited_translation_captures_are_not_compared_as_complete_negative_facts() {
    let file = File {
        id: "a".into(),
        path: "/p/view.html".into(),
        project: Some("p".into()),
        revision: 1,
        hash: "hash".into(),
        source: String::new(),
        analysis: None,
    };
    let before = serde_json::json!({"translation_keys_format":1,"translation_keys":{"coverage":[{"document_id":"a","complete":true}]}});
    let after = serde_json::json!({"translation_keys_format":1,"translation_keys":{"coverage":[{"document_id":"a","complete":false}]}});
    let (eligible, report) = translation_comparison(
        &before,
        &after,
        std::slice::from_ref(&file),
        std::slice::from_ref(&file),
        10,
    );
    assert!(eligible.is_empty());
    assert_eq!(report["excluded_documents_total"], 1);
    assert_eq!(report["complete_for_supported_documents"], false);
    let (eligible, _) =
        translation_comparison(&before, &before, std::slice::from_ref(&file), &[], 10);
    assert!(eligible.contains("a"));
}
#[test]
fn frozen_schema_and_relations_diff_ignore_line_moves_and_project_diagnostics() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("semantic.duckdb")).unwrap();
    let mut migration = doc(
        "ddl",
        "/p/migrations/001.sql",
        "CREATE TABLE users(name TEXT NOT NULL);",
    );
    migration.wing = Some("p".into());
    let mut code = doc("code","/p/models.py","class Base: pass\nclass Other: pass\nclass Child(Base): pass\ndef work(): pass\nregistered = work\ndef test_old(): pass\n");
    code.wing = Some("p".into());
    let mut foreign = doc(
        "foreign",
        "/q/migrations/001.sql",
        "ALTER TABLE unknown ADD COLUMN x INT;",
    );
    foreign.wing = Some("q".into());
    for d in [&migration, &code, &foreign] {
        store.documents().upsert_document(d).unwrap();
    }
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let frozen = frozen_graph(&store.lock().unwrap(), 1).unwrap();
    assert!(frozen["sql"]["tables"].is_array());
    migration.content = "CREATE TABLE users(name TEXT NOT NULL, age INT NOT NULL);".into();
    code.content="class Base: pass\nclass Other: pass\nclass Child(Other): pass\ndef work(): pass\nregistered = None\ndef plain(): pass\n".into();
    store.documents().upsert_document(&migration).unwrap();
    store.documents().upsert_document(&code).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let q = DiffQuery {
        before: 1,
        after: Some(2),
        project: Some("p".into()),
        limit: None,
    };
    let result = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&q)
        .unwrap();
    assert_eq!(result["schema_diff"]["total"], 1, "{result}");
    assert_eq!(
        result["schema_diff"]["items"][0]["column_changes"][0]["column"],
        "age"
    );
    assert_eq!(
        result["schema_diff"]["after_diagnostics"],
        serde_json::json!([])
    );
    let added = result["relations_added"].as_array().unwrap();
    let removed = result["relations_removed"].as_array().unwrap();
    assert!(added.iter().any(|e| e["kind"] == "inheritance"));
    assert!(removed.iter().any(|e| e["kind"] == "non_call_reference"));
    assert!(removed.iter().any(|e| e["kind"] == "entrypoint"));
    assert_eq!(frozen_graph(&store.lock().unwrap(), 1).unwrap(), frozen);
    code.content = format!("# moved\n{}", code.content);
    migration.content = format!("-- moved\n{}", migration.content);
    store.documents().upsert_document(&migration).unwrap();
    store.documents().upsert_document(&code).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let moved = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&DiffQuery {
            before: 2,
            after: Some(3),
            ..q
        })
        .unwrap();
    assert_eq!(moved["schema_diff"]["total"], 0, "{moved}");
    assert_eq!(moved["relations_added_total"], 0);
    assert_eq!(moved["relations_removed_total"], 0);
}
#[test]
fn schema_diff_captures_expression_only_changes_and_marks_legacy_coverage() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("expressions.duckdb")).unwrap();
    let mut migration = doc(
        "ddl",
        "/p/migrations/001.sql",
        "CREATE TABLE calc(a INT DEFAULT 1, b INT GENERATED ALWAYS AS (a + 1) STORED);",
    );
    store.documents().upsert_document(&migration).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let before = frozen_graph(&store.lock().unwrap(), 1).unwrap();
    migration.content =
        "CREATE TABLE calc(a INT DEFAULT 2, b INT GENERATED ALWAYS AS (a + 2) STORED);".into();
    store.documents().upsert_document(&migration).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let after = frozen_graph(&store.lock().unwrap(), 2).unwrap();
    let diff = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&DiffQuery {
            before: 1,
            after: Some(2),
            project: None,
            limit: None,
        })
        .unwrap();
    assert_eq!(diff["schema_diff"]["expression_changes_available"], true);
    assert_eq!(
        diff["schema_diff"]["items"][0]["column_changes_total"], 2,
        "{diff}"
    );
    let mut legacy = before.clone();
    legacy["format_version"] = 18.into();
    let compatibility = schema_changes(&legacy, &after, None, 100);
    assert_eq!(compatibility["expression_changes_available"], false);
    assert_eq!(compatibility["total"], 0, "{compatibility}");
    assert_eq!(frozen_graph(&store.lock().unwrap(), 1).unwrap(), before);
}

#[test]
fn frozen_constraints_diff_and_legacy_coverage_are_explicit() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("constraints.duckdb")).unwrap();
    let mut d = doc(
        "ddl",
        "/p/migrations/001.sql",
        "CREATE TABLE x(a INT CONSTRAINT positive CHECK(a>0), CONSTRAINT uq UNIQUE(a));",
    );
    store.documents().upsert_document(&d).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let before = frozen_graph(&store.lock().unwrap(), 1).unwrap();
    d.content="CREATE TABLE x(a INT CONSTRAINT positive CHECK(a>1), CONSTRAINT uq UNIQUE(a), CONSTRAINT ceiling CHECK(a<100));".into();
    store.documents().upsert_document(&d).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let after = frozen_graph(&store.lock().unwrap(), 2).unwrap();
    let result = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&DiffQuery {
            before: 1,
            after: Some(2),
            project: None,
            limit: Some(1),
        })
        .unwrap();
    assert_eq!(result["schema_diff"]["constraint_changes_available"], true);
    assert_eq!(result["schema_diff"]["total"], 1);
    assert_eq!(result["schema_diff"]["items"][0]["column_changes_total"], 1);
    assert_eq!(
        result["schema_diff"]["items"][0]["constraints_truncated"],
        true
    );
    assert_eq!(result["truncated"], true);
    let mut legacy = before;
    legacy["format_version"] = 19.into();
    let compatibility = schema_changes(&legacy, &after, None, 100);
    assert_eq!(compatibility["constraint_changes_available"], false);
    assert_eq!(compatibility["total"], 0, "{compatibility}");
}

#[test]
fn missing_historical_schema_does_not_invent_a_replayed_snapshot() {
    let result = schema_changes(
        &serde_json::json!({}),
        &serde_json::json!({"sql":{"tables":[]}}),
        None,
        1,
    );
    assert_eq!(result["available"], false);
    assert_eq!(result["replay_performed"], false);
}

#[test]
fn immutable_history_reuses_noop_and_content_blobs_and_diffs_semantic_links() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.duckdb");
    let store = Store::open(&path).unwrap();
    store
        .documents()
        .upsert_document(&doc(
            "a",
            "/p/a.rs",
            "fn main() { foo(); let callback = foo; } fn foo() {}",
        ))
        .unwrap();
    store
        .documents()
        .upsert_document(&doc("text", "/p/readme.md", "unchanged text"))
        .unwrap();
    let first = crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert_eq!(first.index_revision, Some(1));
    assert!(first.revision_created);
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_snapshot()
            .unwrap()
            .index_revision,
        Some(1)
    );
    let graph: String = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT graph_json FROM code_index_revisions WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let frozen: serde_json::Value = serde_json::from_str(&graph).unwrap();
    assert_eq!(frozen["format_version"], INDEX_FORMAT_VERSION);
    assert!(frozen["references"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["reference"]["name"] == "foo"
            && r["reference"]["is_call"] == false
            && r["resolution"] == "resolved_static"));
    let warm = crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert_eq!(warm.index_revision, Some(1));
    assert!(!warm.revision_created);
    let mut a = store.documents().get_document("a").unwrap().unwrap();
    a.content =
        "// moved definitions\nfn main() { bar(); let secret_value=1; } fn foo() {} fn bar() {}"
            .into();
    store.documents().upsert_document(&a).unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap()
            .index_revision,
        Some(2)
    );
    let diff = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&DiffQuery {
            before: 1,
            after: None,
            project: None,
            limit: None,
        })
        .unwrap();
    assert_eq!(diff["file_changes_total"], 1);
    assert_eq!(diff["symbol_changes_total"], 2);
    assert_eq!(diff["edges_added_total"], 1);
    assert_eq!(diff["edges_removed_total"], 1);
    assert!(!diff.to_string().contains("secret_value"));
    assert_eq!(
        store
            .lock()
            .unwrap()
            .query_row(
                "SELECT graph_json FROM code_index_revisions WHERE id=1",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        graph
    );
    let blobs: i64 = store
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM code_index_source_blobs", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(blobs, 3);
    let page = crate::code_index::CodeIndex::new(&store)
        .code_index_history(&HistoryQuery {
            limit: Some(1),
            before_revision: None,
        })
        .unwrap();
    assert_eq!(page["items"][0]["index_rev"], 2);
    assert_eq!(page["has_more"], true);
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_index_history(&HistoryQuery {
                limit: Some(1),
                before_revision: Some(2)
            })
            .unwrap()["items"][0]["index_rev"],
        1
    );
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_index_diff(&DiffQuery {
                before: 1,
                after: Some(2),
                project: None,
                limit: Some(1)
            })
            .unwrap()["truncated"],
        true
    );
}
#[test]
fn deleting_every_document_makes_a_new_chronological_empty_revision() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("history.duckdb")).unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap()
            .index_revision,
        Some(1)
    );
    store
        .documents()
        .upsert_document(&doc("a", "/p/a.rs", "fn a() {}"))
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap()
            .index_revision,
        Some(2)
    );
    store.documents().delete_document("a").unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap()
            .index_revision,
        Some(3)
    );
    let diff = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&DiffQuery {
            before: 2,
            after: Some(3),
            project: None,
            limit: None,
        })
        .unwrap();
    assert_eq!(diff["files"][0]["kind"], "removed");
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap()
            .index_revision,
        Some(3)
    );
    assert!(crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&DiffQuery {
            before: 999,
            after: None,
            project: None,
            limit: None
        })
        .is_err());
}
#[test]
fn late_publication_failure_rolls_back_current_cache_and_revision_together() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("history.duckdb")).unwrap();
    store
        .documents()
        .upsert_document(&doc("a", "/p/a.rs", "fn old() {}"))
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let old_payload: String = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT payload FROM code_index_documents WHERE document_id='a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    store.lock().unwrap().execute_batch("ALTER TABLE code_index_source_blobs RENAME TO original_blobs; CREATE TABLE code_index_source_blobs(content_hash VARCHAR PRIMARY KEY,content VARCHAR NOT NULL CHECK(content NOT LIKE '%new%')); INSERT INTO code_index_source_blobs SELECT * FROM original_blobs; DROP TABLE original_blobs;").unwrap();
    let mut a = store.documents().get_document("a").unwrap().unwrap();
    a.content = "fn new() {}".into();
    store.documents().upsert_document(&a).unwrap();
    assert!(crate::code_index::CodeIndex::new(&store)
        .index_code()
        .is_err());
    assert_eq!(
        store
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM code_index_documents WHERE document_id='a'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        old_payload
    );
    let history = crate::code_index::CodeIndex::new(&store)
        .code_index_history(&HistoryQuery::default())
        .unwrap();
    assert_eq!(history["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_snapshot()
            .unwrap()
            .stale_documents,
        vec!["a"]
    );
}
#[test]
fn client_endpoints_history_is_frozen_and_guards_legacy_capture() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("clients.duckdb")).unwrap();
    store
        .documents()
        .upsert_document(&doc(
            "client",
            "/p/client.js",
            "$.ajax({url:'/target',method:'GET'});",
        ))
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    store
        .documents()
        .upsert_document(&doc(
            "client",
            "/p/client.js",
            "$.ajax({url:'/target',method:'POST'});",
        ))
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let q = DiffQuery {
        before: 1,
        after: Some(2),
        project: None,
        limit: None,
    };
    let diff = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&q)
        .unwrap();
    assert_eq!(diff["client_endpoint_relations_available"], true);
    assert!(diff["relations_added"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["reference_kind"] == "js:jquery:ajax" && r["request_method"] == "POST"));
    assert!(diff["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["request_method"] == "GET"));
    let mut graph = frozen_graph(&store.lock().unwrap(), 1).unwrap();
    graph
        .as_object_mut()
        .unwrap()
        .remove("client_endpoints_format");
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
            [graph.to_string()],
        )
        .unwrap();
    let legacy = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&q)
        .unwrap();
    assert_eq!(legacy["client_endpoint_relations_available"], false);
    assert!(!legacy["relations_added"]
        .as_array()
        .unwrap()
        .iter()
        .chain(legacy["relations_removed"].as_array().unwrap())
        .any(|r| r["reference_kind"] == "js:jquery:ajax"));
}
