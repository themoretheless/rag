use super::*;
use crate::models::{Chunk, Document};

fn setup() -> (tempfile::TempDir, Store, Config) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("status.duckdb");
    let store = crate::testkit::open_test_store(&path).unwrap();
    let config = Config {
        db_path: path,
        embedding_dims: 2,
        llm_enabled: false,
        ..Config::for_tests()
    };
    (root, store, config)
}
fn put(store: &Store, id: &str, project: &str, path: &str, text: &str) {
    store
        .documents()
        .upsert_document(&Document {
            id: id.into(),
            uri: format!("doc://{id}"),
            source_file: Some(path.into()),
            wing: Some(project.into()),
            content: text.into(),
            ..Default::default()
        })
        .unwrap();
}
fn chunk(store: &Store, id: &str, document: &str, content: &str, embedding: Vec<f32>) {
    store
        .chunks()
        .insert_chunks(&[Chunk {
            id: id.into(),
            document_id: document.into(),
            chunk_index: 0,
            content: content.into(),
            embedding,
            char_start: 0,
            char_end: content.chars().count() as i32,
            metadata_json: "{}".into(),
        }])
        .unwrap();
}
#[test]
fn absent_empty_and_uninspected_are_never_healthy() {
    let (_root, store, config) = setup();
    let status = crate::code_index::CodeIndex::new(&store)
        .code_index_status(&StatusQuery::default(), &config)
        .unwrap();
    assert_eq!(status["contract_version"], 1);
    assert_eq!(status["state"], "not_indexed");
    assert_eq!(status["quality"]["healthy"], false);
    assert_eq!(
        status["quality"]["chunks"]["norm_p50"],
        serde_json::Value::Null
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let status = crate::code_index::CodeIndex::new(&store)
        .code_index_status(&StatusQuery::default(), &config)
        .unwrap();
    assert_eq!(status["state"], "empty_scope");
    assert_eq!(status["quality"]["healthy"], false);
    put(&store, "a", "a", "/p/a.py", "def work(): pass\n");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let status = crate::code_index::CodeIndex::new(&store)
        .code_index_status(&StatusQuery::default(), &config)
        .unwrap();
    assert_eq!(status["state"], "observed_checks_passed");
    assert!(status["quality"]["healthy"].is_null());
    assert_eq!(status["quality"]["chunks"]["sampled"], 0);
    assert_eq!(status["quality"]["exhaustive"], false);
    assert_eq!(status["capabilities"]["cpg"]["available"], false);
}
#[test]
fn measured_vectors_and_tokens_are_scoped_bounded_and_do_not_repair() {
    let (_root, store, config) = setup();
    put(&store, "a", "a", "/p/a.py", "def work(): pass\n");
    put(&store, "b", "b", "/p/b.py", "def foreign(): pass\n");
    store
        .embedding_manifest()
        .write_embedding_manifest_from_config(&config)
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    chunk(
        &store,
        &"a".repeat(256),
        "a",
        &"word ".repeat(200),
        vec![0.0, 0.0],
    );
    chunk(&store, "b-size", "a", "short", vec![1.0]);
    chunk(&store, "c-invalid", "a", "short", vec![1.0, 0.0]);
    chunk(&store, "z-foreign", "b", "foreign", vec![0.0, 0.0]);
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE chunks SET embedding=[NULL]::FLOAT[] WHERE id='c-invalid'",
            [],
        )
        .unwrap();
    let q = StatusQuery {
        project: Some("a".into()),
        limit: Some(1),
        max_chunk_tokens: Some(8),
        ..Default::default()
    };
    let status = crate::code_index::CodeIndex::new(&store)
        .code_index_status(&q, &config)
        .unwrap();
    let quality = &status["quality"]["chunks"];
    assert_eq!(quality["total"], 3);
    assert_eq!(quality["sampled"], 3);
    assert_eq!(quality["zero_vectors"], 1);
    assert_eq!(quality["dimension_mismatches"], 1);
    assert_eq!(quality["invalid_vectors"], 1);
    assert_eq!(quality["oversized_estimated_tokens"], 1);
    assert_eq!(quality["examples"].as_array().unwrap().len(), 1);
    assert_eq!(quality["examples_truncated"], true);
    assert_eq!(
        quality["examples"][0]["chunk_id"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        128
    );
    assert_eq!(quality["examples"][0]["chunk_id_truncated"], true);
    assert_eq!(status["quality"]["healthy"], false);
    assert_eq!(
        status["quality"]["embedding_identity"]["matches_config"],
        true
    );
    let again = crate::code_index::CodeIndex::new(&store)
        .code_index_status(&q, &config)
        .unwrap();
    assert_eq!(status["quality"], again["quality"]);
    assert_eq!(status["retrieval"], again["retrieval"]);
    assert!(!status.to_string().contains("word word"));
    assert!(!status.to_string().contains("broken JSON"));
    let limited = crate::code_index::CodeIndex::new(&store)
        .code_index_status(
            &StatusQuery {
                sample_limit: Some(1),
                ..q
            },
            &config,
        )
        .unwrap();
    assert_eq!(limited["quality"]["chunks"]["sampled"], 1);
    assert_eq!(limited["quality"]["chunks"]["sample_truncated"], true);
}
#[test]
fn stale_windows_unsupported_sources_and_corrupt_cache_are_explicit() {
    let (_root, store, config) = setup();
    for id in ["a", "b"] {
        put(
            &store,
            id,
            "a",
            &format!("/p/{id}.py"),
            "def work(): pass\n",
        );
    }
    put(
        &store,
        "unsupported",
        "a",
        "/p/a.not_source",
        "unparsed fixture",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    for id in ["a", "b"] {
        put(
            &store,
            id,
            "a",
            &format!("/p/{id}.py"),
            "def changed(): pass\n",
        );
    }
    let q = StatusQuery {
        project: Some("a".into()),
        limit: Some(1),
        ..Default::default()
    };
    let status = crate::code_index::CodeIndex::new(&store)
        .code_index_status(&q, &config)
        .unwrap();
    assert_eq!(status["coverage"]["stale_count"], 2);
    assert_eq!(status["stale_documents"], serde_json::json!(["a"]));
    assert_eq!(status["coverage"]["evidence_truncated"], true);
    assert_eq!(
        status["coverage"]["unsupported_source_extensions"]["not_source"],
        1
    );
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE code_index_documents SET payload='broken JSON' WHERE document_id='a'",
            [],
        )
        .unwrap();
    let broken = crate::code_index::CodeIndex::new(&store)
        .code_index_status(&q, &config)
        .unwrap();
    assert_eq!(broken["state"], "unavailable");
    assert!(broken["symbols"].is_null());
    assert!(broken["coverage"]["fresh_documents"].is_null());
    assert!(broken["quality"]["reference_resolution_counts"].is_null());
    assert_eq!(broken["quality"]["healthy"], false);
    assert!(!broken.to_string().contains("broken JSON"));
    let foreign = crate::code_index::CodeIndex::new(&store)
        .code_index_status(
            &StatusQuery {
                project: Some("other".into()),
                ..Default::default()
            },
            &config,
        )
        .unwrap();
    assert_eq!(foreign["state"], "empty_scope");
    assert_eq!(foreign["coverage"]["indexable_documents"], 0);
}
#[test]
fn oversized_sample_cells_are_reported_as_unmeasured() {
    let (_root, store, config) = setup();
    put(&store, "a", "a", "/p/a.py", "def work(): pass\n");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    chunk(
        &store,
        "a",
        "a",
        &"界".repeat(TEXT_CHARS + 1),
        vec![1.0, 0.0],
    );
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE chunks SET embedding=list_transform(range(?), x -> 0.5::FLOAT)",
            [VECTOR_BYTES as i64 + 1],
        )
        .unwrap();
    let status = crate::code_index::CodeIndex::new(&store)
        .code_index_status(&StatusQuery::default(), &config)
        .unwrap();
    let quality = &status["quality"]["chunks"];
    assert_eq!(quality["long_text_unmeasured"], 1);
    assert_eq!(quality["large_vectors_unmeasured"], 1);
    assert_eq!(quality["valid_vectors"], 0);
    assert_eq!(quality["tokenized"], 0);
    assert_eq!(quality["bytes_inspected"], 0);
    assert!(quality["norm_p50"].is_null());
}
#[test]
fn sampling_budget_stops_inspection_and_nonunit_vectors_are_not_invalid() {
    let (_root, store, config) = setup();
    put(&store, "a", "a", "/p/a.py", "def work(): pass\n");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    store
        .embedding_manifest()
        .write_embedding_manifest_from_config(&config)
        .unwrap();
    let content = "界".repeat(TEXT_CHARS);
    let rows: Vec<_> = (0..100)
        .map(|index| Chunk {
            id: format!("c{index:03}"),
            document_id: "a".into(),
            chunk_index: index,
            content: content.clone(),
            embedding: vec![3.0, 4.0],
            char_start: 0,
            char_end: TEXT_CHARS as i32,
            metadata_json: "{}".into(),
        })
        .collect();
    store.chunks().insert_chunks(&rows).unwrap();
    let status = crate::code_index::CodeIndex::new(&store)
        .code_index_status(
            &StatusQuery {
                sample_limit: Some(512),
                max_chunk_tokens: Some(100000),
                ..Default::default()
            },
            &config,
        )
        .unwrap();
    let quality = &status["quality"]["chunks"];
    assert_eq!(quality["total"], 100);
    assert!(quality["sampled"].as_u64().unwrap() < 100);
    assert_eq!(quality["byte_budget_exhausted"], true);
    assert!(quality["bytes_inspected"].as_u64().unwrap() <= SAMPLE_BYTES as u64);
    assert_eq!(quality["norm_min"], 5.0);
    assert_eq!(quality["norm_max"], 5.0);
    assert_eq!(quality["unit_norm_vectors"], 0);
    assert_eq!(quality["invalid_vectors"], 0);
    assert!(status["quality"]["healthy"].is_null());
}
#[test]
fn sql_read_evidence_without_definitions_is_not_an_empty_structural_index() {
    let (_root, store, config) = setup();
    put(&store, "sql", "a", "/p/read.sql", "SELECT id FROM users;");
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let status = crate::code_index::CodeIndex::new(&store)
        .code_index_status(&StatusQuery::default(), &config)
        .unwrap();
    assert_eq!(status["symbols"], 0);
    assert_eq!(status["sql_tables"], 0);
    assert_eq!(status["sql_usages"], 1);
    assert_eq!(status["state"], "observed_checks_passed");
    assert!(status["quality"]["healthy"].is_null());
}
#[test]
fn markup_endpoint_evidence_without_definitions_is_not_empty() {
    let (_root, store, config) = setup();
    put(
        &store,
        "markup",
        "a",
        "/p/view.html",
        "<form action='/save'></form>",
    );
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let status = crate::code_index::CodeIndex::new(&store)
        .code_index_status(&StatusQuery::default(), &config)
        .unwrap();
    assert_eq!(status["symbols"], 0);
    assert_eq!(status["state"], "observed_checks_passed", "{status}");
    assert!(status["quality"]["healthy"].is_null());
}
