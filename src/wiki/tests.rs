use super::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingEmbedder {
    calls: Arc<AtomicUsize>,
    dims: usize,
}

#[async_trait]
impl EmbeddingProvider for CountingEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(texts.iter().map(|_| vec![0.5; self.dims]).collect())
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

struct GatedEmbedder {
    started: Arc<tokio::sync::Semaphore>,
    release: Arc<tokio::sync::Semaphore>,
    dims: usize,
}

#[async_trait]
impl EmbeddingProvider for GatedEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.started.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        Ok(texts.iter().map(|_| vec![0.5; self.dims]).collect())
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

fn test_wiki_command(slug: &str) -> WikiWriteCommand {
    WikiWriteCommand {
        slug: slug.into(),
        title: slug.into(),
        content: "A test wiki page with [[Related]] #knowledge".into(),
        wing: None,
        room: None,
        kind: LAYER_WIKI.into(),
        category: None,
        summary: None,
        agent: None,
        options: WriteWikiOpts::default(),
    }
}

async fn mock_wiki_llm(
    started: Arc<tokio::sync::Semaphore>,
    release: Arc<tokio::sync::Semaphore>,
) -> (ChatClient, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let llm = ChatClient::new(
        format!("http://{}/v1", listener.local_addr().unwrap()),
        "test",
        "test",
    )
    .unwrap();
    let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move || {
            let started = started.clone();
            let release = release.clone();
            async move {
                started.add_permits(1);
                release.acquire().await.unwrap().forget();
                // Both compile and consolidate parse their own fields from this
                // deterministic response, letting the same race cover both paths.
                let page = serde_json::json!({"slug":"generated-target","title":"Generated","kind":"concept","content":"generated source synthesis"});
                let mut content = page.clone();
                content["pages"] = serde_json::json!([page]);
                axum::Json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":content.to_string()}}]}))
            }
        }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (llm, server)
}

struct FailingEmbedder {
    dims: usize,
}

#[derive(Debug, PartialEq)]
struct ChunkState {
    id: String,
    index: i32,
    content: String,
    embedding: Vec<f32>,
    char_start: i32,
    char_end: i32,
    metadata_json: String,
}

#[derive(Debug, PartialEq)]
struct WikiIndexState {
    id: String,
    title: String,
    kind: String,
    category: Option<String>,
    summary: Option<String>,
    page_id: Option<String>,
    updated_at: chrono::DateTime<Utc>,
}

