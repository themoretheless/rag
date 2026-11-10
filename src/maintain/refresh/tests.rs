
use super::*;
use crate::embeddings::MockEmbedder;
use crate::models::{Chunk, SearchMode};
use crate::testkit::{test_config, SeedDoc};
use crate::wiki::write_wiki_page;
use async_trait::async_trait;
use chrono::Utc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Seeded body the failing embedder refuses.
const FAILING_TEXT: &str = "second body";

struct FailAfterFirstEmbedder {
    calls: AtomicUsize,
    dims: usize,
}

#[async_trait]
impl EmbeddingProvider for FailAfterFirstEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        if call > 0 {
            return Err(AppError::embeddings("injected provider failure"));
        }
        Ok(texts.iter().map(|_| vec![0.25; self.dims]).collect())
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

struct FailOnTextEmbedder {
    calls: AtomicUsize,
    dims: usize,
}

#[async_trait]
impl EmbeddingProvider for FailOnTextEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        // Re-embedding groups documents into one call; failing on one
        // document's text (not on the Nth call) keeps the failure per
        // document whatever the grouping.
        if texts.iter().any(|text| text.contains(FAILING_TEXT)) {
            return Err(AppError::embeddings("injected provider failure"));
        }
        Ok(texts.iter().map(|_| vec![0.25; self.dims]).collect())
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

fn seed_doc(store: &Store, id: &str, title: &str, content: &str, layer: &str) {
    SeedDoc::new(id, title, content).layer(layer).insert(store);
}

#[test]
fn whitelist_contains_expected_actions() {
    for a in [
        "reindex_fts",
        "rebuild_graph",
        "rebuild_wiki_index",
        "reembed_all",
    ] {
        assert!(REFRESH_ACTION_WHITELIST.contains(&a));
    }
}

#[test]
fn flags_default_safe_suite_when_none_set() {
    let f = MaintainRefreshFlags::from_options(None, None, None, None, None, None, 50);
    assert!(f.reindex_fts);
    assert!(f.rebuild_graph);
    assert!(f.graph_dirty_only);
    assert!(f.rebuild_wiki_index);
    assert!(!f.reembed_all);
}

#[test]
fn flags_explicit_single_action() {
    let f = MaintainRefreshFlags::from_options(Some(true), None, None, None, None, Some(true), 10);
    assert!(f.reindex_fts);
    assert!(!f.rebuild_graph);
    assert!(!f.rebuild_wiki_index);
    assert!(!f.reembed_all);
    assert!(f.dry_run);
    assert_eq!(f.max_docs, 10);
}

#[test]
fn reindex_fts_and_wiki_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("refresh.duckdb");
    let store = Store::open(&path).unwrap();
    seed_doc(&store, "d1", "Alpha", "hello world [[Beta]] #tag", "raw");
    seed_doc(
        &store,
        "w1",
        "Wiki One",
        "compiled page about alpha",
        "wiki",
    );

    let fts = reindex_fts(&store, Some("porter")).unwrap();
    assert!(!fts.backend.is_empty());
    assert_eq!(fts.stemmer, "porter");

    let idx = rebuild_wiki_index(&store).unwrap();
    assert_eq!(idx.entry_count, 1);
    assert_eq!(store.wiki_index().list_wiki_index().unwrap().len(), 1);
}

#[test]
fn rebuild_graph_dirty_then_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("graph.duckdb");
    let store = Store::open(&path).unwrap();
    seed_doc(&store, "d1", "Alpha", "see [[Beta]] and #topic", "raw");
    seed_doc(&store, "d2", "Gamma", "plain text no links", "raw");

    // Dirty: neither has a document node.
    let r1 = rebuild_graph_for_all_or_dirty(&store, true, 50).unwrap();
    assert_eq!(r1.candidate_count, 2);
    assert_eq!(r1.succeeded, 2);
    assert!(store
        .graph()
        .find_node_by_document_id("d1")
        .unwrap()
        .is_some());

    // Dirty again: none left.
    let r2 = rebuild_graph_for_all_or_dirty(&store, true, 50).unwrap();
    assert_eq!(r2.candidate_count, 0);
    assert_eq!(r2.processed, 0);

    // All: rebuild both again.
    let r3 = rebuild_graph_for_all_or_dirty(&store, false, 50).unwrap();
    assert_eq!(r3.candidate_count, 2);
    assert_eq!(r3.succeeded, 2);
}

