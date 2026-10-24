//! Query-transformation integration tests: rewrite fusion, HyDE leg, caching,
//! and fallback semantics — all with a stub LLM and mock embeddings.
//!
//! Covers:
//! - `rewrite=true` fuses a stub-LLM variant list with the original query via
//!   RRF (`query_rewrite` + `multi_query_rrf` reasons, `rewrite_ms` timing).
//! - LLM failure falls back to plain search with `query_rewrite_fallback` /
//!   `hyde_fallback` reasons and identical hits.
//! - The bounded transform cache avoids repeat LLM calls for the same query.
//! - `hyde=true` adds a dense leg from the hypothetical passage (`hyde`
//!   reason, `hyde_ms` timing).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use rag_mcp::chunking::{from_config, Chunker};
use rag_mcp::embeddings::{EmbeddingProvider, MockEmbedder};
use rag_mcp::error::{AppError, Result as RagResult};
use rag_mcp::models::{Chunk, Document, SearchMode};
use rag_mcp::query_transform::TransformLlm;
use rag_mcp::retrieval::{execute_search_with_llm, SearchCommand};
use rag_mcp::{Config, Store};
use tempfile::TempDir;
use uuid::Uuid;

const DIMS: usize = 32;

/// Deterministic stub LLM: fixed reply, counts invocations.
struct StubLlm {
    reply: String,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl TransformLlm for StubLlm {
    async fn complete(&self, _: &str, _: &str, _: u32) -> RagResult<String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(self.reply.clone())
    }

    fn cache_identity(&self) -> String {
        "stub/itest".into()
    }
}

struct FailingLlm;

#[async_trait]
impl TransformLlm for FailingLlm {
    async fn complete(&self, _: &str, _: &str, _: u32) -> RagResult<String> {
        Err(AppError::llm("stub llm unavailable"))
    }

    fn cache_identity(&self) -> String {
        "stub/failing-itest".into()
    }
}

fn open_temp_store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("qt.duckdb")).expect("open store");
    (dir, store)
}

fn test_config(store: &Store) -> Config {
    Config {
        db_path: store.path().to_path_buf(),
        embedding_dims: DIMS,
        default_search_mode: SearchMode::Hybrid,
        ..Config::for_tests()
    }
}