#[async_trait]
impl EmbeddingProvider for FailingEmbedder {
    async fn embed(&self, _texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
        Err(AppError::embeddings("injected wiki embedding failure"))
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

fn open_store() -> Store {
    crate::testkit::temp_store()
}

#[test]
fn get_schema_seeds_default_at_canonical_uri() {
    let store = open_store();
    assert!(store.get_schema_document().unwrap().is_none());

    let view = get_schema(&store).unwrap();
    assert_eq!(view.uri, SCHEMA_URI);
    assert_eq!(view.layer, "schema");
    assert_eq!(view.kind, "schema");
    assert!(view.content.contains("Wiki schema"));
    assert!(!view.created);

    let again = store.get_schema_document().unwrap().expect("stored");
    assert_eq!(again.uri, SCHEMA_URI);
    assert_eq!(again.id, view.document_id);
}

#[test]
fn update_schema_roundtrip_stable_id() {
    let store = open_store();
    let first = update_schema(
        &store,
        "# Custom\n\n- rule one\n",
        Some("Agents schema"),
        Some("test-agent"),
    )
    .unwrap();
    assert!(first.created);
    assert_eq!(first.uri, SCHEMA_URI);
    assert_eq!(first.title, "Agents schema");
    assert_eq!(first.layer, "schema");
    assert_eq!(first.kind, "schema");
    assert!(first.content.contains("rule one"));

    let second = update_schema(&store, "# Custom\n\n- rule two\n", None, None).unwrap();
    assert!(!second.created);
    assert_eq!(second.document_id, first.document_id);
    assert_eq!(second.title, SCHEMA_TITLE);
    assert!(second.content.contains("rule two"));

    let got = get_schema(&store).unwrap();
    assert_eq!(got.document_id, first.document_id);
    assert!(got.content.contains("rule two"));
}

#[test]
fn update_schema_rejects_empty_content() {
    let store = open_store();
    let err = update_schema(&store, "   ", None, None).unwrap_err();
    assert!(err.to_string().contains("empty"));
}

fn sample_config(dims: usize) -> Config {
    Config {
        db_path: std::path::PathBuf::from("./rag.duckdb"),
        embedding_provider: crate::config::EmbeddingProviderKind::Mock,
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
        default_search_mode: crate::models::SearchMode::Vec,
        llm_base_url: "http://127.0.0.1:11434/v1".into(),
        llm_provider: crate::llm::LlmProviderKind::Ollama,
        llm_model: "llama3.2".into(),
        llm_api_key: "ollama".into(),
        llm_enabled: false,
        llm_timeout_secs: 120,
        llm_max_tokens: 4096,
        maint_max_docs: 50,
        maint_near_dup_threshold: 0.92,
        tool_surface: crate::mcp::ToolSurface::Full,
        http_bind: None,
        wiki_require_if_match: false,
    }
}

fn chunk_state(store: &Store, document_id: &str) -> Vec<ChunkState> {
    store
        .list_chunks_for_document(document_id)
        .expect("chunks")
        .into_iter()
        .map(|chunk| ChunkState {
            id: chunk.id,
            index: chunk.chunk_index,
            content: chunk.content,
            embedding: chunk.embedding,
            char_start: chunk.char_start,
            char_end: chunk.char_end,
            metadata_json: chunk.metadata_json,
        })
        .collect()
}

fn outgoing_edge_state(
    store: &Store,
    node_id: &str,
) -> Vec<(String, String, String, Option<String>)> {
    let mut state = store
        .list_graph_edges()
        .expect("graph edges")
        .into_iter()
        .filter(|edge| edge.source_id == node_id)
        .map(|edge| (edge.id, edge.target_id, edge.rel_type, edge.context))
        .collect::<Vec<_>>();
    state.sort();
    state
}

fn wiki_index_state(store: &Store, slug: &str) -> Option<WikiIndexState> {
    store
        .get_wiki_index_by_slug(slug)
        .expect("wiki index")
        .map(|entry| WikiIndexState {
            id: entry.id,
            title: entry.title,
            kind: entry.kind,
            category: entry.category,
            summary: entry.summary,
            page_id: entry.page_id,
            updated_at: entry.updated_at,
        })
}

#[tokio::test]
async fn wiki_write_rejects_identity_mismatch_before_embedding_or_mutation() {
    let store = open_store();
    let config = sample_config(16);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(16));
    let created =
        write_wiki_page_command(&store, &embedder, &config, test_wiki_command("existing"))
            .await
            .unwrap();
    let before_doc = get_wiki_page(&store, "existing").unwrap();
    let before_chunks = chunk_state(&store, &created.document_id);
    let before_edges = outgoing_edge_state(&store, &created.node_id);
    let before_index = wiki_index_state(&store, "existing");
    let before_ops = store.list_ops_log(100).unwrap().len();
    let calls = Arc::new(AtomicUsize::new(0));
    let incompatible: Arc<dyn EmbeddingProvider> = Arc::new(CountingEmbedder {
        calls: calls.clone(),
        dims: 8,
    });
    for changed in [
        Config {
            embedding_model: "another-model".into(),
            ..config.clone()
        },
        Config {
            embedding_dims: 8,
            ..config.clone()
        },
        Config {
            embedding_base_url: "https://another-endpoint.test/v1".into(),
            ..config.clone()
        },
    ] {
        let error = write_wiki_page_command(
            &store,
            &incompatible,
            &changed,
            test_wiki_command("new-page"),
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("embedding manifest mismatch"),
            "{error}"
        );
        let error = update_wiki_page_cas(
            &store,
            &incompatible,
            &changed,
            "existing",
            None,
            "changed content",
            None,
            None,
            None,
            None,
            Some(created.revision),
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("embedding manifest mismatch"),
            "{error}"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(store.find_by_uri("wiki://new-page").unwrap().is_none());
    let after_doc = get_wiki_page(&store, "existing").unwrap();
    assert_eq!(after_doc.content, before_doc.content);
    assert_eq!(after_doc.revision, before_doc.revision);
    assert_eq!(chunk_state(&store, &created.document_id), before_chunks);
    assert_eq!(outgoing_edge_state(&store, &created.node_id), before_edges);
    assert_eq!(wiki_index_state(&store, "existing"), before_index);
    assert_eq!(store.list_ops_log(100).unwrap().len(), before_ops);
    store.require_embedding_manifest_match(&config).unwrap();

    store
        .set_embedding_manifest(&crate::db::store::embedding_migration_manifest(&config))
        .unwrap();
    let error = write_wiki_page_command(
        &store,
        &incompatible,
        &config,
        test_wiki_command("migration-page"),
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("incomplete corpus migration"),
        "{error}"
    );
    store
        .lock()
        .unwrap()
        .execute("DELETE FROM embedding_manifest", [])
        .unwrap();
    let error = write_wiki_page_command(
        &store,
        &incompatible,
        &config,
        test_wiki_command("legacy-page"),
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("missing for a non-empty corpus"),
        "{error}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(store.get_embedding_manifest().unwrap().is_none());
}

#[tokio::test]
async fn wiki_write_rejects_wrong_provider_dimensions_without_document_commit() {
    let store = open_store();
    let config = sample_config(16);
    let calls = Arc::new(AtomicUsize::new(0));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(CountingEmbedder {
        calls: calls.clone(),
        dims: 8,
    });
    let error =
        write_wiki_page_command(&store, &embedder, &config, test_wiki_command("wrong-dims"))
            .await
            .unwrap_err();
    assert!(
        error.to_string().contains("returned dims=8, expected 16"),
        "{error}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(store.count_documents().unwrap(), 0);
    assert!(store.list_ops_log(10).unwrap().is_empty());
    store.require_embedding_manifest_match(&config).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wiki_write_excludes_reembed_all_through_embedding_and_commit() {
    let store = open_store();
    let config = sample_config(16);
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let gated: Arc<dyn EmbeddingProvider> = Arc::new(GatedEmbedder {
        started: started.clone(),
        release: release.clone(),
        dims: 16,
    });
    let writing_store = store.clone();
    let writing_config = config.clone();
    let write = tokio::spawn(async move {
        write_wiki_page_command(
            &writing_store,
            &gated,
            &writing_config,
            test_wiki_command("guarded"),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let migrated = Config {
        embedding_dims: 8,
        embedding_model: "new-model".into(),
        ..config.clone()
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let replacement: Arc<dyn EmbeddingProvider> = Arc::new(CountingEmbedder {
        calls: calls.clone(),
        dims: 8,
    });
    let error = crate::maintain::reembed_all(&store, &replacement, &migrated, usize::MAX)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Busy(_)), "{error}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    release.add_permits(1);
    let written = tokio::time::timeout(std::time::Duration::from_secs(30), write)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(chunk_state(&store, &written.document_id)
        .iter()
        .all(|chunk| chunk.embedding.len() == 16));
    let report = crate::maintain::reembed_all(&store, &replacement, &migrated, usize::MAX)
        .await
        .unwrap();
    assert!(report.errors.is_empty());
    store.require_embedding_manifest_match(&migrated).unwrap();
    assert!(chunk_state(&store, &written.document_id)
        .iter()
        .all(|chunk| chunk.embedding.len() == 8));
    let error = write_wiki_page_command(
        &store,
        &replacement,
        &config,
        test_wiki_command("old-model"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("embedding manifest mismatch"));
    write_wiki_page_command(
        &store,
        &replacement,
        &migrated,
        test_wiki_command("new-model"),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn maintenance_wiki_writes_reuse_only_the_explicit_corpus_lease() {
    use crate::maintain::{ApplyPlanOptions, MaintenanceAction, MaintenancePlanItem};

    let store = open_store();
    let config = sample_config(16);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(16));
    let scoped = store.try_corpus_mutation_scope("test plan").unwrap();
    let blocked = write_wiki_page_command(
        &store,
        &embedder,
        &config,
        test_wiki_command("outside-plan"),
    )
    .await
    .unwrap_err();
    assert!(matches!(blocked, AppError::Busy(_)));
    assert!(store.get_embedding_manifest().unwrap().is_none());
    let actions = [MaintenanceAction::Consolidate, MaintenanceAction::FileAnswer]
            .into_iter().enumerate().map(|(index, action)| MaintenancePlanItem {
                action,
                reason: Some("test nested wiki workflow".into()),
                target_id: Some("source".into()),
                params: serde_json::json!({"title":format!("Page {index}"),"slug":format!("page-{index}"),"content":"nested wiki body"}),
            }).collect();
    let report = crate::maintain::apply_maintenance_plan(
        &scoped,
        &embedder,
        &config,
        None,
        actions,
        &ApplyPlanOptions {
            dry_run: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.applied.len(), 2);
    assert_eq!(store.count_documents().unwrap(), 2);
    let retained = scoped.clone();
    drop(scoped);
    assert!(store.try_corpus_mutation_guard("still leased").is_err());
    drop(retained);
    assert!(store.try_corpus_mutation_guard("finished").is_ok());

    let other_store = open_store();
    let foreign_guard = other_store.try_corpus_mutation_guard("foreign").unwrap();
    assert!(store.with_corpus_mutation_guard(foreign_guard).is_err());
}

#[tokio::test]
async fn maintenance_compile_and_stale_refresh_reuse_the_exclusive_lease() {
    use crate::maintain::{ApplyPlanOptions, MaintenanceAction, MaintenancePlanItem};

    let store = open_store();
    let config = Config {
        llm_enabled: true,
        wiki_require_if_match: true,
        ..sample_config(16)
    };
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(16));
    let raw = ingest_raw(
        &store,
        &embedder,
        &config,
        "Source content".into(),
        Some("Source".into()),
        Some("raw://compiled-source".into()),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let llm = ChatClient::new(
        format!("http://{}/v1", listener.local_addr().unwrap()),
        "test",
        "test",
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let chat_calls = calls.clone();
    let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move || {
            let chat_calls = chat_calls.clone();
            async move {
                chat_calls.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":serde_json::json!({
                    "pages":[{"slug":"compiled-page","title":"Compiled","kind":"source_summary","content":"Compiled source content"}]
                }).to_string()}}]}))
            }
        }));
    let mock = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let scoped = store
        .try_corpus_mutation_scope("compile and refresh")
        .unwrap();
    for action in [
        MaintenanceAction::CompileSource,
        MaintenanceAction::RefreshStaleWiki,
    ] {
        let report = crate::maintain::apply_maintenance_plan(
            &scoped,
            &embedder,
            &config,
            Some(&llm),
            vec![MaintenancePlanItem {
                action,
                reason: None,
                target_id: Some(raw.document_id.clone()),
                params: serde_json::json!({}),
            }],
            &ApplyPlanOptions {
                dry_run: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.applied.len(), 1);
        let page = get_wiki_page(&store, "compiled-page").unwrap();
        let mut source = store.get_document(&raw.document_id).unwrap().unwrap();
        source.updated_at = page.updated_at + chrono::Duration::seconds(1);
        source.content.push_str(" updated source");
        source.content_hash = Some(content_hash(&source.content));
        store.upsert_document(&source).unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(get_wiki_page(&store, "compiled-page").unwrap().revision, 2);
    mock.abort();
}

#[tokio::test]
async fn consolidate_creates_and_updates_with_required_if_match() {
    let store = open_store();
    let config = Config {
        wiki_require_if_match: true,
        ..sample_config(16)
    };
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(16));
    let raw = ingest_raw(
        &store,
        &embedder,
        &config,
        "Raw source".into(),
        Some("Source".into()),
        Some("raw://source".into()),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (llm, mock) = mock_wiki_llm(
        Arc::new(tokio::sync::Semaphore::new(0)),
        Arc::new(tokio::sync::Semaphore::new(2)),
    )
    .await;
    for revision in 1..=2 {
        let result = consolidate(
            &store,
            &embedder,
            &config,
            &llm,
            std::slice::from_ref(&raw.document_id),
            true,
            ConsolidateOpts::default(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.written.unwrap().revision, revision);
    }
    mock.abort();
    let proposal = ConsolidateProposal {
        slug: "generated-target".into(),
        title: "Direct proposal".into(),
        kind: "concept".into(),
        content: "Directly applied synthesis".into(),
        category: None,
        summary: None,
        suggested_links: vec![],
        notes: None,
    };
    let source = store.get_document(&raw.document_id).unwrap().unwrap();
    let result = apply_consolidate_proposal(&store, &embedder, &config, &proposal, &[source], None)
        .await
        .unwrap();
    assert_eq!(result.revision, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generated_wiki_rejects_concurrent_edits_and_creates_during_llm_generation() {
    for consolidate_mode in [false, true] {
        for page_already_exists in [false, true] {
            let store = open_store();
            let config = Config {
                wiki_require_if_match: true,
                ..sample_config(16)
            };
            let embedder: Arc<dyn EmbeddingProvider> =
                Arc::new(crate::embeddings::MockEmbedder::new(16));
            let raw = ingest_raw(
                &store,
                &embedder,
                &config,
                "Raw source".into(),
                Some("Source".into()),
                Some("raw://source".into()),
                None,
                None,
                None,
            )
            .await
            .unwrap();
            let initial_revision = if page_already_exists {
                Some(
                    write_wiki_page_command(
                        &store,
                        &embedder,
                        &config,
                        test_wiki_command("generated-target"),
                    )
                    .await
                    .unwrap()
                    .revision,
                )
            } else {
                None
            };
            let started = Arc::new(tokio::sync::Semaphore::new(0));
            let release = Arc::new(tokio::sync::Semaphore::new(0));
            let (llm, mock) = mock_wiki_llm(started.clone(), release.clone()).await;
            let work_store = store.clone();
            let work_embedder = embedder.clone();
            let work_config = config.clone();
            let work = tokio::spawn(async move {
                if consolidate_mode {
                    consolidate(
                        &work_store,
                        &work_embedder,
                        &work_config,
                        &llm,
                        &[raw.document_id],
                        true,
                        ConsolidateOpts::default(),
                        None,
                    )
                    .await
                    .map(|_| ())
                } else {
                    compile_source(
                        &work_store,
                        &work_embedder,
                        &work_config,
                        &llm,
                        &raw.document_id,
                        false,
                        None,
                    )
                    .await
                    .map(|_| ())
                }
            });
            tokio::time::timeout(std::time::Duration::from_secs(30), started.acquire())
                .await
                .unwrap()
                .unwrap()
                .forget();
            let mut edit = test_wiki_command("generated-target");
            edit.content = "Concurrent editor content must survive".into();
            edit.options.if_match_revision = initial_revision;
            let concurrent = write_wiki_page_command(&store, &embedder, &config, edit)
                .await
                .unwrap();
            let before_chunks = chunk_state(&store, &concurrent.document_id);
            let before_ops = store.list_ops_log(100).unwrap().len();
            release.add_permits(1);
            let error = tokio::time::timeout(std::time::Duration::from_secs(30), work)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert!(
                matches!(error, AppError::Conflict(_)),
                "consolidate={consolidate_mode} existed={page_already_exists}: {error}"
            );
            let kept = get_wiki_page(&store, "generated-target").unwrap();
            assert_eq!(kept.content, "Concurrent editor content must survive");
            assert_eq!(kept.revision, concurrent.revision);
            assert_eq!(chunk_state(&store, &concurrent.document_id), before_chunks);
            assert_eq!(store.list_ops_log(100).unwrap().len(), before_ops);
            mock.abort();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generated_wiki_is_stale_when_source_changes_during_llm_generation() {
    for consolidate_mode in [false, true] {
        let store = open_store();
        let config = sample_config(16);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(16));
        // Longer than the compile prompt's source cap: provenance identifies
        // the complete captured source, not merely its truncated prompt text.
        let original = "Original source body. ".repeat(1_200);
        let raw = ingest_raw(
            &store,
            &embedder,
            &config,
            original.clone(),
            Some("Source".into()),
            Some("raw://source".into()),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let started = Arc::new(tokio::sync::Semaphore::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let (llm, mock) = mock_wiki_llm(started.clone(), release.clone()).await;
        let work_store = store.clone();
        let work_embedder = embedder.clone();
        let work_config = config.clone();
        let source_id = raw.document_id.clone();
        let work = tokio::spawn(async move {
            if consolidate_mode {
                consolidate(
                    &work_store,
                    &work_embedder,
                    &work_config,
                    &llm,
                    &[source_id],
                    true,
                    ConsolidateOpts::default(),
                    None,
                )
                .await
                .map(|_| ())
            } else {
                compile_source(
                    &work_store,
                    &work_embedder,
                    &work_config,
                    &llm,
                    &source_id,
                    false,
                    None,
                )
                .await
                .map(|_| ())
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), started.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        ingest_raw(
            &store,
            &embedder,
            &config,
            "Changed source while LLM was running".into(),
            Some("Source".into()),
            Some("raw://source".into()),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        release.add_permits(1);
        tokio::time::timeout(std::time::Duration::from_secs(30), work)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let page = get_wiki_page(&store, "generated-target").unwrap();
        let source = store.get_document(&raw.document_id).unwrap().unwrap();
        assert!(page.updated_at > source.updated_at);
        let metadata: serde_json::Value = serde_json::from_str(&page.metadata_json).unwrap();
        let versions: Vec<WikiSourceVersion> =
            serde_json::from_value(metadata["source_versions"].clone()).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].document_id, raw.document_id);
        assert_eq!(versions[0].uri, "raw://source");
        assert_eq!(versions[0].content_hash, content_hash(&original));
        assert_ne!(versions[0].content_hash, content_hash(&source.content));
        let stale = find_stale_wiki(&store).unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].wiki_id, page.id);
        assert_eq!(stale[0].link_kind, "source_version");
        mock.abort();
    }
}

#[tokio::test]
async fn source_versions_exclude_old_parents_after_regeneration() {
    for consolidate_mode in [false, true] {
        let store = open_store();
        let config = sample_config(16);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(16));
        let mut sources = Vec::new();
        for name in ["a", "b"] {
            let raw = ingest_raw(
                &store,
                &embedder,
                &config,
                format!("Source {name}"),
                Some(name.into()),
                Some(format!("raw://{name}")),
                None,
                None,
                None,
            )
            .await
            .unwrap();
            sources.push(store.get_document(&raw.document_id).unwrap().unwrap());
        }
        let proposal = ConsolidateProposal {
            slug: "generated-target".into(),
            title: "Generated".into(),
            content: "Initial synthesis from both sources".into(),
            kind: "concept".into(),
            category: None,
            summary: None,
            suggested_links: vec![],
            notes: None,
        };
        apply_consolidate_proposal(&store, &embedder, &config, &proposal, &sources, None)
            .await
            .unwrap();
        let (llm, mock) = mock_wiki_llm(
            Arc::new(tokio::sync::Semaphore::new(0)),
            Arc::new(tokio::sync::Semaphore::new(1)),
        )
        .await;
        if consolidate_mode {
            consolidate(
                &store,
                &embedder,
                &config,
                &llm,
                &[sources[0].id.clone()],
                true,
                ConsolidateOpts::default(),
                None,
            )
            .await
            .unwrap();
        } else {
            compile_source(
                &store,
                &embedder,
                &config,
                &llm,
                &sources[0].id,
                false,
                None,
            )
            .await
            .unwrap();
        }
        mock.abort();
        let mut page = get_wiki_page(&store, "generated-target").unwrap();
        let versions = wiki_source_versions(&page).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].document_id, sources[0].id);

        ingest_raw(
            &store,
            &embedder,
            &config,
            "Changed excluded source".into(),
            Some("b".into()),
            Some("raw://b".into()),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let b = store.find_by_uri("raw://b").unwrap().unwrap();
        assert!(b.updated_at > page.updated_at);
        let b_node = store.find_node_by_document_id(&b.id).unwrap().unwrap();
        let page_node = store.find_node_by_document_id(&page.id).unwrap().unwrap();
        assert!(
            store
                .neighbors(&b_node.id, 1, 500)
                .unwrap()
                .edges
                .iter()
                .any(|edge| edge.rel_type == "related" && edge.target_id == page_node.id),
            "manual provenance link must survive the source graph rebuild"
        );
        assert!(
            find_stale_wiki(&store).unwrap().is_empty(),
            "excluded source must not make regenerated wiki stale; consolidate={consolidate_mode}"
        );

        ingest_raw(
            &store,
            &embedder,
            &config,
            "Changed retained source".into(),
            Some("a".into()),
            Some("raw://a".into()),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let stale = find_stale_wiki(&store).unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].raw_id, sources[0].id);
        assert_eq!(stale[0].link_kind, "source_version");

        // A partial/malformed array must not suppress legacy parents. An
        // explicitly empty valid snapshot, however, records no dependencies.
        let mut metadata: serde_json::Value = serde_json::from_str(&page.metadata_json).unwrap();
        metadata["source_versions"] = serde_json::json!([versions[0], {"broken":true}]);
        page.metadata_json = metadata.to_string();
        store.upsert_document(&page).unwrap();
        assert_eq!(find_stale_wiki(&store).unwrap().len(), 2);
        metadata["source_versions"] = serde_json::json!([]);
        page.metadata_json = metadata.to_string();
        store.upsert_document(&page).unwrap();
        assert!(find_stale_wiki(&store).unwrap().is_empty());
    }
}

#[tokio::test]
async fn source_versions_ignore_metadata_changes_and_report_deleted_parents() {
    let store = open_store();
    let config = sample_config(16);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(16));
    let raw = ingest_raw(
        &store,
        &embedder,
        &config,
        "Original source".into(),
        Some("Source".into()),
        Some("raw://source".into()),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let source = store.get_document(&raw.document_id).unwrap().unwrap();
    let proposal = ConsolidateProposal {
        slug: "summary".into(),
        title: "Summary".into(),
        content: "Source synthesis".into(),
        kind: "concept".into(),
        category: None,
        summary: None,
        suggested_links: vec![],
        notes: None,
    };
    let written =
        apply_consolidate_proposal(&store, &embedder, &config, &proposal, &[source], None)
            .await
            .unwrap();
    let mut source = store.get_document(&raw.document_id).unwrap().unwrap();
    source.updated_at = Utc::now() + chrono::Duration::seconds(1);
    source.pinned = true;
    source.title = "New source title with the same content".into();
    store.upsert_document(&source).unwrap();
    assert!(find_stale_wiki(&store).unwrap().is_empty());
    // Replicas can use different local source IDs; the recorded URI is a
    // fallback identity while the body hash remains the freshness evidence.
    let mut page = get_wiki_page(&store, "summary").unwrap();
    let mut metadata: serde_json::Value = serde_json::from_str(&page.metadata_json).unwrap();
    metadata["source_versions"][0]["document_id"] = serde_json::json!("remote-source-id");
    page.metadata_json = metadata.to_string();
    store.upsert_document(&page).unwrap();
    assert!(find_stale_wiki(&store).unwrap().is_empty());
    store.delete_document(&raw.document_id).unwrap();
    let stale = find_stale_wiki(&store).unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].wiki_id, written.document_id);
    assert_eq!(stale[0].raw_id, "remote-source-id");
    assert_eq!(stale[0].raw_uri, "raw://source");
    assert_eq!(stale[0].link_kind, "source_missing");
    assert!(stale[0].raw_updated_at.is_none());
    let refresh = refresh_stale_wiki(&store, &embedder, &config, None, true, None, None)
        .await
        .unwrap();
    assert_eq!(refresh.stale_count, 1);
    assert!(refresh.raw_sources.is_empty());
    assert!(refresh
        .notes
        .unwrap()
        .contains("1 source reference(s) are unavailable"));
}

#[tokio::test]
async fn wiki_crud_write_get_list_update() {
    let store = open_store();
    let dims = 16usize;
    let config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));

    let written = write_wiki_page(
        &store,
        &embedder,
        &config,
        "cats",
        "Cats",
        "Felines are [[Mammals]] #animals",
        "entity",
        Some("biology"),
        Some("About cats"),
        None,
    )
    .await
    .expect("write");
    assert_eq!(written.uri, "wiki://cats");
    assert_eq!(written.slug, "cats");

    let by_slug = get_wiki_page(&store, "cats").expect("get by slug");
    assert_eq!(by_slug.layer, LAYER_WIKI);
    assert_eq!(by_slug.id, written.document_id);
    assert!(by_slug.content.contains("Felines"));

    let by_uri = get_wiki_page(&store, "wiki://cats").expect("get by uri");
    assert_eq!(by_uri.id, written.document_id);

    let by_id = get_wiki_page(&store, &written.document_id).expect("get by id");
    assert_eq!(by_id.title, "Cats");

    let listed = list_wiki_pages(&store).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, written.document_id);

    let updated = update_wiki_page(
        &store,
        &embedder,
        &config,
        "cats",
        Some("Domestic Cats"),
        "Updated body about cats #pets",
        None,
        None,
        Some("updated summary"),
        None,
    )
    .await
    .expect("update");
    assert_eq!(updated.document_id, written.document_id);

    let after = get_wiki_page(&store, "cats").expect("get after update");
    assert_eq!(after.title, "Domestic Cats");
    assert!(after.content.contains("Updated body"));
    assert_eq!(after.layer, LAYER_WIKI);
}

#[tokio::test]
async fn wiki_update_preserves_omitted_placement_state_and_arbitrary_metadata() {
    let store = open_store();
    let dims = 16usize;
    let config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));

    let written = write_wiki_page(
        &store,
        &embedder,
        &config,
        "preserved-page",
        "Preserved page",
        "original body",
        "concept",
        Some("architecture"),
        Some("original summary"),
        None,
    )
    .await
    .expect("create wiki page");
    let original_index = store
        .get_wiki_index_by_slug("preserved-page")
        .expect("read index")
        .expect("index exists");

    let mut persisted = store
        .get_document(&written.document_id)
        .expect("read document")
        .expect("document exists");
    persisted.wing = Some("alpha".into());
    persisted.room = Some("design".into());
    persisted.source_file = Some("/sources/alpha/design.md".into());
    persisted.status = "archived".into();
    persisted.pinned = true;
    persisted.boost = 2.75;
    persisted.metadata_json =
            r#"{"category":"architecture","summary":"original summary","custom":{"retained":true},"rank":7}"#
                .into();
    let prior_revision = persisted.revision;
    persisted.revision = store
        .upsert_document_cas(&persisted, Some(prior_revision))
        .expect("seed placement and state");
    let metadata_before = persisted.metadata_json.clone();

    let updated = update_wiki_page_cas(
        &store,
        &embedder,
        &config,
        "preserved-page",
        Some("Edited title"),
        "edited body",
        None,
        None,
        None,
        None,
        Some(persisted.revision),
    )
    .await
    .expect("update with omitted optional fields");

    let after = store
        .get_document(&updated.document_id)
        .expect("read updated document")
        .expect("updated document exists");
    assert_eq!(after.wing.as_deref(), Some("alpha"));
    assert_eq!(after.room.as_deref(), Some("design"));
    assert_eq!(
        after.source_file.as_deref(),
        Some("/sources/alpha/design.md")
    );
    assert_eq!(after.status, "archived");
    assert!(after.pinned);
    assert_eq!(after.boost, 2.75);
    assert_eq!(after.kind, "concept");
    assert_eq!(after.metadata_json, metadata_before);
    let after_index = store
        .get_wiki_index_by_slug("preserved-page")
        .expect("read updated index")
        .expect("updated index exists");
    assert_eq!(after_index.id, original_index.id);
    assert_eq!(after_index.kind, "concept");
    assert_eq!(after_index.category.as_deref(), Some("architecture"));
    assert_eq!(after_index.summary.as_deref(), Some("original summary"));

    let overlaid = update_wiki_page_cas(
        &store,
        &embedder,
        &config,
        "preserved-page",
        None,
        "edited body again",
        None,
        Some("systems"),
        Some("new summary"),
        None,
        Some(after.revision),
    )
    .await
    .expect("metadata overlay update");
    let overlaid = store
        .get_document(&overlaid.document_id)
        .expect("read overlaid document")
        .expect("overlaid document exists");
    let metadata: serde_json::Value =
        serde_json::from_str(&overlaid.metadata_json).expect("valid metadata object");
    assert_eq!(metadata["custom"]["retained"], true);
    assert_eq!(metadata["rank"], 7);
    assert_eq!(metadata["category"], "systems");
    assert_eq!(metadata["summary"], "new summary");
    assert_eq!(overlaid.wing.as_deref(), Some("alpha"));
    assert!(overlaid.pinned);
}

#[tokio::test]
async fn wiki_require_if_match_blocks_update_without_revision() {
    let store = open_store();
    let dims = 16usize;
    let mut config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));

    let written = write_wiki_page(
        &store,
        &embedder,
        &config,
        "cas-page",
        "CAS Page",
        "first body",
        "wiki",
        None,
        None,
        None,
    )
    .await
    .expect("create without if_match is allowed");
    assert_eq!(written.revision, 1);

    config.wiki_require_if_match = true;
    let err = write_wiki_page(
        &store,
        &embedder,
        &config,
        "cas-page",
        "CAS Page",
        "second body without if_match",
        "wiki",
        None,
        None,
        None,
    )
    .await
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("if_match_revision") && msg.contains("RAG_WIKI_REQUIRE_IF_MATCH"),
        "expected require-if-match error, got {msg}"
    );

    let ok = write_wiki_page_with_opts(
        &store,
        &embedder,
        &config,
        "cas-page",
        "CAS Page",
        "second body with if_match",
        "wiki",
        None,
        None,
        None,
        WriteWikiOpts {
            if_match_revision: Some(written.revision),
            ..Default::default()
        },
    )
    .await
    .expect("update with matching revision");
    assert_eq!(ok.revision, 2);
}