#[test]
fn graph_respects_max_docs_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cap.duckdb");
    let store = Store::open(&path).unwrap();
    for i in 0..5 {
        seed_doc(
            &store,
            &format!("d{i}"),
            &format!("Doc{i}"),
            "content",
            "raw",
        );
    }
    let r = rebuild_graph_for_all_or_dirty(&store, false, 2).unwrap();
    assert_eq!(r.candidate_count, 5);
    assert_eq!(r.processed, 2);
    assert_eq!(r.skipped_cap, 3);
    assert_eq!(r.succeeded, 2);
}

#[tokio::test]
async fn maintain_refresh_dry_run_and_apply() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("maint.duckdb");
    let store = Store::open(&path).unwrap();
    let dims = 8usize;
    let config = test_config(path.clone(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));

    seed_doc(&store, "d1", "Alpha", "hello [[Beta]] #x", "raw");
    seed_doc(&store, "w1", "Wiki", "wiki body", "wiki");
    store
        .embedding_manifest()
        .write_embedding_manifest_from_config(&config)
        .unwrap();

    let dry_flags = MaintainRefreshFlags {
        reindex_fts: true,
        rebuild_graph: true,
        graph_dirty_only: true,
        rebuild_wiki_index: true,
        reembed_all: false,
        dry_run: true,
        max_docs: 50,
    };
    let dry = maintain_refresh(&store, &embedder, &config, dry_flags)
        .await
        .unwrap();
    assert!(dry.dry_run);
    assert!(dry.reindex_fts.as_ref().unwrap().dry_run);
    assert_eq!(dry.rebuild_graph.as_ref().unwrap().candidate_count, 2);
    // dry_run does not create nodes
    assert!(store
        .graph()
        .find_node_by_document_id("d1")
        .unwrap()
        .is_none());
    assert!(dry.ops_log_id.is_some());

    let apply_flags = MaintainRefreshFlags {
        reindex_fts: true,
        rebuild_graph: true,
        graph_dirty_only: true,
        rebuild_wiki_index: true,
        reembed_all: true,
        dry_run: false,
        max_docs: 50,
    };
    let applied = maintain_refresh(&store, &embedder, &config, apply_flags)
        .await
        .unwrap();
    assert!(!applied.dry_run);
    assert_eq!(applied.rebuild_graph.as_ref().unwrap().succeeded, 2);
    assert_eq!(applied.rebuild_wiki_index.as_ref().unwrap().entry_count, 1);
    assert!(applied
        .reembed_all
        .as_ref()
        .is_some_and(|report| report.chunks_reembedded > 0));
    assert!(store
        .graph()
        .find_node_by_document_id("d1")
        .unwrap()
        .is_some());
    assert_eq!(store.wiki_index().list_wiki_index().unwrap().len(), 1);
    let generation_after_refresh = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(!generation_after_refresh.dirty);
    let _hits = crate::db::search::search(
        &store,
        &crate::db::search::SearchQuery {
            mode: SearchMode::Lex,
            query_text: Some("hello".into()),
            top_k: 5,
            fts_stemmer: config.fts_stemmer.clone(),
            ..crate::db::search::SearchQuery::default()
        },
    )
    .unwrap();
    let generation_after_search = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert_eq!(generation_after_search, generation_after_refresh);

    let ops = store.ops_log().list_ops_log(10).unwrap();
    assert!(ops.iter().any(|o| o.op == "maintain_refresh"));
    assert!(ops.iter().any(|o| o.op == "maintain_refresh_dry_run"));
}

