use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use super::*;
use crate::embeddings::MockEmbedder;
use crate::error::Result;

#[test]
fn dodo_children_are_projects_with_nested_rooms() {
    let root = Path::new("/workspace/Sources");
    let path = Path::new("/workspace/Sources/Dodo/referralprogram/src/main.rs");
    assert_eq!(
        inferred_scope(root, path),
        ("referralprogram".into(), "src".into())
    );
    assert!(belongs_to_dodo_group(root, path));
}

#[test]
fn dodo_group_tag_is_added_once() {
    let tagged = add_metadata_tag(r#"{"tags":["backend"]}"#, "Dodo").unwrap();
    let tagged_again = add_metadata_tag(&tagged, "Dodo").unwrap();
    let value: serde_json::Value = serde_json::from_str(&tagged_again).unwrap();
    assert_eq!(value["tags"], serde_json::json!(["backend", "Dodo"]));
}

struct CountingEmbedder {
    inner: MockEmbedder,
    batches: Arc<AtomicUsize>,
}

struct GatedEmbedder {
    inner: MockEmbedder,
    calls: AtomicUsize,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

struct RecordingEmbedder {
    dims: usize,
    calls: Arc<Mutex<Vec<Vec<String>>>>,
}

struct FailingBatchEmbedder {
    dims: usize,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl EmbeddingProvider for CountingEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.batches.fetch_add(1, Ordering::Relaxed);
        self.inner.embed(texts).await
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}

#[async_trait]
impl EmbeddingProvider for GatedEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.inner.embed(texts).await
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}

#[async_trait]
impl EmbeddingProvider for RecordingEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.calls.lock().unwrap().push(texts.to_vec());
        Ok(texts
            .iter()
            .enumerate()
            .map(|(index, _)| vec![(index + 1) as f32; self.dims])
            .collect())
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

#[async_trait]
impl EmbeddingProvider for FailingBatchEmbedder {
    async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Err(AppError::embeddings("injected batch embedding failure"))
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

#[test]
fn only_explicit_not_found_is_classified_as_deleted() {
    assert!(metadata_means_missing(Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "gone",
    )))
    .unwrap());
    let error = metadata_means_missing(Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "temporarily unreadable",
    )))
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn permission_denied_source_is_reported_without_deleting_indexed_state() {
    let root = tempfile::tempdir().unwrap();
    let config = Config {
        db_path: root.path().join("permission-denied.duckdb"),
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let source = "/vault/private.md";
    let document = crate::models::Document {
        id: "permission-doc".into(),
        uri: "file:///vault/private.md".into(),
        title: "Private".into(),
        content: "indexed content".into(),
        content_hash: Some(content_hash("indexed content")),
        source_file: Some(source.into()),
        layer: "raw".into(),
        kind: "document".into(),
        ..Default::default()
    };
    store.upsert_document(&document).unwrap();
    store
        .upsert_source_manifest(SourceManifestWrite {
            canonical_path: source,
            canonical_root: "/vault",
            size_bytes: 15,
            mtime_ns: 1,
            content_hash: document.content_hash.as_deref().unwrap(),
            document_id: &document.id,
        })
        .unwrap();
    let preloaded = store
        .load_source_manifest_root(Path::new("/vault"))
        .unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let service = SourceSyncService::new(&store, &embedder, &config);
    let mut report = SourceSyncReport::default();

    let cancelled = service
        .remove_deleted_with_probe(
            &BTreeSet::new(),
            &preloaded,
            &mut report,
            &SourceSyncControl::default(),
            0,
            |_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "temporarily unreadable",
                ))
            },
        )
        .unwrap();

    assert!(!cancelled);
    assert!(report.deleted.is_empty());
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].error.contains("preserving indexed state"));
    assert!(store.get_document(&document.id).unwrap().is_some());
    assert!(store
        .load_source_manifest_root(Path::new("/vault"))
        .unwrap()
        .contains_key(source));
}

#[tokio::test]
async fn store_clones_serialize_source_sync_runs() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("README.md"), "# Serialized").unwrap();
    let config = Arc::new(Config {
        db_path: db.path().join("serialized.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 16,
        ..Config::for_tests()
    });
    let store = Arc::new(Store::open(&config.db_path).unwrap());
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(GatedEmbedder {
        inner: MockEmbedder::new(16),
        calls: AtomicUsize::new(0),
        entered: entered.clone(),
        release: release.clone(),
    });
    let command = SourceSyncCommand::new(root.path().to_path_buf());

    let first = {
        let store = store.clone();
        let config = config.clone();
        let embedder = embedder.clone();
        let command = command.clone();
        tokio::spawn(async move {
            SourceSyncService::new(store.as_ref(), &embedder, config.as_ref())
                .sync(command)
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), entered.notified())
        .await
        .expect("first sync reached embedding");

    let second_scanning = Arc::new(tokio::sync::Notify::new());
    let (second_spawned_tx, second_spawned_rx) = tokio::sync::oneshot::channel();
    let second = {
        let store = store.clone();
        let config = config.clone();
        let embedder = embedder.clone();
        let scanning = second_scanning.clone();
        tokio::spawn(async move {
            let control = SourceSyncControl::new(CancellationToken::new(), move |progress| {
                if progress.phase == SourceSyncPhase::Scanning {
                    scanning.notify_one();
                }
            });
            let _ = second_spawned_tx.send(());
            SourceSyncService::new(store.as_ref(), &embedder, config.as_ref())
                .sync_with_control(command, control)
                .await
        })
    };
    second_spawned_rx.await.unwrap();
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(100),
        second_scanning.notified(),
    )
    .await
    .is_err());

    release.notify_one();
    first.await.unwrap().unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        second_scanning.notified(),
    )
    .await
    .expect("second sync entered after first released the lane");
    let outcome = second.await.unwrap().unwrap();
    assert!(matches!(outcome, SourceSyncOutcome::Completed(_)));
    assert_eq!(store.list_documents().unwrap().len(), 1);
}

