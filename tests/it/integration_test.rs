//! Integration tests: DuckDB store + mock embedder roundtrip.
//!
//! Covers ingest (chunk → embed → store), semantic search ranking,
//! delete, and stats against a temp-dir database.

use std::sync::Arc;

use chrono::Utc;
use rag_mcp::chunking::{from_config, Chunker};
use rag_mcp::db::search::search_chunks;
use rag_mcp::embeddings::{MockEmbedder, EmbeddingProvider};
use rag_mcp::models::{Chunk, Document};
use rag_mcp::Store;
use tempfile::TempDir;
use uuid::Uuid;

const DIMS: usize = 32;
const CHUNK_SIZE: usize = 800;
const CHUNK_OVERLAP: usize = 120;

/// Open a store whose file lives under a keep-alive temp directory.
fn open_temp_store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("integration.duckdb");
    let store = Store::open(&path).expect("open store");
    (dir, store)
}

/// Chunk + mock-embed + persist a document. Returns `(document_id, chunk_count)`.
async fn ingest_text(
    store: &Store,
    embedder: &MockEmbedder,
    text: &str,
    title: &str,
    uri: &str,
) -> (String, usize) {
    let now = Utc::now();
    let document_id = Uuid::new_v4().to_string();

    // Re-ingest by uri: drop previous doc+chunks if present.
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
    assert_eq!(
        embeddings.len(),
        pieces.len(),
        "embedder must return one vector per chunk"
    );

    let mut chunks = Vec::with_capacity(pieces.len());
    for (i, ((content, char_start, char_end), embedding)) in
        pieces.into_iter().zip(embeddings).enumerate()
    {
        chunks.push(Chunk {
            id: Uuid::new_v4().to_string(),
            document_id: document_id.clone(),
            chunk_index: i as i32,
            content,
            embedding,
            char_start,
            char_end,
            metadata_json: "{}".into(),
        });
    }

    let chunk_count = chunks.len();
    store.insert_chunks(&chunks).expect("insert_chunks");
    (document_id, chunk_count)
}

#[tokio::test]
async fn ingest_search_delete_stats_roundtrip() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);

    // Empty store stats.
    let (docs0, chunks0, nodes0, edges0) = store.stats().expect("stats empty");
    assert_eq!(docs0, 0);
    assert_eq!(chunks0, 0);
    assert_eq!(nodes0, 0);
    assert_eq!(edges0, 0);

    // Two clearly distinct texts so mock (hash) embeddings differ.
    // Search uses the same text as doc A so cosine(query, A) == 1.0 and
    // ranks above doc B (mock embeddings are deterministic hash, not semantic).
    let text_a = "The xylophone quantum resonance protocol uses rare earth magnets.";
    let text_b = "Banana farming in tropical climates requires consistent rainfall patterns.";

    let (id_a, n_a) = ingest_text(
        &store,
        &embedder,
        text_a,
        "Xylophone Protocol",
        "doc://xylophone",
    )
    .await;
    let (id_b, n_b) = ingest_text(
        &store,
        &embedder,
        text_b,
        "Banana Farming",
        "doc://banana",
    )
    .await;

    assert!(n_a >= 1, "doc A should produce at least one chunk");
    assert!(n_b >= 1, "doc B should produce at least one chunk");
    assert_ne!(id_a, id_b);

    let (docs1, chunks1, _, _) = store.stats().expect("stats after ingest");
    assert_eq!(docs1, 2);
    assert_eq!(chunks1, (n_a + n_b) as u64);

    // Distinctive query matching doc A content exactly → top hit must be A.
    let query = text_a.to_string();
    let query_emb = embedder
        .embed(&[query])
        .await
        .expect("embed query")
        .into_iter()
        .next()
        .expect("query vector");

    let hits = search_chunks(&store, &query_emb, 5, None).expect("search");
    assert!(
        !hits.is_empty(),
        "search should return at least one hit after ingest"
    );
    assert_eq!(
        hits[0].document_id, id_a,
        "top hit should be from the xylophone document, got title={:?} uri={:?}",
        hits[0].document_title, hits[0].document_uri
    );
    assert_eq!(hits[0].document_title, "Xylophone Protocol");
    assert!(
        (hits[0].score - 1.0).abs() < 1e-5,
        "exact-text mock query should score ~1.0, got {}",
        hits[0].score
    );
    assert!(
        hits[0].content.contains("xylophone") || hits[0].content.contains("Xylophone")
            || hits[0].content.contains("quantum resonance"),
        "top hit content should reflect distinctive terms: {:?}",
        hits[0].content
    );

    // Filter by document_id limits results to that doc.
    let filtered = search_chunks(&store, &query_emb, 5, Some(&id_b)).expect("search filtered");
    for hit in &filtered {
        assert_eq!(hit.document_id, id_b);
    }

    // Delete A; B remains.
    assert!(store.delete_document(&id_a).expect("delete A"));
    assert!(store.get_document(&id_a).expect("get A").is_none());
    assert!(store
        .list_chunks_for_document(&id_a)
        .expect("chunks A")
        .is_empty());

    let (docs2, chunks2, _, _) = store.stats().expect("stats after delete A");
    assert_eq!(docs2, 1);
    assert_eq!(chunks2, n_b as u64);
    assert!(store.get_document(&id_b).expect("get B").is_some());

    // Search after delete should not surface A.
    let hits_after = search_chunks(&store, &query_emb, 5, None).expect("search after delete");
    for hit in &hits_after {
        assert_ne!(hit.document_id, id_a);
    }

    // Delete remaining and confirm empty stats.
    assert!(store.delete_document(&id_b).expect("delete B"));
    let (docs3, chunks3, _, _) = store.stats().expect("stats empty again");
    assert_eq!(docs3, 0);
    assert_eq!(chunks3, 0);
    assert!(!store.delete_document(&id_b).expect("delete missing"));
}

