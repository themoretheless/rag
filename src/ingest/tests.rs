use super::*;
use async_trait::async_trait;

use crate::embeddings::MockEmbedder;
use crate::web_ingest::{FetchedPage, UrlFetcher};

struct MockFetcher {
    page: FetchedPage,
}

#[async_trait]
impl UrlFetcher for MockFetcher {
    async fn fetch(&self, _url: &str, _max_bytes: usize) -> crate::error::Result<FetchedPage> {
        Ok(self.page.clone())
    }
}

fn html_page(final_url: &str, html: &str) -> FetchedPage {
    FetchedPage {
        final_url: final_url.into(),
        status: 200,
        content_type: Some("text/html; charset=utf-8".into()),
        body: html.as_bytes().to_vec(),
    }
}

fn url_test_config(db_path: std::path::PathBuf, allow: &[&str]) -> Config {
    Config {
        db_path,
        embedding_dims: 8,
        chunk_size: 64,
        chunk_overlap: 8,
        ingest_url_allow: allow.iter().map(|p| p.to_string()).collect(),
        ..Config::for_tests()
    }
}

struct FailingEmbedder {
    dims: usize,
}

struct BarrierEmbedder {
    barrier: Arc<tokio::sync::Barrier>,
    inner: MockEmbedder,
}