#[tokio::test]
async fn small_documents_share_one_embedding_call_and_keep_path_mapping() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    for (name, body) in [
        ("c.md", "charlie body"),
        ("a.md", "alpha body"),
        ("b.md", "bravo body"),
    ] {
        std::fs::write(root.path().join(name), body).unwrap();
    }
    let config = Config {
        db_path: db.path().join("mapped-batch.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 4,
        chunk_size: 1_024,
        chunk_overlap: 0,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(RecordingEmbedder {
        dims: 4,
        calls: calls.clone(),
    });

    let report = SourceSyncService::new(&store, &embedder, &config)
        .sync(SourceSyncCommand::new(root.path().to_path_buf()))
        .await
        .unwrap();

    let canonical_paths =
        ["a.md", "b.md", "c.md"].map(|name| root.path().join(name).canonicalize().unwrap());
    assert_eq!(
        report.added,
        canonical_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
    );
    assert!(report.errors.is_empty());
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.as_slice(),
        &[vec![
            "alpha body".to_string(),
            "bravo body".to_string(),
            "charlie body".to_string(),
        ]]
    );
    for (index, path) in canonical_paths.iter().enumerate() {
        let document = store
            .find_by_uri(&format!("file://{}", path.display()))
            .unwrap()
            .unwrap();
        let chunks = store.list_chunks_for_document(&document.id).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].embedding[0], (index + 1) as f32);
    }
}

#[tokio::test]
async fn sixty_five_small_documents_split_at_the_batch_cap() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    for index in 0..65 {
        std::fs::write(
            root.path().join(format!("{index:03}.md")),
            format!("body {index}"),
        )
        .unwrap();
    }
    let config = Config {
        db_path: db.path().join("batch-cap.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 4,
        chunk_size: 1_024,
        chunk_overlap: 0,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(RecordingEmbedder {
        dims: 4,
        calls: calls.clone(),
    });

    let report = SourceSyncService::new(&store, &embedder, &config)
        .sync(SourceSyncCommand::new(root.path().to_path_buf()))
        .await
        .unwrap();

    assert!(report.errors.is_empty());
    assert_eq!(report.added.len(), 65);
    assert_eq!(store.list_documents().unwrap().len(), 65);
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(Vec::len)
            .collect::<Vec<_>>(),
        vec![64, 1]
    );
}

#[tokio::test]
async fn chunk_count_splits_a_batch_before_the_document_count_cap() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    for index in 0..33 {
        std::fs::write(root.path().join(format!("{index:03}.md")), "abcdefgh").unwrap();
    }
    let config = Config {
        db_path: db.path().join("chunk-batch-cap.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 4,
        chunk_size: 4,
        chunk_overlap: 0,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(RecordingEmbedder {
        dims: 4,
        calls: calls.clone(),
    });

    let report = SourceSyncService::new(&store, &embedder, &config)
        .sync(SourceSyncCommand::new(root.path().to_path_buf()))
        .await
        .unwrap();

    assert!(report.errors.is_empty());
    assert_eq!(report.added.len(), 33);
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(Vec::len)
            .collect::<Vec<_>>(),
        vec![64, 2]
    );
}

#[tokio::test]
async fn document_over_chunk_cap_is_embedded_alone_without_internal_splitting() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.md"), "aaaa").unwrap();
    std::fs::write(root.path().join("b.md"), "b".repeat(260)).unwrap();
    std::fs::write(root.path().join("c.md"), "cccc").unwrap();
    let config = Config {
        db_path: db.path().join("large-document-batch.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 4,
        chunk_size: 4,
        chunk_overlap: 0,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(RecordingEmbedder {
        dims: 4,
        calls: calls.clone(),
    });

    let report = SourceSyncService::new(&store, &embedder, &config)
        .sync(SourceSyncCommand::new(root.path().to_path_buf()))
        .await
        .unwrap();

    assert!(report.errors.is_empty());
    assert_eq!(report.added.len(), 3);
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(Vec::len)
            .collect::<Vec<_>>(),
        vec![1, 65, 1]
    );
}