#[tokio::test]
async fn reembed_all_updates_vectors_and_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reembed.duckdb");
    let store = Store::open(&path).unwrap();
    let dims = 16usize;
    let config = test_config(path.clone(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));

    seed_doc(&store, "d1", "A", "reembed body one", "raw");
    // Wrong-dim chunks from seed (8); reembed should fix to 16.
    store
        .embedding_manifest()
        .set_embedding_manifest(&EmbeddingManifest {
            id: "default".into(),
            provider: "mock".into(),
            model: "mock".into(),
            dims: 8,
            base_url: None,
            content_fingerprint: None,
            updated_at: Utc::now(),
        })
        .unwrap();

    let report = reembed_all(&store, &embedder, &config, 50).await.unwrap();
    assert_eq!(report.documents_succeeded, 1);
    assert_eq!(report.chunks_considered, 1);
    assert_eq!(report.attached_chunks_considered, 1);
    assert_eq!(report.orphan_chunks, 0);
    assert_eq!(report.chunks_reembedded, report.attached_chunks_considered);
    assert!(report.errors.is_empty());
    let man = report.manifest.unwrap();
    assert_eq!(man.dims, dims as i32);

    let chunks = store.chunks().list_chunks_for_document("d1").unwrap();
    assert!(chunks.iter().all(|c| c.embedding.len() == dims));
    store
        .embedding_manifest()
        .require_embedding_manifest_match(&config)
        .expect("identity matches after reembed_all");
}

#[tokio::test]
async fn graph_and_uncapped_reembed_use_multiple_bounded_pages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paged-refresh.duckdb");
    let store = Store::open(&path).unwrap();
    let dims = 8usize;
    let config = test_config(path, dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    for index in 0..70 {
        let id = format!("doc-{index:03}");
        seed_doc(&store, &id, &id, "paged body", "wiki");
    }

    let graph = rebuild_graph_for_all_or_dirty(&store, false, usize::MAX).unwrap();
    assert_eq!(graph.candidate_count, 70);
    assert_eq!(graph.processed, 70);
    assert_eq!(graph.succeeded, 70);
    assert_eq!(graph.documents.len(), REPORT_DOCUMENT_CAP);
    assert_eq!(graph.document_results_truncated, 6);

    let store = store
        .corpus_lane()
        .try_corpus_mutation_scope("paged reembed test")
        .unwrap();
    let reembed = reembed_all(&store, &embedder, &config, usize::MAX)
        .await
        .unwrap();
    assert_eq!(reembed.documents_considered, 70);
    assert_eq!(reembed.documents_processed, 70);
    assert_eq!(reembed.documents_succeeded, 70);
    assert_eq!(reembed.skipped_cap, 0);
    assert_eq!(reembed.documents.len(), REPORT_DOCUMENT_CAP);
    assert_eq!(reembed.document_results_truncated, 6);
    assert!(reembed.errors.is_empty());
    store
        .embedding_manifest()
        .require_embedding_manifest_match(&config)
        .expect("complete paged migration publishes target identity");
}

#[tokio::test]
async fn maintain_refresh_clamps_user_max_docs_to_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("refresh-service-cap.duckdb");
    let store = Store::open(&path).unwrap();
    let mut config = test_config(path, 8);
    config.maint_max_docs = 2;
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    for index in 0..3 {
        let id = format!("doc-{index}");
        seed_doc(&store, &id, &id, "body", "wiki");
    }

    let report = maintain_refresh(
        &store,
        &embedder,
        &config,
        MaintainRefreshFlags {
            reindex_fts: false,
            rebuild_graph: true,
            graph_dirty_only: false,
            rebuild_wiki_index: false,
            reembed_all: false,
            dry_run: true,
            max_docs: 999,
        },
    )
    .await
    .unwrap();
    let graph = report.rebuild_graph.unwrap();
    assert_eq!(graph.candidate_count, 3);
    assert_eq!(graph.processed, 2);
    assert_eq!(graph.skipped_cap, 1);
}

