//! Hybrid / lexical search integration tests with mock embeddings.
//!
//! Covers:
//! - ingest docs with distinctive keywords
//! - `mode=lex` ranks keyword hits
//! - `mode=hybrid` (RRF of lex + vec) surfaces those hits
//! - diversity `collapse_by_document` + `max_chunks_per_document`
//!
//! Lex uses DuckDB FTS BM25 when available; otherwise the same API falls back
//! to term-frequency ranking in Rust (read-your-writes either way).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use rag_mcp::chunking::{from_config, Chunker};
use rag_mcp::db::search::{search, DiversityMode, SearchQuery};
use rag_mcp::embeddings::{EmbeddingProvider, MockEmbedder};
use rag_mcp::models::{Chunk, Document, SearchMode};
use rag_mcp::Store;
use tempfile::TempDir;
use uuid::Uuid;

const DIMS: usize = 32;
/// Small chunks so one document can yield multiple hits for diversity tests.
const CHUNK_SIZE: usize = 80;
const CHUNK_OVERLAP: usize = 10;

fn open_temp_store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("hybrid.duckdb");
    let store = Store::open(&path).expect("open store");
    (dir, store)
}

/// Chunk + mock-embed + persist. Returns `(document_id, chunk_count)`.
async fn ingest_text(
    store: &Store,
    embedder: &MockEmbedder,
    text: &str,
    title: &str,
    uri: &str,
) -> (String, usize) {
    let now = Utc::now();
    let document_id = Uuid::new_v4().to_string();

    if let Some(existing) = store.find_by_uri(uri).expect("find_by_uri") {
        store
            .delete_document(&existing.id)
            .expect("delete previous by uri");
    }

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

    let chunker = from_config(CHUNK_SIZE, CHUNK_OVERLAP);
    let pieces: Vec<(String, i32, i32)> = Chunker::chunk(&chunker, text);
    if pieces.is_empty() {
        return (document_id, 0);
    }

    let texts: Vec<String> = pieces.iter().map(|(c, _, _)| c.clone()).collect();
    let embeddings = embedder.embed(&texts).await.expect("embed chunks");
    assert_eq!(embeddings.len(), pieces.len());

    let mut chunks = Vec::with_capacity(pieces.len());
    for (i, ((content, char_start, char_end), embedding)) in
        pieces.into_iter().zip(embeddings.into_iter()).enumerate()
    {
        chunks.push(Chunk {
            id: Uuid::new_v4().to_string(),
            document_id: document_id.clone(),
            chunk_index: i as i32,
            content,
            embedding,
            char_start,
            char_end,
        });
    }

    let chunk_count = chunks.len();
    store.insert_chunks(&chunks).expect("insert_chunks");
    (document_id, chunk_count)
}

#[tokio::test]
async fn lex_finds_distinctive_keywords() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);

    // Rare tokens that will not appear in the other docs.
    let text_xylo =
        "The xylophone quantum resonance protocol uses rare earth magnets and zephyr coils.";
    let text_banana =
        "Banana farming in tropical climates requires consistent rainfall patterns and shade.";
    let text_quokka =
        "The quokka is a small marsupial found on Rottnest Island near Perth Australia.";

    let (id_xylo, n_xylo) = ingest_text(
        &store,
        &embedder,
        text_xylo,
        "Xylophone Protocol",
        "doc://xylophone",
    )
    .await;
    let (id_banana, _) = ingest_text(
        &store,
        &embedder,
        text_banana,
        "Banana Farming",
        "doc://banana",
    )
    .await;
    let (id_quokka, _) = ingest_text(
        &store,
        &embedder,
        text_quokka,
        "Quokka Notes",
        "doc://quokka",
    )
    .await;

    assert!(n_xylo >= 1);
    assert_ne!(id_xylo, id_banana);
    assert_ne!(id_xylo, id_quokka);

    // search() rebuilds FTS / uses TF fallback before ranking (read-your-writes).
    let hits = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Lex,
            top_k: 5,
            query_text: Some("xylophone zephyr".into()),
            ..SearchQuery::default()
        },
    )
    .expect("lex search");

    assert!(
        !hits.is_empty(),
        "lex search should return hits for distinctive keywords"
    );
    assert_eq!(
        hits[0].document_id, id_xylo,
        "top lex hit should be xylophone doc, got title={:?} uri={:?}",
        hits[0].document_title, hits[0].document_uri
    );
    assert!(
        hits[0].score_lex.is_some(),
        "lex mode must set score_lex"
    );
    assert!(hits[0].score > 0.0);
    let content_lower = hits[0].content.to_lowercase();
    assert!(
        content_lower.contains("xylophone") || content_lower.contains("zephyr"),
        "hit content should carry query keywords: {:?}",
        hits[0].content
    );

    // Quokka keyword must surface the quokka document.
    let q_hits = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Lex,
            top_k: 3,
            query_text: Some("quokka marsupial".into()),
            ..SearchQuery::default()
        },
    )
    .expect("lex quokka");
    assert!(!q_hits.is_empty());
    assert_eq!(q_hits[0].document_id, id_quokka);
}