/// Stale if_match must fail with Conflict and leave body+chunks intact
/// (CAS before delete_chunks / re-embed).
#[tokio::test]
async fn stale_if_match_preserves_body_and_chunks() {
    let store = open_store();
    let dims = 16usize;
    let config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));

    let original_body = "CAS preserve body: unique retrieval text for chunks";
    let written = write_wiki_page(
        &store,
        &embedder,
        &config,
        "cas-preserve",
        "CAS Preserve",
        original_body,
        "wiki",
        None,
        None,
        None,
    )
    .await
    .expect("create");
    assert_eq!(written.revision, 1);

    let chunks_before = store
        .list_chunks_for_document(&written.document_id)
        .expect("list chunks");
    assert!(
        !chunks_before.is_empty(),
        "expected chunks after successful wiki write"
    );
    let chunk_ids_before: Vec<String> = chunks_before.iter().map(|c| c.id.clone()).collect();

    let err = write_wiki_page_with_opts(
        &store,
        &embedder,
        &config,
        "cas-preserve",
        "CAS Preserve",
        "should not replace body or wipe chunks",
        "wiki",
        None,
        None,
        None,
        WriteWikiOpts {
            // Stale: page is at revision 1.
            if_match_revision: Some(0),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, AppError::Conflict(_)),
        "expected Conflict on stale if_match, got {err:?}"
    );

    let doc = store
        .get_document(&written.document_id)
        .expect("get")
        .expect("exists");
    assert_eq!(doc.revision, 1, "revision must not advance on CAS failure");
    assert_eq!(
        doc.content, original_body,
        "body must be preserved on stale if_match"
    );

    let chunks_after = store
        .list_chunks_for_document(&written.document_id)
        .expect("list chunks after conflict");
    assert_eq!(
        chunks_after.len(),
        chunks_before.len(),
        "chunk count must not change on CAS failure"
    );
    let chunk_ids_after: Vec<String> = chunks_after.iter().map(|c| c.id.clone()).collect();
    assert_eq!(
        chunk_ids_after, chunk_ids_before,
        "chunks must not be deleted on stale if_match"
    );
    assert!(
        chunks_after
            .iter()
            .any(|c| c.content.contains("unique retrieval")),
        "chunk text must still reflect original body"
    );
}

