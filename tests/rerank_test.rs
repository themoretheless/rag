//! Rerank stage integration tests with mock embeddings and a temp store.
//!
//! Covers:
//! - `rerank=true` with the deterministic mock provider reorders hits and
//!   stamps `score_rerank` (triple-score pattern preserved).
//! - Provider failure (unreachable openai_compat endpoint) falls back to the
//!   un-reranked ranking with a `rerank_fallback` explanation reason.
//! - Default-off: `rerank=true` with `RAG_RERANK_PROVIDER=off` is a no-op.

use chrono::Utc;
use rag_mcp::chunking::{from_config, Chunker};
use rag_mcp::embeddings::{EmbeddingProvider, MockEmbedder};
use rag_mcp::models::{Chunk, Document, SearchMode};
use rag_mcp::rerank::RerankProviderKind;
use rag_mcp::retrieval::{execute_search, SearchCommand};
use rag_mcp::{Config, Store};
use tempfile::TempDir;
use uuid::Uuid;

const DIMS: usize = 32;

fn open_temp_store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("rerank.duckdb")).expect("open store");
    (dir, store)
}

/// Chunk + mock-embed + persist as a single chunk per document.
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
    assert!(!pieces.is_empty());
    let texts: Vec<String> = pieces.iter().map(|(c, _, _)| c.clone()).collect();
    let embeddings = embedder.embed(&texts).await.expect("embed chunks");
    let chunks: Vec<Chunk> = pieces
        .into_iter()
        .zip(embeddings)
        .enumerate()
        .map(|(i, ((content, char_start, char_end), embedding))| Chunk {
            id: Uuid::new_v4().to_string(),
            document_id: document_id.clone(),
            chunk_index: i as i32,
            content,
            embedding,
            char_start,
            char_end,
            metadata_json: "{}".into(),
        })
        .collect();
    store.insert_chunks(&chunks).expect("insert_chunks");
}

fn search_command(query: &str, rerank: Option<bool>) -> SearchCommand {
    SearchCommand {
        query: query.into(),
        mode: Some("lex".into()),
        default_mode: SearchMode::Lex,
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
        timeout_ms: Some(5_000),
        fts_stemmer: "porter".into(),
        rrf_k: None,
        rerank,
        rewrite: None,
        hyde: None,
    }
}

/// Corpus where the lexical baseline and the token-overlap mock reranker
/// disagree on the top hit:
/// - doc A repeats `alpha` many times (high lexical tf) amid filler tokens
///   (low Jaccard overlap with the query),
/// - doc B mentions only `beta` once in a minimal body (lower lexical score,
///   but higher Jaccard overlap with the two-term query).
async fn seed_divergent_corpus(store: &Store, embedder: &MockEmbedder) {
    let mut doc_a = String::new();
    for _ in 0..40 {
        doc_a.push_str("alpha ");
    }
    for i in 0..10 {
        doc_a.push_str(&format!("filler{i} "));
    }
    let doc_b = "beta";
    ingest_text(store, embedder, &doc_a, "Doc A", "doc://a").await;
    ingest_text(store, embedder, doc_b, "Doc B", "doc://b").await;
}

fn rerank_config(provider: RerankProviderKind) -> Config {
    Config {
        rerank_provider: provider,
        rerank_base_url: match provider {
            RerankProviderKind::OpenAiCompat => "http://127.0.0.1:9".into(),
            _ => String::new(),
        },
        rerank_model: match provider {
            RerankProviderKind::OpenAiCompat => "rerank-model".into(),
            _ => String::new(),
        },
        rerank_top_n: 50,
        ..Config::for_tests()
    }
}

#[tokio::test]
async fn rerank_changes_order_and_stamps_score() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);
    seed_divergent_corpus(&store, &embedder).await;
    let config = rerank_config(RerankProviderKind::Mock);

    let baseline = execute_search(
        &store,
        &embedder,
        &config,
        search_command("alpha beta", None),
    )
    .await
    .expect("baseline search");
    assert_eq!(baseline.len(), 2);
    assert_eq!(baseline[0].document_title, "Doc A");
    assert!(baseline.iter().all(|h| h.score_rerank.is_none()));

    let reranked = execute_search(
        &store,
        &embedder,
        &config,
        search_command("alpha beta", Some(true)),
    )
    .await
    .expect("reranked search");
    assert_eq!(reranked.len(), 2);
    assert_eq!(reranked[0].document_title, "Doc B");
    // Rerank score present, descending, and the primary score untouched.
    assert!(reranked.iter().all(|h| h.score_rerank.is_some()));
    assert!(reranked[0].score_rerank.unwrap() >= reranked[1].score_rerank.unwrap());
    assert!(reranked.iter().all(|h| h
        .explanation
        .as_ref()
        .unwrap()
        .reasons
        .contains(&"rerank".to_string())));
    let baseline_scores: Vec<(String, f32)> = baseline
        .iter()
        .map(|h| (h.document_title.clone(), h.score))
        .collect();
    let reranked_scores: Vec<(String, f32)> = reranked
        .iter()
        .map(|h| (h.document_title.clone(), h.score))
        .collect();
    let mut sorted_baseline = baseline_scores.clone();
    sorted_baseline.sort_by(|a, b| a.0.cmp(&b.0));
    let mut sorted_reranked = reranked_scores.clone();
    sorted_reranked.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(sorted_baseline, sorted_reranked);
}

#[tokio::test]
async fn rerank_provider_failure_falls_back_to_unreranked_order() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);
    seed_divergent_corpus(&store, &embedder).await;
    // Port 9 (discard) refuses connections: the provider errors every time.
    let config = rerank_config(RerankProviderKind::OpenAiCompat);

    let baseline = execute_search(
        &store,
        &embedder,
        &config,
        search_command("alpha beta", None),
    )
    .await
    .expect("baseline search");
    let fallback = execute_search(
        &store,
        &embedder,
        &config,
        search_command("alpha beta", Some(true)),
    )
    .await
    .expect("search must not fail because of the reranker");

    let baseline_order: Vec<&str> = baseline.iter().map(|h| h.chunk_id.as_str()).collect();
    let fallback_order: Vec<&str> = fallback.iter().map(|h| h.chunk_id.as_str()).collect();
    assert_eq!(baseline_order, fallback_order);
    assert!(fallback.iter().all(|h| h.score_rerank.is_none()));
    assert!(fallback.iter().all(|h| h
        .explanation
        .as_ref()
        .unwrap()
        .reasons
        .contains(&"rerank_fallback".to_string())));
}

#[tokio::test]
async fn rerank_param_is_noop_when_provider_off() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);
    seed_divergent_corpus(&store, &embedder).await;
    let config = rerank_config(RerankProviderKind::Off);

    let baseline = execute_search(
        &store,
        &embedder,
        &config,
        search_command("alpha beta", None),
    )
    .await
    .expect("baseline search");
    let requested = execute_search(
        &store,
        &embedder,
        &config,
        search_command("alpha beta", Some(true)),
    )
    .await
    .expect("rerank=true with provider off");

    let baseline_order: Vec<&str> = baseline.iter().map(|h| h.chunk_id.as_str()).collect();
    let requested_order: Vec<&str> = requested.iter().map(|h| h.chunk_id.as_str()).collect();
    assert_eq!(baseline_order, requested_order);
    assert!(requested.iter().all(|h| h.score_rerank.is_none()));
}
