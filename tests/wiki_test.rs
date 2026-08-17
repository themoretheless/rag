//! Integration tests for the Karpathy-style wiki compile layer.
//!
//! Covers: write_wiki_page, file_answer, wiki index rebuild, raw-layer
//! immutability / re-ingest policy, and ops_log entries for those ops.

use std::sync::Arc;

use rag_mcp::config::EmbeddingProviderKind;
use rag_mcp::embeddings::{EmbeddingProvider, MockEmbedder};
use rag_mcp::models::{DocumentMetaUpdate, SearchMode};
use rag_mcp::wiki::{self, assert_content_mutable, FileAnswerCitation};
use rag_mcp::{
    file_answer, get_wiki_page, list_wiki_pages, write_wiki_page, AppError, Config, Store,
    LAYER_RAW, LAYER_WIKI,
};
use tempfile::TempDir;

const DIMS: usize = 32;

fn open_temp_store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("wiki_test.duckdb");
    let store = Store::open(&path).expect("open store");
    (dir, store)
}

fn test_config(dims: usize) -> Config {
    Config {
        db_path: std::path::PathBuf::from("./rag.duckdb"),
        embedding_provider: EmbeddingProviderKind::Mock,
        embedding_base_url: "https://api.openai.com/v1".into(),
        embedding_api_key: String::new(),
        embedding_model: "mock".into(),
        embedding_dims: dims,
        chunk_size: 200,
        chunk_overlap: 20,
        default_top_k: 5,
        ingest_roots: Vec::new(),
        max_context_tokens: 4096,
        max_chunks_per_doc: 3,
        fts_stemmer: "porter".into(),
        default_search_mode: SearchMode::Vec,
        llm_base_url: "http://127.0.0.1:11434/v1".into(),
        llm_provider: rag_mcp::llm::LlmProviderKind::Ollama,
        llm_model: "llama3.2".into(),
        llm_api_key: "ollama".into(),
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

fn mock_embedder(dims: usize) -> Arc<dyn EmbeddingProvider> {
    Arc::new(MockEmbedder::new(dims))
}

#[tokio::test]
async fn write_wiki_page_get_list_and_ops_log() {
    let (_dir, store) = open_temp_store();
    let config = test_config(DIMS);
    let embedder = mock_embedder(DIMS);

    let written = write_wiki_page(
        &store,
        &embedder,
        &config,
        "rust-ownership",
        "Rust Ownership",
        "Ownership moves values. See [[Borrow Checker]] #rust",
        "concept",
        Some("languages"),
        Some("Ownership moves values"),
        Some("wiki-agent"),
    )
    .await
    .expect("write_wiki_page");

    assert_eq!(written.uri, "wiki://rust-ownership");
    assert_eq!(written.slug, "rust-ownership");
    assert!(written.chunk_count >= 1);
    assert!(!written.node_id.is_empty());
    assert!(
        written.edge_count >= 1,
        "expected wikilink/tag edges, got {}",
        written.edge_count
    );

    let by_slug = get_wiki_page(&store, "rust-ownership").expect("get by slug");
    assert_eq!(by_slug.layer, LAYER_WIKI);
    assert_eq!(by_slug.id, written.document_id);
    assert_eq!(by_slug.title, "Rust Ownership");
    assert!(by_slug.content.contains("Ownership moves"));

    let by_uri = get_wiki_page(&store, "wiki://rust-ownership").expect("get by uri");
    assert_eq!(by_uri.id, written.document_id);

    let listed = list_wiki_pages(&store).expect("list_wiki_pages");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, written.document_id);

    let idx = store
        .get_wiki_index_by_slug("rust-ownership")
        .expect("index lookup")
        .expect("index entry present");
    assert_eq!(idx.page_id.as_deref(), Some(written.document_id.as_str()));
    assert_eq!(idx.kind, "concept");
    assert_eq!(idx.category.as_deref(), Some("languages"));

    let ops = store.list_ops_log(20).expect("list_ops_log");
    let wiki_op = ops
        .iter()
        .find(|e| e.op == "wiki_write")
        .expect("ops_log wiki_write entry");
    assert_eq!(wiki_op.prefix.as_deref(), Some("WIKI"));
    assert_eq!(
        wiki_op.entity_id.as_deref(),
        Some(written.document_id.as_str())
    );
    assert_eq!(wiki_op.agent_name.as_deref(), Some("wiki-agent"));
    assert!(wiki_op.seq >= 1, "seq should be assigned, got {}", wiki_op.seq);
}

#[tokio::test]
async fn file_answer_persists_citations_index_and_ops_log() {
    let (_dir, store) = open_temp_store();
    let config = test_config(DIMS);
    let embedder = mock_embedder(DIMS);

    let cites = vec![FileAnswerCitation {
        document_id: Some("doc-raw-1".into()),
        uri: Some("raw://alpha-paper".into()),
        title: Some("Alpha Paper".into()),
        chunk_id: Some("chunk-1".into()),
        quote: Some("alpha is foundational".into()),
        note: Some("supports claim".into()),
    }];

    let wr = file_answer(
        &store,
        &embedder,
        &config,
        "Why Alpha Matters",
        "Alpha is important because of [[Beta]] #research",
        None,
        Some(cites),
        Some("answer-agent"),
    )
    .await
    .expect("file_answer");

    assert_eq!(wr.slug, "why-alpha-matters");
    assert_eq!(wr.uri, "wiki://why-alpha-matters");
    assert!(wr.chunk_count >= 1);
    assert!(!wr.node_id.is_empty());
    assert!(
        wr.edge_count >= 1,
        "wikilink/tag graph edges expected, got {}",
        wr.edge_count
    );

    let doc = store
        .get_document(&wr.document_id)
        .expect("get")
        .expect("wiki doc");
    assert_eq!(doc.layer, LAYER_WIKI);
    assert_eq!(doc.kind, "wiki");
    assert!(doc.content.contains("Alpha is important"));
    assert!(doc.content.contains("## Citations"));
    assert!(doc.content.contains("Alpha Paper"));

    let meta: serde_json::Value =
        serde_json::from_str(&doc.metadata_json).expect("metadata_json");
    assert_eq!(meta["category"], "answers");
    assert_eq!(meta["filed_as"], "answer");
    assert_eq!(meta["citations"].as_array().expect("citations").len(), 1);

    let idx = store
        .get_wiki_index_by_slug("why-alpha-matters")
        .expect("index")
        .expect("entry");
    assert_eq!(idx.page_id.as_deref(), Some(wr.document_id.as_str()));
    assert_eq!(idx.category.as_deref(), Some("answers"));

    let ops = store.list_ops_log(20).expect("ops");
    let fa = ops
        .iter()
        .find(|e| e.op == "file_answer")
        .expect("ops_log file_answer entry");
    assert_eq!(fa.prefix.as_deref(), Some("FILE"));
    assert_eq!(fa.entity_id.as_deref(), Some(wr.document_id.as_str()));
    assert_eq!(fa.agent_name.as_deref(), Some("answer-agent"));
    assert!(fa.message.contains("wiki://why-alpha-matters"));
    let payload: serde_json::Value =
        serde_json::from_str(&fa.payload_json).expect("payload_json");
    assert_eq!(payload["citation_count"], 1);
}

#[tokio::test]
async fn rebuild_index_from_wiki_docs_drops_orphan_catalog_rows() {
    let (_dir, store) = open_temp_store();
    let config = test_config(DIMS);
    let embedder = mock_embedder(DIMS);

    let a = write_wiki_page(
        &store,
        &embedder,
        &config,
        "alpha",
        "Alpha",
        "Alpha page body #tag-a",
        "entity",
        Some("entities"),
        Some("Alpha summary"),
        None,
    )
    .await
    .expect("write alpha");

    let b = write_wiki_page(
        &store,
        &embedder,
        &config,
        "beta",
        "Beta",
        "Beta page body",
        "wiki",
        None,
        None,
        None,
    )
    .await
    .expect("write beta");

    // Hand-written catalog row not backed by a wiki document.
    store
        .update_wiki_index_entry_fields(
            "orphan-note",
            Some("Orphan Note".into()),
            Some("Should vanish on rebuild".into()),
            Some("concept".into()),
            Some("notes".into()),
            None,
        )
        .expect("manual index row");

    assert_eq!(store.list_wiki_index().expect("list").len(), 3);
    assert!(store
        .get_wiki_index_by_slug("orphan-note")
        .expect("get orphan")
        .is_some());

    let n = store
        .rebuild_wiki_index_from_docs()
        .expect("rebuild_wiki_index_from_docs");
    assert_eq!(n, 2, "rebuild should index only layer=wiki documents");

    let entries = store.list_wiki_index().expect("list after rebuild");
    assert_eq!(entries.len(), 2);
    assert!(store
        .get_wiki_index_by_slug("orphan-note")
        .expect("orphan gone")
        .is_none());

    let alpha = store
        .get_wiki_index_by_slug("alpha")
        .expect("alpha")
        .expect("present");
    assert_eq!(alpha.page_id.as_deref(), Some(a.document_id.as_str()));
    assert_eq!(alpha.kind, "entity");
    assert_eq!(alpha.category.as_deref(), Some("entities"));

    let beta = store
        .get_wiki_index_by_slug("beta")
        .expect("beta")
        .expect("present");
    assert_eq!(beta.page_id.as_deref(), Some(b.document_id.as_str()));
}

#[tokio::test]
async fn raw_layer_immutable_content_update_refused_reingest_allowed() {
    let (_dir, store) = open_temp_store();
    let config = test_config(DIMS);
    let embedder = mock_embedder(DIMS);

    let raw = wiki::ingest_raw(
        &store,
        &embedder,
        &config,
        "verbatim raw source v1".into(),
        Some("Source Alpha".into()),
        Some("raw://alpha".into()),
        Some("projects".into()),
        Some("lab".into()),
        Some("/vault/alpha.md".into()),
    )
    .await
    .expect("ingest_raw");

    let raw_doc = store
        .get_document(&raw.document_id)
        .expect("get")
        .expect("raw present");
    assert_eq!(raw_doc.layer, LAYER_RAW);
    assert_eq!(raw_doc.content, "verbatim raw source v1");
    assert_eq!(raw_doc.wing.as_deref(), Some("projects"));
    assert_eq!(raw_doc.room.as_deref(), Some("lab"));

    // Policy helper: raw body is not mutable.
    let mut_err = assert_content_mutable(&raw_doc).expect_err("raw must be immutable");
    assert!(
        matches!(mut_err, AppError::Forbidden(_)),
        "expected Forbidden, got {mut_err}"
    );

    // Store meta path refuses content rewrite on layer=raw.
    let meta_err = store
        .update_document_meta(
            &raw.document_id,
            &DocumentMetaUpdate {
                content: Some("sneaky overwrite".into()),
                ..Default::default()
            },
        )
        .expect_err("content update on raw must fail");
    assert!(
        matches!(meta_err, AppError::Conflict(_)),
        "expected Conflict, got {meta_err}"
    );

    // wiki write must not overwrite a raw document at a colliding wiki:// uri.
    let collision = wiki::ingest_raw(
        &store,
        &embedder,
        &config,
        "raw parked on wiki uri".into(),
        Some("Stolen".into()),
        Some("wiki://stolen-slug".into()),
        None,
        None,
        None,
    )
    .await
    .expect("ingest at wiki-like uri");

    let write_err = write_wiki_page(
        &store,
        &embedder,
        &config,
        "stolen-slug",
        "Wiki Title",
        "trying to overwrite raw",
        "wiki",
        None,
        None,
        None,
    )
    .await
    .expect_err("write_wiki_page over raw uri must fail");
    assert!(
        matches!(write_err, AppError::Forbidden(_)),
        "expected Forbidden, got {write_err}"
    );
    let still_raw = store
        .get_document(&collision.document_id)
        .expect("get")
        .expect("still present");
    assert_eq!(still_raw.layer, LAYER_RAW);
    assert_eq!(still_raw.content, "raw parked on wiki uri");

    // get_wiki_page refuses raw documents.
    let get_err = get_wiki_page(&store, &raw.document_id).expect_err("raw is not wiki");
    assert!(matches!(get_err, AppError::Forbidden(_)));

    // Re-ingest same uri is the allowed content replacement path for raw.
    let again = wiki::ingest_raw(
        &store,
        &embedder,
        &config,
        "verbatim raw source v2".into(),
        Some("Source Alpha".into()),
        Some("raw://alpha".into()),
        Some("projects".into()),
        Some("lab".into()),
        Some("/vault/alpha.md".into()),
    )
    .await
    .expect("re-ingest replace");
    assert_eq!(again.document_id, raw.document_id);

    let replaced = store
        .get_document(&raw.document_id)
        .expect("get")
        .expect("present");
    assert_eq!(replaced.content, "verbatim raw source v2");
    assert_eq!(replaced.layer, LAYER_RAW);

    let sources = store
        .list_documents_by_layer(LAYER_RAW)
        .expect("list_sources");
    assert_eq!(sources.len(), 2, "two raw uris: alpha + stolen-slug");

    // Re-ingest and initial ingest both append ops_log.
    let ops = store.list_ops_log(50).expect("ops");
    let ingest_ops: Vec<_> = ops.iter().filter(|e| e.op == "ingest_raw").collect();
    assert!(
        ingest_ops.len() >= 3,
        "expected ingest_raw ops for initial+collision+reingest, got {}",
        ingest_ops.len()
    );
    assert!(ingest_ops.iter().any(|e| e.prefix.as_deref() == Some("INGEST")));
    assert!(ingest_ops
        .iter()
        .any(|e| e.entity_id.as_deref() == Some(raw.document_id.as_str())));
}

#[tokio::test]
async fn wiki_write_file_answer_and_rebuild_share_ops_log_timeline() {
    let (_dir, store) = open_temp_store();
    let config = test_config(DIMS);
    let embedder = mock_embedder(DIMS);

    let page = write_wiki_page(
        &store,
        &embedder,
        &config,
        "timeline-page",
        "Timeline Page",
        "First durable note.",
        "wiki",
        None,
        None,
        Some("agent-a"),
    )
    .await
    .expect("write");

    let _answer = file_answer(
        &store,
        &embedder,
        &config,
        "Timeline Answer",
        "Filed answer body with detail.",
        Some("timeline-answer"),
        None,
        Some("agent-a"),
    )
    .await
    .expect("file_answer");

    // Corrupt catalog then rebuild (MCP rebuild_index path = store rebuild + optional log).
    store
        .update_wiki_index_entry_fields(
            "ghost",
            Some("Ghost".into()),
            Some("orphan".into()),
            None,
            None,
            None,
        )
        .expect("ghost entry");
    let rebuilt = store
        .rebuild_wiki_index_from_docs()
        .expect("rebuild_index");
    assert_eq!(rebuilt, 2);
    assert!(store
        .get_wiki_index_by_slug("ghost")
        .expect("lookup")
        .is_none());

    // Explicit ops_log append (Karpathy log.md style) after rebuild.
    let logged = store
        .append_ops_log(&rag_mcp::OpsLogEntry {
            id: String::new(),
            seq: 0,
            ts: chrono::Utc::now(),
            op: "rebuild_index".into(),
            prefix: Some("INDEX".into()),
            message: format!("rebuilt wiki_index entries={rebuilt}"),
            entity_id: None,
            entity_kind: Some("wiki_index".into()),
            payload_json: serde_json::json!({ "entry_count": rebuilt }).to_string(),
            agent_name: Some("agent-a".into()),
        })
        .expect("append rebuild_index op");
    assert!(logged.seq >= 1);
    assert_eq!(logged.op, "rebuild_index");

    let ops = store.list_ops_log(50).expect("timeline");
    let ops_names: Vec<&str> = ops.iter().map(|e| e.op.as_str()).collect();
    assert!(
        ops_names.contains(&"wiki_write"),
        "timeline missing wiki_write: {ops_names:?}"
    );
    assert!(
        ops_names.contains(&"file_answer"),
        "timeline missing file_answer: {ops_names:?}"
    );
    assert!(
        ops_names.contains(&"rebuild_index"),
        "timeline missing rebuild_index: {ops_names:?}"
    );

    // Newest-first: rebuild_index should appear before older wiki_write when listing recent.
    let recent = store.list_ops_log(5).expect("recent");
    assert_eq!(recent[0].op, "rebuild_index");
    assert!(recent.iter().any(|e| e.entity_id.as_deref() == Some(page.document_id.as_str())));
}
