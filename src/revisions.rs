//! Revision timeline restoration with optimistic concurrency and derived-state rebuild.

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;
use crate::db::store::DocumentDerivedWrite;
use crate::db::Store;
use crate::document_indexer::DocumentIndexer;
use crate::embeddings::EmbeddingProvider;
use crate::error::{AppError, Result};
use crate::models::OpsLogEntry;

#[derive(Debug, Clone, Deserialize)]
pub struct RestoreRevisionCommand {
    pub document_id: String,
    pub revision: i64,
    /// Current revision observed by the caller. Required to avoid clobbering a
    /// write that happened while the revision timeline was open.
    pub if_match_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreRevisionResult {
    pub document_id: String,
    pub restored_from_revision: i64,
    pub revision: i64,
    pub etag: String,
    pub chunk_count: usize,
    pub node_id: String,
    pub edge_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionDiffResult {
    pub document_id: String,
    pub from_revision: i64,
    pub to_revision: i64,
    pub title_changed: bool,
    pub metadata_changed: bool,
    pub placement_changed: bool,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub changes: Vec<RevisionLineChange>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionLineChange {
    /// `added` or `removed`.
    pub kind: String,
    pub line: usize,
    pub content: String,
    pub content_truncated: bool,
}

pub struct RevisionService<'a> {
    store: &'a Store,
    embedder: &'a Arc<dyn EmbeddingProvider>,
    config: &'a Config,
}

impl<'a> RevisionService<'a> {
    pub fn new(
        store: &'a Store,
        embedder: &'a Arc<dyn EmbeddingProvider>,
        config: &'a Config,
    ) -> Self {
        Self {
            store,
            embedder,
            config,
        }
    }

    /// Restore a historical body/metadata snapshot as a new head revision.
    ///
    /// Chunking and embedding finish before the atomic document/chunk/graph
    /// transaction. The current revision is checked both before that work and
    /// again by the transaction's CAS boundary.
    pub async fn restore(&self, command: RestoreRevisionCommand) -> Result<RestoreRevisionResult> {
        let document_id = command.document_id.trim();
        if document_id.is_empty() {
            return Err(AppError::config("document_id must be non-empty"));
        }
        if command.revision < 1 || command.if_match_revision < 1 {
            return Err(AppError::config(
                "revision and if_match_revision must be >= 1",
            ));
        }

        let current = self
            .store
            .get_document(document_id)?
            .ok_or_else(|| AppError::not_found(format!("document '{document_id}'")))?;
        if is_source_controlled(&current) {
            return Err(AppError::forbidden(format!(
                "document '{document_id}' is source-controlled and cannot be restored; restore the source file and synchronize it instead"
            )));
        }
        if current.revision != command.if_match_revision {
            return Err(AppError::conflict(format!(
                "etag mismatch for document {document_id}: expected revision {}, current revision {}",
                command.if_match_revision, current.revision
            )));
        }
        let mut restored = self
            .store
            .get_document_revision(document_id, command.revision)?
            .ok_or_else(|| {
                AppError::not_found(format!(
                    "revision {} for document '{document_id}'",
                    command.revision
                ))
            })?;
        if is_source_controlled(&restored) {
            return Err(AppError::forbidden(format!(
                "revision {} for document '{document_id}' is a source-controlled snapshot and cannot be restored; restore the source file and synchronize it instead",
                command.revision
            )));
        }
        restored.created_at = current.created_at;
        restored.updated_at = Utc::now();

        self.store.ensure_embedding_manifest(self.config)?;
        self.store
            .require_embedding_dims_match(self.config.embedding_dims)?;
        let chunks = DocumentIndexer::new(self.embedder.as_ref(), self.config)
            .build_chunks(&restored)
            .await?;
        let write = self.store.write_document_atomic(
            &restored,
            Some(command.if_match_revision),
            DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
        )?;
        let now = Utc::now();
        if let Err(error) = self.store.append_ops_log(&OpsLogEntry {
            id: Uuid::new_v4().to_string(),
            seq: 0,
            ts: now,
            op: "restore_revision".into(),
            prefix: Some("RESTORE".into()),
            message: format!(
                "restored document {document_id} from revision {} as revision {}",
                command.revision, write.revision
            ),
            entity_id: Some(document_id.to_string()),
            entity_kind: Some(restored.layer.clone()),
            payload_json: serde_json::json!({
                "from_revision": command.revision,
                "previous_revision": command.if_match_revision,
                "new_revision": write.revision,
            })
            .to_string(),
            agent_name: None,
        }) {
            tracing::warn!(%error, document_id, revision = write.revision, "revision restored but audit log append failed");
        }

        Ok(RestoreRevisionResult {
            document_id: document_id.to_string(),
            restored_from_revision: command.revision,
            revision: write.revision,
            etag: crate::models::format_document_etag(write.revision),
            chunk_count: chunks.len(),
            node_id: write.node_id.unwrap_or_default(),
            edge_count: write.edge_count,
        })
    }

    /// Compare two stored revisions. `to_revision=None` means the current head.
    pub fn diff(
        &self,
        document_id: &str,
        from_revision: i64,
        to_revision: Option<i64>,
    ) -> Result<RevisionDiffResult> {
        let document_id = document_id.trim();
        if document_id.is_empty() || from_revision < 1 || to_revision.is_some_and(|rev| rev < 1) {
            return Err(AppError::config(
                "document_id must be non-empty and revisions must be >= 1",
            ));
        }
        let current = self
            .store
            .get_document(document_id)?
            .ok_or_else(|| AppError::not_found(format!("document '{document_id}'")))?;
        let from = self.snapshot(document_id, from_revision, &current)?;
        let requested_to = to_revision.unwrap_or(current.revision);
        let to = self.snapshot(document_id, requested_to, &current)?;
        let (changes, added_lines, removed_lines, truncated) =
            line_diff(&from.content, &to.content, 400);
        Ok(RevisionDiffResult {
            document_id: document_id.to_string(),
            from_revision,
            to_revision: requested_to,
            title_changed: from.title != to.title,
            metadata_changed: from.metadata_json != to.metadata_json,
            placement_changed: from.wing != to.wing
                || from.room != to.room
                || from.layer != to.layer
                || from.kind != to.kind
                || from.status != to.status,
            added_lines,
            removed_lines,
            changes,
            truncated,
        })
    }

    /// Load a revision only while its live document head still exists.
    pub fn snapshot_at(&self, document_id: &str, revision: i64) -> Result<crate::models::Document> {
        let document_id = document_id.trim();
        if document_id.is_empty() || revision < 1 {
            return Err(AppError::config(
                "document_id must be non-empty and revision must be >= 1",
            ));
        }
        let current = self
            .store
            .get_document(document_id)?
            .ok_or_else(|| AppError::not_found(format!("document '{document_id}'")))?;
        self.snapshot(document_id, revision, &current)
    }

    fn snapshot(
        &self,
        document_id: &str,
        revision: i64,
        current: &crate::models::Document,
    ) -> Result<crate::models::Document> {
        if revision == current.revision {
            return Ok(current.clone());
        }
        self.store
            .get_document_revision(document_id, revision)?
            .ok_or_else(|| {
                AppError::not_found(format!("revision {revision} for document '{document_id}'"))
            })
    }
}

fn is_source_controlled(document: &crate::models::Document) -> bool {
    document.layer.eq_ignore_ascii_case("raw")
        || document
            .source_file
            .as_deref()
            .is_some_and(|source| !source.trim().is_empty())
}

fn line_diff(
    before: &str,
    after: &str,
    max_changes: usize,
) -> (Vec<RevisionLineChange>, usize, usize, bool) {
    const LOOKAHEAD: usize = 16;
    let before = before.split('\n').collect::<Vec<_>>();
    let after = after.split('\n').collect::<Vec<_>>();
    let mut old_index = 0;
    let mut new_index = 0;
    let mut added = 0_usize;
    let mut removed = 0_usize;
    let mut changes = Vec::new();
    while old_index < before.len() || new_index < after.len() {
        if before.get(old_index) == after.get(new_index) {
            old_index += 1;
            new_index += 1;
            continue;
        }
        let next_match = (0..=LOOKAHEAD)
            .flat_map(|old_delta| (0..=LOOKAHEAD).map(move |new_delta| (old_delta, new_delta)))
            .filter(|(old_delta, new_delta)| *old_delta + *new_delta > 0)
            .filter(|(old_delta, new_delta)| {
                before.get(old_index + old_delta) == after.get(new_index + new_delta)
            })
            .min_by_key(|(old_delta, new_delta)| old_delta + new_delta);
        let (old_delta, new_delta) = next_match.unwrap_or((
            usize::from(old_index < before.len()),
            usize::from(new_index < after.len()),
        ));
        for offset in 0..old_delta {
            removed += 1;
            if changes.len() < max_changes {
                let (content, content_truncated) = bounded_line(before[old_index + offset]);
                changes.push(RevisionLineChange {
                    kind: "removed".into(),
                    line: old_index + offset + 1,
                    content,
                    content_truncated,
                });
            }
        }
        for offset in 0..new_delta {
            added += 1;
            if changes.len() < max_changes {
                let (content, content_truncated) = bounded_line(after[new_index + offset]);
                changes.push(RevisionLineChange {
                    kind: "added".into(),
                    line: new_index + offset + 1,
                    content,
                    content_truncated,
                });
            }
        }
        old_index += old_delta;
        new_index += new_delta;
    }
    let truncated = added.saturating_add(removed) > changes.len();
    (changes, added, removed, truncated)
}

fn bounded_line(line: &str) -> (String, bool) {
    const MAX_CHARS: usize = 4_096;
    if line.chars().count() <= MAX_CHARS {
        return (line.to_string(), false);
    }
    let mut content = line.chars().take(MAX_CHARS).collect::<String>();
    content.push('…');
    (content, true)
}

#[cfg(test)]
mod tests {
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
        assert_eq!(
            store.get_document("doc-1").unwrap().unwrap().content,
            "first body [[Original]]"
        );
        assert!(store
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
        assert_eq!(store.get_document("doc-1").unwrap().unwrap().revision, 3);

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
            .await
            .expect("committed restore must not fail when audit logging is unavailable");
        assert_eq!(restored_without_audit.revision, 4);
        assert_eq!(store.get_document("doc-1").unwrap().unwrap().revision, 4);

        assert!(store.delete_document("doc-1").unwrap());
        assert!(store.list_document_revisions("doc-1").unwrap().is_empty());
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
            .write_document_atomic(
                &document,
                None,
                DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
            )
            .unwrap();
        document.content = "second".into();
        let chunks = indexer.build_chunks(&document).await.unwrap();
        store
            .write_document_atomic(
                &document,
                Some(1),
                DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
            )
            .unwrap();

        let before_head = store.get_document(&document.id).unwrap().unwrap();
        let before_chunks = store
            .list_chunks_for_document(&document.id)
            .unwrap()
            .into_iter()
            .map(|chunk| (chunk.id, chunk.content, chunk.embedding))
            .collect::<Vec<_>>();
        let before_node = store
            .find_node_by_document_id(&document.id)
            .unwrap()
            .unwrap();
        let before_edges = store
            .list_graph_edges()
            .unwrap()
            .into_iter()
            .filter(|edge| edge.source_id == before_node.id || edge.target_id == before_node.id)
            .map(|edge| (edge.id, edge.source_id, edge.target_id, edge.rel_type))
            .collect::<Vec<_>>();
        let before_history = store
            .list_document_revisions(&document.id)
            .unwrap()
            .into_iter()
            .map(|revision| (revision.revision, revision.content))
            .collect::<Vec<_>>();
        let before_ops = store.list_recent_ops(100).unwrap().len();

        let error = RevisionService::new(&store, &embedder, &config)
            .restore(RestoreRevisionCommand {
                document_id: document.id.clone(),
                revision: 1,
                if_match_revision: 2,
            })
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::Forbidden(_)));
        let after_head = store.get_document(&document.id).unwrap().unwrap();
        assert_eq!(after_head.content, "second");
        assert_eq!(after_head.revision, before_head.revision);
        assert_eq!(
            store
                .list_chunks_for_document(&document.id)
                .unwrap()
                .into_iter()
                .map(|chunk| (chunk.id, chunk.content, chunk.embedding))
                .collect::<Vec<_>>(),
            before_chunks
        );
        let after_node = store
            .find_node_by_document_id(&document.id)
            .unwrap()
            .unwrap();
        assert_eq!(after_node.id, before_node.id);
        assert_eq!(after_node.label, before_node.label);
        assert_eq!(
            store
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
                .list_document_revisions(&document.id)
                .unwrap()
                .into_iter()
                .map(|revision| (revision.revision, revision.content))
                .collect::<Vec<_>>(),
            before_history
        );
        assert_eq!(store.list_recent_ops(100).unwrap().len(), before_ops);
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
            .write_document_atomic(
                &historical_raw,
                Some(1),
                DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
            )
            .unwrap();
        let before = store.get_document(&historical_raw.id).unwrap().unwrap();
        let error = RevisionService::new(&store, &embedder, &config)
            .restore(RestoreRevisionCommand {
                document_id: historical_raw.id.clone(),
                revision: 1,
                if_match_revision: 2,
            })
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::Forbidden(_)));
        let after = store.get_document(&historical_raw.id).unwrap().unwrap();
        assert_eq!(after.content, before.content);
        assert_eq!(after.layer, before.layer);
        assert_eq!(after.revision, before.revision);
        assert_eq!(
            store
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
                .get_document(&current_source.id)
                .unwrap()
                .unwrap()
                .content,
            "second"
        );
    }
}