#[tokio::test]
async fn reembed_all_refuses_orphan_chunk_without_publishing_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reembed-orphan.duckdb");
    let store = Store::open(&path).unwrap();
    let dims = 16usize;
    let config = test_config(path, dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let legacy_vector = vec![0.75; 8];
    store
        .chunks()
        .insert_chunks(&[Chunk {
            id: "orphan-c0".into(),
            document_id: "missing-document".into(),
            chunk_index: 0,
            content: "unowned legacy vector".into(),
            embedding: legacy_vector.clone(),
            char_start: 0,
            char_end: 21,
            metadata_json: "{}".into(),
        }])
        .unwrap();

    let refresh = maintain_refresh(
        &store,
        &embedder,
        &config,
        MaintainRefreshFlags {
            reindex_fts: false,
            rebuild_graph: false,
            graph_dirty_only: true,
            rebuild_wiki_index: false,
            reembed_all: true,
            dry_run: false,
            max_docs: 50,
        },
    )
    .await
    .expect("orphan integrity refusal is a structured report");
    assert_eq!(refresh.errors.len(), 1);
    assert!(refresh.errors[0].starts_with(REEMBED_ALL_INCOMPLETE_CODE));
    assert!(refresh.errors[0].contains("orphan_chunks=1"));
    let summary_id = refresh.ops_log_id.clone().expect("terminal ops log");
    let report = refresh.reembed_all.expect("nested reembed report");

    assert_eq!(report.documents_considered, 0);
    assert_eq!(report.documents_processed, 0);
    assert_eq!(report.chunks_considered, 1);
    assert_eq!(report.attached_chunks_considered, 0);
    assert_eq!(report.orphan_chunks, 1);
    assert_eq!(report.chunks_reembedded, 0);
    assert!(reembed_all_incomplete(&report));
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].starts_with(REEMBED_ALL_INCOMPLETE_CODE));
    assert!(report.errors[0].contains("no target embedding manifest was published"));
    assert!(report.manifest.is_none());
    assert!(store
        .embedding_manifest()
        .get_embedding_manifest()
        .unwrap()
        .is_none());

    let summary = store
        .ops_log()
        .list_ops_log(10)
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == summary_id)
        .expect("maintain_refresh summary");
    let payload: serde_json::Value = serde_json::from_str(&summary.payload_json).unwrap();
    assert_eq!(payload["errors"].as_array().unwrap().len(), 1);
    assert_eq!(payload["reembed_all"]["orphan_chunks"], 1);

    let chunks = store.chunks().all_chunks_with_embeddings().unwrap();
    assert_eq!(chunks.len(), 1, "reembed_all must not prune the orphan");
    assert_eq!(chunks[0].embedding, legacy_vector);
}

#[tokio::test]
async fn capped_same_dimension_identity_migration_remains_closed_until_resumed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reembed-cap.duckdb");
    let store = Store::open(&path).unwrap();
    let old_config = test_config(path, 8);
    let mut target_config = old_config.clone();
    target_config.embedding_model = "next-model-same-dims".into();
    store
        .embedding_manifest()
        .write_embedding_manifest_from_config(&old_config)
        .unwrap();
    seed_doc(&store, "d1", "A", "first body", "raw");
    seed_doc(&store, "d2", "B", "second body", "raw");
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    let first = reembed_all(&store, &embedder, &target_config, 1)
        .await
        .unwrap();
    assert_eq!(first.documents_processed, 1);
    assert_eq!(first.pending_documents, 1);
    assert!(first.resumable);
    assert!(store
        .embedding_manifest()
        .require_embedding_manifest_match(&old_config)
        .is_err());
    assert!(store
        .embedding_manifest()
        .require_embedding_manifest_match(&target_config)
        .is_err());
    let resumed = reembed_all(&store, &embedder, &target_config, 1)
        .await
        .unwrap();
    assert_eq!(resumed.documents_processed, 1);
    assert_eq!(resumed.pending_documents, 0);
    assert!(resumed.errors.is_empty());
    store
        .embedding_manifest()
        .require_embedding_manifest_match(&target_config)
        .unwrap();
    assert!(store
        .embedding_manifest()
        .require_embedding_manifest_match(&old_config)
        .is_err());
}