#[tokio::test]
async fn raw_layer_immutable_rejects_wiki_overwrite_and_content_update() {
    let store = open_store();
    let dims = 16usize;
    let config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));

    let raw = ingest_raw(
        &store,
        &embedder,
        &config,
        "raw source text".into(),
        Some("Raw Title".into()),
        Some("wiki://stolen-slug".into()),
        None,
        None,
        None,
    )
    .await
    .expect("ingest raw at wiki-like uri");

    let raw_doc = store.get_document(&raw.document_id).unwrap().expect("raw");
    assert_eq!(raw_doc.layer, LAYER_RAW);
    assert!(assert_content_mutable(&raw_doc).is_err());

    // write_wiki_page must not overwrite raw content at same uri.
    let err = write_wiki_page(
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
    .unwrap_err();
    assert!(
        matches!(err, AppError::Forbidden(_)),
        "expected Forbidden, got {err}"
    );

    // get_wiki_page refuses raw documents.
    let get_err = get_wiki_page(&store, &raw.document_id).unwrap_err();
    assert!(matches!(get_err, AppError::Forbidden(_)));

    // Re-ingest replace is allowed (does not call assert_content_mutable).
    let again = ingest_raw(
        &store,
        &embedder,
        &config,
        "replaced raw content".into(),
        Some("Raw Title".into()),
        Some("wiki://stolen-slug".into()),
        None,
        None,
        None,
    )
    .await
    .expect("re-ingest replace");
    assert_eq!(again.document_id, raw.document_id);
    let replaced = store.get_document(&raw.document_id).unwrap().unwrap();
    assert_eq!(replaced.content, "replaced raw content");
    assert_eq!(replaced.layer, LAYER_RAW);
}

