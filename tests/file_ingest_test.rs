//! Integration tests: extract .docx/.html fixtures and run them through the
//! real ingest pipeline (chunk → mock-embed → store → search), verifying
//! searchable content and heading metadata.

use std::io::Write;
use std::sync::Arc;

use rag_mcp::config::EmbeddingProviderKind;
use rag_mcp::db::search::search_chunks;
use rag_mcp::embeddings::MockEmbedder;
use rag_mcp::ingest::{IngestFileCommand, IngestService};
use rag_mcp::models::SearchMode;
use rag_mcp::{Config, Store};
use tempfile::TempDir;

const DIMS: usize = 32;

fn open_temp_store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("file_ingest.duckdb");
    let store = Store::open(&path).expect("open store");
    (dir, store)
}

fn test_config(db_path: std::path::PathBuf, ingest_roots: Vec<std::path::PathBuf>) -> Config {
    Config {
        db_path,
        embedding_provider: EmbeddingProviderKind::Mock,
        embedding_base_url: String::new(),
        embedding_api_key: String::new().into(),
        embedding_model: "mock".to_string(),
        embedding_dims: DIMS,
        chunk_size: 800,
        chunk_overlap: 120,
        code_chunking: rag_mcp::config::CodeChunkingMode::Structural,
        default_top_k: 5,
        ingest_roots,
        max_context_tokens: 4096,
        max_chunks_per_doc: 10,
        fts_stemmer: "porter".to_string(),
        default_search_mode: SearchMode::Vec,
        llm_provider: rag_mcp::llm::LlmProviderKind::Ollama,
        llm_base_url: "http://127.0.0.1:11434/v1".to_string(),
        llm_model: "llama3.2".to_string(),
        llm_api_key: "ollama".into(),
        llm_enabled: false,
        llm_timeout_secs: 120,
        llm_max_tokens: 4096,
        maint_max_docs: 50,
        maint_near_dup_threshold: 0.92,
        tool_surface: rag_mcp::mcp::ToolSurface::Full,
        http_bind: None,
        wiki_require_if_match: false,
        rerank_provider: rag_mcp::rerank::RerankProviderKind::Off,
        rerank_base_url: String::new(),
        rerank_api_key: String::new(),
        rerank_model: String::new(),
        rerank_top_n: 50,
        rerank_timeout_secs: 30,
        query_rewrite_variants: 3,
        query_rewrite_provider: None,
        query_rewrite_model: None,
        query_rewrite_cache_size: 256,
        hyde_provider: None,
        hyde_model: None,
        hyde_cache_size: 256,
        ingest_url_allow: Vec::new(),
        ingest_url_timeout_secs: 30,
        ingest_url_max_bytes: 10 * 1024 * 1024,
        reembed_batch: 100,
        fts_stale_generation_threshold: 64,
        fts_maint_interval_secs: 300,
        ..Config::for_tests()
    }
}

fn write_docx_fixture(path: &std::path::Path) {
    const STYLES: &str = r#"<?xml version="1.0"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:styleId="Heading1">
    <w:name w:val="heading 1"/><w:outlineLvl w:val="0"/>
  </w:style>
</w:styles>"#;
    const DOCUMENT: &str = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
  <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Migration Runbook</w:t></w:r></w:p>
  <w:p><w:r><w:t>Drain traffic before switching the primary database.</w:t></w:r></w:p>
</w:body>
</w:document>"#;
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).expect("create docx"));
    let options = zip::write::SimpleFileOptions::default();
    writer
        .start_file("word/styles.xml", options)
        .expect("styles entry");
    writer.write_all(STYLES.as_bytes()).expect("write styles");
    writer
        .start_file("word/document.xml", options)
        .expect("document entry");
    writer
        .write_all(DOCUMENT.as_bytes())
        .expect("write document");
    writer.finish().expect("finish docx");
}

fn write_html_fixture(path: &std::path::Path) {
    const HTML: &str = r#"<!DOCTYPE html><html><head><title>Page</title>
        <style>p { margin: 0; }</style></head><body>
        <nav><a href="/">Navigation junk</a></nav>
        <article><h1>Deploy Checklist</h1>
        <p>Snapshot the store before applying schema migrations.</p></article>
        <footer>Footer junk</footer>
        <script>track()</script>
        </body></html>"#;
    std::fs::write(path, HTML).expect("write html fixture");
}

async fn ingest_path(
    service: &IngestService<'_>,
    path: &std::path::Path,
) -> rag_mcp::models::IngestResult {
    service
        .ingest_file(IngestFileCommand {
            path: path.display().to_string(),
            title: None,
            uri: None,
            metadata_json: None,
            wing: None,
            room: None,
        })
        .await
        .expect("ingest_file")
}

