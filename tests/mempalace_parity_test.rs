//! MemPalace feature parity integration tests.
//!
//! Covers palace taxonomy (wing/room), content-hash dedupe, delete_by_source,
//! temporal KG (add/query/invalidate/supersede), diary write/read, wake_up,
//! and tunnel create/follow. Uses a temp DuckDB file and the mock embedder.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use rag_mcp::config::EmbeddingProviderKind;
use rag_mcp::embeddings::{EmbeddingProvider, MockEmbedder};
use rag_mcp::models::{Document, GraphNode, SearchMode, StatusReport};
use rag_mcp::{
    content_hash, diary_read, diary_write, wake_up, Config, Store, REL_TUNNEL,
};
use tempfile::TempDir;
use uuid::Uuid;

const DIMS: usize = 32;

fn open_temp_store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mempalace_parity.duckdb");
    let store = Store::open(&path).expect("open store");
    (dir, store)
}

fn test_config(db_path: PathBuf, dims: usize) -> Config {
    Config {
        db_path,
        embedding_provider: EmbeddingProviderKind::Mock,
        embedding_base_url: "https://api.openai.com/v1".to_string(),
        embedding_api_key: String::new(),
        embedding_model: "mock".to_string(),
        embedding_dims: dims,
        chunk_size: 800,
        chunk_overlap: 120,
        default_top_k: 5,
        ingest_roots: Vec::new(),
        max_context_tokens: 4096,
        max_chunks_per_doc: 3,
        fts_stemmer: "porter".to_string(),
        default_search_mode: SearchMode::Vec,
        llm_provider: rag_mcp::llm::LlmProviderKind::Ollama,
        llm_base_url: "http://127.0.0.1:11434/v1".to_string(),
        llm_model: "llama3.2".to_string(),
        llm_api_key: "ollama".to_string(),
        llm_enabled: false,
        llm_timeout_secs: 120,
        llm_max_tokens: 4096,
        maint_max_docs: 50,
        maint_near_dup_threshold: 0.92,
        tool_surface: rag_mcp::mcp::ToolSurface::Full,
        http_bind: None,
            wiki_require_if_match: false,
    }
}

fn ts(y: i32, m: u32, d: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
}

fn upsert_doc(
    store: &Store,
    uri: &str,
    title: &str,
    content: &str,
    wing: Option<&str>,
    room: Option<&str>,
    source_file: Option<&str>,
    hash: Option<&str>,
) -> Document {
    let now = Utc::now();
    let doc = Document {
        id: Uuid::new_v4().to_string(),
        uri: uri.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        metadata_json: "{}".to_string(),
        created_at: now,
        updated_at: now,
        wing: wing.map(str::to_string),
        room: room.map(str::to_string),
        source_file: source_file.map(str::to_string),
        content_hash: hash.map(str::to_string),
        ..Default::default()
    };
    store.upsert_document(&doc).expect("upsert_document");
    doc
}