#[tokio::test]
async fn reembed_all_groups_small_documents_into_one_provider_call() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reembed-grouped.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 16);
    for (id, body) in [
        ("g1", "alpha body"),
        ("g2", "beta body"),
        ("g3", "gamma body"),
    ] {
        seed_doc(&store, id, id, body, "raw");
    }
    let counting = Arc::new(FailOnTextEmbedder {
        calls: AtomicUsize::new(0),
        dims: 16,
    });
    let embedder: Arc<dyn EmbeddingProvider> = counting.clone();

    let report = reembed_all(&store, &embedder, &config, 3).await.unwrap();

    assert_eq!(report.documents_succeeded, 3);
    assert_eq!(report.documents_failed, 0);
    assert_eq!(counting.calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn partial_migration_marker_refuses_old_and_target_until_full_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reembed-partial.duckdb");
    let store = Store::open(&path).unwrap();
    let old_config = test_config(path.clone(), 8);
    let target_config = test_config(path, 16);
    store
        .embedding_manifest()
        .write_embedding_manifest_from_config(&old_config)
        .unwrap();
    seed_doc(&store, "d1", "A", "first body", "raw");
    seed_doc(&store, "d2", "B", "second body", "raw");

    let failing = Arc::new(FailOnTextEmbedder {
        calls: AtomicUsize::new(0),
        dims: 16,
    });
    let failing_embedder: Arc<dyn EmbeddingProvider> = failing.clone();
    let partial = reembed_all(&store, &failing_embedder, &target_config, 2)
        .await
        .expect("best-effort reembed report");
    assert_eq!(partial.documents_succeeded, 1);
    assert_eq!(partial.documents_failed, 1);
    assert_eq!(partial.skipped_cap, 0);
    // One grouped call fails, then each document is retried alone.
    assert_eq!(failing.calls.load(Ordering::Relaxed), 3);

    drop(store);
    let store = Store::open(&target_config.db_path).expect("reopen partial migration");
    let marker = store
        .embedding_manifest()
        .get_embedding_manifest()
        .unwrap()
        .unwrap();
    assert!(marker
        .content_fingerprint
        .as_deref()
        .is_some_and(|fingerprint| fingerprint.starts_with("migration-incomplete:")));
    let target_error = store
        .embedding_manifest()
        .require_embedding_manifest_match(&target_config)
        .expect_err("target identity must remain refused");
    assert!(target_error
        .to_string()
        .contains("incomplete corpus migration"));
    let old_error = store
        .embedding_manifest()
        .require_embedding_manifest_match(&old_config)
        .expect_err("old identity must remain refused for mixed vectors");
    assert!(old_error
        .to_string()
        .contains("incomplete corpus migration"));

    let mut dimensions = store
        .chunks()
        .all_chunks_with_embeddings()
        .unwrap()
        .into_iter()
        .map(|chunk| chunk.embedding.len())
        .collect::<Vec<_>>();
    dimensions.sort_unstable();
    assert_eq!(dimensions, vec![8, 16]);

    let recovery_embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let recovered = reembed_all(&store, &recovery_embedder, &target_config, 2)
        .await
        .expect("complete retry");
    // Resume skips the already-migrated document (progress survived reopen).
    assert_eq!(recovered.documents_processed, 1);
    assert_eq!(recovered.documents_succeeded, 1);
    assert_eq!(recovered.documents_failed, 0);
    assert_eq!(recovered.skipped_cap, 0);
    assert_eq!(recovered.pending_documents, 0);
    store
        .embedding_manifest()
        .require_embedding_manifest_match(&target_config)
        .expect("target identity is published after complete retry");
    assert!(store
        .chunks()
        .all_chunks_with_embeddings()
        .unwrap()
        .iter()
        .all(|chunk| chunk.embedding.len() == 16));
    // Completion cleared the resume state and logged a complete marker.
    let fingerprint = crate::db::store::embedding_manifest_from_config(&target_config)
        .content_fingerprint
        .unwrap();
    assert_eq!(
        store
            .reembed_progress()
            .count_reembed_done(&fingerprint)
            .unwrap(),
        0
    );
    assert!(store
        .ops_log()
        .list_ops_log(10)
        .unwrap()
        .iter()
        .any(|entry| entry.op == "reembed_progress"
            && entry.payload_json.contains("\"complete\":true")));
}

#[tokio::test]
async fn direct_refresh_reports_partial_reembed_as_top_level_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("refresh-partial-reembed.duckdb");
    let store = Store::open(&path).unwrap();
    let old_config = test_config(path.clone(), 8);
    let target_config = test_config(path, 16);
    store
        .embedding_manifest()
        .write_embedding_manifest_from_config(&old_config)
        .unwrap();
    seed_doc(&store, "d1", "A", "first body", "raw");
    seed_doc(&store, "d2", "B", "second body", "raw");
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(FailOnTextEmbedder {
        calls: AtomicUsize::new(0),
        dims: 16,
    });

    let report = maintain_refresh(
        &store,
        &embedder,
        &target_config,
        MaintainRefreshFlags {
            reindex_fts: false,
            rebuild_graph: false,
            graph_dirty_only: true,
            rebuild_wiki_index: false,
            reembed_all: true,
            dry_run: false,
            max_docs: 2,
        },
    )
    .await
    .expect("best-effort refresh report");

    let reembed = report.reembed_all.expect("reembed report");
    assert_eq!(reembed.documents_succeeded, 1);
    assert_eq!(reembed.documents_failed, 1);
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].starts_with(REEMBED_ALL_INCOMPLETE_CODE));
    store
        .embedding_manifest()
        .require_embedding_manifest_match(&target_config)
        .expect_err("partial refresh must leave vector reads fail-closed");
}

