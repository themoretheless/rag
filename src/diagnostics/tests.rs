use super::*;
use crate::embeddings::MockEmbedder;
use crate::models::{Chunk, Document, GraphEdge, GraphNode, WikiIndexEntry};

#[test]
fn status_and_doctor_use_canonical_embedding_identity() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory
        .path()
        .join("diagnostics-embedding-identity.duckdb");
    let store = crate::testkit::open_test_store(&path).expect("open store");
    let config = Config {
        db_path: path,
        embedding_dims: 8,
        ..Config::for_tests()
    };
    store
        .documents()
        .upsert_document(&Document {
            id: "identity-doc".into(),
            uri: "text://identity-doc".into(),
            title: "Identity".into(),
            content: "searchable body".into(),
            ..Document::default()
        })
        .expect("insert document");
    store
        .chunks()
        .insert_chunks(&[Chunk {
            id: "identity-chunk".into(),
            document_id: "identity-doc".into(),
            chunk_index: 0,
            content: "searchable body".into(),
            embedding: vec![0.1; 8],
            char_start: 0,
            char_end: 15,
            metadata_json: "{}".into(),
        }])
        .expect("insert chunk");

    let mut stale = crate::db::store::embedding_manifest_from_config(&config);
    stale.content_fingerprint = Some("another-endpoint-fingerprint".into());
    store
        .embedding_manifest()
        .set_embedding_manifest(&stale)
        .expect("set mismatch");

    let service = DiagnosticsService::new(&store, &config);
    let status = service.status().expect("status");
    assert!(!status.embedding_manifest_match);
    assert!(!status.ready_for_search);
    let doctor = service.doctor().expect("doctor");
    assert!(!doctor.embed_ok);
    assert!(!doctor.ready_for_search);
    assert!(!doctor.ok);
    assert!(doctor
        .repair_hint
        .as_deref()
        .is_some_and(|hint| hint.contains("complete uncapped reembed_all")));

    stale.content_fingerprint = None;
    store
        .embedding_manifest()
        .set_embedding_manifest(&stale)
        .expect("set legacy manifest");
    assert!(
        !service
            .status()
            .expect("legacy status")
            .embedding_manifest_match
    );
    let legacy_doctor = service.doctor().expect("legacy doctor");
    assert!(!legacy_doctor.embed_ok);
    assert!(!legacy_doctor.ready_for_search);
}

#[test]
fn status_reports_the_configured_snapshot_footprint() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory
        .path()
        .join("diagnostics-snapshot-footprint.duckdb");
    let store = crate::testkit::open_test_store(&path).expect("open store");
    let config = Config {
        db_path: path,
        embedding_dims: 8,
        ..Config::for_tests()
    };
    let service = DiagnosticsService::new(&store, &config);

    // Unconfigured retention is not reported as an empty budget.
    let unconfigured = serde_json::to_value(service.status().expect("status")).unwrap();
    assert!(unconfigured.get("auto_backup").is_none(), "{unconfigured}");

    let backups = directory.path().join("backups");
    std::fs::create_dir_all(&backups).expect("backup dir");
    std::fs::write(backups.join("rag-auto-a.duckdb"), b"12345").expect("snapshot");
    std::fs::write(backups.join("notes.txt"), b"0000000000").expect("non-snapshot");
    let config = Config {
        auto_backup_dir: Some(backups),
        ..config
    };
    let service = DiagnosticsService::new(&store, &config);
    let configured = serde_json::to_value(service.status().expect("footprint status"))
        .expect("serialize footprint");

    // Only snapshot groups (`rag-auto-*.duckdb` and their sidecars) are billed.
    assert_eq!(configured["auto_backup"]["snapshots"], 1);
    assert_eq!(configured["auto_backup"]["total_bytes"], 5);
    assert!(configured["auto_backup"]["free_bytes"].as_u64().is_some());
}