#[test]
fn update_wiki_page_missing_is_not_found() {
    let store = open_store();
    let err = get_wiki_page(&store, "no-such-page").unwrap_err();
    assert!(matches!(err, AppError::NotFound(_)));
}

#[tokio::test]
async fn file_answer_writes_wiki_graph_index_and_ops_log() {
    let store = open_store();
    let dims = 16usize;
    let config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));

    let cites = vec![FileAnswerCitation {
        document_id: Some("doc-src".into()),
        uri: Some("raw://alpha".into()),
        title: Some("Alpha source".into()),
        chunk_id: Some("c1".into()),
        quote: Some("alpha fact".into()),
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
        Some("agent-test"),
    )
    .await
    .expect("file_answer");

    assert_eq!(wr.slug, "why-alpha-matters");
    assert_eq!(wr.uri, "wiki://why-alpha-matters");
    assert!(!wr.node_id.is_empty());
    assert!(wr.chunk_count >= 1);
    assert!(wr.edge_count >= 1, "wikilink/tag edges expected");

    let doc = store
        .get_document(&wr.document_id)
        .unwrap()
        .expect("wiki doc");
    assert_eq!(doc.layer, "wiki");
    assert_eq!(doc.kind, "wiki");
    assert!(doc.content.contains("Alpha is important"));
    assert!(doc.content.contains("## Citations"));
    assert!(doc.content.contains("Alpha source"));
    let meta: serde_json::Value = serde_json::from_str(&doc.metadata_json).unwrap();
    assert_eq!(meta["category"], "answers");
    assert_eq!(meta["filed_as"], "answer");
    assert!(meta["citations"].as_array().unwrap().len() == 1);

    let idx = store
        .get_wiki_index_by_slug("why-alpha-matters")
        .unwrap()
        .expect("index entry");
    assert_eq!(idx.page_id.as_deref(), Some(wr.document_id.as_str()));
    assert_eq!(idx.category.as_deref(), Some("answers"));

    let ops = store.list_ops_log(20).unwrap();
    let fa = ops
        .iter()
        .find(|e| e.op == "file_answer")
        .expect("ops_log file_answer row");
    assert_eq!(fa.prefix.as_deref(), Some("FILE"));
    assert_eq!(fa.entity_id.as_deref(), Some(wr.document_id.as_str()));
    assert_eq!(fa.agent_name.as_deref(), Some("agent-test"));
}