#[tokio::test]
async fn refresh_final_fts_failure_returns_report_and_terminal_ops_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("refresh-finalize-failure.duckdb");
    let store = Store::open(&path).unwrap();
    let dims = 8usize;
    let mut config = test_config(path, dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    store
        .embedding_manifest()
        .write_embedding_manifest_from_config(&config)
        .unwrap();
    seed_doc(
        &store,
        "d1",
        "Refresh",
        "durable reembed before FTS failure",
        "raw",
    );
    store.maintenance().ensure_fts(&config.fts_stemmer).unwrap();
    config.fts_stemmer = crate::db::fts::test_fault::fail("refresh");

    let report = maintain_refresh(
        &store,
        &embedder,
        &config,
        MaintainRefreshFlags {
            reindex_fts: false,
            rebuild_graph: false,
            graph_dirty_only: true,
            rebuild_wiki_index: false,
            reembed_all: true,
            dry_run: false,
            max_docs: 50,
        },
    )
    .await
    .expect("aggregate report survives terminal FTS failure");

    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].contains("FTS_FINALIZATION_FAILED"));
    assert!(report
        .reembed_all
        .as_ref()
        .is_some_and(|result| result.chunks_reembedded > 0));
    let generation = {
        let conn = store.lock().unwrap();
        crate::db::fts_generation_state(&conn).unwrap()
    };
    assert!(generation.dirty, "next lexical read must retry FTS");
    let summary_id = report.ops_log_id.as_deref().expect("terminal summary id");
    let summary = store
        .ops_log()
        .list_ops_log(10)
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == summary_id)
        .expect("terminal summary ops_log row");
    let payload: serde_json::Value = serde_json::from_str(&summary.payload_json).unwrap();
    assert_eq!(payload["errors"].as_array().unwrap().len(), 1);
    assert!(store
        .ops_log()
        .list_ops_log(10)
        .unwrap()
        .iter()
        .any(|entry| entry.op == "fts_finalization_failed"));
}

#[tokio::test]
async fn write_wiki_then_refresh_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wiki.duckdb");
    let store = Store::open(&path).unwrap();
    let dims = 8usize;
    let config = test_config(path.clone(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    store
        .embedding_manifest()
        .write_embedding_manifest_from_config(&config)
        .unwrap();

    write_wiki_page(
        &store,
        &embedder,
        &config,
        crate::wiki::WikiPageWrite {
            slug: "alpha-page",
            title: "Alpha Page",
            content: "Body with [[Other]]",
            kind: "wiki",
            category: Some("notes"),
            summary: Some("summary line"),
            agent: None,
        },
    )
    .await
    .unwrap();

    // Wipe index and rebuild via refresh.
    {
        let conn = store.lock().unwrap();
        conn.execute("DELETE FROM wiki_index", []).unwrap();
    }
    assert!(store.wiki_index().list_wiki_index().unwrap().is_empty());

    let flags = MaintainRefreshFlags {
        reindex_fts: false,
        rebuild_graph: false,
        graph_dirty_only: true,
        rebuild_wiki_index: true,
        reembed_all: false,
        dry_run: false,
        max_docs: 50,
    };
    let r = maintain_refresh(&store, &embedder, &config, flags)
        .await
        .unwrap();
    assert_eq!(r.rebuild_wiki_index.unwrap().entry_count, 1);
}

#[tokio::test]
async fn reembed_all_capped_migration_runs_one_resumable_batch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reembed-cap.duckdb");
    let store = Store::open(&path).unwrap();
    let old_config = test_config(path.clone(), 8);
    let mut target_config = old_config.clone();
    target_config.embedding_model = "next-model-same-dims".into();
    store
        .embedding_manifest()
        .write_embedding_manifest_from_config(&old_config)
        .unwrap();
    seed_doc(&store, "d1", "A", "first body", "raw");
    seed_doc(&store, "d2", "B", "second body", "raw");

    let failing = Arc::new(FailAfterFirstEmbedder {
        calls: AtomicUsize::new(0),
        dims: 8,
    });
    let embedder: Arc<dyn EmbeddingProvider> = failing.clone();

    // A capped migration is no longer refused: it processes one batch.
    let batch = reembed_all(&store, &embedder, &target_config, 1)
        .await
        .expect("capped migration runs one resumable batch");
    assert!(batch.resumable);
    assert_eq!(batch.documents_processed, 1);
    assert_eq!(batch.documents_succeeded, 1);
    assert_eq!(batch.skipped_cap, 1);
    assert_eq!(batch.pending_documents, 1);
    assert!(reembed_all_incomplete(&batch));
    assert_eq!(failing.calls.load(Ordering::Relaxed), 1);

    // Fail-closed while partial: neither old nor target identity matches.
    store
        .embedding_manifest()
        .require_embedding_manifest_match(&old_config)
        .expect_err("old identity refused once mixed vectors exist");
    store
        .embedding_manifest()
        .require_embedding_manifest_match(&target_config)
        .expect_err("target identity refused until migration completes");

    // ops_log carries resumable progress for operators.
    let ops = store.ops_log().list_ops_log(10).unwrap();
    let progress = ops
        .iter()
        .find(|entry| entry.op == "reembed_progress")
        .expect("reembed progress entry");
    let payload: serde_json::Value = serde_json::from_str(&progress.payload_json).unwrap();
    assert_eq!(payload["documents_done"], 1);
    assert_eq!(payload["documents_pending"], 1);
    assert_eq!(payload["complete"], false);
}