fn graph_node(id: &str, label: &str, document_id: Option<&str>) -> GraphNode {
    GraphNode {
        id: id.to_string(),
        kind: "document".to_string(),
        label: label.to_string(),
        document_id: document_id.map(str::to_string),
        uri: None,
        resolved: true,
        metadata_json: "{}".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Wing / room taxonomy
// ---------------------------------------------------------------------------

#[test]
fn wing_room_taxonomy() {
    let (_dir, store) = open_temp_store();

    upsert_doc(
        &store,
        "doc://research/rag-1",
        "RAG notes",
        "hybrid search notes",
        Some("research"),
        Some("rag"),
        None,
        None,
    );
    upsert_doc(
        &store,
        "doc://research/rag-2",
        "RRF notes",
        "fusion ranking",
        Some("research"),
        Some("rag"),
        None,
        None,
    );
    upsert_doc(
        &store,
        "doc://research/llm-1",
        "LLM wiki",
        "karpathy style",
        Some("research"),
        Some("llm"),
        None,
        None,
    );
    upsert_doc(
        &store,
        "doc://ops/runbook",
        "Runbook",
        "deploy steps",
        Some("ops"),
        Some("runbooks"),
        None,
        None,
    );
    // Unscoped document (no wing) for taxonomy unscoped_count.
    upsert_doc(
        &store,
        "doc://loose",
        "Loose note",
        "no wing",
        None,
        None,
        None,
        None,
    );

    let wings = store.list_wings().expect("list_wings");
    assert_eq!(wings.len(), 2);
    assert_eq!(wings[0].wing, "ops");
    assert_eq!(wings[0].document_count, 1);
    assert_eq!(wings[1].wing, "research");
    assert_eq!(wings[1].document_count, 3);

    let research_rooms = store.list_rooms(Some("research")).expect("list_rooms");
    assert_eq!(research_rooms.len(), 2);
    assert_eq!(research_rooms[0].room, "llm");
    assert_eq!(research_rooms[0].document_count, 1);
    assert_eq!(research_rooms[1].room, "rag");
    assert_eq!(research_rooms[1].document_count, 2);
    assert_eq!(research_rooms[0].wing.as_deref(), Some("research"));

    let tax = store.get_taxonomy().expect("get_taxonomy");
    assert_eq!(tax.total_documents, 5);
    assert_eq!(tax.unscoped_count, 1);
    assert_eq!(tax.wings.len(), 2);

    let research = tax
        .wings
        .iter()
        .find(|w| w.wing == "research")
        .expect("research wing");
    assert_eq!(research.document_count, 3);
    assert_eq!(research.rooms.len(), 2);
    assert!(research.rooms.iter().any(|r| r.room == "rag" && r.document_count == 2));
    assert!(research.rooms.iter().any(|r| r.room == "llm" && r.document_count == 1));
}

// ---------------------------------------------------------------------------
// check_duplicate
// ---------------------------------------------------------------------------

#[test]
fn check_duplicate_by_content_hash_and_uri() {
    let (_dir, store) = open_temp_store();

    let body = "Verbatim drawer body for dedupe probe.";
    let hash = content_hash(body);
    let doc = upsert_doc(
        &store,
        "drawer://alpha",
        "Alpha drawer",
        body,
        Some("palace"),
        Some("drawers"),
        Some("/notes/alpha.md"),
        Some(&hash),
    );

    // Content-hash probe (from raw content).
    let by_content = store
        .check_duplicate(Some(body), None, None)
        .expect("check by content");
    assert!(by_content.is_duplicate);
    assert_eq!(by_content.content_hash.as_deref(), Some(hash.as_str()));
    assert_eq!(by_content.matches.len(), 1);
    assert_eq!(by_content.matches[0].id, doc.id);
    assert_eq!(by_content.matches[0].match_reason, "content_hash");
    assert_eq!(by_content.matches[0].wing.as_deref(), Some("palace"));
    assert_eq!(by_content.matches[0].room.as_deref(), Some("drawers"));

    // URI probe.
    let by_uri = store
        .check_duplicate(None, None, Some("drawer://alpha"))
        .expect("check by uri");
    assert!(by_uri.is_duplicate);
    assert_eq!(by_uri.matches.len(), 1);
    assert_eq!(by_uri.matches[0].match_reason, "uri");

    // Both hash + uri → combined reason.
    let both = store
        .check_duplicate(Some(body), None, Some("drawer://alpha"))
        .expect("check both");
    assert!(both.is_duplicate);
    assert_eq!(both.matches.len(), 1);
    assert_eq!(both.matches[0].match_reason, "content_hash+uri");

    // Fresh content is not a duplicate.
    let fresh = store
        .check_duplicate(Some("totally different body text"), None, None)
        .expect("fresh");
    assert!(!fresh.is_duplicate);
    assert!(fresh.matches.is_empty());

    // Missing all inputs → error.
    let err = store
        .check_duplicate(None, None, None)
        .expect_err("needs input");
    assert!(err.to_string().contains("check_duplicate"));
}

// ---------------------------------------------------------------------------
// delete_by_source
// ---------------------------------------------------------------------------

#[test]
fn delete_by_source_removes_matching_docs() {
    let (_dir, store) = open_temp_store();

    let src = "/vault/project/readme.md";
    let keep = upsert_doc(
        &store,
        "doc://keep",
        "Keep me",
        "other source",
        Some("project"),
        Some("docs"),
        Some("/vault/other.md"),
        None,
    );
    let d1 = upsert_doc(
        &store,
        "doc://src-1",
        "From source 1",
        "chunk a",
        Some("project"),
        Some("docs"),
        Some(src),
        None,
    );
    let d2 = upsert_doc(
        &store,
        "doc://src-2",
        "From source 2",
        "chunk b",
        Some("project"),
        Some("docs"),
        Some(src),
        None,
    );

    let n = store.delete_by_source(src).expect("delete_by_source");
    assert_eq!(n, 2);
    assert!(store.get_document(&d1.id).unwrap().is_none());
    assert!(store.get_document(&d2.id).unwrap().is_none());
    assert!(store.get_document(&keep.id).unwrap().is_some());

    // Second call is a no-op.
    let n2 = store.delete_by_source(src).expect("delete again");
    assert_eq!(n2, 0);

    // Unknown source → 0.
    assert_eq!(
        store
            .delete_by_source("/vault/missing.md")
            .expect("missing"),
        0
    );
}

// ---------------------------------------------------------------------------
// Temporal KG: add / query / invalidate / supersede
// ---------------------------------------------------------------------------

#[test]
fn kg_add_query_invalidate_supersede() {
    let (_dir, store) = open_temp_store();

    let f1 = store
        .kg_add(
            "Alice",
            "works_at",
            "Acme",
            Some(ts(2020, 1, 1)),
            None,
            Some("doc:alice"),
            Some(0.95),
            None,
        )
        .expect("kg_add");
    assert_eq!(f1.subject, "Alice");
    assert_eq!(f1.predicate, "works_at");
    assert_eq!(f1.object, "Acme");
    assert_eq!(f1.status, "active");

    // Idempotent open SPO.
    let again = store
        .kg_add("Alice", "works_at", "Acme", None, None, None, None, None)
        .expect("idempotent");
    assert_eq!(again.id, f1.id);

    let hits = store
        .kg_query(Some("Alice"), Some("works_at"), None, None)
        .expect("query active");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, f1.id);

    // Supersede employer at a shared boundary.
    let at = ts(2023, 6, 1);
    let successor = store
        .kg_supersede(
            "Alice",
            "works_at",
            "Acme",
            "Globex",
            Some(at),
            Some("doc:alice-2"),
            None,
        )
        .expect("supersede");
    assert_eq!(successor.object, "Globex");
    assert_eq!(successor.status, "active");
    assert_eq!(successor.valid_from, Some(at));

    let active = store
        .kg_query(Some("Alice"), Some("works_at"), None, None)
        .expect("active after supersede");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].object, "Globex");

    // Point-in-time before boundary sees Acme.
    let before = store
        .kg_query(
            Some("Alice"),
            Some("works_at"),
            None,
            Some(ts(2021, 1, 1)),
        )
        .expect("at_time before");
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].object, "Acme");

    // Half-open: at boundary only successor.
    let at_b = store
        .kg_query(Some("Alice"), Some("works_at"), None, Some(at))
        .expect("at_time boundary");
    assert_eq!(at_b.len(), 1);
    assert_eq!(at_b[0].object, "Globex");

    // Invalidate a separate fact.
    store
        .kg_add(
            "Bob",
            "lives_in",
            "Paris",
            Some(ts(2019, 1, 1)),
            None,
            None,
            None,
            None,
        )
        .expect("add bob");
    let ended = ts(2022, 6, 1);
    let inv = store
        .kg_invalidate("Bob", "lives_in", "Paris", Some(ended))
        .expect("invalidate");
    assert_eq!(inv.len(), 1);
    assert_eq!(inv[0].status, "invalidated");
    assert_eq!(inv[0].valid_to, Some(ended));

    let bob_active = store
        .kg_query(Some("Bob"), None, None, None)
        .expect("bob active");
    assert!(bob_active.is_empty());

    let bob_mid = store
        .kg_query(Some("Bob"), None, None, Some(ts(2020, 1, 1)))
        .expect("bob mid");
    assert_eq!(bob_mid.len(), 1);

    let stats = store.kg_stats().expect("kg_stats");
    assert!(stats.total_facts >= 3);
    assert!(stats.active_facts >= 1);
    assert!(stats.invalidated_facts >= 1);
    assert!(stats.superseded_facts >= 1);
}

