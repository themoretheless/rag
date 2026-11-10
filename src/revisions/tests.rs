
use super::*;
use crate::embeddings::MockEmbedder;
use crate::models::Document;

#[test]
fn bounded_line_diff_handles_insertions_and_truncates_payload() {
    let (changes, added, removed, truncated) = line_diff("a\nb\nc", "a\nx\nb\nc\nz", 1);
    assert_eq!((added, removed), (2, 0));
    assert_eq!(changes.len(), 1);
    assert!(truncated);
    assert_eq!(changes[0].kind, "added");
    assert_eq!(changes[0].content, "x");
}

#[tokio::test]
async fn restore_creates_new_head_and_rejects_stale_cas() {
    let root = tempfile::tempdir().unwrap();
    let config = Config {
        db_path: root.path().join("revisions.duckdb"),
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    store
        .embedding_manifest()
        .ensure_embedding_manifest(&config)
        .unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let indexer = DocumentIndexer::new(embedder.as_ref(), &config);
    let mut document = Document {
        id: "doc-1".into(),
        uri: "wiki://revision-test".into(),
        title: "Revision test".into(),
        content: "first body [[Original]]".into(),
        layer: "wiki".into(),
        ..Default::default()
    };
    let chunks = indexer.build_chunks(&document).await.unwrap();
    let first = store
        .documents()
        .write_document_atomic(
            &document,
            None,
            DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
        )
        .unwrap();
    assert_eq!(first.revision, 1);

    document.content = "second body [[Changed]]".into();
    document.updated_at = Utc::now();
    let chunks = indexer.build_chunks(&document).await.unwrap();
    let second = store
        .documents()
        .write_document_atomic(
            &document,
            Some(1),
            DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
        )
        .unwrap();
    assert_eq!(second.revision, 2);

    let diff = RevisionService::new(&store, &embedder, &config)
        .diff("doc-1", 1, None)
        .unwrap();
    assert_eq!((diff.from_revision, diff.to_revision), (1, 2));
    assert!(diff.added_lines > 0);
    assert!(diff.removed_lines > 0);
    assert!(diff
        .changes
        .iter()
        .any(|line| line.content.contains("first body")));
    assert!(diff
        .changes
        .iter()
        .any(|line| line.content.contains("second body")));

    let restored = RevisionService::new(&store, &embedder, &config)
        .restore(RestoreRevisionCommand {
            document_id: document.id.clone(),
            revision: 1,
            if_match_revision: 2,
        })
        .await
        .unwrap();
    assert_eq!(restored.revision, 3);
    let replicated = store.sync().pull_sync_changes("observer", 0, 10).unwrap();
    assert_eq!(replicated.len(), 3);
    let payload = crate::db::sync::validate_wiki_change(&replicated[2].change).unwrap();
    assert_eq!(payload.content, "first body [[Original]]");
    assert_eq!(
        store
            .wiki_index()
            .get_wiki_index_by_slug("revision-test")
            .unwrap()
            .unwrap()
            .summary
            .as_deref(),
        Some("first body [[Original]]")
    );
    assert_eq!(
        store
            .documents()
            .get_document("doc-1")
            .unwrap()
            .unwrap()
            .content,
        "first body [[Original]]"
    );
    assert!(store
        .ops_log()
        .list_recent_ops(10)
        .unwrap()
        .iter()
        .any(|entry| entry.op == "restore_revision"));

    let stale = RevisionService::new(&store, &embedder, &config)
        .restore(RestoreRevisionCommand {
            document_id: document.id.clone(),
            revision: 2,
            if_match_revision: 2,
        })
        .await;
    assert!(matches!(stale, Err(AppError::Conflict(_))));
    assert_eq!(
        store
            .documents()
            .get_document("doc-1")
            .unwrap()
            .unwrap()
            .revision,
        3
    );

    store
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE ops_log")
        .unwrap();
    let restored_without_audit = RevisionService::new(&store, &embedder, &config)
        .restore(RestoreRevisionCommand {
            document_id: document.id.clone(),
            revision: 2,
            if_match_revision: 3,
        })
        .await;
    assert!(
        restored_without_audit.is_err(),
        "wiki restore requires its audit/journal transaction to commit"
    );
    assert_eq!(
        store
            .documents()
            .get_document("doc-1")
            .unwrap()
            .unwrap()
            .revision,
        3
    );
    assert_eq!(
        store
            .documents()
            .get_document("doc-1")
            .unwrap()
            .unwrap()
            .content,
        "first body [[Original]]"
    );
    assert_eq!(
        store
            .sync()
            .pull_sync_changes("observer", 0, 10)
            .unwrap()
            .len(),
        3
    );

    assert!(store.documents().delete_document("doc-1").unwrap());
    assert!(store
        .documents()
        .list_document_revisions("doc-1")
        .unwrap()
        .is_empty());
    assert!(matches!(
        RevisionService::new(&store, &embedder, &config).snapshot_at("doc-1", 1),
        Err(AppError::NotFound(_))
    ));
}

#[tokio::test]
async fn raw_revision_restore_preserves_source_immutability() {
    let root = tempfile::tempdir().unwrap();
    let config = Config {
        db_path: root.path().join("raw-revisions.duckdb"),
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let indexer = DocumentIndexer::new(embedder.as_ref(), &config);
    let mut document = Document {
        id: "raw-1".into(),
        uri: "file:///source.md".into(),
        title: "Source".into(),
        content: "first".into(),
        source_file: Some("/source.md".into()),
        layer: "raw".into(),
        ..Default::default()
    };
    let chunks = indexer.build_chunks(&document).await.unwrap();
    store
        .documents()
        .write_document_atomic(
            &document,
            None,
            DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
        )
        .unwrap();
    document.content = "second".into();
    let chunks = indexer.build_chunks(&document).await.unwrap();
    store
        .documents()
        .write_document_atomic(
            &document,
            Some(1),
            DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
        )
        .unwrap();

    let before_head = store
        .documents()
        .get_document(&document.id)
        .unwrap()
        .unwrap();
    let before_chunks = store
        .chunks()
        .list_chunks_for_document(&document.id)
        .unwrap()
        .into_iter()
        .map(|chunk| (chunk.id, chunk.content, chunk.embedding))
        .collect::<Vec<_>>();
    let before_node = store
        .graph()
        .find_node_by_document_id(&document.id)
        .unwrap()
        .unwrap();
    let before_edges = store
        .graph()
        .list_graph_edges()
        .unwrap()
        .into_iter()
        .filter(|edge| edge.source_id == before_node.id || edge.target_id == before_node.id)
        .map(|edge| (edge.id, edge.source_id, edge.target_id, edge.rel_type))
        .collect::<Vec<_>>();
    let before_history = store
        .documents()
        .list_document_revisions(&document.id)
        .unwrap()
        .into_iter()
        .map(|revision| (revision.revision, revision.content))
        .collect::<Vec<_>>();
    let before_ops = store.ops_log().list_recent_ops(100).unwrap().len();

    let error = RevisionService::new(&store, &embedder, &config)
        .restore(RestoreRevisionCommand {
            document_id: document.id.clone(),
            revision: 1,
            if_match_revision: 2,
        })
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Forbidden(_)));
    let after_head = store
        .documents()
        .get_document(&document.id)
        .unwrap()
        .unwrap();
    assert_eq!(after_head.content, "second");
    assert_eq!(after_head.revision, before_head.revision);
    assert_eq!(
        store
            .chunks()
            .list_chunks_for_document(&document.id)
            .unwrap()
            .into_iter()
            .map(|chunk| (chunk.id, chunk.content, chunk.embedding))
            .collect::<Vec<_>>(),
        before_chunks
    );
    let after_node = store
        .graph()
        .find_node_by_document_id(&document.id)
        .unwrap()
        .unwrap();
    assert_eq!(after_node.id, before_node.id);
    assert_eq!(after_node.label, before_node.label);
    assert_eq!(
        store
            .graph()
            .list_graph_edges()
            .unwrap()
            .into_iter()
            .filter(|edge| edge.source_id == after_node.id || edge.target_id == after_node.id)
            .map(|edge| (edge.id, edge.source_id, edge.target_id, edge.rel_type))
            .collect::<Vec<_>>(),
        before_edges
    );
    assert_eq!(
        store
            .documents()
            .list_document_revisions(&document.id)
            .unwrap()
            .into_iter()
            .map(|revision| (revision.revision, revision.content))
            .collect::<Vec<_>>(),
        before_history
    );
    assert_eq!(
        store.ops_log().list_recent_ops(100).unwrap().len(),
        before_ops
    );
}