/// Chunk + mock-embed + persist one document.
async fn ingest_text(store: &Store, embedder: &MockEmbedder, text: &str, title: &str, uri: &str) {
    let now = Utc::now();
    let document_id = Uuid::new_v4().to_string();
    let doc = Document {
        id: document_id.clone(),
        uri: uri.to_string(),
        title: title.to_string(),
        content: text.to_string(),
        metadata_json: "{}".to_string(),
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    store.upsert_document(&doc).expect("upsert_document");
    let chunker = from_config(2_000, 0);
    let pieces: Vec<(String, i32, i32)> = Chunker::chunk(&chunker, text);
    let texts: Vec<String> = pieces.iter().map(|(c, _, _)| c.clone()).collect();
    let embeddings = embedder.embed(&texts).await.expect("embed chunks");
    let chunks: Vec<Chunk> = pieces
        .into_iter()
        .zip(embeddings)
        .map(|((content, char_start, char_end), embedding)| Chunk {
            id: Uuid::new_v4().to_string(),
            document_id: document_id.clone(),
            chunk_index: 0,
            content,
            embedding,
            char_start,
            char_end,
            metadata_json: "{}".into(),
        })
        .enumerate()
        .map(|(i, mut chunk)| {
            chunk.chunk_index = i as i32;
            chunk
        })
        .collect();
    store.insert_chunks(&chunks).expect("insert_chunks");
}

fn command(query: &str, mode: &str, rewrite: bool, hyde: bool) -> SearchCommand {
    SearchCommand {
        query: query.into(),
        mode: Some(mode.into()),
        default_mode: SearchMode::Hybrid,
        top_k: Some(5),
        default_top_k: 5,
        document_id: None,
        wing: None,
        room: None,
        layer: None,
        source_file: None,
        include_archived: false,
        min_score: None,
        diversity: None,
        group_by: None,
        recency_half_life_days: None,
        max_context_tokens: None,
        max_chunks_per_document: None,
        context_expansion: None,
        neighbor_chunks: None,
        timeout_ms: Some(10_000),
        fts_stemmer: "porter".into(),
        rrf_k: None,
        rerank: None,
        rewrite: Some(rewrite),
        hyde: Some(hyde),
    }
}

fn reasons(hit: &rag_mcp::models::SearchHit) -> Vec<String> {
    hit.explanation
        .as_ref()
        .map(|e| e.reasons.clone())
        .unwrap_or_default()
}

#[tokio::test]
async fn rewrite_fuses_variant_lists_and_stamps_explanation() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);
    ingest_text(
        &store,
        &embedder,
        "quartz lattice resonance primer",
        "A",
        "file:///a.md",
    )
    .await;
    ingest_text(
        &store,
        &embedder,
        "onyx ledger reconciliation guide",
        "B",
        "file:///b.md",
    )
    .await;
    let config = test_config(&store);
    // The variant mentions only doc B's distinctive token.
    let stub = StubLlm {
        reply: "onyx ledger\nquartz primer".into(),
        calls: Arc::new(AtomicUsize::new(0)),
    };

    let baseline = execute_search_with_llm(
        &store,
        &embedder,
        &config,
        command("quartz lattice", "lex", false, false),
        None,
    )
    .await
    .expect("baseline search");
    let rewritten = execute_search_with_llm(
        &store,
        &embedder,
        &config,
        command("quartz lattice", "lex", true, false),
        Some(&stub),
    )
    .await
    .expect("rewrite search");

    assert_eq!(stub.calls.load(Ordering::Relaxed), 1);
    let titles: Vec<&str> = rewritten
        .iter()
        .map(|h| h.document_title.as_str())
        .collect();
    assert!(titles.contains(&"A"), "original hit kept: {titles:?}");
    assert!(
        titles.contains(&"B"),
        "variant-only hit fused in: {titles:?}"
    );
    assert!(baseline.iter().all(|h| h.document_title == "A"));
    let first = &rewritten[0];
    let r = reasons(first);
    assert!(r.contains(&"query_rewrite".to_string()), "{r:?}");
    assert!(r.contains(&"multi_query_rrf".to_string()), "{r:?}");
    assert!(first.explanation.as_ref().unwrap().rewrite_ms.is_some());
    assert!(first.score_rrf.is_some());
}

#[tokio::test]
async fn rewrite_llm_error_falls_back_to_plain_search() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);
    ingest_text(
        &store,
        &embedder,
        "fallback token needle",
        "A",
        "file:///fa.md",
    )
    .await;
    let config = test_config(&store);

    let baseline = execute_search_with_llm(
        &store,
        &embedder,
        &config,
        command("fallback needle", "lex", false, false),
        None,
    )
    .await
    .expect("baseline");
    let fallback = execute_search_with_llm(
        &store,
        &embedder,
        &config,
        command("fallback needle", "lex", true, false),
        Some(&FailingLlm),
    )
    .await
    .expect("fallback search must not fail");

    let base_ids: Vec<&str> = baseline.iter().map(|h| h.chunk_id.as_str()).collect();
    let fall_ids: Vec<&str> = fallback.iter().map(|h| h.chunk_id.as_str()).collect();
    assert_eq!(base_ids, fall_ids);
    assert!(reasons(&fallback[0]).contains(&"query_rewrite_fallback".to_string()));
}

#[tokio::test]
async fn rewrite_cache_avoids_repeat_llm_calls() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);
    ingest_text(&store, &embedder, "cached cobalt memo", "C", "file:///c.md").await;
    let config = test_config(&store);
    let stub = StubLlm {
        reply: "cobalt memo variant".into(),
        calls: Arc::new(AtomicUsize::new(0)),
    };

    for _ in 0..2 {
        execute_search_with_llm(
            &store,
            &embedder,
            &config,
            command("cached cobalt query", "lex", true, false),
            Some(&stub),
        )
        .await
        .expect("rewrite search");
    }
    assert_eq!(
        stub.calls.load(Ordering::Relaxed),
        1,
        "second call served from cache"
    );
}