#[async_trait]
impl EmbeddingProvider for FailingEmbedder {
    async fn embed(&self, _texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
        Err(AppError::embeddings("injected embedding failure"))
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

#[async_trait]
impl EmbeddingProvider for BarrierEmbedder {
    async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
        self.barrier.wait().await;
        self.inner.embed(texts).await
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}

fn test_config(db_path: std::path::PathBuf) -> Config {
    Config {
        db_path,
        embedding_dims: 8,
        chunk_size: 64,
        chunk_overlap: 8,
        ..Config::for_tests()
    }
}

fn command(text: &str) -> IngestCommand {
    IngestCommand {
        text: text.into(),
        title: Some("Atomic page".into()),
        uri: Some("wiki://atomic-page".into()),
        metadata_json: Some("{}".into()),
        wing: Some("tests".into()),
        room: Some("atomicity".into()),
        source_file: None,
        layer: "wiki".into(),
        kind: "wiki".into(),
        immutable: false,
    }
}

struct RecordingEmbedder {
    inner: MockEmbedder,
    texts: std::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl EmbeddingProvider for RecordingEmbedder {
    async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
        self.texts.lock().unwrap().extend(texts.iter().cloned());
        self.inner.embed(texts).await
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}

#[tokio::test]
async fn reingest_reuses_vectors_of_unchanged_chunks() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path().join("reuse.duckdb"));
    let store = crate::testkit::open_test_store(&config.db_path).expect("open");
    let recording = Arc::new(RecordingEmbedder {
        inner: MockEmbedder::new(8),
        texts: std::sync::Mutex::new(Vec::new()),
    });
    let embedder: Arc<dyn EmbeddingProvider> = recording.clone();
    let service = IngestService::new(&store, &embedder, &config);
    let stable: String = (0..6)
        .map(|index| format!("Stable paragraph number {index} keeps exactly this text.\n\n"))
        .collect();
    let original = format!("{stable}Closing paragraph changes later.");
    let original = original.as_str();
    service
        .ingest(command(original))
        .await
        .expect("first ingest");
    let first_vectors: Vec<Vec<f32>> = {
        let document = store
            .documents()
            .find_by_uri("wiki://atomic-page")
            .unwrap()
            .unwrap();
        store
            .chunks()
            .list_chunks_for_document(&document.id)
            .unwrap()
            .into_iter()
            .map(|chunk| chunk.embedding)
            .collect()
    };
    recording.texts.lock().unwrap().clear();

    // Identical content: every vector is reused, no provider call.
    let same = service
        .ingest(command(original))
        .await
        .expect("identical re-ingest");
    assert_eq!(same.op, "updated");
    assert!(recording.texts.lock().unwrap().is_empty());
    let document = store
        .documents()
        .find_by_uri("wiki://atomic-page")
        .unwrap()
        .unwrap();
    let reused: Vec<Vec<f32>> = store
        .chunks()
        .list_chunks_for_document(&document.id)
        .unwrap()
        .into_iter()
        .map(|chunk| chunk.embedding)
        .collect();
    assert_eq!(reused, first_vectors);

    // An edit embeds only text the document did not already have.
    let edited = format!("{stable}Closing paragraph is now quite different.");
    service
        .ingest(command(&edited))
        .await
        .expect("edited re-ingest");
    let embedded = recording.texts.lock().unwrap().clone();
    assert!(!embedded.is_empty());
    let document = store
        .documents()
        .find_by_uri("wiki://atomic-page")
        .unwrap()
        .unwrap();
    let chunk_texts: Vec<String> = store
        .chunks()
        .list_chunks_for_document(&document.id)
        .unwrap()
        .into_iter()
        .map(|chunk| chunk.content)
        .collect();
    assert!(chunk_texts.len() > 2, "{chunk_texts:?}");
    assert!(embedded.len() < chunk_texts.len(), "{embedded:?}");
    assert!(embedded.iter().all(|text| chunk_texts.contains(text)));
}

#[tokio::test]
async fn ingest_file_delegates_to_atomic_pipeline_with_canonical_source() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("source.md");
    std::fs::write(&source, "# Source\n\nBody links to [[Target]].").expect("write source");
    let mut config = test_config(temp.path().join("ingest-file.duckdb"));
    config.ingest_roots = vec![temp.path().to_path_buf()];
    let store = crate::testkit::open_test_store(&config.db_path).expect("store");
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));

    let result = IngestService::new(&store, &embedder, &config)
        .ingest_file(IngestFileCommand {
            path: source.display().to_string(),
            wing: Some("tests".into()),
            room: Some("files".into()),
            ..Default::default()
        })
        .await
        .expect("ingest file");

    let canonical = source.canonicalize().expect("canonical source");
    let document = store
        .documents()
        .get_document(&result.document_id)
        .expect("load document")
        .expect("document");
    assert_eq!(document.uri, format!("file://{}", canonical.display()));
    assert_eq!(document.source_file.as_deref(), canonical.to_str());
    assert_eq!(document.title, "source.md");
    assert_eq!(document.layer, "raw");
    assert_eq!(document.kind, "document");
    assert_eq!(document.revision, 1);
    assert_eq!(
        store
            .chunks()
            .list_chunks_for_document(&document.id)
            .expect("chunks")
            .len(),
        result.chunk_count
    );
    assert!(store
        .graph()
        .find_node_by_document_id(&document.id)
        .expect("graph lookup")
        .is_some());
    let mut retained = document.clone();
    retained.metadata_json = r#"{"custom":"keep"}"#.into();
    retained.status = "archived".into();
    retained.pinned = true;
    store.documents().upsert_document(&retained).unwrap();
    std::fs::write(&source, "# Source\n\nChanged body.").unwrap();
    IngestService::new(&store, &embedder, &config)
        .ingest_file(IngestFileCommand {
            path: source.display().to_string(),
            ..Default::default()
        })
        .await
        .unwrap();
    let updated = store
        .documents()
        .get_document(&document.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&updated.metadata_json).unwrap()["custom"],
        "keep"
    );
    assert_eq!(updated.status, "archived");
    assert!(updated.pinned);
    assert_eq!(updated.wing, retained.wing);
    assert_eq!(updated.room, retained.room);
}

#[tokio::test]
async fn source_manifest_failure_rolls_back_document_and_derived_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path().join("source-manifest-atomic.duckdb"));
    let store = crate::testkit::open_test_store(&config.db_path).expect("store");
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE source_manifest")
        .unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));

    let error = IngestService::new(&store, &embedder, &config)
        .ingest_source(
            IngestCommand {
                source_file: Some("/sources/atomic.md".into()),
                layer: "raw".into(),
                kind: "document".into(),
                ..command("Source body links to [[Target]].")
            },
            SourceManifestStamp {
                canonical_path: "/sources/atomic.md".into(),
                canonical_root: "/sources".into(),
                size_bytes: 32,
                mtime_ns: 1,
            },
        )
        .await
        .expect_err("manifest write must fail");

    assert!(error.to_string().contains("source_manifest"));
    assert!(store.documents().list_documents().unwrap().is_empty());
    assert!(store
        .graph()
        .get_graph_view(crate::models::GraphFilter::default())
        .unwrap()
        .nodes
        .is_empty());
    assert!(store.graph().list_graph_edges().unwrap().is_empty());
}