#[test]
fn status_layer_health_counts_match_legacy_semantics() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("status-layer-health.duckdb");
    let store = crate::testkit::open_test_store(&path).expect("open store");

    for (id, layer) in [
        ("raw-no-node", "raw"),
        ("raw-non-wiki-backlink", "raw"),
        ("raw-wiki-backlink", "raw"),
        ("raw-unresolved-wiki-backlink", "raw"),
    ] {
        store
            .documents()
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("file://{id}.md"),
                title: id.into(),
                layer: layer.into(),
                ..Document::default()
            })
            .expect("insert raw document");
    }

    for node in [
        graph_node(
            "target-non-wiki",
            Some("raw-non-wiki-backlink"),
            Some("file://raw-non-wiki-backlink.md"),
            true,
        ),
        graph_node("source-non-wiki", None, Some("file://notes.md"), true),
        graph_node(
            "target-wiki",
            Some("raw-wiki-backlink"),
            Some("file://raw-wiki-backlink.md"),
            true,
        ),
        graph_node("source-wiki", None, Some("wiki://compiled"), true),
        graph_node(
            "target-unresolved",
            Some("raw-unresolved-wiki-backlink"),
            Some("file://raw-unresolved-wiki-backlink.md"),
            true,
        ),
        graph_node("source-unresolved-wiki", None, Some("wiki://draft"), false),
    ] {
        store
            .graph()
            .upsert_graph_node(&node)
            .expect("insert graph node");
    }
    store
        .graph()
        .insert_graph_edges(&[
            graph_edge("edge-non-wiki", "source-non-wiki", "target-non-wiki"),
            graph_edge("edge-wiki", "source-wiki", "target-wiki"),
            graph_edge(
                "edge-unresolved-wiki",
                "source-unresolved-wiki",
                "target-unresolved",
            ),
        ])
        .expect("insert graph edges");

    // Legacy behavior deliberately treats every raw as uncompiled when the
    // corpus contains no layer=wiki document, even if a wiki:// source node exists.
    let without_wiki = store
        .maintenance()
        .layer_health_counts()
        .expect("layer health");
    assert_eq!(without_wiki.raw_count, 4);
    assert_eq!(without_wiki.wiki_count, 0);
    assert_eq!(without_wiki.index_entry_count, 0);
    assert_eq!(without_wiki.indexed_pages, 0);
    assert_eq!(without_wiki.uncompiled_raw_count, 4);

    for id in ["wiki-indexed", "wiki-unindexed"] {
        store
            .documents()
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("wiki://{id}"),
                title: id.into(),
                layer: "wiki".into(),
                kind: "wiki".into(),
                ..Document::default()
            })
            .expect("insert wiki document");
    }
    for (id, page_id) in [
        ("index-present", Some("wiki-indexed")),
        ("index-orphan", Some("missing-wiki")),
    ] {
        store
            .wiki_index()
            .upsert_wiki_index_entry(&WikiIndexEntry {
                id: id.into(),
                slug: id.into(),
                title: id.into(),
                kind: "wiki".into(),
                category: None,
                summary: None,
                page_id: page_id.map(str::to_string),
                updated_at: chrono::Utc::now(),
            })
            .expect("insert wiki index entry");
    }

    let counts = store
        .maintenance()
        .layer_health_counts()
        .expect("layer health");
    assert_eq!(counts.raw_count, 4);
    assert_eq!(counts.wiki_count, 2);
    assert_eq!(counts.index_entry_count, 2);
    assert_eq!(counts.indexed_pages, 1);
    assert_eq!(counts.uncompiled_raw_count, 3);

    let config = Config {
        db_path: path,
        ..Config::for_tests()
    };
    let report = DiagnosticsService::new(&store, &config)
        .status()
        .expect("status");
    assert_eq!(report.raw_count, counts.raw_count);
    assert_eq!(report.wiki_count, counts.wiki_count);
    assert_eq!(report.index_entry_count, counts.index_entry_count);
    assert_eq!(report.index_coverage, 0.5);
    assert_eq!(report.uncompiled_raw_count, counts.uncompiled_raw_count);
}

fn graph_node(id: &str, document_id: Option<&str>, uri: Option<&str>, resolved: bool) -> GraphNode {
    GraphNode {
        id: id.into(),
        kind: "document".into(),
        label: id.into(),
        document_id: document_id.map(str::to_string),
        uri: uri.map(str::to_string),
        resolved,
        metadata_json: "{}".into(),
    }
}

fn graph_edge(id: &str, source_id: &str, target_id: &str) -> GraphEdge {
    GraphEdge {
        id: id.into(),
        source_id: source_id.into(),
        target_id: target_id.into(),
        rel_type: "related".into(),
        weight: 1.0,
        context: None,
    }
}

#[tokio::test]
async fn repair_ignores_intentionally_empty_documents() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("empty-document.duckdb");
    let store = crate::testkit::open_test_store(&path).expect("open store");
    store
        .documents()
        .upsert_document(&Document {
            id: "empty-source".into(),
            uri: "file://empty.md".into(),
            title: "Empty source".into(),
            content: "   ".into(),
            ..Document::default()
        })
        .expect("insert empty document");

    let config = Config {
        db_path: path,
        embedding_dims: 8,
        ..Config::for_tests()
    };
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    let report = DiagnosticsService::new(&store, &config)
        .repair(&embedder, true, Some(10))
        .await
        .expect("dry-run repair");

    assert_eq!(report.before.documents_without_chunks, 0);
    assert_eq!(report.documents_considered, 0);
    assert!(report.documents_repaired.is_empty());
}