// ---------------------------------------------------------------------------
// Diary write / read + wake_up
// ---------------------------------------------------------------------------

#[tokio::test]
async fn diary_write_read_and_wake_up() {
    let (dir, store) = open_temp_store();
    let config = test_config(dir.path().join("mempalace_parity.duckdb"), DIMS);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(DIMS));

    let w1 = diary_write(
        &store,
        &embedder,
        &config,
        "Claude",
        "noticed the FTS index was empty",
        None,
        Some("observations"),
        None,
        true,
    )
    .await
    .expect("diary_write 1");
    assert_eq!(w1.entry.agent_name, "claude");
    assert_eq!(w1.entry.wing, "agents/claude");
    assert!(w1.chunk_count >= 1);
    assert!(w1.ops_log_id.is_some());
    // Verbatim storage: body unchanged.
    assert_eq!(w1.entry.content, "noticed the FTS index was empty");

    let w2 = diary_write(
        &store,
        &embedder,
        &config,
        "claude",
        "second note same agent",
        None,
        None,
        None,
        true,
    )
    .await
    .expect("diary_write 2");
    assert_ne!(w1.entry.id, w2.entry.id);

    let read = diary_read(&store, "CLAUDE", 10).expect("diary_read");
    assert!(read.len() >= 2);
    assert!(read.iter().any(|e| e.content == "noticed the FTS index was empty"));
    assert!(read.iter().any(|e| e.id == w2.entry.id));
    // Newest first.
    assert!(read[0].created_at >= read[1].created_at);

    // Pin a wiki page so wake_up surfaces it.
    let now = Utc::now();
    store
        .upsert_document(&Document {
            id: "pin-1".into(),
            uri: "wiki://pinned".into(),
            title: "Pinned page".into(),
            content: "important conventions".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            layer: "wiki".into(),
            kind: "wiki".into(),
            pinned: true,
            boost: 3.0,
            ..Default::default()
        })
        .expect("pin doc");

    let status = StatusReport {
        storage_capabilities: rag_mcp::storage::duckdb_capability_names(),
        backend: "duckdb".into(),
        schema_version: store.schema_version().ok().flatten().unwrap_or(0),
        fts_ready: store.fts_ready().unwrap_or(false),
        document_count: 0,
        chunk_count: 0,
        node_count: 0,
        edge_count: 0,
        raw_count: 0,
        wiki_count: 0,
        index_entry_count: 0,
        index_coverage: 0.0,
        uncompiled_raw_count: 0,
        embedding_manifest_match: true,
        embed_provider: "mock".into(),
        embed_model: "mock".into(),
        wings: store.list_wings().unwrap_or_default(),
        embed_dims: DIMS,
        ready_for_search: true,
        ingest_roots_configured: false,
        db_path: store.path().display().to_string(),
    };

    let report = wake_up(&store, status, Some("alice"), 5, 20).expect("wake_up alice");
    // Alice has no diary; scope still normalized.
    assert_eq!(report.agent_name.as_deref(), Some("alice"));
    assert!(report.diary.is_empty());
    assert_eq!(report.pinned.len(), 1);
    assert_eq!(report.pinned[0].title, "Pinned page");
    assert!(report.schema.is_none());

    let report_claude = wake_up(
        &store,
        StatusReport {
            storage_capabilities: rag_mcp::storage::duckdb_capability_names(),
            backend: "duckdb".into(),
            schema_version: 0,
            fts_ready: false,
            document_count: 0,
            chunk_count: 0,
            node_count: 0,
            edge_count: 0,
            raw_count: 0,
            wiki_count: 0,
            index_entry_count: 0,
            index_coverage: 0.0,
            uncompiled_raw_count: 0,
            embedding_manifest_match: true,
            embed_provider: "mock".into(),
            embed_model: "mock".into(),
            wings: Vec::new(),
            embed_dims: DIMS,
            ready_for_search: true,
            ingest_roots_configured: false,
            db_path: store.path().display().to_string(),
        },
        Some("Claude"),
        5,
        20,
    )
    .expect("wake_up claude");
    assert_eq!(report_claude.agent_name.as_deref(), Some("claude"));
    assert_eq!(report_claude.diary.len(), 2);
    assert!(report_claude
        .diary
        .iter()
        .any(|e| e.content == "second note same agent"));
}