#[tokio::test]
async fn reingest_preserves_organization_and_omitted_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path().join("preserve.duckdb"));
    let store = crate::testkit::open_test_store(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    let service = IngestService::new(&store, &embedder, &config);
    let inserted = service.ingest(command("Original source")).await.unwrap();
    let mut doc = store
        .documents()
        .get_document(&inserted.document_id)
        .unwrap()
        .unwrap();
    doc.status = "archived".into();
    doc.pinned = true;
    doc.boost = 4.0;
    doc.wing = Some("project".into());
    doc.room = Some("sources".into());
    doc.metadata_json = "{\"custom\":true}".into();
    store.documents().upsert_document(&doc).unwrap();
    let mut update = command("Changed source");
    update.metadata_json = None;
    update.wing = None;
    update.room = None;
    service.ingest(update).await.unwrap();
    let updated = store
        .documents()
        .get_document(&inserted.document_id)
        .unwrap()
        .unwrap();
    assert_eq!(updated.status, "archived");
    assert!(updated.pinned);
    assert_eq!(updated.boost, 4.0);
    assert_eq!(updated.wing, doc.wing);
    assert_eq!(updated.room, doc.room);
    assert_eq!(updated.metadata_json, doc.metadata_json);
    assert_eq!(updated.content, "Changed source");
}

#[tokio::test]
async fn embedding_failure_leaves_document_chunks_and_graph_unchanged() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path().join("ingest-atomic.duckdb"));
    let store = crate::testkit::open_test_store(&config.db_path).expect("store");
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    let service = IngestService::new(&store, &embedder, &config);
    let inserted = service
        .ingest(command("Original body links to [[Old target]]."))
        .await
        .expect("initial ingest");

    let original_document = store
        .documents()
        .get_document(&inserted.document_id)
        .unwrap()
        .expect("document");
    let original_chunks = store
        .chunks()
        .list_chunks_for_document(&inserted.document_id)
        .unwrap();
    let original_node = store
        .graph()
        .find_node_by_document_id(&inserted.document_id)
        .unwrap()
        .expect("node");
    let original_edges: Vec<_> = store
        .graph()
        .list_graph_edges()
        .unwrap()
        .into_iter()
        .filter(|edge| edge.source_id == original_node.id)
        .collect();

    let failing_embedder: Arc<dyn EmbeddingProvider> = Arc::new(FailingEmbedder { dims: 8 });
    let failing_service = IngestService::new(&store, &failing_embedder, &config);
    let error = failing_service
        .update_document(UpdateDocumentCommand {
            document_id: inserted.document_id.clone(),
            update: DocumentMetaUpdate {
                title: Some("Changed title".into()),
                content: Some("Changed body links to [[New target]].".into()),
                ..Default::default()
            },
        })
        .await
        .expect_err("embedding must fail before persistence");
    assert!(error.to_string().contains("injected embedding failure"));

    let persisted_document = store
        .documents()
        .get_document(&inserted.document_id)
        .unwrap()
        .expect("document after failure");
    assert_eq!(persisted_document.revision, original_document.revision);
    assert_eq!(persisted_document.title, original_document.title);
    assert_eq!(persisted_document.content, original_document.content);
    let persisted_chunks = store
        .chunks()
        .list_chunks_for_document(&inserted.document_id)
        .unwrap();
    assert_eq!(persisted_chunks.len(), original_chunks.len());
    assert_eq!(
        persisted_chunks
            .iter()
            .map(|chunk| (&chunk.id, &chunk.content))
            .collect::<Vec<_>>(),
        original_chunks
            .iter()
            .map(|chunk| (&chunk.id, &chunk.content))
            .collect::<Vec<_>>()
    );
    let persisted_node = store
        .graph()
        .find_node_by_document_id(&inserted.document_id)
        .unwrap()
        .expect("node after failure");
    assert_eq!(persisted_node.id, original_node.id);
    assert_eq!(persisted_node.label, original_node.label);
    let persisted_edges: Vec<_> = store
        .graph()
        .list_graph_edges()
        .unwrap()
        .into_iter()
        .filter(|edge| edge.source_id == persisted_node.id)
        .collect();
    assert_eq!(persisted_edges.len(), original_edges.len());
    assert_eq!(persisted_edges[0].id, original_edges[0].id);
    assert!(store
        .graph()
        .find_nodes_by_label("New target")
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn concurrent_initial_ingest_never_creates_duplicate_uri_owners() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = Arc::new(test_config(temp.path().join("initial-race.duckdb")));
    let store = Arc::new(crate::testkit::open_test_store(&config.db_path).expect("store"));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(BarrierEmbedder {
        barrier: Arc::new(tokio::sync::Barrier::new(2)),
        inner: MockEmbedder::new(8),
    });

    let mut tasks = Vec::new();
    for text in ["first contender", "second contender"] {
        let store = store.clone();
        let config = config.clone();
        let embedder = embedder.clone();
        tasks.push(tokio::spawn(async move {
            IngestService::new(store.as_ref(), &embedder, config.as_ref())
                .ingest(command(text))
                .await
        }));
    }
    let results = [
        tasks.remove(0).await.unwrap(),
        tasks.remove(0).await.unwrap(),
    ];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(AppError::Conflict(_))))
            .count(),
        1
    );
    let documents = store.documents().list_documents().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].uri, "wiki://atomic-page");
}