#[tokio::test]
async fn hybrid_finds_distinctive_keywords_with_mock_embeddings() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);

    let text_a = "Nebulium thrusters propel the starship through the asteroid belt.";
    let text_b = "Tomato soup recipes call for basil cream and croutons.";

    let (id_a, _) = ingest_text(
        &store,
        &embedder,
        text_a,
        "Nebulium Thrusters",
        "doc://nebulium",
    )
    .await;
    let (id_b, _) = ingest_text(
        &store,
        &embedder,
        text_b,
        "Tomato Soup",
        "doc://tomato",
    )
    .await;
    assert_ne!(id_a, id_b);

    // Mock embeddings are hash-based: exact query text matches doc A embedding.
    let query = "Nebulium thrusters propel the starship through the asteroid belt.";
    let query_emb = embedder.embed_one(query);

    let hits = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Hybrid,
            top_k: 5,
            query_text: Some("nebulium thrusters".into()),
            query_embedding: Some(query_emb),
            ..SearchQuery::default()
        },
    )
    .expect("hybrid search");

    assert!(
        !hits.is_empty(),
        "hybrid search should return at least one hit"
    );
    assert_eq!(
        hits[0].document_id, id_a,
        "hybrid top hit should be nebulium doc, got title={:?} uri={:?}",
        hits[0].document_title, hits[0].document_uri
    );
    assert!(
        hits[0].score_rrf.is_some(),
        "hybrid hits must set score_rrf"
    );
    assert!(hits[0].score > 0.0);
    // Fusion should attach at least one of the component scores when present.
    assert!(
        hits[0].score_vec.is_some() || hits[0].score_lex.is_some(),
        "expected score_vec and/or score_lex on hybrid hit"
    );
    assert!(
        hits[0].content.to_lowercase().contains("nebulium")
            || hits[0].content.to_lowercase().contains("thrusters"),
        "top hybrid hit content: {:?}",
        hits[0].content
    );
    if hits[0].snippet.is_some() {
        // Snippets are optional depending on finalize path; if set, non-empty.
        assert!(!hits[0].snippet.as_ref().unwrap().is_empty());
    }
}