#[tokio::test]
async fn reembed_status_reports_match_pending_and_completion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reembed-status.duckdb");
    let store = Store::open(&path).unwrap();
    let old_config = test_config(path.clone(), 8);
    let target_config = test_config(path, 16);
    store
        .embedding_manifest()
        .write_embedding_manifest_from_config(&old_config)
        .unwrap();
    for id in ["d1", "d2", "d3"] {
        seed_doc(&store, id, id, "status body", "raw");
    }

    // Identity match: search open, no migration section.
    let status = reembed_status(&store, &old_config).unwrap();
    assert!(status.identity_match);
    assert!(status.vector_search_open);
    assert!(status.migration.is_none());

    // Mismatch: all three documents pending, search closed.
    let status = reembed_status(&store, &target_config).unwrap();
    assert!(!status.identity_match);
    assert!(!status.vector_search_open);
    let migration = status.migration.expect("migration status");
    assert_eq!(migration.documents_done, 0);
    assert_eq!(migration.documents_pending, 3);
    assert_eq!(migration.documents_total, 3);
    assert_eq!(migration.batch_size, target_config.reembed_batch);

    // Batch 1: two documents migrate; status reflects the remaining one.
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let batch = reembed_all(&store, &embedder, &target_config, 2)
        .await
        .unwrap();
    assert!(batch.resumable);
    assert_eq!(batch.documents_succeeded, 2);
    assert_eq!(batch.pending_documents, 1);
    store
        .embedding_manifest()
        .require_embedding_manifest_match(&target_config)
        .expect_err("search stays closed while one document is pending");
    let status = reembed_status(&store, &target_config).unwrap();
    let migration = status.migration.expect("migration status");
    assert_eq!(migration.documents_done, 2);
    assert_eq!(migration.documents_pending, 1);

    // Simulate a restart between batches: reopen the same database file.
    drop(store);
    let store = Store::open(&target_config.db_path).unwrap();
    let status = reembed_status(&store, &target_config).unwrap();
    assert_eq!(status.migration.unwrap().documents_pending, 1);

    // Batch 2 completes the migration: manifest flips atomically, search opens.
    let final_batch = reembed_all(&store, &embedder, &target_config, 2)
        .await
        .unwrap();
    assert_eq!(final_batch.documents_succeeded, 1);
    assert_eq!(final_batch.pending_documents, 0);
    assert!(!reembed_all_incomplete(&final_batch));
    store
        .embedding_manifest()
        .require_embedding_manifest_match(&target_config)
        .expect("manifest flipped after final batch");
    assert!(store
        .chunks()
        .all_chunks_with_embeddings()
        .unwrap()
        .iter()
        .all(|chunk| chunk.embedding.len() == 16));
    let status = reembed_status(&store, &target_config).unwrap();
    assert!(status.identity_match);
    assert!(status.vector_search_open);
    assert!(status.migration.is_none());
}