#[tokio::test]
async fn concurrent_updates_use_the_revision_observed_before_embedding() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = Arc::new(test_config(temp.path().join("update-race.duckdb")));
    let store = Arc::new(crate::testkit::open_test_store(&config.db_path).expect("store"));
    let initial_embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    IngestService::new(store.as_ref(), &initial_embedder, config.as_ref())
        .ingest(command("initial"))
        .await
        .unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(BarrierEmbedder {
        barrier: Arc::new(tokio::sync::Barrier::new(2)),
        inner: MockEmbedder::new(8),
    });

    let mut tasks = Vec::new();
    for text in ["update one", "update two"] {
        let store = store.clone();
        let config = config.clone();
        let embedder = embedder.clone();
        tasks.push(tokio::spawn(async move {
            IngestService::new(store.as_ref(), &embedder, config.as_ref())
                .ingest(command(text))
                .await
        }));
    }
    let results = [
        tasks.remove(0).await.unwrap(),
        tasks.remove(0).await.unwrap(),
    ];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(AppError::Conflict(_))))
            .count(),
        1
    );
    let document = store.documents().list_documents().unwrap().pop().unwrap();
    assert_eq!(document.revision, 2);
    assert!(matches!(
        document.content.as_str(),
        "update one" | "update two"
    ));
}

#[tokio::test]
async fn ingest_url_disabled_without_allowlist() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path().join("url-disabled.duckdb"));
    let store = crate::testkit::open_test_store(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    let fetcher = MockFetcher {
        page: html_page("https://docs.rs/x", "<p>body</p>"),
    };
    let error = IngestService::new(&store, &embedder, &config)
        .ingest_url(
            &fetcher,
            IngestUrlCommand {
                url: "https://docs.rs/x".into(),
                ..Default::default()
            },
        )
        .await
        .expect_err("disabled without RAG_INGEST_URL_ALLOW");
    assert!(error.to_string().contains("ingest_url is disabled"));
    assert!(store.documents().list_documents().unwrap().is_empty());
}