#[tokio::test]
async fn file_answer_embedding_failure_preserves_document_chunks_graph_and_side_effects() {
    let store = open_store();
    let dims = 16usize;
    let config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));

    let written = file_answer(
        &store,
        &embedder,
        &config,
        "Atomic Answer",
        "Original answer links to [[Old target]].",
        Some("atomic-answer"),
        None,
        Some("agent-test"),
    )
    .await
    .expect("initial file_answer");
    let original_document = store
        .get_document(&written.document_id)
        .unwrap()
        .expect("original document");
    let original_chunks = chunk_state(&store, &written.document_id);
    let original_node = store
        .find_node_by_document_id(&written.document_id)
        .unwrap()
        .expect("original graph node");
    let original_edges = outgoing_edge_state(&store, &original_node.id);
    let original_index = wiki_index_state(&store, "atomic-answer");
    let original_ops_count = store.list_ops_log(100).unwrap().len();

    let failing_embedder: Arc<dyn EmbeddingProvider> = Arc::new(FailingEmbedder { dims });
    let error = file_answer(
        &store,
        &failing_embedder,
        &config,
        "Changed Answer",
        "Changed answer links to [[New target]].",
        Some("atomic-answer"),
        None,
        Some("agent-test"),
    )
    .await
    .expect_err("embedding must fail before persistence");
    assert!(error
        .to_string()
        .contains("injected wiki embedding failure"));

    let persisted_document = store
        .get_document(&written.document_id)
        .unwrap()
        .expect("document after embedding failure");
    assert_eq!(persisted_document.revision, original_document.revision);
    assert_eq!(persisted_document.title, original_document.title);
    assert_eq!(persisted_document.content, original_document.content);
    assert_eq!(chunk_state(&store, &written.document_id), original_chunks);
    let persisted_node = store
        .find_node_by_document_id(&written.document_id)
        .unwrap()
        .expect("graph node after embedding failure");
    assert_eq!(persisted_node.id, original_node.id);
    assert_eq!(persisted_node.label, original_node.label);
    assert_eq!(
        outgoing_edge_state(&store, &persisted_node.id),
        original_edges
    );
    assert!(store.find_nodes_by_label("New target").unwrap().is_empty());
    assert_eq!(wiki_index_state(&store, "atomic-answer"), original_index);
    assert_eq!(store.list_ops_log(100).unwrap().len(), original_ops_count);
}

#[tokio::test]
async fn wiki_graph_fault_rolls_back_document_chunks_graph_and_side_effects() {
    let store = open_store();
    let dims = 16usize;
    let config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));

    let written = write_wiki_page(
        &store,
        &embedder,
        &config,
        "atomic-wiki-update",
        "Original Wiki Title",
        "Original body links to [[Old target]].",
        "wiki",
        Some("tests"),
        Some("original summary"),
        Some("agent-test"),
    )
    .await
    .expect("initial wiki write");
    let original_document = store
        .get_document(&written.document_id)
        .unwrap()
        .expect("original document");
    let original_chunks = chunk_state(&store, &written.document_id);
    let original_node = store
        .find_node_by_document_id(&written.document_id)
        .unwrap()
        .expect("original graph node");
    let original_edges = outgoing_edge_state(&store, &original_node.id);
    let original_index = wiki_index_state(&store, "atomic-wiki-update");
    let original_ops_count = store.list_ops_log(100).unwrap().len();

    // Test-only fault injector: the graph rebuild inserts the first edge,
    // then the duplicate violates this constraint after doc/chunk writes.
    store
            .lock()
            .unwrap()
            .execute(
                "CREATE UNIQUE INDEX wiki_rollback_edge_key ON graph_edges(source_id, target_id, rel_type)",
                [],
            )
            .expect("fault index");

    let error = update_wiki_page_cas(
        &store,
        &embedder,
        &config,
        "atomic-wiki-update",
        Some("Changed Wiki Title"),
        "Changed body [[Repeated target]] and [[Repeated target]].",
        None,
        None,
        Some("changed summary"),
        Some("agent-test"),
        Some(written.revision),
    )
    .await
    .expect_err("duplicate graph edge must fail the wiki transaction");
    assert!(
        error.to_string().contains("duplicate")
            || error.to_string().contains("constraint")
            || error.to_string().contains("unique"),
        "unexpected fault: {error}"
    );

    let persisted_document = store
        .get_document(&written.document_id)
        .unwrap()
        .expect("document after graph rollback");
    assert_eq!(persisted_document.revision, original_document.revision);
    assert_eq!(persisted_document.title, original_document.title);
    assert_eq!(persisted_document.content, original_document.content);
    assert_eq!(chunk_state(&store, &written.document_id), original_chunks);
    let persisted_node = store
        .find_node_by_document_id(&written.document_id)
        .unwrap()
        .expect("graph node after rollback");
    assert_eq!(persisted_node.id, original_node.id);
    assert_eq!(persisted_node.label, original_node.label);
    assert_eq!(
        outgoing_edge_state(&store, &persisted_node.id),
        original_edges
    );
    assert!(store
        .find_nodes_by_label("Repeated target")
        .unwrap()
        .is_empty());
    assert!(store
        .list_document_revisions(&written.document_id)
        .unwrap()
        .is_empty());
    assert_eq!(
        wiki_index_state(&store, "atomic-wiki-update"),
        original_index
    );
    assert_eq!(store.list_ops_log(100).unwrap().len(), original_ops_count);
}