#[tokio::test]
async fn ingest_docx_fixture_is_searchable_with_heading_metadata() {
    let (dir, store) = open_temp_store();
    let fixture = dir.path().join("runbook.docx");
    write_docx_fixture(&fixture);
    let config = test_config(
        dir.path().join("file_ingest.duckdb"),
        vec![dir.path().to_path_buf()],
    );
    let embedder: Arc<dyn rag_mcp::embeddings::EmbeddingProvider> =
        Arc::new(MockEmbedder::new(DIMS));
    let service = IngestService::new(&store, &embedder, &config);

    let result = ingest_path(&service, &fixture).await;
    assert!(result.chunk_count >= 1, "docx should produce chunks");

    let document = store
        .get_document(&result.document_id)
        .expect("get")
        .expect("doc");
    assert!(
        document.content.contains("# Migration Runbook"),
        "heading line: {}",
        document.content
    );
    assert!(
        document.content.contains("Drain traffic"),
        "body text: {}",
        document.content
    );
    let metadata: serde_json::Value =
        serde_json::from_str(&document.metadata_json).expect("metadata json");
    assert_eq!(metadata["format"], "docx");

    // Heading-aware chunking must attach section metadata to the chunks.
    let chunks = store
        .list_chunks_for_document(&result.document_id)
        .expect("chunks");
    let has_heading_path = chunks.iter().any(|chunk| {
        serde_json::from_str::<serde_json::Value>(&chunk.metadata_json)
            .ok()
            .and_then(|value| value.get("heading_path").cloned())
            .and_then(|value| value.as_array().map(|a| !a.is_empty()))
            .unwrap_or(false)
    });
    assert!(
        has_heading_path,
        "expected heading_path chunk metadata: {chunks:?}"
    );

    // Mock embeddings are deterministic: searching the extracted text must
    // rank this document's chunk first.
    let query = embedder
        .embed(std::slice::from_ref(&document.content))
        .await
        .expect("embed query");
    let hits = search_chunks(&store, &query[0], 5, None).expect("search");
    assert!(!hits.is_empty(), "expected search hits");
    assert_eq!(hits[0].document_id, result.document_id);
    assert!(hits[0].content.contains("Migration Runbook"));
}

#[tokio::test]
async fn ingest_html_fixture_drops_boilerplate_and_is_searchable() {
    let (dir, store) = open_temp_store();
    let fixture = dir.path().join("page.html");
    write_html_fixture(&fixture);
    let config = test_config(
        dir.path().join("file_ingest.duckdb"),
        vec![dir.path().to_path_buf()],
    );
    let embedder: Arc<dyn rag_mcp::embeddings::EmbeddingProvider> =
        Arc::new(MockEmbedder::new(DIMS));
    let service = IngestService::new(&store, &embedder, &config);

    let result = ingest_path(&service, &fixture).await;
    assert!(result.chunk_count >= 1);

    let document = store
        .get_document(&result.document_id)
        .expect("get")
        .expect("doc");
    assert!(
        document.content.contains("# Deploy Checklist"),
        "heading: {}",
        document.content
    );
    assert!(
        document.content.contains("Snapshot the store"),
        "main text: {}",
        document.content
    );
    assert!(
        !document.content.contains("Navigation junk"),
        "nav dropped: {}",
        document.content
    );
    assert!(
        !document.content.contains("Footer junk"),
        "footer dropped: {}",
        document.content
    );
    assert!(
        !document.content.contains("track()"),
        "script dropped: {}",
        document.content
    );

    let query = embedder
        .embed(std::slice::from_ref(&document.content))
        .await
        .expect("embed query");
    let hits = search_chunks(&store, &query[0], 5, None).expect("search");
    assert_eq!(hits[0].document_id, result.document_id);
}

#[tokio::test]
async fn ingest_broken_docx_surfaces_per_file_error() {
    let (dir, store) = open_temp_store();
    let fixture = dir.path().join("broken.docx");
    std::fs::write(&fixture, b"definitely not a zip archive").expect("write broken docx");
    let config = test_config(
        dir.path().join("file_ingest.duckdb"),
        vec![dir.path().to_path_buf()],
    );
    let embedder: Arc<dyn rag_mcp::embeddings::EmbeddingProvider> =
        Arc::new(MockEmbedder::new(DIMS));
    let service = IngestService::new(&store, &embedder, &config);

    let error = service
        .ingest_file(IngestFileCommand {
            path: fixture.display().to_string(),
            title: None,
            uri: None,
            metadata_json: None,
            wing: None,
            room: None,
        })
        .await
        .expect_err("broken docx must fail");
    assert!(
        error.to_string().contains("docx"),
        "error names format: {error}"
    );
}