#[tokio::test]
async fn reingest_same_uri_replaces_content() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);

    let uri = "doc://stable-uri";
    let (id1, n1) = ingest_text(&store, &embedder, "first version alpha", "V1", uri).await;
    assert_eq!(n1, 1);

    // Manual re-ingest mimicking pipeline: delete by uri then insert fresh id.
    // Our helper deletes by uri first, so id may change; document count stays 1.
    let (id2, n2) = ingest_text(&store, &embedder, "second version beta", "V2", uri).await;
    assert_eq!(n2, 1);
    assert_ne!(id1, id2, "helper assigns a new uuid after delete");

    let listed = store.list_documents().expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id2);
    assert_eq!(listed[0].content, "second version beta");

    let (docs, chunks, _, _) = store.stats().expect("stats");
    assert_eq!(docs, 1);
    assert_eq!(chunks, 1);
}

#[tokio::test]
async fn mock_embedder_via_provider_trait_is_deterministic() {
    // Ensure Arc<dyn EmbeddingProvider> path used by the server works the same.
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let a = provider
        .embed(&[String::from("shared query text")])
        .await
        .unwrap();
    let b = provider
        .embed(&[String::from("shared query text")])
        .await
        .unwrap();
    assert_eq!(a, b);
    assert_eq!(a[0].len(), 16);
    assert_eq!(provider.dimensions(), 16);
}

#[tokio::test]
async fn search_on_empty_store_returns_no_hits() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);
    let q = embedder.embed_one("anything");
    let hits = search_chunks(&store, &q, 5, None).expect("search empty");
    assert!(hits.is_empty());
}

#[tokio::test]
async fn multi_chunk_document_search_still_finds_doc() {
    let (_dir, store) = open_temp_store();
    // Small windows force multiple chunks.
    let embedder = MockEmbedder::new(DIMS);
    let now = Utc::now();
    let document_id = Uuid::new_v4().to_string();
    let text = "alpha word one. beta word two. gamma word three. delta word four. epsilon word five.";

    let doc = Document {
        id: document_id.clone(),
        uri: "doc://multi".into(),
        title: "Multi".into(),
        content: text.into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    store.upsert_document(&doc).unwrap();

    let chunker = from_config(20, 4);
    let pieces = Chunker::chunk(&chunker, text);
    assert!(pieces.len() > 1, "expected multi-chunk split, got {}", pieces.len());

    let texts: Vec<String> = pieces.iter().map(|(c, _, _)| c.clone()).collect();
    let embeddings = embedder.embed(&texts).await.unwrap();
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

    // Use first chunk text as query so score is exact for that chunk.
    let query_text = chunks[0].content.clone();
    store.insert_chunks(&chunks).unwrap();

    let query_emb = embedder.embed_one(&query_text);
    let hits = search_chunks(&store, &query_emb, 3, None).unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].document_id, document_id);
    assert!((hits[0].score - 1.0).abs() < 1e-5);
}