#[tokio::test]
async fn hyde_adds_dense_leg_in_hybrid_mode() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);
    let config = test_config(&store);
    store.ensure_embedding_manifest(&config).expect("manifest");
    ingest_text(
        &store,
        &embedder,
        "velvet accretion dynamics overview",
        "H",
        "file:///h.md",
    )
    .await;
    let stub = StubLlm {
        reply: "A hypothetical passage about velvet accretion dynamics.".into(),
        calls: Arc::new(AtomicUsize::new(0)),
    };

    let hits = execute_search_with_llm(
        &store,
        &embedder,
        &config,
        command("velvet accretion", "hybrid", false, true),
        Some(&stub),
    )
    .await
    .expect("hyde search");

    assert_eq!(stub.calls.load(Ordering::Relaxed), 1);
    assert!(!hits.is_empty());
    let r = reasons(&hits[0]);
    assert!(r.contains(&"hyde".to_string()), "{r:?}");
    assert!(hits[0].explanation.as_ref().unwrap().hyde_ms.is_some());
}

#[tokio::test]
async fn hyde_llm_error_falls_back_to_plain_search() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);
    let config = test_config(&store);
    store.ensure_embedding_manifest(&config).expect("manifest");
    ingest_text(
        &store,
        &embedder,
        "mercury tide tables",
        "M",
        "file:///m.md",
    )
    .await;

    let baseline = execute_search_with_llm(
        &store,
        &embedder,
        &config,
        command("mercury tide", "hybrid", false, false),
        None,
    )
    .await
    .expect("baseline");
    let fallback = execute_search_with_llm(
        &store,
        &embedder,
        &config,
        command("mercury tide", "hybrid", false, true),
        Some(&FailingLlm),
    )
    .await
    .expect("hyde fallback must not fail");

    assert_eq!(fallback.len(), baseline.len());
    assert!(reasons(&fallback[0]).contains(&"hyde_fallback".to_string()));
}

#[tokio::test]
async fn hyde_in_lex_mode_is_a_documented_noop_fallback() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);
    ingest_text(
        &store,
        &embedder,
        "lex only garnet note",
        "L",
        "file:///l.md",
    )
    .await;
    let config = test_config(&store);
    let stub = StubLlm {
        reply: "unused".into(),
        calls: Arc::new(AtomicUsize::new(0)),
    };

    let hits = execute_search_with_llm(
        &store,
        &embedder,
        &config,
        command("lex garnet", "lex", false, true),
        Some(&stub),
    )
    .await
    .expect("lex hyde fallback");

    assert_eq!(
        stub.calls.load(Ordering::Relaxed),
        0,
        "no LLM call in lex mode"
    );
    assert!(reasons(&hits[0]).contains(&"hyde_fallback".to_string()));
}

#[tokio::test]
async fn rewrite_and_hyde_combine_as_extra_fused_legs() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);
    let config = test_config(&store);
    store.ensure_embedding_manifest(&config).expect("manifest");
    ingest_text(
        &store,
        &embedder,
        "ember drift hypothesis",
        "E",
        "file:///e.md",
    )
    .await;
    let stub = StubLlm {
        reply: "ember drift variant\nsecond variant".into(),
        calls: Arc::new(AtomicUsize::new(0)),
    };

    let hits = execute_search_with_llm(
        &store,
        &embedder,
        &config,
        command("ember drift", "hybrid", true, true),
        Some(&stub),
    )
    .await
    .expect("combined search");

    assert!(!hits.is_empty());
    let r = reasons(&hits[0]);
    assert!(r.contains(&"query_rewrite".to_string()), "{r:?}");
    assert!(r.contains(&"hyde".to_string()), "{r:?}");
    assert!(r.contains(&"multi_query_rrf".to_string()), "{r:?}");
    // One LLM call for rewrite, one for HyDE.
    assert_eq!(stub.calls.load(Ordering::Relaxed), 2);
}
