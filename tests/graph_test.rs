//! Integration tests for the object graph: wikilink stubs, tag nodes,
//! stub promote on title match, backlinks, and neighbors.
//!
//! Uses a temp DuckDB file and the deterministic mock embedder.

use chrono::Utc;
use rag_mcp::chunking::{from_config, Chunker};
use rag_mcp::embeddings::{EmbeddingProvider, MockEmbedder};
use rag_mcp::models::{Chunk, Document};
use rag_mcp::{rebuild_document_graph, Store, REL_TAGGED, REL_WIKILINK};
use tempfile::TempDir;
use uuid::Uuid;

const DIMS: usize = 32;
const CHUNK_SIZE: usize = 800;
const CHUNK_OVERLAP: usize = 120;

fn open_temp_store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("graph.duckdb");
    let store = Store::open(&path).expect("open store");
    (dir, store)
}

/// Chunk + mock-embed + persist a document, then rebuild its graph slice.
/// Returns `(document_id, graph_node_id, edge_count)`.
async fn ingest_with_graph(
    store: &Store,
    embedder: &MockEmbedder,
    text: &str,
    title: &str,
    uri: &str,
) -> (String, String, usize) {
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
    if !pieces.is_empty() {
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
        store.insert_chunks(&chunks).expect("insert_chunks");
    }

    let (node_id, edge_count) =
        rebuild_document_graph(store, &doc).expect("rebuild_document_graph");
    (document_id, node_id, edge_count)
}

/// Ingest A with `[[B]]` and `#idea` → stub B + tag idea → ingest B (title B)
/// → promote stub → backlinks from A → neighbors of A include B and tag.
#[tokio::test]
async fn ingest_stub_promote_backlinks_neighbors() {
    let (_dir, store) = open_temp_store();
    let embedder = MockEmbedder::new(DIMS);

    // --- 1. Ingest A linking to missing B and tagging #idea ---
    let text_a = "Note A discusses [[B]] under the #idea theme.";
    let (id_a, node_a, edges_a) = ingest_with_graph(
        &store,
        &embedder,
        text_a,
        "A",
        "doc://a",
    )
    .await;

    assert!(!id_a.is_empty());
    assert!(!node_a.is_empty());
    assert_eq!(
        edges_a, 2,
        "A should emit one wikilink edge and one tagged edge"
    );

    let node_a_row = store
        .find_node_by_document_id(&id_a)
        .expect("find A node")
        .expect("A document node");
    assert_eq!(node_a_row.id, node_a);
    assert_eq!(node_a_row.kind, "document");
    assert!(node_a_row.resolved);
    assert_eq!(node_a_row.label, "A");

    // --- 2. Stub B created for unresolved [[B]] ---
    let stubs_b = store.find_nodes_by_label("B").expect("find B");
    assert_eq!(stubs_b.len(), 1, "exactly one node labeled B");
    let stub_b = &stubs_b[0];
    assert_eq!(stub_b.kind, "stub");
    assert!(!stub_b.resolved);
    assert!(stub_b.document_id.is_none());
    let stub_b_id = stub_b.id.clone();

    // Tag #idea
    let tags = store.find_nodes_by_label("idea").expect("find idea tag");
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].kind, "tag");
    assert!(tags[0].resolved);
    let tag_idea_id = tags[0].id.clone();

    // Outgoing edges from A: wikilink → stub B, tagged → idea
    let view_a = store
        .neighbors(&node_a, 1, 50)
        .expect("neighbors of A after first ingest");
    assert!(
        view_a.nodes.iter().any(|n| n.id == stub_b_id),
        "neighbors of A should include stub B"
    );
    assert!(
        view_a.nodes.iter().any(|n| n.id == tag_idea_id),
        "neighbors of A should include tag #idea"
    );
    assert!(
        view_a
            .edges
            .iter()
            .any(|e| e.source_id == node_a && e.target_id == stub_b_id && e.rel_type == REL_WIKILINK),
        "expected wikilink A → B, edges={:?}",
        view_a.edges
    );
    assert!(
        view_a
            .edges
            .iter()
            .any(|e| e.source_id == node_a && e.target_id == tag_idea_id && e.rel_type == REL_TAGGED),
        "expected tagged A → idea, edges={:?}",
        view_a.edges
    );

    // --- 3. Ingest B with title B → promote the stub (stable node id) ---
    let text_b = "I am document B, the target of A's wikilink.";
    let (id_b, node_b, _edges_b) = ingest_with_graph(
        &store,
        &embedder,
        text_b,
        "B",
        "doc://b",
    )
    .await;

    assert_eq!(
        node_b, stub_b_id,
        "promoting stub must keep the same graph node id"
    );

    let promoted = store
        .find_node_by_id(&stub_b_id)
        .expect("find promoted")
        .expect("promoted node");
    assert_eq!(promoted.kind, "document");
    assert!(promoted.resolved);
    assert_eq!(promoted.document_id.as_deref(), Some(id_b.as_str()));
    assert_eq!(promoted.label, "B");
    assert_eq!(promoted.uri.as_deref(), Some("doc://b"));

    // No leftover stub for label B
    let b_nodes = store.find_nodes_by_label("B").expect("find B after promote");
    assert!(
        b_nodes.iter().all(|n| n.kind == "document" && n.resolved),
        "B must be a resolved document, not a stub: {:?}",
        b_nodes
    );

    // --- 4. Backlinks: B is targeted by A's wikilink ---
    let back = store.backlinks(&node_b).expect("backlinks of B");
    assert!(
        back.edges
            .iter()
            .any(|e| e.source_id == node_a && e.target_id == node_b && e.rel_type == REL_WIKILINK),
        "backlinks of B should include wikilink from A, edges={:?}",
        back.edges
    );
    assert!(
        back.nodes.iter().any(|n| n.id == node_a),
        "backlink source node A should be present"
    );
    assert!(
        back.nodes.iter().any(|n| n.id == node_b),
        "target node B should be present in backlinks view"
    );

    // --- 5. Neighbors of A still reach promoted B and #idea ---
    let neigh = store.neighbors(&node_a, 1, 50).expect("neighbors of A");
    assert!(
        neigh.nodes.iter().any(|n| n.id == node_b && n.kind == "document"),
        "neighbors of A should include promoted document B"
    );
    assert!(
        neigh.nodes.iter().any(|n| n.id == tag_idea_id && n.kind == "tag"),
        "neighbors of A should still include tag idea"
    );

    // Undirected: from B, depth 1 should reach A
    let from_b = store.neighbors(&node_b, 1, 50).expect("neighbors of B");
    assert!(
        from_b.nodes.iter().any(|n| n.id == node_a),
        "undirected neighbors of B should include A"
    );

    // Graph stats sanity
    let stats = store.graph_stats().expect("graph_stats");
    assert!(stats.total_nodes >= 3, "at least A, B, idea: {:?}", stats);
    assert!(
        stats.edges_by_rel_type.get(REL_WIKILINK).copied().unwrap_or(0) >= 1
    );
    assert!(
        stats.edges_by_rel_type.get(REL_TAGGED).copied().unwrap_or(0) >= 1
    );
    assert_eq!(
        stats.nodes_by_kind.get("stub").copied().unwrap_or(0),
        0,
        "no unresolved stubs remain for this scenario"
    );
}