#[tokio::test]
async fn ingest_url_enforces_allowlist_on_request_and_redirect() {
    let temp = tempfile::tempdir().unwrap();
    let config = url_test_config(temp.path().join("url-allow.duckdb"), &["docs.rs"]);
    let store = crate::testkit::open_test_store(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    let service = IngestService::new(&store, &embedder, &config);

    let error = service
        .ingest_url(
            &MockFetcher {
                page: html_page("https://evil.com/x", "<p>never fetched</p>"),
            },
            IngestUrlCommand {
                url: "https://evil.com/x".into(),
                ..Default::default()
            },
        )
        .await
        .expect_err("request host not allowlisted");
    assert!(error.to_string().contains("not in RAG_INGEST_URL_ALLOW"));

    // Redirect escaping the allowlist is refused after the fetch.
    let error = service
        .ingest_url(
            &MockFetcher {
                page: html_page("https://evil.com/landing", "<p>redirected</p>"),
            },
            IngestUrlCommand {
                url: "https://docs.rs/redirect-me".into(),
                ..Default::default()
            },
        )
        .await
        .expect_err("redirect target not allowlisted");
    assert!(error.to_string().contains("not in RAG_INGEST_URL_ALLOW"));
    assert!(store.documents().list_documents().unwrap().is_empty());
}

#[tokio::test]
async fn ingest_url_extracts_html_and_is_searchable_and_deduped() {
    let temp = tempfile::tempdir().unwrap();
    let config = url_test_config(temp.path().join("url-ingest.duckdb"), &["docs.rs"]);
    let store = crate::testkit::open_test_store(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    let service = IngestService::new(&store, &embedder, &config);
    let html = r#"<!DOCTYPE html><html><body>
            <nav><a href="/">Home</a></nav>
            <article><h1>Quirky Astrolabe Guide</h1>
            <p>The astrolabe measures stellar altitude precisely.</p></article>
            <footer>Copyright</footer></body></html>"#;
    let fetcher = MockFetcher {
        page: html_page("https://docs.rs/astrolabe", html),
    };

    let result = service
        .ingest_url(
            &fetcher,
            IngestUrlCommand {
                url: "https://docs.rs/astrolabe".into(),
                wing: Some("tests".into()),
                room: Some("web".into()),
                ..Default::default()
            },
        )
        .await
        .expect("ingest url");
    assert_eq!(result.final_url, "https://docs.rs/astrolabe");
    assert_eq!(result.format, "html");
    assert_eq!(result.http_status, 200);
    assert!(result.ingest.chunk_count > 0);

    let document = store
        .documents()
        .get_document(&result.ingest.document_id)
        .unwrap()
        .expect("document");
    assert_eq!(document.uri, "https://docs.rs/astrolabe");
    assert_eq!(
        document.source_file.as_deref(),
        Some("https://docs.rs/astrolabe")
    );
    assert!(document.content.contains("# Quirky Astrolabe Guide"));
    assert!(
        !document.content.contains("Home"),
        "nav boilerplate dropped"
    );
    let metadata: serde_json::Value = serde_json::from_str(&document.metadata_json).unwrap();
    assert_eq!(metadata["source_url"], "https://docs.rs/astrolabe");
    assert_eq!(metadata["format"], "html");
    assert!(metadata["fetched_at"].as_str().is_some());

    // Content is searchable through the lexical path.
    store.maintenance().ensure_fts(&config.fts_stemmer).unwrap();
    let hits = crate::db::search::search(
        &store,
        &crate::db::search::SearchQuery {
            mode: crate::models::SearchMode::Lex,
            query_text: Some("astrolabe".into()),
            top_k: 5,
            fts_stemmer: config.fts_stemmer.clone(),
            ..crate::db::search::SearchQuery::default()
        },
    )
    .unwrap();
    assert!(
        hits.iter().any(|hit| hit.document_id == document.id),
        "ingested page must be searchable: {hits:?}"
    );

    // ops_log recorded the fetch.
    assert!(store
        .ops_log()
        .list_ops_log(10)
        .unwrap()
        .iter()
        .any(|entry| entry.op == "ingest_url"));

    // Re-ingest of the same final URL upserts the same document (dedup by uri).
    let again = service
        .ingest_url(
            &fetcher,
            IngestUrlCommand {
                url: "https://docs.rs/astrolabe".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(again.ingest.document_id, document.id);
    assert_eq!(store.documents().list_documents().unwrap().len(), 1);
}