#[tokio::test]
async fn late_catalog_fault_rolls_back_new_wiki_page_and_audit() {
    let store = open_store();
    let dims = 16usize;
    let config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));
    let ops_before = store.list_ops_log(100).unwrap().len();

    let error = write_wiki_page_with_opts(
        &store,
        &embedder,
        &config,
        "late-create-rollback",
        "Late create rollback",
        "New body links to [[Never committed]].",
        "wiki",
        Some("tests"),
        Some("must roll back"),
        Some("agent-test"),
        WriteWikiOpts {
            op: Some(crate::db::store::TEST_FAIL_WIKI_WRITE_AFTER_INDEX_OP.into()),
            ..Default::default()
        },
    )
    .await
    .expect_err("late catalog fault must roll back the complete wiki write");

    assert!(error
        .to_string()
        .contains("injected wiki write failure after catalog update"));
    assert!(store
        .find_by_uri("wiki://late-create-rollback")
        .unwrap()
        .is_none());
    assert!(wiki_index_state(&store, "late-create-rollback").is_none());
    assert!(store
        .find_nodes_by_label("Never committed")
        .unwrap()
        .is_empty());
    assert_eq!(store.list_ops_log(100).unwrap().len(), ops_before);
}

#[tokio::test]
async fn late_catalog_fault_rolls_back_existing_wiki_revision_and_derived_state() {
    let store = open_store();
    let dims = 16usize;
    let config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));
    let written = write_wiki_page(
        &store,
        &embedder,
        &config,
        "late-update-rollback",
        "Original title",
        "Original body links to [[Old target]].",
        "concept",
        Some("original"),
        Some("original summary"),
        Some("agent-test"),
    )
    .await
    .expect("initial wiki write");
    let original_document = store
        .get_document(&written.document_id)
        .unwrap()
        .expect("original document");
    let original_chunks = chunk_state(&store, &written.document_id);
    let original_node = store
        .find_node_by_document_id(&written.document_id)
        .unwrap()
        .expect("original node");
    let original_edges = outgoing_edge_state(&store, &original_node.id);
    let original_index = wiki_index_state(&store, "late-update-rollback");
    let original_ops_count = store.list_ops_log(100).unwrap().len();

    let error = write_wiki_page_with_opts(
            &store,
            &embedder,
            &config,
            "late-update-rollback",
            "Changed title",
            "Changed body links to [[New target]].",
            "concept",
            Some("changed"),
            Some("changed summary"),
            Some("agent-test"),
            WriteWikiOpts {
                op: Some(crate::db::store::TEST_FAIL_WIKI_WRITE_AFTER_INDEX_OP.into()),
                if_match_revision: Some(written.revision),
                extra_metadata: Some(serde_json::json!({"source_versions":[{
                    "document_id":"new-source", "uri":"raw://new-source", "content_hash":"new-source-hash"
                }]})),
                ..Default::default()
            },
        )
        .await
        .expect_err("late catalog fault must roll back the existing wiki update");

    assert!(error
        .to_string()
        .contains("injected wiki write failure after catalog update"));
    let persisted = store
        .get_document(&written.document_id)
        .unwrap()
        .expect("document survives rollback");
    assert_eq!(persisted.revision, original_document.revision);
    assert_eq!(persisted.title, original_document.title);
    assert_eq!(persisted.content, original_document.content);
    assert_eq!(persisted.metadata_json, original_document.metadata_json);
    assert_eq!(chunk_state(&store, &written.document_id), original_chunks);
    let persisted_node = store
        .find_node_by_document_id(&written.document_id)
        .unwrap()
        .expect("node survives rollback");
    assert_eq!(persisted_node.id, original_node.id);
    assert_eq!(persisted_node.label, original_node.label);
    assert_eq!(
        outgoing_edge_state(&store, &persisted_node.id),
        original_edges
    );
    assert!(store.find_nodes_by_label("New target").unwrap().is_empty());
    assert_eq!(
        wiki_index_state(&store, "late-update-rollback"),
        original_index
    );
    assert!(store
        .list_document_revisions(&written.document_id)
        .unwrap()
        .is_empty());
    assert_eq!(store.list_ops_log(100).unwrap().len(), original_ops_count);
}

#[tokio::test]
async fn find_stale_wiki_via_metadata_and_graph() {
    use chrono::Duration;

    let store = open_store();
    let dims = 16usize;
    let config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));

    let older = Utc::now() - Duration::days(5);
    let newer = Utc::now();

    let raw = ingest_raw(
        &store,
        &embedder,
        &config,
        "version one of the source".into(),
        Some("Alpha".into()),
        Some("raw://alpha".into()),
        None,
        None,
        None,
    )
    .await
    .expect("ingest raw");

    // Pin raw to older timestamp so wiki (now) is fresh.
    let mut raw_doc = store.get_document(&raw.document_id).unwrap().unwrap();
    raw_doc.updated_at = older;
    store.upsert_document(&raw_doc).unwrap();

    let wiki = write_wiki_page_with_opts(
        &store,
        &embedder,
        &config,
        "alpha-summary",
        "Alpha summary",
        "Summary of Alpha.\nsource: raw://alpha\n",
        "source_summary",
        Some("sources"),
        Some("Alpha summary"),
        None,
        WriteWikiOpts {
            op: Some("wiki_write".into()),
            prefix: Some("WIKI".into()),
            message: None,
            extra_metadata: Some(serde_json::json!({
                "source_id": raw.document_id,
                "source_uri": "raw://alpha",
            })),
            extra_payload: None,
            metadata_json_override: None,
            if_match_revision: None,
        },
    )
    .await
    .expect("write wiki");

    let raw_node = store
        .find_node_by_document_id(&raw.document_id)
        .unwrap()
        .expect("raw node");
    let wiki_node = store
        .find_node_by_document_id(&wiki.document_id)
        .unwrap()
        .expect("wiki node");
    store
        .link_nodes(&raw_node.id, &wiki_node.id, "related", 1.0)
        .unwrap();

    let empty = find_stale_wiki(&store).unwrap();
    assert!(
        empty.is_empty(),
        "expected no stale while wiki newer than raw, got {empty:?}"
    );

    // Bump raw past wiki without re-ingest (keeps related edge).
    let mut raw_doc = store.get_document(&raw.document_id).unwrap().unwrap();
    raw_doc.updated_at = newer;
    raw_doc.content = "version two of the source - changed".into();
    store.upsert_document(&raw_doc).unwrap();

    let mut wiki_doc = store.get_document(&wiki.document_id).unwrap().unwrap();
    wiki_doc.updated_at = older;
    store.upsert_document(&wiki_doc).unwrap();

    let stale = find_stale_wiki(&store).unwrap();
    assert_eq!(stale.len(), 1, "expected one stale pair, got {stale:?}");
    assert_eq!(stale[0].wiki_id, wiki.document_id);
    assert_eq!(stale[0].raw_id, raw.document_id);
    assert!(
        stale[0].link_kind == "metadata"
            || stale[0].link_kind == "content_source"
            || stale[0].link_kind == "graph_related",
        "link_kind={}",
        stale[0].link_kind
    );

    let report = refresh_stale_wiki(&store, &embedder, &config, None, true, None, Some("tester"))
        .await
        .expect("refresh dry_run");
    assert_eq!(report.stale_count, 1);
    assert_eq!(report.raw_sources.len(), 1);
    assert_eq!(report.raw_sources[0].raw_id, raw.document_id);
    assert!(!report.applied);
    assert!(report.dry_run);
    assert!(report.recompiled.is_empty());

    let ops = store.list_ops_log(10).unwrap();
    assert!(
        ops.iter().any(|e| e.op == "refresh_stale_wiki"),
        "ops_log should record refresh_stale_wiki"
    );
}