// ---------------------------------------------------------------------------
// Tunnel create / follow
// ---------------------------------------------------------------------------

#[test]
fn tunnel_create_and_follow() {
    let (_dir, store) = open_temp_store();

    store
        .upsert_graph_node(&graph_node("n1", "A", Some("d1")))
        .expect("node n1");
    store
        .upsert_graph_node(&graph_node("n2", "B", Some("d2")))
        .expect("node n2");
    store
        .upsert_graph_node(&graph_node("n3", "C", Some("d3")))
        .expect("node n3");

    // Non-tunnel noise must not appear in follow.
    store
        .link_nodes("n1", "n2", "wikilink", 1.0)
        .expect("wikilink noise");

    let t12 = store
        .create_tunnel("n1", "n2", 1.0, Some("bridge a-b"))
        .expect("create_tunnel n1-n2");
    assert_eq!(t12.rel_type, REL_TUNNEL);
    assert_eq!(t12.context.as_deref(), Some("bridge a-b"));
    assert_eq!(t12.source_id, "n1");
    assert_eq!(t12.target_id, "n2");

    // Idempotent undirected update.
    let again = store
        .create_tunnel("n2", "n1", 0.5, Some("updated"))
        .expect("update tunnel");
    assert_eq!(again.id, t12.id);
    assert!((again.weight - 0.5).abs() < 1e-9);

    let _t23 = store
        .create_tunnel("n2", "n3", 1.0, None)
        .expect("create_tunnel n2-n3");

    let all = store.list_tunnels(None).expect("list all");
    assert_eq!(all.len(), 2);

    // Depth 1 from n1 reaches n2 only.
    let hop1 = store.follow_tunnels("n1", 1, 100).expect("follow depth 1");
    assert_eq!(hop1.nodes.len(), 2);
    assert!(hop1.nodes.iter().any(|n| n.id == "n1"));
    assert!(hop1.nodes.iter().any(|n| n.id == "n2"));
    assert!(!hop1.nodes.iter().any(|n| n.id == "n3"));
    assert!(hop1.edges.iter().all(|e| e.rel_type == REL_TUNNEL));

    // Depth 2 reaches n3 along tunnels only.
    let hop2 = store.follow_tunnels("n1", 2, 100).expect("follow depth 2");
    assert_eq!(hop2.nodes.len(), 3);
    assert!(hop2.nodes.iter().any(|n| n.id == "n3"));
    assert_eq!(hop2.edges.len(), 2);
    assert!(hop2.edges.iter().all(|e| e.rel_type == REL_TUNNEL));

    // Self-loop refused.
    let loop_err = store.create_tunnel("n1", "n1", 1.0, None);
    assert!(loop_err.is_err());
}