#[tokio::test]
async fn repair_selects_only_a_bounded_missing_chunk_batch() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("bounded-doctor-repair.duckdb");
    let store = crate::testkit::open_test_store(&path).expect("open store");
    for index in 0..5 {
        store
            .documents()
            .upsert_document(&Document {
                id: format!("missing-{index}"),
                uri: format!("text://missing-{index}"),
                title: format!("Missing {index}"),
                content: format!("searchable missing body {index}"),
                ..Document::default()
            })
            .expect("insert missing document");
    }
    store
        .documents()
        .upsert_document(&Document {
            id: "schema-placeholder".into(),
            uri: "schema://placeholder".into(),
            title: "Schema".into(),
            content: "not a searchable document".into(),
            layer: "SCHEMA".into(),
            ..Document::default()
        })
        .expect("insert schema placeholder");

    let (total, selected) = store
        .documents()
        .list_documents_missing_chunks(2)
        .expect("bounded missing documents");
    assert_eq!(total, 5);
    assert_eq!(selected.len(), 2);

    let config = Config {
        db_path: path,
        embedding_dims: 8,
        maint_max_docs: 2,
        ..Config::for_tests()
    };
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    let report = DiagnosticsService::new(&store, &config)
        .repair(&embedder, true, Some(usize::MAX))
        .await
        .expect("bounded dry-run repair");
    assert_eq!(report.documents_considered, 2);
    assert!(report.documents_repaired.is_empty());
}

#[tokio::test]
async fn repair_embeds_missing_documents_in_one_provider_call() {
    struct Counting {
        inner: MockEmbedder,
        calls: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl EmbeddingProvider for Counting {
        async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.inner.embed(texts).await
        }
        fn dimensions(&self) -> usize {
            self.inner.dimensions()
        }
    }

    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("grouped-doctor-repair.duckdb");
    let store = crate::testkit::open_test_store(&path).expect("open store");
    for index in 0..3 {
        store
            .documents()
            .upsert_document(&Document {
                id: format!("missing-{index}"),
                uri: format!("text://missing-{index}"),
                title: format!("Missing {index}"),
                content: format!("searchable missing body {index}"),
                ..Document::default()
            })
            .expect("insert missing document");
    }
    let config = Config {
        db_path: path,
        embedding_dims: 8,
        ..Config::for_tests()
    };
    let counting = Arc::new(Counting {
        inner: MockEmbedder::new(8),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let embedder: Arc<dyn EmbeddingProvider> = counting.clone();
    let report = DiagnosticsService::new(&store, &config)
        .repair(&embedder, false, Some(10))
        .await
        .expect("repair");

    assert_eq!(report.documents_repaired.len(), 3);
    assert_eq!(counting.calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(
        store
            .documents()
            .list_documents_missing_chunks(10)
            .unwrap()
            .0,
        0
    );
}

#[tokio::test]
async fn repair_final_fts_failure_returns_committed_report() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("doctor-finalize-failure.duckdb");
    let store = crate::testkit::open_test_store(&path).expect("open store");
    store
        .documents()
        .upsert_document(&Document {
            id: "missing-chunks".into(),
            uri: "text://missing-chunks".into(),
            title: "Missing chunks".into(),
            content: "repair this durable document".into(),
            layer: "raw".into(),
            kind: "document".into(),
            ..Document::default()
        })
        .expect("insert document without chunks");
    let config = Config {
        db_path: path,
        embedding_dims: 8,
        fts_stemmer: crate::db::fts::test_fault::fail("doctor"),
        ..Config::for_tests()
    };
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));

    let report = DiagnosticsService::new(&store, &config)
        .repair(&embedder, false, Some(10))
        .await
        .expect("aggregate repair report survives terminal FTS failure");
    assert_eq!(report.documents_repaired, vec!["missing-chunks"]);
    assert_eq!(report.after.documents_without_chunks, 0);
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].contains("FTS_FINALIZATION_FAILED"));
    assert!(!store
        .chunks()
        .list_chunks_for_document("missing-chunks")
        .unwrap()
        .is_empty());
    let generation = {
        let conn = store.lock().unwrap();
        crate::db::fts_generation_state(&conn).unwrap()
    };
    assert!(generation.dirty, "next lexical read must retry FTS");
    assert!(store
        .ops_log()
        .list_recent_ops(10)
        .unwrap()
        .iter()
        .any(|entry| entry.op == "fts_finalization_failed"));
}