#[tokio::test]
async fn apply_consolidate_proposal_writes_wiki_graph_index_and_ops() {
    let store = open_store();
    let dims = 16usize;
    let config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));

    let a = ingest_raw(
        &store,
        &embedder,
        &config,
        "Alpha talks about hybrid search and BM25.".into(),
        Some("Alpha note".into()),
        Some("raw://alpha-note".into()),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let b = ingest_raw(
        &store,
        &embedder,
        &config,
        "Beta covers vector embeddings and RRF fusion.".into(),
        Some("Beta note".into()),
        Some("raw://beta-note".into()),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let doc_a = store.get_document(&a.document_id).unwrap().unwrap();
    let doc_b = store.get_document(&b.document_id).unwrap().unwrap();

    let proposal = ConsolidateProposal {
            slug: "hybrid-search".into(),
            title: "Hybrid Search".into(),
            kind: "concept".into(),
            category: Some("retrieval".into()),
            content: "Hybrid search fuses BM25 and vectors via [[RRF]] #search\n\n## Sources\n- Alpha note\n- Beta note\n".into(),
            summary: Some("BM25 + vec fusion".into()),
            suggested_links: vec!["RRF".into(), "[[FTS]]".into()],
            notes: Some("merged alpha+beta".into()),
        };

    let wr = apply_consolidate_proposal(
        &store,
        &embedder,
        &config,
        &proposal,
        &[doc_a, doc_b],
        Some("agent-c"),
    )
    .await
    .expect("apply_consolidate_proposal");

    assert_eq!(wr.uri, "wiki://hybrid-search");
    assert_eq!(wr.slug, "hybrid-search");
    assert!(wr.chunk_count >= 1);
    assert!(!wr.node_id.is_empty());

    let wiki = get_wiki_page(&store, "hybrid-search").unwrap();
    assert_eq!(wiki.layer, LAYER_WIKI);
    assert_eq!(wiki.kind, "concept");
    assert!(wiki.content.contains("BM25"));
    let meta: serde_json::Value = serde_json::from_str(&wiki.metadata_json).unwrap();
    assert_eq!(meta["filed_as"], "consolidate");
    assert_eq!(meta["category"], "retrieval");
    let from = meta["source_ids"].as_array().expect("source_ids");
    assert_eq!(from.len(), 2);

    let idx = store
        .get_wiki_index_by_slug("hybrid-search")
        .unwrap()
        .expect("index");
    assert_eq!(idx.page_id.as_deref(), Some(wr.document_id.as_str()));
    assert_eq!(idx.category.as_deref(), Some("retrieval"));

    // Graph related edges from both sources.
    let wiki_node = store
        .find_node_by_document_id(&wr.document_id)
        .unwrap()
        .expect("wiki node");
    for src_id in [&a.document_id, &b.document_id] {
        let src_node = store
            .find_node_by_document_id(src_id)
            .unwrap()
            .expect("src node");
        let view = store.neighbors(&src_node.id, 1, 50).unwrap();
        let linked = view.edges.iter().any(|e| {
            e.rel_type == "related"
                && ((e.source_id == src_node.id && e.target_id == wiki_node.id)
                    || (e.target_id == src_node.id && e.source_id == wiki_node.id))
        });
        assert!(linked, "expected related edge from {src_id} to wiki");
    }

    let ops = store.list_ops_log(20).unwrap();
    assert!(
        ops.iter().any(|e| e.op == "consolidate_write"),
        "ops_log should record consolidate_write"
    );

    // Stored parent arrays are sufficient when graph relations are lost.
    for src_id in [&a.document_id, &b.document_id] {
        let node = store.find_node_by_document_id(src_id).unwrap().unwrap();
        store.delete_edges_from(&node.id).unwrap();
        let mut raw = store.get_document(src_id).unwrap().unwrap();
        raw.updated_at = Utc::now() + chrono::Duration::seconds(1);
        raw.content.push_str(" updated body");
        raw.content_hash = Some(content_hash(&raw.content));
        store.upsert_document(&raw).unwrap();
    }
    let stale = find_stale_wiki(&store).unwrap();
    assert_eq!(stale.len(), 2);
    assert!(stale
        .iter()
        .all(|item| item.wiki_id == wr.document_id && item.link_kind == "source_version"));
    let parents = stale
        .iter()
        .map(|item| item.raw_id.as_str())
        .collect::<HashSet<_>>();
    assert_eq!(
        parents,
        HashSet::from([a.document_id.as_str(), b.document_id.as_str()])
    );
}

#[test]
fn stale_wiki_resolves_each_parent_array_without_graph_edges() {
    let store = open_store();
    let now = Utc::now();
    let raw = Document {
        id: "raw-parent".into(),
        uri: "raw://parent".into(),
        title: "Parent".into(),
        content: "raw content".into(),
        layer: LAYER_RAW.into(),
        updated_at: now,
        ..Document::default()
    };
    store.upsert_document(&raw).unwrap();
    for (index, (field, reference)) in [
        ("source_ids", raw.id.as_str()),
        ("source_uris", raw.uri.as_str()),
        ("consolidated_from", raw.id.as_str()),
    ]
    .into_iter()
    .enumerate()
    {
        store
            .upsert_document(&Document {
                id: format!("wiki-{index}"),
                uri: format!("wiki://array-{index}"),
                title: field.into(),
                content: "synthesis without source markers".into(),
                layer: LAYER_WIKI.into(),
                updated_at: now - chrono::Duration::seconds(1),
                metadata_json: serde_json::json!({field:[reference,reference,"missing",null,42]})
                    .to_string(),
                ..Document::default()
            })
            .unwrap();
    }
    let stale = find_stale_wiki(&store).unwrap();
    assert_eq!(stale.len(), 3);
    assert!(stale
        .iter()
        .all(|item| item.raw_id == raw.id && item.link_kind == "metadata"));
}

#[tokio::test]
async fn consolidate_rejects_empty_ids() {
    let store = open_store();
    let dims = 16usize;
    let config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));
    let llm = ChatClient::with_limits("http://127.0.0.1:9/v1", "x", "m", 1, 16).unwrap();
    let err = consolidate(
        &store,
        &embedder,
        &config,
        &llm,
        &[],
        false,
        ConsolidateOpts::default(),
        None,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("at least one"));
}

#[tokio::test]
async fn refresh_stale_wiki_apply_without_llm_lists_only() {
    use chrono::Duration;

    let store = open_store();
    let dims = 16usize;
    let config = sample_config(dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(crate::embeddings::MockEmbedder::new(dims));

    let older = Utc::now() - Duration::days(3);
    let newer = Utc::now();

    let raw = ingest_raw(
        &store,
        &embedder,
        &config,
        "body".into(),
        Some("B".into()),
        Some("raw://b".into()),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let wiki = write_wiki_page_with_opts(
        &store,
        &embedder,
        &config,
        "b-sum",
        "B sum",
        "source: raw://b",
        "source_summary",
        None,
        None,
        None,
        WriteWikiOpts {
            extra_metadata: Some(serde_json::json!({
                "source_uri": "raw://b",
                "source_id": raw.document_id,
            })),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let mut wiki_doc = store.get_document(&wiki.document_id).unwrap().unwrap();
    wiki_doc.updated_at = older;
    store.upsert_document(&wiki_doc).unwrap();

    let mut raw_doc = store.get_document(&raw.document_id).unwrap().unwrap();
    raw_doc.updated_at = newer;
    store.upsert_document(&raw_doc).unwrap();

    let report = refresh_stale_wiki(&store, &embedder, &config, None, false, Some(10), None)
        .await
        .unwrap();
    assert_eq!(report.stale_count, 1);
    assert!(!report.applied);
    assert!(report
        .notes
        .as_deref()
        .unwrap_or("")
        .contains("no ChatClient"));
}