#[tokio::test]
async fn diversity_collapse_by_document_limits_chunks() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);

    // Long multi-chunk document: same rare keyword repeated so many chunks match.
    // CHUNK_SIZE=80 → several chunks, each mentioning "klystron".
    let mut long_doc = String::new();
    for i in 0..12 {
        long_doc.push_str(&format!(
            "Section {i}: the klystron amplifier circuit design notes for laboratory use. "
        ));
    }
    let other = "Completely unrelated gardening tips about roses and tulips in spring.";

    let (id_kly, n_kly) = ingest_text(
        &store,
        &embedder,
        &long_doc,
        "Klystron Notes",
        "doc://klystron",
    )
    .await;
    let (id_garden, _) = ingest_text(
        &store,
        &embedder,
        other,
        "Garden Tips",
        "doc://garden",
    )
    .await;

    assert!(
        n_kly >= 3,
        "expected multi-chunk klystron doc, got {n_kly} chunks"
    );

    // Without collapse: many hits can share the same document_id.
    let uncapped = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Lex,
            top_k: 20,
            query_text: Some("klystron amplifier".into()),
            diversity: None,
            max_chunks_per_document: None,
            ..SearchQuery::default()
        },
    )
    .expect("lex uncapped");

    assert!(!uncapped.is_empty());
    let from_kly_uncapped = uncapped
        .iter()
        .filter(|h| h.document_id == id_kly)
        .count();
    assert!(
        from_kly_uncapped >= 2,
        "need multiple klystron chunks before collapse (got {from_kly_uncapped})"
    );

    // Explicit collapse_by_document with max 1.
    let collapsed = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Lex,
            top_k: 20,
            query_text: Some("klystron amplifier".into()),
            diversity: Some(DiversityMode::CollapseByDocument),
            max_chunks_per_document: Some(1),
            ..SearchQuery::default()
        },
    )
    .expect("lex collapsed");

    assert!(!collapsed.is_empty());
    let mut per_doc: HashMap<String, usize> = HashMap::new();
    for h in &collapsed {
        *per_doc.entry(h.document_id.clone()).or_insert(0) += 1;
    }
    for (doc_id, count) in &per_doc {
        assert!(
            *count <= 1,
            "collapse max=1 violated for doc {doc_id}: {count} hits"
        );
    }
    assert!(
        collapsed.iter().any(|h| h.document_id == id_kly),
        "klystron doc should still appear after collapse"
    );

    // Cap of 2 retains up to two chunks from the multi-chunk doc.
    let cap2 = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Lex,
            top_k: 20,
            query_text: Some("klystron".into()),
            diversity: Some(DiversityMode::CollapseByDocument),
            max_chunks_per_document: Some(2),
            ..SearchQuery::default()
        },
    )
    .expect("lex cap2");
    let kly_cap2 = cap2.iter().filter(|h| h.document_id == id_kly).count();
    assert!(
        kly_cap2 <= 2,
        "max_chunks_per_document=2 violated: {kly_cap2}"
    );
    assert!(kly_cap2 >= 1);

    // Hybrid + collapse: max 1 per document (also via max_chunks without diversity enum).
    let query_emb = embedder.embed_one(&long_doc);
    let hybrid_collapsed = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Hybrid,
            top_k: 10,
            query_text: Some("klystron amplifier circuit".into()),
            query_embedding: Some(query_emb),
            diversity: Some(DiversityMode::CollapseByDocument),
            max_chunks_per_document: Some(1),
            ..SearchQuery::default()
        },
    )
    .expect("hybrid collapsed");

    assert!(!hybrid_collapsed.is_empty());
    let mut seen: HashSet<String> = HashSet::new();
    for h in &hybrid_collapsed {
        assert!(
            seen.insert(h.document_id.clone()),
            "duplicate document_id after hybrid collapse: {}",
            h.document_id
        );
        if h.score_rrf.is_some() {
            assert!(h.score > 0.0);
        }
    }
    // Garden doc is unrelated; klystron must be present.
    assert!(hybrid_collapsed.iter().any(|h| h.document_id == id_kly));
    let _ = id_garden; // kept for symmetry / future filter asserts
}

#[tokio::test]
async fn hybrid_requires_text_and_embedding() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);

    let (_id, _) = ingest_text(
        &store,
        &embedder,
        "simple payload with keyword foobar",
        "Simple",
        "doc://simple",
    )
    .await;

    let missing_text = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Hybrid,
            top_k: 3,
            query_text: None,
            query_embedding: Some(embedder.embed_one("foobar")),
            ..SearchQuery::default()
        },
    );
    assert!(
        missing_text.is_err(),
        "hybrid without query_text should error"
    );

    let missing_emb = search(
        &store,
        &SearchQuery {
            mode: SearchMode::Hybrid,
            top_k: 3,
            query_text: Some("foobar".into()),
            query_embedding: None,
            ..SearchQuery::default()
        },
    );
    assert!(
        missing_emb.is_err(),
        "hybrid without query_embedding should error"
    );
}

/// Smoke: Arc store + concurrent-style sequential lex/hybrid calls stay consistent.
#[tokio::test]
async fn store_arc_lex_and_hybrid_smoke() {
    let (_dir, store) = open_temp_store();
    let store = Arc::new(store);
    let embedder = MockEmbedder::new(DIMS);

    let (id, _) = ingest_text(
        store.as_ref(),
        &embedder,
        "Zyzzyva is a tropical American weevil genus used as a distinctive token.",
        "Zyzzyva",
        "doc://zyzzyva",
    )
    .await;

    let lex = search(
        store.as_ref(),
        &SearchQuery {
            mode: SearchMode::Lex,
            top_k: 3,
            query_text: Some("zyzzyva weevil".into()),
            ..SearchQuery::default()
        },
    )
    .expect("lex");
    assert!(!lex.is_empty());
    assert_eq!(lex[0].document_id, id);

    let emb = embedder.embed_one("Zyzzyva is a tropical American weevil genus used as a distinctive token.");
    let hybrid = search(
        store.as_ref(),
        &SearchQuery {
            mode: SearchMode::Hybrid,
            top_k: 3,
            query_text: Some("zyzzyva".into()),
            query_embedding: Some(emb),
            diversity: Some(DiversityMode::CollapseByDocument),
            max_chunks_per_document: Some(1),
            ..SearchQuery::default()
        },
    )
    .expect("hybrid");
    assert!(!hybrid.is_empty());
    assert_eq!(hybrid[0].document_id, id);
    assert!(hybrid[0].score_rrf.is_some());
}