#[tokio::test]
async fn restore_rejects_current_or_historical_source_controlled_snapshots() {
    let root = tempfile::tempdir().unwrap();
    let config = Config {
        db_path: root.path().join("source-controlled-history.duckdb"),
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Store::open(&config.db_path).unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
    let indexer = DocumentIndexer::new(embedder.as_ref(), &config);

    // Simulate legacy state that predates the Store layer-transition guard:
    // revision 1 is raw/source-backed while the current head is mutable.
    let mut historical_raw = Document {
        id: "historical-source".into(),
        uri: "file:///historical-source.md".into(),
        title: "Historical source".into(),
        content: "verbatim source body".into(),
        source_file: Some("/vault/historical-source.md".into()),
        layer: "raw".into(),
        ..Default::default()
    };
    let chunks = indexer.build_chunks(&historical_raw).await.unwrap();
    store
        .documents()
        .write_document_atomic(
            &historical_raw,
            None,
            DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
        )
        .unwrap();
    historical_raw.layer = "wiki".into();
    historical_raw.kind = "wiki".into();
    historical_raw.source_file = None;
    historical_raw.content = "mutable current head".into();
    historical_raw.updated_at = Utc::now();
    let chunks = indexer.build_chunks(&historical_raw).await.unwrap();
    store
        .documents()
        .write_document_atomic(
            &historical_raw,
            Some(1),
            DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
        )
        .unwrap();
    let before = store
        .documents()
        .get_document(&historical_raw.id)
        .unwrap()
        .unwrap();
    let error = RevisionService::new(&store, &embedder, &config)
        .restore(RestoreRevisionCommand {
            document_id: historical_raw.id.clone(),
            revision: 1,
            if_match_revision: 2,
        })
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Forbidden(_)));
    let after = store
        .documents()
        .get_document(&historical_raw.id)
        .unwrap()
        .unwrap();
    assert_eq!(after.content, before.content);
    assert_eq!(after.layer, before.layer);
    assert_eq!(after.revision, before.revision);
    assert_eq!(
        store
            .documents()
            .list_document_revisions(&historical_raw.id)
            .unwrap()
            .len(),
        1
    );

    // A non-raw head with an explicit source-file owner is source-controlled
    // too and cannot bypass the guard merely by changing its layer label.
    let mut current_source = Document {
        id: "current-source".into(),
        uri: "wiki://current-source".into(),
        title: "Current source".into(),
        content: "first".into(),
        source_file: Some("/vault/current-source.md".into()),
        layer: "wiki".into(),
        kind: "wiki".into(),
        ..Default::default()
    };
    let chunks = indexer.build_chunks(&current_source).await.unwrap();
    store
        .documents()
        .write_document_atomic(
            &current_source,
            None,
            DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
        )
        .unwrap();
    current_source.content = "second".into();
    current_source.updated_at = Utc::now();
    let chunks = indexer.build_chunks(&current_source).await.unwrap();
    store
        .documents()
        .write_document_atomic(
            &current_source,
            Some(1),
            DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
        )
        .unwrap();
    let error = RevisionService::new(&store, &embedder, &config)
        .restore(RestoreRevisionCommand {
            document_id: current_source.id.clone(),
            revision: 1,
            if_match_revision: 2,
        })
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Forbidden(_)));
    assert_eq!(
        store
            .documents()
            .get_document(&current_source.id)
            .unwrap()
            .unwrap()
            .content,
        "second"
    );
}