#[tokio::test]
async fn embedding_failure_writes_none_of_a_multi_document_batch() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    for name in ["c.md", "a.md", "b.md"] {
        std::fs::write(root.path().join(name), format!("body {name}")).unwrap();
    }
    let config = Config {
        db_path: db.path().join("failed-batch.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 4,
        chunk_size: 1_024,
        chunk_overlap: 0,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(FailingBatchEmbedder {
        dims: 4,
        calls: calls.clone(),
    });

    let report = SourceSyncService::new(&store, &embedder, &config)
        .sync(SourceSyncCommand::new(root.path().to_path_buf()))
        .await
        .unwrap();

    let expected_paths = ["a.md", "b.md", "c.md"]
        .map(|name| {
            root.path()
                .join(name)
                .canonicalize()
                .unwrap()
                .display()
                .to_string()
        })
        .to_vec();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        report
            .errors
            .iter()
            .map(|error| error.path.clone())
            .collect::<Vec<_>>(),
        expected_paths
    );
    assert!(report
        .errors
        .iter()
        .all(|error| error.error.contains("injected batch embedding failure")));
    assert!(report.added.is_empty());
    assert_eq!(report.counters.embedded, 0);
    assert!(store.list_documents().unwrap().is_empty());
    assert!(store
        .load_source_manifest_root(root.path())
        .unwrap()
        .is_empty());
    assert!(store.list_graph_edges().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_during_embedding_writes_none_of_the_current_batch() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    for name in ["a.md", "b.md", "c.md"] {
        std::fs::write(root.path().join(name), format!("body {name}")).unwrap();
    }
    let config = Arc::new(Config {
        db_path: db.path().join("cancel-embedding.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 4,
        chunk_size: 1_024,
        chunk_overlap: 0,
        ..Config::for_tests()
    });
    let store = Arc::new(Store::open(&config.db_path).unwrap());
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(GatedEmbedder {
        inner: MockEmbedder::new(4),
        calls: AtomicUsize::new(0),
        entered: entered.clone(),
        release,
    });
    let cancellation = CancellationToken::new();
    let task = {
        let store = store.clone();
        let config = config.clone();
        let embedder = embedder.clone();
        let cancellation = cancellation.clone();
        let path = root.path().to_path_buf();
        tokio::spawn(async move {
            SourceSyncService::new(store.as_ref(), &embedder, config.as_ref())
                .sync_with_control(
                    SourceSyncCommand::new(path),
                    SourceSyncControl::new(cancellation, |_| {}),
                )
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), entered.notified())
        .await
        .expect("batch reached embedding");

    cancellation.cancel();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), task)
        .await
        .expect("cancelled sync returned")
        .unwrap()
        .unwrap();

    let SourceSyncOutcome::Cancelled(report) = outcome else {
        panic!("expected cancellation");
    };
    assert_eq!(report.counters.extracted, 3);
    assert!(report.added.is_empty());
    assert!(report.updated.is_empty());
    assert!(store.list_documents().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_between_commits_persists_exactly_the_sorted_prefix() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    for name in ["c.md", "a.md", "b.md"] {
        std::fs::write(root.path().join(name), format!("body {name}")).unwrap();
    }
    let config = Config {
        db_path: db.path().join("cancel-commits.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 4,
        chunk_size: 1_024,
        chunk_overlap: 0,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    store.ensure_fts(&config.fts_stemmer).unwrap();
    let generation_before = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    let calls = Arc::new(Mutex::new(Vec::new()));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(RecordingEmbedder {
        dims: 4,
        calls: calls.clone(),
    });
    let cancellation = CancellationToken::new();
    let cancellation_from_progress = cancellation.clone();
    let progress = Arc::new(Mutex::new(Vec::new()));
    let progress_from_callback = progress.clone();
    let control = SourceSyncControl::new(cancellation, move |snapshot| {
        if snapshot.phase == SourceSyncPhase::Syncing
            && snapshot.current_path.is_none()
            && snapshot.added == 1
        {
            cancellation_from_progress.cancel();
        }
        progress_from_callback.lock().unwrap().push(snapshot);
    });

    let outcome = SourceSyncService::new(&store, &embedder, &config)
        .sync_with_control(SourceSyncCommand::new(root.path().to_path_buf()), control)
        .await
        .unwrap();

    let SourceSyncOutcome::Cancelled(report) = outcome else {
        panic!("expected cancellation");
    };
    let canonical_first = root
        .path()
        .join("a.md")
        .canonicalize()
        .unwrap()
        .display()
        .to_string();
    assert_eq!(report.added, vec![canonical_first]);
    assert_eq!(report.counters.embedded, 1);
    assert_eq!(store.list_documents().unwrap().len(), 1);
    assert_eq!(store.list_documents().unwrap()[0].title, "a.md");
    assert_eq!(calls.lock().unwrap().len(), 1);
    let generation_after_cancel = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(generation_after_cancel.dirty);
    assert_eq!(
        generation_after_cancel.chunks_generation,
        generation_before.chunks_generation + 1
    );
    assert_eq!(
        generation_after_cancel.index_generation,
        generation_before.index_generation
    );
    assert_eq!(
        generation_after_cancel.rebuild_count,
        generation_before.rebuild_count
    );
    let progress = progress.lock().unwrap();
    let final_snapshot = progress.last().unwrap();
    assert_eq!(final_snapshot.phase, SourceSyncPhase::Cancelled);
    assert_eq!(final_snapshot.processed_files, 1);
    assert!(!progress
        .iter()
        .any(|snapshot| snapshot.phase == SourceSyncPhase::RefreshingFts));
    drop(progress);

    let hits = crate::db::search::search(
        &store,
        &crate::db::search::SearchQuery {
            mode: crate::models::SearchMode::Lex,
            query_text: Some("body".into()),
            top_k: 5,
            fts_stemmer: config.fts_stemmer.clone(),
            ..crate::db::search::SearchQuery::default()
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].document_title, "a.md");
    let generation_after_search = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(!generation_after_search.dirty);
    assert_eq!(
        generation_after_search.rebuild_count,
        generation_after_cancel.rebuild_count + 1
    );
}

#[tokio::test]
async fn source_commit_keeps_the_revision_captured_before_embedding() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let source = root.path().join("a.md");
    std::fs::write(&source, "initial source").unwrap();
    let config = Arc::new(Config {
        db_path: db.path().join("source-cas.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 4,
        chunk_size: 1_024,
        chunk_overlap: 0,
        ..Config::for_tests()
    });
    let store = Arc::new(Store::open(&config.db_path).unwrap());
    let initial_embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(4));
    SourceSyncService::new(store.as_ref(), &initial_embedder, config.as_ref())
        .sync(SourceSyncCommand::new(root.path().to_path_buf()))
        .await
        .unwrap();

    std::fs::write(&source, "source sync contender").unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let gated_embedder: Arc<dyn EmbeddingProvider> = Arc::new(GatedEmbedder {
        inner: MockEmbedder::new(4),
        calls: AtomicUsize::new(0),
        entered: entered.clone(),
        release: release.clone(),
    });
    let task = {
        let store = store.clone();
        let config = config.clone();
        let embedder = gated_embedder.clone();
        let path = root.path().to_path_buf();
        tokio::spawn(async move {
            SourceSyncService::new(store.as_ref(), &embedder, config.as_ref())
                .sync(SourceSyncCommand::new(path))
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), entered.notified())
        .await
        .expect("source update reached embedding");

    let canonical = source.canonicalize().unwrap();
    let existing = store
        .find_by_uri(&format!("file://{}", canonical.display()))
        .unwrap()
        .unwrap();
    IngestService::new(store.as_ref(), &initial_embedder, config.as_ref())
        .ingest(IngestCommand {
            text: "external winner".into(),
            title: Some("a.md".into()),
            uri: Some(existing.uri.clone()),
            metadata_json: Some(existing.metadata_json.clone()),
            wing: existing.wing.clone(),
            room: existing.room.clone(),
            source_file: existing.source_file.clone(),
            layer: existing.layer.clone(),
            kind: existing.kind.to_string(),
            immutable: false,
        })
        .await
        .unwrap();
    release.notify_one();

    let report = tokio::time::timeout(std::time::Duration::from_secs(30), task)
        .await
        .expect("source sync completed")
        .unwrap()
        .unwrap();
    let persisted = store.get_document(&existing.id).unwrap().unwrap();
    assert_eq!(persisted.content, "external winner");
    assert_eq!(persisted.revision, 2);
    assert!(report.added.is_empty());
    assert!(report.updated.is_empty());
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].error.contains("revision"));
}

#[tokio::test]
async fn sync_preflight_updates_one_changed_file_and_cleans_deleted_manifest() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let project = root.path().join("alpha");
    std::fs::create_dir(&project).unwrap();
    let source = project.join("README.md");
    std::fs::write(&source, "# Alpha\nbody").unwrap();
    let canonical_project = project.canonicalize().unwrap();
    let canonical_source = source.canonicalize().unwrap().display().to_string();
    let config = Config {
        db_path: db.path().join("test.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let embed_batches = Arc::new(AtomicUsize::new(0));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(CountingEmbedder {
        inner: MockEmbedder::new(16),
        batches: embed_batches.clone(),
    });
    let service = SourceSyncService::new(&store, &embedder, &config);
    let command = SourceSyncCommand {
        path: project.clone(),
        remove_deleted: false,
        wing: Some("alpha".into()),
        room: Some("root".into()),
        max_file_bytes: None,
    };

    let first = service.sync(command.clone()).await.unwrap();
    assert!(first.errors.is_empty(), "first={first:?}");
    assert_eq!(first.added, vec![canonical_source.clone()]);
    assert_eq!(first.counters.preflight, 1);
    assert_eq!(first.counters.extracted, 1);
    assert_eq!(first.counters.embedded, 1);
    let batches_after_first = embed_batches.load(Ordering::Relaxed);
    assert!(batches_after_first > 0);

    let second = service.sync(command.clone()).await.unwrap();
    assert!(second.errors.is_empty(), "second={second:?}");
    assert_eq!(second.skipped, vec![canonical_source.clone()]);
    assert_eq!(second.counters.preflight, 1);
    assert_eq!(second.counters.extracted, 0);
    assert_eq!(second.counters.embedded, 0);
    assert_eq!(embed_batches.load(Ordering::Relaxed), batches_after_first);

    let document = store.list_documents().unwrap().pop().unwrap();
    assert_eq!(document.wing.as_deref(), Some("alpha"));
    assert_eq!(document.room.as_deref(), Some("root"));
    let roots = store.list_source_roots(Some("alpha")).unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(
        roots[0].canonical_root,
        canonical_project.display().to_string()
    );
    assert_eq!(roots[0].file_count, 1);
    assert!(roots[0].last_seen.is_some());
    assert!(roots[0].last_synced.is_some());
    assert!(store.list_source_roots(Some("other")).unwrap().is_empty());

    let batches_before_repair = embed_batches.load(Ordering::Relaxed);
    store.delete_chunks_for_document(&document.id).unwrap();
    let repaired = service.sync(command.clone()).await.unwrap();
    assert!(repaired.errors.is_empty(), "repaired={repaired:?}");
    assert_eq!(repaired.updated, vec![canonical_source.clone()]);
    assert_eq!(repaired.counters.extracted, 1);
    assert_eq!(repaired.counters.embedded, 1);
    assert!(embed_batches.load(Ordering::Relaxed) > batches_before_repair);

    let warm_after_repair = service.sync(command.clone()).await.unwrap();
    assert_eq!(warm_after_repair.skipped, vec![canonical_source.clone()]);
    assert_eq!(warm_after_repair.counters.extracted, 0);
    assert_eq!(warm_after_repair.counters.embedded, 0);

    std::fs::write(&source, "# Alpha\nbody changed and longer").unwrap();
    let changed = service.sync(command.clone()).await.unwrap();
    assert!(changed.errors.is_empty(), "changed={changed:?}");
    assert_eq!(changed.updated, vec![canonical_source.clone()]);
    assert_eq!(changed.counters.preflight, 1);
    assert_eq!(changed.counters.extracted, 1);
    assert_eq!(changed.counters.embedded, 1);
    assert!(embed_batches.load(Ordering::Relaxed) > batches_after_first);

    std::fs::remove_file(&source).unwrap();
    let deleted = service
        .sync(SourceSyncCommand {
            remove_deleted: true,
            ..command
        })
        .await
        .unwrap();
    assert!(deleted.errors.is_empty(), "deleted={deleted:?}");
    assert_eq!(deleted.deleted, vec![canonical_source]);
    assert_eq!(deleted.counters.preflight, 0);
    assert_eq!(deleted.counters.extracted, 0);
    assert_eq!(deleted.counters.embedded, 0);
    assert!(store.list_documents().unwrap().is_empty());
    assert!(store
        .load_source_manifest_root(&canonical_project)
        .unwrap()
        .is_empty());
    assert!(store.list_source_roots(None).unwrap().is_empty());
}

#[tokio::test]
async fn completed_sync_prepares_fts_before_the_first_user_search() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let source = root.path().join("README.md");
    std::fs::write(&source, "alpha baseline").unwrap();
    let config = Config {
        db_path: db.path().join("sync-fts.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let service = SourceSyncService::new(&store, &embedder, &config);
    store.ensure_fts(&config.fts_stemmer).unwrap();
    let generation_state = || {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    let search = |query: &str| {
        crate::db::search::search(
            &store,
            &crate::db::search::SearchQuery {
                mode: crate::models::SearchMode::Lex,
                query_text: Some(query.to_string()),
                top_k: 5,
                timeout_ms: Some(5_000),
                fts_stemmer: config.fts_stemmer.clone(),
                ..crate::db::search::SearchQuery::default()
            },
        )
        .unwrap()
    };
    let before = generation_state();

    let phases = Arc::new(Mutex::new(Vec::new()));
    let phases_from_progress = phases.clone();
    let outcome = service
        .sync_with_control(
            SourceSyncCommand::new(root.path().to_path_buf()),
            SourceSyncControl::new(CancellationToken::new(), move |progress| {
                phases_from_progress.lock().unwrap().push(progress.phase);
            }),
        )
        .await
        .unwrap();
    let SourceSyncOutcome::Completed(first) = outcome else {
        panic!("expected completed sync");
    };
    assert_eq!(first.added.len(), 1);
    {
        let phases = phases.lock().unwrap();
        assert!(phases.ends_with(&[SourceSyncPhase::RefreshingFts, SourceSyncPhase::Completed,]));
    }
    let after_sync = generation_state();
    assert!(!after_sync.dirty);
    assert_eq!(after_sync.chunks_generation, before.chunks_generation + 1);
    assert_eq!(
        after_sync.index_generation,
        Some(after_sync.chunks_generation)
    );
    assert_eq!(after_sync.rebuild_count, before.rebuild_count + 1);

    let hits = search("alpha");
    assert_eq!(hits.len(), 1);
    let after_first_search = generation_state();
    assert_eq!(after_first_search, after_sync);

    let warm = service
        .sync(SourceSyncCommand::new(root.path().to_path_buf()))
        .await
        .unwrap();
    assert_eq!(warm.skipped.len(), 1);
    let after_warm_sync = generation_state();
    assert_eq!(after_warm_sync, after_sync);

    std::fs::write(&source, "beta replacement").unwrap();
    let changed = service
        .sync(SourceSyncCommand::new(root.path().to_path_buf()))
        .await
        .unwrap();
    assert_eq!(changed.updated.len(), 1);
    let after_changed_sync = generation_state();
    assert!(!after_changed_sync.dirty);
    assert_eq!(
        after_changed_sync.chunks_generation,
        after_sync.chunks_generation + 1
    );
    assert_eq!(
        after_changed_sync.rebuild_count,
        after_sync.rebuild_count + 1
    );
    let hits = search("beta");
    assert_eq!(hits.len(), 1);
    let after_changed_search = generation_state();
    assert_eq!(after_changed_search, after_changed_sync);

    std::fs::remove_file(source).unwrap();
    let deleted = service
        .sync(SourceSyncCommand {
            remove_deleted: true,
            ..SourceSyncCommand::new(root.path().to_path_buf())
        })
        .await
        .unwrap();
    assert_eq!(deleted.deleted.len(), 1);
    let after_delete_sync = generation_state();
    assert!(!after_delete_sync.dirty);
    assert_eq!(
        after_delete_sync.chunks_generation,
        after_changed_sync.chunks_generation + 1
    );
    assert_eq!(
        after_delete_sync.rebuild_count,
        after_changed_sync.rebuild_count + 1
    );
    assert!(search("beta").is_empty());
    assert_eq!(generation_state(), after_delete_sync);
}

#[tokio::test]
async fn terminal_fts_failure_preserves_committed_sync_report_and_dirty_fallback() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let source = root.path().join("README.md");
    std::fs::write(&source, "durable source-sync mutation").unwrap();
    let canonical_root = root.path().canonicalize().unwrap();
    let canonical_source = source.canonicalize().unwrap().display().to_string();
    let mut config = Config {
        db_path: db.path().join("sync-fts-finalization-failure.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    store.ensure_fts(&config.fts_stemmer).unwrap();
    config.fts_stemmer = crate::db::fts::test_fault::panic("source-sync");
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let progress = Arc::new(Mutex::new(Vec::new()));
    let progress_from_callback = progress.clone();

    let outcome = SourceSyncService::new(&store, &embedder, &config)
        .sync_with_control(
            SourceSyncCommand::new(root.path().to_path_buf()),
            SourceSyncControl::new(CancellationToken::new(), move |snapshot| {
                progress_from_callback.lock().unwrap().push(snapshot);
            }),
        )
        .await
        .expect("finalization failure must remain an aggregate outcome");
    let SourceSyncOutcome::Completed(report) = outcome else {
        panic!("expected completed-with-errors source sync");
    };

    assert_eq!(report.added, vec![canonical_source.clone()]);
    assert_eq!(report.counters.preflight, 1);
    assert_eq!(report.counters.extracted, 1);
    assert_eq!(report.counters.embedded, 1);
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].error.contains(FTS_FINALIZATION_ERROR_CODE));
    let finalization = report
        .finalization_error
        .as_ref()
        .expect("structured finalization error");
    assert_eq!(finalization.code, FTS_FINALIZATION_ERROR_CODE);
    assert_eq!(finalization.stage, "refresh_fts");
    assert!(finalization.durable_mutation_committed);
    assert!(finalization.retryable);
    assert!(finalization.fallback_dirty_marked);
    assert!(finalization.dirty_marker_error.is_none());
    assert!(finalization.ops_log_error.is_none());
    assert_eq!(store.list_documents().unwrap().len(), 1);
    assert!(store
        .load_source_manifest_root(&canonical_root)
        .unwrap()
        .contains_key(&canonical_source));
    let generation = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(generation.dirty, "next lexical read must retry FTS");
    assert!(store
        .list_recent_ops(10)
        .unwrap()
        .iter()
        .any(|entry| entry.op == "fts_finalization_failed"));
    let progress = progress.lock().unwrap();
    assert_eq!(progress.last().unwrap().phase, SourceSyncPhase::Completed);
    assert_eq!(progress.last().unwrap().errors, 1);
    let phases = progress
        .iter()
        .map(|snapshot| snapshot.phase)
        .collect::<Vec<_>>();
    assert!(phases.ends_with(&[SourceSyncPhase::RefreshingFts, SourceSyncPhase::Completed,]));
}

#[tokio::test]
async fn completed_sync_with_file_error_still_prepares_fts() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let source = root.path().join("README.md");
    std::fs::write(&source, "alpha baseline").unwrap();
    let config = Config {
        db_path: db.path().join("sync-fts-with-error.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let service = SourceSyncService::new(&store, &embedder, &config);
    let command = || SourceSyncCommand {
        max_file_bytes: Some(32),
        ..SourceSyncCommand::new(root.path().to_path_buf())
    };

    let initial = service.sync(command()).await.unwrap();
    assert_eq!(initial.added.len(), 1);
    assert!(initial.errors.is_empty());
    let generation_before = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };

    std::fs::write(&source, "needle replacement").unwrap();
    let oversized = root.path().join("oversized.md");
    std::fs::write(&oversized, "x".repeat(64)).unwrap();
    let outcome = service
        .sync_with_control(command(), SourceSyncControl::default())
        .await
        .unwrap();
    let SourceSyncOutcome::Completed(report) = outcome else {
        panic!("expected completed sync with a per-file error");
    };

    assert_eq!(
        report.updated,
        vec![source.canonicalize().unwrap().display().to_string()]
    );
    assert_eq!(report.errors.len(), 1);
    assert_eq!(
        report.errors[0].path,
        oversized.canonicalize().unwrap().display().to_string()
    );
    assert!(report.errors[0].error.contains("exceeds max_file_bytes"));
    let generation_after_sync = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(!generation_after_sync.dirty);
    assert_eq!(
        generation_after_sync.chunks_generation,
        generation_before.chunks_generation + 1
    );
    assert_eq!(
        generation_after_sync.index_generation,
        Some(generation_after_sync.chunks_generation)
    );
    assert_eq!(
        generation_after_sync.rebuild_count,
        generation_before.rebuild_count + 1
    );

    let hits = crate::db::search::search(
        &store,
        &crate::db::search::SearchQuery {
            mode: crate::models::SearchMode::Lex,
            query_text: Some("needle".into()),
            top_k: 5,
            timeout_ms: Some(5_000),
            fts_stemmer: config.fts_stemmer.clone(),
            ..crate::db::search::SearchQuery::default()
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].document_title, "README.md");
    let generation_after_search = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert_eq!(generation_after_search, generation_after_sync);
}

#[tokio::test]
async fn remove_deleted_prunes_existing_sources_newly_excluded_by_scan_policy() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let cached_directory = root.path().join("project/.cache");
    let environment = root.path().join("project/environment");
    std::fs::create_dir_all(&cached_directory).unwrap();
    std::fs::create_dir_all(environment.join("lib")).unwrap();
    let retained = root.path().join("project/README.md");
    let cached = cached_directory.join("dependency.py");
    let virtualized = environment.join("lib/dependency.py");
    std::fs::write(&retained, "# Retained").unwrap();
    std::fs::write(&cached, "cached = True").unwrap();
    std::fs::write(&virtualized, "installed = True").unwrap();

    let config = Config {
        db_path: db.path().join("policy-prune.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let service = SourceSyncService::new(&store, &embedder, &config);
    let canonical_root = root.path().canonicalize().unwrap();

    // Selecting a skipped directory as the root models content indexed by
    // an older scan policy while keeping the setup on the public sync path.
    service
        .sync(SourceSyncCommand::new(cached_directory))
        .await
        .unwrap();
    service
        .sync(SourceSyncCommand::new(root.path().to_path_buf()))
        .await
        .unwrap();
    assert_eq!(store.list_documents().unwrap().len(), 3);

    // A regular sentinel turns an already-indexed directory into an
    // explicit virtual-environment exclusion without deleting its files.
    std::fs::write(environment.join("pyvenv.cfg"), "home = /python").unwrap();
    let report = service
        .sync(SourceSyncCommand {
            remove_deleted: true,
            ..SourceSyncCommand::new(root.path().to_path_buf())
        })
        .await
        .unwrap();

    let retained = retained.canonicalize().unwrap().display().to_string();
    let mut excluded = vec![
        cached.canonicalize().unwrap().display().to_string(),
        virtualized.canonicalize().unwrap().display().to_string(),
    ];
    excluded.sort();
    assert!(report.errors.is_empty(), "report={report:?}");
    assert_eq!(report.deleted, excluded);
    assert_eq!(report.skipped, vec![retained.clone()]);
    assert!(cached.exists());
    assert!(virtualized.exists());

    let documents = store.list_documents().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].source_file.as_deref(), Some(retained.as_str()));
    let manifest = store.load_source_manifest_root(&canonical_root).unwrap();
    assert_eq!(manifest.len(), 1);
    assert!(manifest.contains_key(&retained));
}

#[tokio::test]
async fn controlled_sync_reports_progress_and_cancels_before_the_next_write() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.md"), "# A").unwrap();
    std::fs::write(root.path().join("b.md"), "# B").unwrap();
    let config = Config {
        db_path: db.path().join("test.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let cancellation = CancellationToken::new();
    let cancellation_from_progress = cancellation.clone();
    let phases = Arc::new(Mutex::new(Vec::new()));
    let phases_from_progress = phases.clone();
    let control = SourceSyncControl::new(cancellation, move |progress| {
        phases_from_progress.lock().unwrap().push(progress.phase);
        if progress.phase == SourceSyncPhase::Syncing && progress.current_path.is_some() {
            cancellation_from_progress.cancel();
        }
    });

    let outcome = SourceSyncService::new(&store, &embedder, &config)
        .sync_with_control(
            SourceSyncCommand {
                path: root.path().to_path_buf(),
                remove_deleted: false,
                wing: None,
                room: None,
                max_file_bytes: None,
            },
            control,
        )
        .await
        .unwrap();

    let SourceSyncOutcome::Cancelled(report) = outcome else {
        panic!("expected a cancelled source sync");
    };
    assert!(report.added.is_empty());
    assert!(store.list_documents().unwrap().is_empty());
    let phases = phases.lock().unwrap();
    assert!(phases.contains(&SourceSyncPhase::Scanning));
    assert!(phases.contains(&SourceSyncPhase::Syncing));
    assert_eq!(phases.last(), Some(&SourceSyncPhase::Cancelled));
}

#[tokio::test]
async fn warm_sync_reapplies_an_explicit_scope_change() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("README.md"), "# Scope").unwrap();
    let config = Config {
        db_path: db.path().join("scope.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let service = SourceSyncService::new(&store, &embedder, &config);
    let mut command = SourceSyncCommand {
        path: root.path().to_path_buf(),
        remove_deleted: false,
        wing: Some("alpha".into()),
        room: Some("root".into()),
        max_file_bytes: None,
    };
    service.sync(command.clone()).await.unwrap();

    command.wing = Some("  beta  ".into());
    let changed = service.sync(command).await.unwrap();

    assert_eq!(changed.updated.len(), 1);
    assert_eq!(changed.counters.extracted, 1);
    assert_eq!(changed.counters.embedded, 1);
    let document = store.list_documents().unwrap().pop().unwrap();
    assert_eq!(document.wing.as_deref(), Some("beta"));
    assert_eq!(document.room.as_deref(), Some("root"));
}

#[tokio::test]
async fn empty_source_is_a_stable_non_searchable_manifest_entry() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("empty.md"), "  \n").unwrap();
    let config = Config {
        db_path: db.path().join("empty.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let service = SourceSyncService::new(&store, &embedder, &config);
    let command = SourceSyncCommand {
        path: root.path().to_path_buf(),
        remove_deleted: false,
        wing: Some("empty".into()),
        room: None,
        max_file_bytes: None,
    };

    let first = service.sync(command.clone()).await.unwrap();
    assert_eq!(first.added.len(), 1);
    assert_eq!(store.integrity_counts().unwrap().0, 0);
    let second = service.sync(command).await.unwrap();
    assert_eq!(second.skipped.len(), 1);
    assert_eq!(second.counters.extracted, 0);
    assert_eq!(second.counters.embedded, 0);
}

#[tokio::test]
async fn manifest_moves_between_parent_and_child_roots_without_losing_cleanup() {
    let sources = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let project = sources.path().join("alpha");
    std::fs::create_dir(&project).unwrap();
    let source = project.join("README.md");
    std::fs::write(&source, "# Alpha").unwrap();
    let config = Config {
        db_path: db.path().join("roots.duckdb"),
        ingest_roots: vec![sources.path().to_path_buf()],
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let service = SourceSyncService::new(&store, &embedder, &config);
    service
        .sync(SourceSyncCommand {
            path: sources.path().to_path_buf(),
            remove_deleted: false,
            wing: Some("alpha".into()),
            room: None,
            max_file_bytes: None,
        })
        .await
        .unwrap();

    let moved = service
        .sync(SourceSyncCommand {
            path: project.clone(),
            remove_deleted: false,
            wing: Some("alpha".into()),
            room: None,
            max_file_bytes: None,
        })
        .await
        .unwrap();
    assert_eq!(moved.skipped.len(), 1);
    assert_eq!(moved.counters.extracted, 1);
    assert_eq!(moved.counters.embedded, 0);
    let canonical_project = project.canonicalize().unwrap();
    let manifest = store.load_source_manifest_root(&canonical_project).unwrap();
    assert_eq!(manifest.len(), 1);
    assert_eq!(
        manifest.values().next().unwrap().canonical_root,
        canonical_project.display().to_string()
    );

    let canonical_parent = sources.path().canonicalize().unwrap();
    let moved_to_parent = service
        .sync(SourceSyncCommand {
            path: sources.path().to_path_buf(),
            remove_deleted: false,
            wing: Some("alpha".into()),
            room: None,
            max_file_bytes: None,
        })
        .await
        .unwrap();
    assert_eq!(moved_to_parent.skipped.len(), 1);
    assert_eq!(moved_to_parent.counters.extracted, 1);
    assert_eq!(moved_to_parent.counters.embedded, 0);
    let manifest = store.load_source_manifest_root(&canonical_parent).unwrap();
    assert_eq!(manifest.len(), 1);
    assert_eq!(
        manifest.values().next().unwrap().canonical_root,
        canonical_parent.display().to_string()
    );

    let moved_back_to_child = service
        .sync(SourceSyncCommand {
            path: project.clone(),
            remove_deleted: false,
            wing: Some("alpha".into()),
            room: None,
            max_file_bytes: None,
        })
        .await
        .unwrap();
    assert_eq!(moved_back_to_child.skipped.len(), 1);
    assert_eq!(moved_back_to_child.counters.extracted, 1);
    assert_eq!(moved_back_to_child.counters.embedded, 0);

    std::fs::remove_file(source).unwrap();
    let removed = service
        .sync(SourceSyncCommand {
            path: project,
            remove_deleted: true,
            wing: Some("alpha".into()),
            room: None,
            max_file_bytes: None,
        })
        .await
        .unwrap();
    assert_eq!(removed.deleted.len(), 1);
    assert!(store.list_documents().unwrap().is_empty());
}

#[tokio::test]
async fn oversized_sources_are_reported_and_never_silently_stale() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let source = root.path().join("large.md");
    std::fs::write(&source, "indexed body").unwrap();
    let config = Config {
        db_path: db.path().join("oversize.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let service = SourceSyncService::new(&store, &embedder, &config);
    let mut command = SourceSyncCommand {
        path: root.path().to_path_buf(),
        remove_deleted: false,
        wing: Some("large".into()),
        room: None,
        max_file_bytes: Some(100),
    };
    service.sync(command.clone()).await.unwrap();
    let original = store.list_documents().unwrap().pop().unwrap().content;

    std::fs::write(&source, "now much larger than the configured cap").unwrap();
    command.max_file_bytes = Some(3);
    command.remove_deleted = true;
    let report = service.sync(command).await.unwrap();
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].error.contains("exceeds max_file_bytes"));
    assert!(report.updated.is_empty());
    assert!(report.deleted.is_empty());
    assert_eq!(
        store.list_documents().unwrap().pop().unwrap().content,
        original
    );
}

#[test]
fn source_stamp_rejects_a_file_that_exceeds_the_scan_guard() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("large.md");
    std::fs::write(&source, "too large").unwrap();
    let policy = SourceScanPolicy::default().with_max_bytes(3);

    let error = source_stamp(&source, &policy).unwrap_err();
    assert!(error.to_string().contains("exceeds max_file_bytes"));
}

#[tokio::test]
async fn unchanged_files_between_changed_ones_keep_one_batch() {
    let root = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    for (name, body) in [
        ("a.md", "alpha body"),
        ("b.md", "bravo body"),
        ("c.md", "charlie body"),
    ] {
        std::fs::write(root.path().join(name), body).unwrap();
    }
    let config = Config {
        db_path: db.path().join("interleaved-skip.duckdb"),
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 4,
        chunk_size: 1_024,
        chunk_overlap: 0,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(RecordingEmbedder {
        dims: 4,
        calls: calls.clone(),
    });
    let service = SourceSyncService::new(&store, &embedder, &config);
    service
        .sync(SourceSyncCommand::new(root.path().to_path_buf()))
        .await
        .unwrap();
    calls.lock().unwrap().clear();

    // b.md stays unchanged between the two edited files.
    std::fs::write(root.path().join("a.md"), "alpha body, edited").unwrap();
    std::fs::write(root.path().join("c.md"), "charlie body, edited").unwrap();
    let report = service
        .sync(SourceSyncCommand::new(root.path().to_path_buf()))
        .await
        .unwrap();

    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let b_path = root.path().join("b.md").canonicalize().unwrap();
    assert_eq!(report.skipped, vec![b_path.display().to_string()]);
    assert_eq!(report.updated.len(), 2);
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[vec![
            "alpha body, edited".to_string(),
            "charlie body, edited".to_string(),
        ]]
    );
}

fn nonblocking_fixture(root: &Path) -> (Arc<Store>, Arc<dyn EmbeddingProvider>, Config) {
    let config = Config {
        ingest_roots: vec![root.to_path_buf()],
        embedding_dims: 8,
        ..Config::for_tests()
    };
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    (Arc::new(crate::testkit::temp_store()), embedder, config)
}

#[test]
fn source_sync_command_new_has_conservative_defaults() {
    let command = SourceSyncCommand::new(PathBuf::from("/root"));
    assert_eq!(command.path, PathBuf::from("/root"));
    assert!(!command.remove_deleted);
    assert!(command.wing.is_none());
    assert!(command.room.is_none());
    assert!(command.max_file_bytes.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_sources_nonblocking_completes_and_second_run_skips_unchanged() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.md"), "# One\nfirst").unwrap();
    std::fs::write(root.path().join("two.txt"), "second").unwrap();
    let (store, embedder, config) = nonblocking_fixture(root.path());
    let progress = Arc::new(AtomicUsize::new(0));
    let seen = progress.clone();
    let control = SourceSyncControl::new(CancellationToken::new(), move |_| {
        seen.fetch_add(1, Ordering::Relaxed);
    });

    let outcome = sync_sources_nonblocking(
        store.clone(),
        embedder.clone(),
        config.clone(),
        SourceSyncCommand::new(root.path().to_path_buf()),
        control,
    )
    .await
    .unwrap();
    let SourceSyncOutcome::Completed(report) = outcome else {
        panic!("expected completion");
    };
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(report.added.len(), 2);
    assert!(progress.load(Ordering::Relaxed) > 0);
    assert_eq!(store.list_documents().unwrap().len(), 2);

    let again = sync_sources_nonblocking(
        store.clone(),
        embedder,
        config,
        SourceSyncCommand::new(root.path().to_path_buf()),
        SourceSyncControl::default(),
    )
    .await
    .unwrap();
    let SourceSyncOutcome::Completed(report) = again else {
        panic!("expected completion");
    };
    assert!(report.added.is_empty());
    assert_eq!(report.skipped.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_sources_nonblocking_honours_pre_cancelled_control() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.md"), "# One\nfirst").unwrap();
    let (store, embedder, config) = nonblocking_fixture(root.path());
    let token = CancellationToken::new();
    token.cancel();

    let outcome = sync_sources_nonblocking(
        store.clone(),
        embedder,
        config,
        SourceSyncCommand::new(root.path().to_path_buf()),
        SourceSyncControl::new(token, |_| {}),
    )
    .await
    .unwrap();
    assert!(matches!(outcome, SourceSyncOutcome::Cancelled(_)));
    assert!(store.list_documents().unwrap().is_empty());
}
