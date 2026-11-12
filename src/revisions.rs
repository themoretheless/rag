//! Revision timeline restoration with optimistic concurrency and derived-state rebuild.

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::db::Store;
use crate::db::store::DocumentDerivedWrite;
use crate::document_indexer::DocumentIndexer;
use crate::embeddings::EmbeddingProvider;
use crate::error::{AppError, Result};
use crate::models::{OpsLogEntry, WikiIndexEntry};

#[derive(Debug, Clone, Deserialize)]
pub struct RestoreRevisionCommand {
    pub document_id: String,
    pub revision: i64,
    /// Current revision observed by the caller. Required to avoid clobbering a
    /// write that happened while the revision timeline was open.
    pub if_match_revision: i64,
}

pub use rag_mcp_core::RestoreRevisionResult;

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

pub use rag_mcp_core::RevisionLineChange;

pub struct RevisionService<'a> {
    store: &'a Store,
    embedder: &'a Arc<dyn EmbeddingProvider>,
    config: &'a Config,
}

impl<'a> RevisionService<'a> {
    /// Bind the service to a store, embedder and config.
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
            return Err(AppError::invalid("document_id must be non-empty"));
        }
        if command.revision < 1 || command.if_match_revision < 1 {
            return Err(AppError::invalid(
                "revision and if_match_revision must be >= 1",
            ));
        }

        let current =
            crate::util::blocking_section(|| self.store.documents().get_document(document_id))?
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
        let mut restored = crate::util::blocking_section(|| {
            self.store
                .documents()
                .get_document_revision(document_id, command.revision)
        })?
        .ok_or_else(|| {
            AppError::not_found(format!(
                "revision {} for document '{document_id}'",
                command.revision
            ))
        })?;
        if is_source_controlled(&restored) {
            return Err(AppError::forbidden(format!(
                "revision {} for document '{document_id}' is a source-controlled snapshot and \
                cannot be restored; restore the source file and synchronize it instead",
                command.revision
            )));
        }
        restored.created_at = current.created_at;
        restored.updated_at = Utc::now();

        let _embedding_guard = self
            .store
            .corpus_lane()
            .try_embedding_write_guard(self.config, "revision restore")?;
        // Restoring usually brings back text the document had: reuse vectors.
        let chunk_embedder = crate::util::blocking_section(|| {
            crate::document_indexer::reusing_document_vectors(
                self.store,
                self.embedder,
                &restored.id,
            )
        })?;
        let chunks = DocumentIndexer::new(chunk_embedder.as_ref(), self.config)
            .build_chunks(&restored)
            .await?;
        let now = Utc::now();
        let audit_entry = OpsLogEntry::new(
            "restore_revision",
            "RESTORE",
            format!(
                "restored document {document_id} from revision {} as revision {}",
                command.revision,
                command.if_match_revision + 1
            ),
        )
        .at(now)
        .entity(Some(document_id.to_string()), restored.layer.as_str())
        .payload(
            serde_json::json!({
                "from_revision": command.revision,
                "previous_revision": command.if_match_revision,
                "new_revision": command.if_match_revision + 1,
            })
            .to_string(),
        );
        // Index lookup and the atomic commit run off the async workers.
        let write = crate::util::blocking_section(|| -> Result<_> {
            Ok(if crate::models::Layer::Wiki.is(&restored.layer) {
                let slug = restored
                    .uri
                    .strip_prefix("wiki://")
                    .filter(|slug| !slug.is_empty())
                    .ok_or_else(|| AppError::config("wiki revision has no canonical wiki URI"))?;
                let existing_index = self.store.wiki_index().get_wiki_index_by_slug(slug)?;
                let metadata = restored.metadata()?;
                let index = WikiIndexEntry {
                    id: existing_index
                        .map(|entry| entry.id)
                        .unwrap_or_else(|| format!("idx-{}", restored.id)),
                    slug: slug.into(),
                    title: restored.title.clone(),
                    kind: restored.kind.to_string(),
                    category: metadata
                        .get("category")
                        .and_then(|value| value.as_str())
                        .map(str::to_string),
                    summary: Some(
                        metadata
                            .get("summary")
                            .and_then(|value| value.as_str())
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                restored
                                    .content
                                    .lines()
                                    .map(str::trim)
                                    .find(|line| !line.is_empty())
                                    .unwrap_or("")
                                    .chars()
                                    .take(240)
                                    .collect()
                            }),
                    ),
                    page_id: Some(restored.id.clone()),
                    updated_at: now,
                };
                let mode = crate::db::sync::WikiSyncWrite::Local(
                    crate::db::sync::SyncIdentity::current()?,
                );
                self.store
                    .documents()
                    .write_wiki_document_atomic(crate::db::store::WikiDocumentWrite {
                        doc: &restored,
                        if_match_revision: Some(command.if_match_revision),
                        chunks: &chunks,
                        index_entry: &index,
                        audit_entry: &audit_entry,
                        sync: &mode,
                        create_only: false,
                    })?
                    .0
                    .ok_or_else(|| AppError::db("wiki restore returned no committed document"))?
            } else {
                let write = self.store.documents().write_document_atomic(
                    &restored,
                    Some(command.if_match_revision),
                    DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
                )?;
                self.store.ops_log().record(&audit_entry);
                write
            })
        })?;

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
            return Err(AppError::invalid(
                "document_id must be non-empty and revisions must be >= 1",
            ));
        }
        let current = self
            .store
            .documents()
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
            return Err(AppError::invalid(
                "document_id must be non-empty and revision must be >= 1",
            ));
        }
        let current = self
            .store
            .documents()
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
            .documents()
            .get_document_revision(document_id, revision)?
            .ok_or_else(|| {
                AppError::not_found(format!("revision {revision} for document '{document_id}'"))
            })
    }
}

fn is_source_controlled(document: &crate::models::Document) -> bool {
    crate::models::Layer::Raw.is(&document.layer)
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
mod tests;
