//! Document and chunk persistence over DuckDB.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use duckdb::{params, AccessMode, Config as DuckDbConfig, Connection};

use super::rows;
use super::schema;
use super::EmbeddingIdentity;
use crate::error::{AppError, Result};
use crate::models::{
    Chunk, DiaryEntry, Document, DocumentFilter, DocumentMetaApplyResult, DocumentMetaUpdate,
    DocumentRevisionPage, DocumentRevisionSummary, DuplicateCheckResult, DuplicateMatch,
    EmbeddingManifest, OpsLogEntry, PlacementUpdate, RoomCount, Taxonomy, TaxonomyRoom,
    TaxonomyWing, VacuumStoreReport, WikiIndexEntry, WingCount,
};
use crate::util::{
    content_hash, format_db_timestamp as format_ts, parse_db_timestamp, slugify as shared_slugify,
    wiki_slug_from_uri, SlugPolicy,
};

const EMBEDDING_MIGRATION_FINGERPRINT_PREFIX: &str = "migration-incomplete:";

#[cfg(test)]
pub(crate) const TEST_FAIL_WIKI_WRITE_AFTER_INDEX_OP: &str = "__test_fail_wiki_write_after_index__";

/// Shared SELECT list for document rows (order matches [`rows::document`]).
pub(super) const DOCUMENT_SELECT: &str = r#"
    id, uri, title, content, metadata_json,
    content_hash, wing, room, source_file, layer, kind,
    CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR),
    COALESCE(status, 'active'),
    COALESCE(pinned, false),
    COALESCE(boost, 1.0),
    COALESCE(revision, 1)
"#;

/// Optional filters for wiki catalog listing (`GET /v1/wiki`, [`Store::list_wiki_page_metas_filtered`]).
#[derive(Debug, Clone, Default)]
pub struct WikiPageMetaFilter {
    /// Case-insensitive substring on title / slug / uri / summary / category / kind.
    pub q: Option<String>,
    /// Max rows after filters (omit for all matches).
    pub limit: Option<usize>,
    /// Rows to skip after filters (default 0).
    pub offset: Option<usize>,
    /// Exact document `kind` (empty kind rows match filter `"wiki"`).
    pub kind: Option<String>,
    /// Case-insensitive exact match on metadata `category`.
    pub category: Option<String>,
    pub wing: Option<String>,
    pub room: Option<String>,
}

/// Thread-safe DuckDB-backed document/chunk store.
///
/// DuckDB connections are not `Sync`; access is serialized via [`Mutex`].
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
    corpus_mutation_lane: Arc<tokio::sync::RwLock<()>>,
    // Present only on an explicitly scoped clone used by a bulk workflow.
    // Clones keep the lease alive if nested work outlives its transport request.
    corpus_mutation_guard: Option<Arc<CorpusMutationGuard>>,
}

pub(crate) type CorpusMutationGuard = tokio::sync::OwnedRwLockWriteGuard<()>;

/// Keeps the embedding identity stable from preflight through document commit.
pub(crate) struct EmbeddingWriteGuard {
    _shared: Option<tokio::sync::OwnedRwLockReadGuard<()>>,
    _exclusive: Option<Arc<CorpusMutationGuard>>,
}

/// Persisted safety outcome for a failed derived-index finalization.
pub(crate) struct FtsFinalizationFailure {
    pub message: String,
    pub dirty_marker_written: bool,
    pub dirty_marker_error: Option<String>,
    pub ops_log_error: Option<String>,
}

/// Derived state that must stay consistent with a document write.
pub(crate) enum DocumentDerivedWrite<'a> {
    /// Keep chunks and graph unchanged (metadata-only update).
    Preserve,
    /// Keep chunks/edges, but refresh the existing document-node label.
    RefreshGraphLabel,
    /// Replace chunks and rebuild the document's outgoing graph slice.
    ReplaceChunksAndGraph(&'a [Chunk]),
}

/// Outcome of an atomic document + derived-state write.
#[derive(Debug)]
pub(crate) struct AtomicDocumentWriteResult {
    pub revision: i64,
    pub node_id: Option<String>,
    pub edge_count: usize,
}

/// Outcome of one exact-duplicate maintenance group.
///
/// `merged` contains every source that was archived or deleted. Raw sources
/// protected from a requested hard delete are also listed in `raw_protected`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExactDuplicateMergeResult {
    pub merged: Vec<String>,
    pub raw_protected: Vec<String>,
}

/// Aggregate layer/index health used by diagnostics without loading document bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LayerHealthCounts {
    pub raw_count: u64,
    pub wiki_count: u64,
    pub index_entry_count: u64,
    pub indexed_pages: u64,
    pub uncompiled_raw_count: u64,
}

/// Lean uncompiled-raw sample row for lint / bounded compile guidance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UncompiledRawSample {
    pub document_id: String,
    pub uri: String,
    pub wing: Option<String>,
    pub room: Option<String>,
}

/// One `(wing, room)` shelf of uncompiled raw docs, for compile-queue ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UncompiledRawShelf {
    pub wing: Option<String>,
    pub room: Option<String>,
    pub docs: u64,
}

/// The ranked compile debt: the requested top shelves plus how many shelves exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UncompiledShelfQueue {
    pub shelves: Vec<UncompiledRawShelf>,
    pub total_shelves: u64,
}

/// The compile link of a raw document: an inbound edge from a resolved `wiki://` node to
/// the graph node of that document.
///
/// This is the one definition of "compiled" in the data plane. The health metric, the
/// lint sample and the catalog `compiled` filter all interpolate it, so a compile batch
/// cannot be paged by a rule that the `uncompiled_raw_count` metric disagrees with.
///
/// Written as one correlated equality (`target.document_id = alias.id`) so DuckDB
/// decorrelates it into a hash semi-join. The previous form nested a `LIMIT 1`
/// scalar subquery inside the `EXISTS`, which ran per raw document: >190 s (then
/// spilled until the disk filled) vs 150 ms on 15k raw documents / 60k edges.
/// A document with several graph nodes counts as compiled when any of them has
/// the link, where the old form looked at one arbitrary node.
pub(crate) fn compiled_link_exists(alias: &str) -> String {
    format!(
        r#"EXISTS (
          SELECT 1
          FROM graph_nodes target
          JOIN graph_edges edge ON edge.target_id = target.id
          JOIN graph_nodes source ON source.id = edge.source_id
          WHERE target.document_id = {alias}.id
            AND source.id <> edge.target_id
            AND source.resolved
            AND starts_with(source.uri, 'wiki://')
        )"#
    )
}

/// Lean first-chunk vector sample for bounded maintenance comparisons.
///
/// Keeping this shape in the data plane prevents maintenance code from loading
/// complete document bodies and every persisted chunk merely to compare a
/// deterministic, capped set of representative vectors.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DocumentEmbeddingSample {
    pub document_id: String,
    pub title: String,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub embedding: Vec<f32>,
}

impl Store {
    /// Open (or create) a DuckDB database at `path` and run schema migrations.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let conn = Connection::open(path)?;
        schema::migrate(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: path.to_path_buf(),
            corpus_mutation_lane: Arc::new(tokio::sync::RwLock::new(())),
            corpus_mutation_guard: None,
        })
    }

    /// Open an existing, current-schema rag-mcp database without any writes.
    ///
    /// Unlike [`Self::open`], this never creates parent directories or a new
    /// database and never runs schema migrations. It is intended for direct
    /// inspectors; all durable mutations must still go through the one-writer
    /// gateway.
    pub fn open_read_only(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AppError::not_found(format!(
                    "read-only DuckDB file '{}' does not exist",
                    path.display()
                ))
            } else {
                AppError::Io(error)
            }
        })?;
        if !metadata.is_file() {
            return Err(AppError::config(format!(
                "read-only DuckDB path '{}' is not a file",
                path.display()
            )));
        }

        let config = DuckDbConfig::default()
            .access_mode(AccessMode::ReadOnly)
            .map_err(|error| {
                AppError::db(format!("cannot configure DuckDB read-only mode: {error}"))
            })?;
        let conn = Connection::open_with_flags(path, config).map_err(|error| {
            AppError::db(format!(
                "cannot open DuckDB '{}' read-only: {error}",
                path.display()
            ))
        })?;
        let version = schema::current_schema_version(&conn).map_err(|error| {
            AppError::config(format!(
                "DuckDB '{}' is not a compatible rag-mcp database: {error}",
                path.display()
            ))
        })?;
        if version != Some(schema::SCHEMA_VERSION) {
            return Err(AppError::config(format!(
                "DuckDB '{}' has incompatible schema version {}; expected {}. Upgrade a writable copy through the gateway before inspecting it",
                path.display(),
                version
                    .map(|version| version.to_string())
                    .unwrap_or_else(|| "missing".into()),
                schema::SCHEMA_VERSION
            )));
        }

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: path.to_path_buf(),
            corpus_mutation_lane: Arc::new(tokio::sync::RwLock::new(())),
            corpus_mutation_guard: None,
        })
    }

    /// Filesystem path of the opened database.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The raw DuckDB connection, for persistence code only.
    ///
    /// Production code outside `db` goes through Store methods and the `db::*`
    /// repositories; the wider visibility exists only so tests can seed and
    /// inspect fixtures directly.
    #[cfg(not(test))]
    pub(in crate::db) fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.lock_connection()
    }

    #[cfg(test)]
    pub(crate) fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.lock_connection()
    }

    /// Run `work` in one transaction under the Store lock. Commits on `Ok`;
    /// an `Err` (or panic) drops the transaction, which rolls it back.
    pub(in crate::db) fn with_tx<T>(
        &self,
        work: impl FnOnce(&duckdb::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let value = work(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    fn lock_connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| AppError::db(format!("database lock poisoned: {e}")))
    }

    /// Insert or replace a document row by primary key `id` (last-write-wins).
    ///
    /// Bumps `revision` based on the previous row for this `id` (or starts at 1).
    /// When `doc.content_hash` is empty/`None`, computes [`content_hash`] of
    /// `doc.content` and stores that fingerprint.
    pub fn upsert_document(&self, doc: &Document) -> Result<()> {
        self.upsert_document_cas(doc, None).map(|_| ())
    }

    /// CAS upsert: if `if_match_revision` is `Some`, the existing row (by `id`)
    /// must have that revision or [`AppError::Conflict`] is returned.
    ///
    /// Returns the **new** revision written (1 for insert).
    pub fn upsert_document_cas(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
    ) -> Result<i64> {
        self.with_tx(|tx| {
            let new_rev = upsert_document_cas_locked(tx, doc, if_match_revision)?;
            Ok(new_rev)
        })
    }

    /// Persist a document and its derived state under one DuckDB transaction.
    ///
    /// Callers must finish chunking and embedding before entering this method.
    /// Embedding JSON is also prepared before the database lock/transaction so
    /// serialization failures cannot leave a partially-updated document.
    pub(crate) fn write_document_atomic(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
        derived: DocumentDerivedWrite<'_>,
    ) -> Result<AtomicDocumentWriteResult> {
        self.write_document_atomic_with_manifest(doc, if_match_revision, derived, None, None)
    }

    /// Persist an unchunked document (e.g. `schema://agents`), rebuild its
    /// outgoing graph slice and append its audit row in one transaction.
    ///
    /// Either all three land or none do: a graph or ops-log failure leaves the
    /// previous document, graph and log untouched.
    pub(crate) fn write_unchunked_document_with_audit(
        &self,
        doc: &Document,
        audit: &OpsLogEntry,
    ) -> Result<AtomicDocumentWriteResult> {
        self.with_tx(|tx| {
            let revision = upsert_document_cas_locked(tx, doc, None)?;
            let (node_id, edge_count) =
                crate::graph::resolve::rebuild_document_graph_locked(tx, doc)?;
            append_ops_log_locked(tx, audit)?;
            Ok(AtomicDocumentWriteResult {
                revision,
                node_id: Some(node_id),
                edge_count,
            })
        })
    }

    /// Persist a synchronized source document and its manifest ownership in the
    /// same transaction as chunks and graph state.
    pub(crate) fn write_source_document_atomic(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
        chunks: &[Chunk],
        manifest: crate::db::SourceManifestWrite<'_>,
    ) -> Result<AtomicDocumentWriteResult> {
        if manifest.document_id != doc.id {
            return Err(AppError::config(format!(
                "source manifest document {} does not match {}",
                manifest.document_id, doc.id
            )));
        }
        let document_hash = doc
            .content_hash
            .clone()
            .unwrap_or_else(|| content_hash(&doc.content));
        if manifest.content_hash != document_hash {
            return Err(AppError::config(
                "source manifest content hash does not match document",
            ));
        }
        self.write_document_atomic_with_manifest(
            doc,
            if_match_revision,
            DocumentDerivedWrite::ReplaceChunksAndGraph(chunks),
            Some(manifest),
            None,
        )
    }

    fn write_document_atomic_with_manifest(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
        derived: DocumentDerivedWrite<'_>,
        manifest: Option<crate::db::SourceManifestWrite<'_>>,
        sync_identity: Option<&super::sync::SyncIdentity>,
    ) -> Result<AtomicDocumentWriteResult> {
        let prepared_embeddings = match &derived {
            DocumentDerivedWrite::ReplaceChunksAndGraph(chunks) => {
                Some(owned_embedding_json(&doc.id, chunks)?)
            }
            DocumentDerivedWrite::Preserve | DocumentDerivedWrite::RefreshGraphLabel => None,
        };

        self.with_tx(|tx| {
            let previous = if canonical_wiki_slug(&doc.uri).is_some() {
                get_document_locked(tx, &doc.id)?
            } else {
                None
            };
            let result = write_document_locked(
                tx,
                doc,
                if_match_revision,
                derived,
                prepared_embeddings.as_deref(),
                manifest,
            )?;
            journal_generic_wiki_change_locked(tx, previous.as_ref(), doc, sync_identity)?;
            Ok(result)
        })
    }

    /// Persist a wiki document and its chunks, graph, catalog row, and audit
    /// event, replication journal and applied cursor under one transaction.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_wiki_document_atomic(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
        chunks: &[Chunk],
        index_entry: &WikiIndexEntry,
        audit_entry: &OpsLogEntry,
        sync: &super::sync::WikiSyncWrite<'_>,
        create_only: bool,
    ) -> Result<(Option<AtomicDocumentWriteResult>, i64)> {
        if index_entry.page_id.as_deref() != Some(doc.id.as_str()) {
            return Err(AppError::config(format!(
                "wiki index page id {:?} does not match document {}",
                index_entry.page_id, doc.id
            )));
        }
        if let Some(entity_id) = audit_entry.entity_id.as_deref() {
            if entity_id != doc.id {
                return Err(AppError::config(format!(
                    "wiki audit entity id {entity_id} does not match document {}",
                    doc.id
                )));
            }
        }
        let prepared_embeddings = owned_embedding_json(&doc.id, chunks)?;

        let local_event = super::sync::SyncChangeInput {
            origin_seq: 1,
            entity_kind: "wiki".into(),
            entity_id: doc.uri.clone(),
            operation: "upsert".into(),
            payload_json: serde_json::to_string(&super::sync::WikiSyncPayload {
                slug: index_entry.slug.clone(),
                title: doc.title.clone(),
                content: doc.content.clone(),
                wing: doc.wing.clone(),
                room: doc.room.clone(),
                kind: Some(doc.kind.to_string()),
                category: index_entry.category.clone(),
                summary: index_entry.summary.clone(),
                metadata_json: Some(doc.metadata_json.clone()),
            })?,
            content_hash: doc.content_hash.clone(),
            created_at: doc
                .updated_at
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        };
        super::sync::validate_wiki_change(&local_event)?;
        let incoming = match sync {
            super::sync::WikiSyncWrite::Local(_) => None,
            super::sync::WikiSyncWrite::Push { change, .. } => Some(*change),
            super::sync::WikiSyncWrite::Pull { event, .. } => Some(&event.change),
        };
        if incoming.is_some_and(|event| event.entity_id != doc.uri) {
            return Err(AppError::config(
                "sync event does not belong to prepared wiki document",
            ));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        if create_only {
            let existing: i64 = tx.query_row(
                "SELECT COUNT(*) FROM documents WHERE uri = ? OR id = ?",
                params![&doc.uri, &doc.id],
                |row| row.get(0),
            )?;
            if existing != 0 {
                return Err(AppError::conflict("wiki page already exists"));
            }
        }
        if let Some(sequence) = super::sync::sync_duplicate_locked(&tx, sync)? {
            return Ok((None, sequence));
        }
        if super::sync::defer_pulled_document_locked(&tx, sync, &doc.uri)? {
            let sequence = super::sync::commit_sync_write_locked(&tx, sync, &local_event)?;
            tx.commit()?;
            return Ok((None, sequence));
        }
        super::review::accept_locked(&tx, doc, audit_entry)?;
        let result = write_document_locked(
            &tx,
            doc,
            if_match_revision,
            DocumentDerivedWrite::ReplaceChunksAndGraph(chunks),
            Some(&prepared_embeddings),
            None,
        )?;
        upsert_wiki_index_entry_locked(&tx, index_entry)?;
        #[cfg(test)]
        if audit_entry.op == TEST_FAIL_WIKI_WRITE_AFTER_INDEX_OP {
            return Err(AppError::db(
                "injected wiki write failure after catalog update",
            ));
        }
        append_ops_log_locked(&tx, audit_entry)?;
        let sequence = super::sync::commit_sync_write_locked(&tx, sync, &local_event)?;
        #[cfg(test)]
        if audit_entry.op == "__test_fail_wiki_after_sync_journal__" {
            return Err(AppError::db("injected wiki failure after sync journal"));
        }
        tx.commit()?;
        Ok((Some(result), sequence))
    }

    /// Insert chunk rows. Embeddings are stored as JSON float arrays in `embedding_json`.
    pub fn insert_chunks(&self, chunks: &[Chunk]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }

        let embedding_json = embedding_json(chunks)?;
        let conn = self.lock()?;
        super::fts::mark_fts_dirty(&conn)?;
        insert_chunks_locked(&conn, chunks, &embedding_json)
    }

    /// Delete a document, its chunks, and its graph node + incident edges.
    ///
    /// Returns `true` if a document row was removed.
    pub fn delete_document(&self, id: &str) -> Result<bool> {
        self.with_tx(|tx| {
            let deleted = delete_document_locked(tx, id)?;
            Ok(deleted)
        })
    }

    /// Apply a maintenance batch of hard deletes and tombstones atomically.
    ///
    /// Every target must exist and an id may occur only once across the two
    /// lists. Any validation or storage failure rolls back the entire group, so
    /// a compression caller never loses a partial prefix without a report.
    pub(crate) fn apply_document_dispositions_atomic(
        &self,
        delete_ids: &[String],
        tombstone_ids: &[String],
    ) -> Result<(u64, u64, u64)> {
        let mut seen = std::collections::HashSet::new();
        for id in delete_ids.iter().chain(tombstone_ids) {
            if id.trim().is_empty() || !seen.insert(id.as_str()) {
                return Err(AppError::config(format!(
                    "duplicate or empty document disposition id '{id}'"
                )));
            }
        }

        self.with_tx(|tx| {
            let mut deleted = 0u64;
            let mut tombstoned = 0u64;
            let mut chunks_deleted = 0u64;

            for id in tombstone_ids {
                let Some(document) = get_document_locked(tx, id)? else {
                    return Err(AppError::not_found(format!(
                        "document not found for tombstone: {id}"
                    )));
                };
                let expected_revision = document.revision;
                let mut applied = apply_document_meta_update(
                    document,
                    &DocumentMetaUpdate {
                        status: Some("tombstone".into()),
                        ..Default::default()
                    },
                )?;
                applied.document.revision =
                    upsert_document_cas_locked(tx, &applied.document, Some(expected_revision))?;
                tombstoned += 1;
            }

            for id in delete_ids {
                let chunk_count: i64 = tx.query_row(
                    "SELECT COUNT(*)::BIGINT FROM chunks WHERE document_id = ?",
                    params![id],
                    |row| row.get(0),
                )?;
                if !delete_document_locked(tx, id)? {
                    return Err(AppError::not_found(format!(
                        "document not found for delete: {id}"
                    )));
                }
                deleted += 1;
                chunks_deleted = chunks_deleted.saturating_add(chunk_count.max(0) as u64);
            }
            Ok((deleted, tombstoned, chunks_deleted))
        })
    }

    /// Validate and apply one exact-content duplicate group atomically.
    ///
    /// Actual document bodies are hashed inside the transaction; persisted
    /// `content_hash` values and caller-provided ids are never trusted as proof
    /// of equality. All sources are validated before the first mutation and any
    /// later storage failure rolls the whole group back.
    pub(crate) fn merge_exact_duplicates_atomic(
        &self,
        keep_id: &str,
        source_ids: &[String],
        declared_hash: Option<&str>,
        hard_delete: bool,
        allow_raw_delete: bool,
    ) -> Result<ExactDuplicateMergeResult> {
        self.merge_exact_duplicates_atomic_with(
            keep_id,
            source_ids,
            declared_hash,
            hard_delete,
            allow_raw_delete,
            |_| Ok(()),
        )
    }

    fn merge_exact_duplicates_atomic_with(
        &self,
        keep_id: &str,
        source_ids: &[String],
        declared_hash: Option<&str>,
        hard_delete: bool,
        allow_raw_delete: bool,
        mut after_mutation: impl FnMut(usize) -> Result<()>,
    ) -> Result<ExactDuplicateMergeResult> {
        let keep_id = keep_id.trim();
        if keep_id.is_empty() {
            return Err(AppError::config(
                "merge_exact_dup requires a non-empty canonical document id",
            ));
        }
        if source_ids.is_empty() {
            return Err(AppError::config(
                "merge_exact_dup requires at least one source document id",
            ));
        }
        let mut seen = std::collections::HashSet::with_capacity(source_ids.len());
        for source_id in source_ids {
            let normalized = source_id.trim();
            if normalized.is_empty()
                || normalized != source_id
                || normalized == keep_id
                || !seen.insert(normalized)
            {
                return Err(AppError::config(format!(
                    "merge_exact_dup source ids must be non-empty, unique, and different from canonical id '{keep_id}'"
                )));
            }
        }

        self.with_tx(|tx| {
            let keep = get_document_locked(tx, keep_id)?
                .ok_or_else(|| AppError::not_found(format!("document not found: {keep_id}")))?;
            let expected_hash = content_hash(&keep.content);
            if declared_hash.is_some_and(|declared| declared != expected_hash) {
                return Err(AppError::conflict(format!(
                    "merge_exact_dup declared content_hash does not match canonical document {keep_id}"
                )));
            }

            let mut sources = Vec::with_capacity(source_ids.len());
            for source_id in source_ids {
                let source = get_document_locked(tx, source_id)?
                    .ok_or_else(|| AppError::not_found(format!("document not found: {source_id}")))?;
                if content_hash(&source.content) != expected_hash {
                    return Err(AppError::conflict(format!(
                        "merge_exact_dup source {source_id} is not an exact content duplicate of {keep_id}"
                    )));
                }
                sources.push(source);
            }

            let mut merged = Vec::with_capacity(sources.len());
            let mut raw_protected = Vec::new();
            for (index, source) in sources.into_iter().enumerate() {
                if hard_delete && (!crate::models::Layer::Raw.is(&source.layer) || allow_raw_delete) {
                    if !delete_document_locked(tx, &source.id)? {
                        return Err(AppError::not_found(format!(
                            "document disappeared during merge_exact_dup: {}",
                            source.id
                        )));
                    }
                } else {
                    let expected_revision = source.revision;
                    let mut applied = apply_document_meta_update(
                        source,
                        &DocumentMetaUpdate {
                            status: Some("archived".into()),
                            ..Default::default()
                        },
                    )?;
                    let source_id = applied.document.id.clone();
                    applied.document.revision =
                        upsert_document_cas_locked(tx, &applied.document, Some(expected_revision))?;
                    if hard_delete {
                        raw_protected.push(source_id);
                    }
                }
                merged.push(source_ids[index].clone());
                after_mutation(index + 1)?;
            }
            Ok(ExactDuplicateMergeResult {
                merged,
                raw_protected,
            })
        })
    }

    /// Count chunks without loading their text or vectors.
    pub(crate) fn count_chunks_for_document(&self, document_id: &str) -> Result<u64> {
        let conn = self.lock()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*)::BIGINT FROM chunks WHERE document_id = ?",
            params![document_id],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as u64)
    }

    /// Delete only chunks for a document (used on uri re-ingest to keep graph node stable).
    pub fn delete_chunks_for_document(&self, document_id: &str) -> Result<()> {
        let conn = self.lock()?;
        super::fts::mark_fts_dirty(&conn)?;
        conn.execute(
            "DELETE FROM chunks WHERE document_id = ?",
            params![document_id],
        )?;
        Ok(())
    }

    /// Delete all documents whose `source_file` equals `source`, plus their chunks.
    ///
    /// Returns the number of document rows removed.
    pub fn delete_by_source(&self, source: &str) -> Result<u64> {
        self.delete_by_source_ids(source)
            .map(|document_ids| document_ids.len() as u64)
    }

    /// Delete by exact source and return the ids selected inside the same
    /// transaction that performs the deletion.
    pub fn delete_by_source_ids(&self, source: &str) -> Result<Vec<String>> {
        self.delete_source_state_ids_with(source, |_| Ok(()))
            .map(|(document_ids, _)| document_ids)
    }

    /// Atomically remove every document/derived row and manifest ownership for
    /// one filesystem source. Returns `(documents_deleted, manifest_deleted)`.
    pub(crate) fn delete_source_state(&self, source: &str) -> Result<(u64, bool)> {
        self.delete_source_state_with(source, |_| Ok(()))
    }

    fn delete_source_state_with(
        &self,
        source: &str,
        after_document: impl FnMut(usize) -> Result<()>,
    ) -> Result<(u64, bool)> {
        self.delete_source_state_ids_with(source, after_document)
            .map(|(document_ids, manifest_deleted)| (document_ids.len() as u64, manifest_deleted))
    }

    fn delete_source_state_ids_with(
        &self,
        source: &str,
        mut after_document: impl FnMut(usize) -> Result<()>,
    ) -> Result<(Vec<String>, bool)> {
        self.with_tx(|tx| {
            let manifest_present: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM source_manifest WHERE canonical_path = ?)",
                params![source],
                |row| row.get(0),
            )?;
            let mut ids = Vec::new();
            {
                let mut stmt = tx.prepare("SELECT id FROM documents WHERE source_file = ?")?;
                crate::db::rows::for_each_row(&mut stmt, params![source], |row| {
                    ids.push(row.get::<_, String>(0)?);
                    Ok(())
                })?;
            }
            for (index, id) in ids.iter().enumerate() {
                delete_document_locked(tx, id)?;
                after_document(index + 1)?;
            }
            tx.execute(
                "DELETE FROM source_manifest WHERE canonical_path = ?",
                params![source],
            )?;
            Ok((ids, manifest_present))
        })
    }

}

fn canonical_wiki_slug(uri: &str) -> Option<&str> {
    let slug = uri.strip_prefix("wiki://")?;
    (!slug.is_empty() && crate::util::slugify(slug, crate::util::SlugPolicy::WikiPage) == slug)
        .then_some(slug)
}

/// Generic writes must publish the same wiki snapshot as the dedicated writer.
/// Lifecycle and other local-only columns deliberately do not trigger an event.
/// The dedicated wiki writer calls `write_document_locked` directly and appends
/// its own typed local/incoming event, so it never passes through this helper.
fn journal_generic_wiki_change_locked(
    conn: &Connection,
    previous: Option<&Document>,
    doc: &Document,
    identity: Option<&super::sync::SyncIdentity>,
) -> Result<()> {
    if !crate::models::Layer::Wiki.is(&doc.layer) {
        return Ok(());
    }
    let Some(slug) = canonical_wiki_slug(&doc.uri) else {
        return Ok(());
    };
    if previous.is_some_and(|old| {
        old.uri == doc.uri
            && old.title == doc.title
            && old.content == doc.content
            && old.wing == doc.wing
            && old.room == doc.room
            && old.kind == doc.kind
            && old.metadata_json == doc.metadata_json
    }) {
        return Ok(());
    }

    let existing_index = {
        let mut statement = conn.prepare(
            "SELECT id, COALESCE(slug, label, id), COALESCE(title, label, id), \
             COALESCE(kind, 'wiki'), category, summary, COALESCE(page_id, document_id), \
             CAST(updated_at AS VARCHAR) FROM wiki_index WHERE slug = ? LIMIT 1",
        )?;
        let mut rows = statement.query(params![slug])?;
        rows.next()?.map(row_to_wiki_index).transpose()?
    };
    let (summary, category) = meta_summary_category(&doc.metadata_json);
    let (old_summary, old_category) = previous
        .map(|old| meta_summary_category(&old.metadata_json))
        .unwrap_or_default();
    let category = if category != old_category {
        category
    } else {
        existing_index
            .as_ref()
            .and_then(|entry| entry.category.clone())
            .or(category)
    };
    let summary = if summary != old_summary {
        summary
    } else {
        existing_index
            .as_ref()
            .and_then(|entry| entry.summary.clone())
            .or(summary)
    }
    .unwrap_or_else(|| crate::util::first_prose_line(&doc.content, 240));
    let event = super::sync::SyncChangeInput {
        origin_seq: 1,
        entity_kind: "wiki".into(),
        entity_id: doc.uri.clone(),
        operation: "upsert".into(),
        payload_json: serde_json::to_string(&super::sync::WikiSyncPayload {
            slug: slug.into(),
            title: doc.title.clone(),
            content: doc.content.clone(),
            wing: doc.wing.clone(),
            room: doc.room.clone(),
            kind: Some(doc.kind.to_string()),
            category: category.clone(),
            summary: Some(summary.clone()),
            metadata_json: Some(doc.metadata_json.clone()),
        })?,
        content_hash: Some(content_hash(&doc.content)),
        created_at: doc
            .updated_at
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
    };
    super::sync::validate_wiki_change(&event)?;
    let environment_identity;
    let identity = match identity {
        Some(identity) => identity,
        None => {
            environment_identity = super::sync::SyncIdentity::current()?;
            &environment_identity
        }
    };
    upsert_wiki_index_entry_locked(
        conn,
        &WikiIndexEntry {
            id: existing_index
                .map(|entry| entry.id)
                .unwrap_or_else(|| format!("idx-{}", doc.id)),
            slug: slug.into(),
            title: doc.title.clone(),
            kind: doc.kind.to_string(),
            category,
            summary: Some(summary),
            page_id: Some(doc.id.clone()),
            updated_at: doc.updated_at,
        },
    )?;
    super::sync::local_change_locked(conn, identity, &event)?;
    Ok(())
}

fn write_document_locked(
    conn: &Connection,
    doc: &Document,
    if_match_revision: Option<i64>,
    derived: DocumentDerivedWrite<'_>,
    prepared_embeddings: Option<&[String]>,
    manifest: Option<crate::db::SourceManifestWrite<'_>>,
) -> Result<AtomicDocumentWriteResult> {
    let revision = upsert_document_cas_locked(conn, doc, if_match_revision)?;
    let (node_id, edge_count) = match (derived, prepared_embeddings) {
        (DocumentDerivedWrite::Preserve, None) => (None, 0),
        (DocumentDerivedWrite::RefreshGraphLabel, None) => {
            (refresh_document_graph_label_locked(conn, doc)?, 0)
        }
        (DocumentDerivedWrite::ReplaceChunksAndGraph(chunks), Some(embedding_json)) => {
            super::fts::mark_fts_dirty(conn)?;
            conn.execute("DELETE FROM chunks WHERE document_id = ?", params![doc.id])?;
            insert_chunks_locked(conn, chunks, embedding_json)?;
            let (node_id, edge_count) =
                crate::graph::resolve::rebuild_document_graph_locked(conn, doc)?;
            (Some(node_id), edge_count)
        }
        _ => unreachable!("derived write and prepared embedding state must match"),
    };
    if let Some(manifest) = manifest {
        super::source_manifest::upsert_source_manifest_locked(conn, manifest)?;
    }
    Ok(AtomicDocumentWriteResult {
        revision,
        node_id,
        edge_count,
    })
}

fn apply_document_meta_update(
    mut doc: Document,
    update: &DocumentMetaUpdate,
) -> Result<DocumentMetaApplyResult> {
    let mut content_changed = false;
    let mut title_changed = false;
    let persisted_raw = crate::models::Layer::Raw.is(&doc.layer);

    // Layer transition policy is deliberately asymmetric: mutable documents
    // may be frozen by moving to `raw`, but a persisted raw source cannot leave
    // that layer through a generic metadata patch. Compiled/mutable content must
    // be created as a separate document instead of relabelling the source.
    if persisted_raw
        && update
            .layer
            .as_deref()
            .map(str::trim)
            .filter(|layer| !layer.is_empty())
            .is_some_and(|layer| !crate::models::Layer::Raw.is(layer))
    {
        return Err(AppError::conflict(format!(
            "document {} is layer=raw (immutable source); changing its layer is forbidden",
            doc.id
        )));
    }

    if let Some(ref wing) = update.wing {
        doc.wing = if wing.is_empty() {
            None
        } else {
            Some(wing.clone())
        };
    }
    if let Some(ref room) = update.room {
        doc.room = if room.is_empty() {
            None
        } else {
            Some(room.clone())
        };
    }
    if let Some(ref status) = update.status {
        let status = status.trim();
        if !status.is_empty() {
            doc.status = status.to_string();
        }
    }
    if let Some(ref layer) = update.layer {
        let layer = layer.trim();
        if !layer.is_empty() {
            doc.layer = crate::models::Layer::canonicalize(layer).into_owned();
        }
    }
    if let Some(ref kind) = update.kind {
        let kind = kind.trim();
        if !kind.is_empty() {
            doc.kind = kind.into();
        }
    }
    if let Some(ref source_file) = update.source_file {
        doc.source_file = if source_file.is_empty() {
            None
        } else {
            Some(source_file.clone())
        };
    }
    if let Some(ref title) = update.title {
        let title = title.trim();
        if !title.is_empty() && title != doc.title {
            doc.title = title.to_string();
            title_changed = true;
        } else if !title.is_empty() {
            // Same title: still count as written but not "changed" for graph label.
            doc.title = title.to_string();
        }
    }
    if let Some(ref metadata_json) = update.metadata_json {
        let metadata_json = metadata_json.trim();
        if metadata_json.is_empty() {
            doc.metadata_json = "{}".into();
        } else {
            serde_json::from_str::<serde_json::Value>(metadata_json).map_err(|error| {
                AppError::config(format!("metadata_json is not valid JSON: {error}"))
            })?;
            doc.metadata_json = metadata_json.to_string();
        }
    }
    if let Some(pinned) = update.pinned {
        doc.pinned = pinned;
    }
    if let Some(boost) = update.boost {
        if !boost.is_finite() || boost <= 0.0 {
            return Err(AppError::config(format!(
                "boost must be finite and > 0 (got {boost})"
            )));
        }
        doc.boost = boost;
    }
    if let Some(ref new_content) = update.content {
        if new_content.as_str() != doc.content.as_str() {
            // Check the persisted layer, not the already-applied patch: a
            // `{layer: "wiki", content: ...}` request must not bypass raw
            // immutability in one operation.
            if persisted_raw {
                return Err(AppError::conflict(format!(
                    "document {} is layer=raw (immutable body); refuse content change via update_document_meta",
                    doc.id
                )));
            }
            doc.content = new_content.clone();
            doc.content_hash = Some(content_hash(new_content));
            content_changed = true;
        }
    }

    doc.updated_at = Utc::now();
    Ok(DocumentMetaApplyResult {
        document: doc,
        content_changed,
        title_changed,
    })
}

fn get_document_locked(conn: &Connection, id: &str) -> Result<Option<Document>> {
    let sql = format!("SELECT {DOCUMENT_SELECT} FROM documents WHERE id = ?");
    let mut statement = conn.prepare(&sql)?;
    let mut rows = statement.query(params![id])?;
    match rows.next()? {
        Some(row) => Ok(Some(rows::document(row)?)),
        None => Ok(None),
    }
}

pub(super) fn delete_document_locked(conn: &duckdb::Connection, id: &str) -> Result<bool> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM documents WHERE id = ?)",
        params![id],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(false);
    }
    super::fts::mark_fts_dirty(conn)?;
    // GRAPH_DESIGN.md §6.3: never wipe incident edges here. The node is demoted to
    // an unresolved stub and edges from other notes survive.
    super::graph::demote_graph_for_document_locked(conn, id)?;
    conn.execute("DELETE FROM chunks WHERE document_id = ?", params![id])?;
    conn.execute(
        "DELETE FROM wiki_index WHERE document_id = ? OR page_id = ?",
        params![id, id],
    )?;
    conn.execute(
        "DELETE FROM collection_dependencies WHERE document_id = ? OR depends_on_document_id = ?",
        params![id, id],
    )?;
    conn.execute(
        "UPDATE collection_entries SET parent_document_id = NULL WHERE parent_document_id = ?",
        params![id],
    )?;
    conn.execute(
        "DELETE FROM collection_entries WHERE document_id = ?",
        params![id],
    )?;
    conn.execute(
        "DELETE FROM source_manifest WHERE document_id = ?",
        params![id],
    )?;
    conn.execute(
        "DELETE FROM document_revisions WHERE document_id = ?",
        params![id],
    )?;
    let deleted = conn.execute("DELETE FROM documents WHERE id = ?", params![id])?;
    Ok(deleted > 0)
}

fn upsert_document_cas_locked(
    conn: &Connection,
    doc: &Document,
    if_match_revision: Option<i64>,
) -> Result<i64> {
    let hash = match doc.content_hash.as_deref() {
        Some(h) if !h.is_empty() => h.to_string(),
        _ => content_hash(&doc.content),
    };
    let layer = if doc.layer.trim().is_empty() {
        std::borrow::Cow::Borrowed("raw")
    } else {
        crate::models::Layer::canonicalize(&doc.layer)
    };
    let layer = layer.as_ref();
    let kind = if doc.kind.is_empty() {
        "document"
    } else {
        doc.kind.as_str()
    };
    let status = if doc.status.is_empty() {
        "active"
    } else {
        doc.status.as_str()
    };

    let conflicting_uri_owner = {
        let mut stmt =
            conn.prepare("SELECT id FROM documents WHERE uri = ? AND id <> ? LIMIT 1")?;
        let mut rows = stmt.query(params![doc.uri, doc.id])?;
        match rows.next()? {
            Some(row) => Some(row.get::<_, String>(0)?),
            None => None,
        }
    };
    if let Some(owner) = conflicting_uri_owner {
        return Err(AppError::conflict(format!(
            "document uri '{}' is already owned by document {owner}",
            doc.uri
        )));
    }

    let current_rev: Option<i64> = match conn.query_row(
        "SELECT COALESCE(revision, 1) FROM documents WHERE id = ?",
        params![doc.id],
        |row| row.get(0),
    ) {
        Ok(revision) => Some(revision),
        Err(duckdb::Error::QueryReturnedNoRows) => None,
        Err(error) => return Err(error.into()),
    };

    let new_rev = match current_rev {
        Some(rev) => {
            if let Some(expected) = if_match_revision {
                if rev != expected {
                    return Err(AppError::conflict(format!(
                        "etag mismatch for document {}: expected revision {} ({}), current revision {} ({})",
                        doc.id,
                        expected,
                        crate::models::format_document_etag(expected),
                        rev,
                        crate::models::format_document_etag(rev),
                    )));
                }
            }
            rev.saturating_add(1)
        }
        None => {
            if let Some(expected) = if_match_revision {
                return Err(AppError::conflict(format!(
                    "etag mismatch: if_match_revision={expected} but document {} does not exist",
                    doc.id
                )));
            }
            1
        }
    };

    let boost = if doc.boost.is_finite() && doc.boost > 0.0 {
        doc.boost
    } else {
        1.0
    };
    if current_rev.is_some() {
        conn.execute(
            r#"
            INSERT INTO document_revisions
              (document_id, revision, uri, title, content, metadata_json, content_hash,
               wing, room, source_file, layer, kind, status, pinned, boost,
               created_at, updated_at, superseded_at)
            SELECT id, COALESCE(revision, 1), uri, title, content, metadata_json, content_hash,
                   wing, room, source_file, layer, kind, status, pinned, boost,
                   created_at, updated_at, CURRENT_TIMESTAMP
            FROM documents WHERE id = ?
            ON CONFLICT (document_id, revision) DO NOTHING
            "#,
            params![doc.id],
        )?;
        // DuckDB implements INSERT OR REPLACE as delete+insert. Repeated
        // autosync updates can hit an ART index delete bug and invalidate
        // the whole connection. UPDATE preserves the indexed row identity.
        conn.execute(
            r#"
            UPDATE documents SET
              uri = ?, title = ?, content = ?, metadata_json = ?,
              content_hash = ?, wing = ?, room = ?, source_file = ?,
              layer = ?, kind = ?, status = ?, pinned = ?, boost = ?,
              revision = ?, created_at = CAST(? AS TIMESTAMP),
              updated_at = CAST(? AS TIMESTAMP)
            WHERE id = ?
            "#,
            params![
                doc.uri,
                doc.title,
                doc.content,
                doc.metadata_json,
                hash,
                doc.wing,
                doc.room,
                doc.source_file,
                layer,
                kind,
                status,
                doc.pinned,
                boost,
                new_rev,
                format_ts(doc.created_at),
                format_ts(doc.updated_at),
                doc.id,
            ],
        )?;
    } else {
        conn.execute(
            r#"
            INSERT INTO documents
              (id, uri, title, content, metadata_json,
               content_hash, wing, room, source_file, layer, kind, status,
               pinned, boost, revision,
               created_at, updated_at)
            VALUES
              (?, ?, ?, ?, ?,
               ?, ?, ?, ?, ?, ?, ?,
               ?, ?, ?,
               CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))
            "#,
            params![
                doc.id,
                doc.uri,
                doc.title,
                doc.content,
                doc.metadata_json,
                hash,
                doc.wing,
                doc.room,
                doc.source_file,
                layer,
                kind,
                status,
                doc.pinned,
                boost,
                new_rev,
                format_ts(doc.created_at),
                format_ts(doc.updated_at),
            ],
        )?;
    }
    Ok(new_rev)
}

/// JSON-encode chunk embeddings for [`insert_chunks_locked`], before any lock
/// is taken so a serialization failure cannot leave a partial write.
pub(crate) fn embedding_json(chunks: &[Chunk]) -> Result<Vec<String>> {
    chunks
        .iter()
        .map(|chunk| Ok(serde_json::to_string(&chunk.embedding)?))
        .collect()
}

/// [`embedding_json`] after checking that every chunk belongs to `document_id`.
pub(crate) fn owned_embedding_json(document_id: &str, chunks: &[Chunk]) -> Result<Vec<String>> {
    if let Some(chunk) = chunks.iter().find(|chunk| chunk.document_id != document_id) {
        return Err(AppError::config(format!(
            "chunk {} belongs to document {}, expected {}",
            chunk.id, chunk.document_id, document_id
        )));
    }
    embedding_json(chunks)
}

pub(crate) fn insert_chunks_locked(
    conn: &Connection,
    chunks: &[Chunk],
    embedding_json: &[String],
) -> Result<()> {
    if chunks.len() != embedding_json.len() {
        return Err(AppError::db(
            "prepared embedding count does not match chunk count",
        ));
    }
    if chunks.is_empty() {
        return Ok(());
    }

    let now = format_ts(Utc::now());
    let digests: Vec<i64> = embedding_json
        .iter()
        .map(|json| embedding_digest(json))
        .collect();
    let rows: Vec<(&Chunk, &String, &i64)> = chunks
        .iter()
        .zip(embedding_json)
        .zip(&digests)
        .map(|((chunk, json), digest)| (chunk, json, digest))
        .collect();
    rows::insert_rows(
        conn,
        "INSERT INTO chunks (id, document_id, chunk_index, content, embedding_json, \
         char_start, char_end, created_at, metadata_json, embedding_digest) VALUES",
        "(?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), ?, ?)",
        &rows,
        |(chunk, embedding_json, digest), values| {
            values.extend([
                &chunk.id as &dyn duckdb::ToSql,
                &chunk.document_id,
                &chunk.chunk_index,
                &chunk.content,
                *embedding_json,
                &chunk.char_start,
                &chunk.char_end,
                &now,
                &chunk.metadata_json,
                *digest,
            ]);
        },
    )
}

/// 64-bit digest stored in `chunks.embedding_digest` (first 8 bytes of the
/// blake3 hash of the stored JSON). Any digest change marks a changed vector.
pub(crate) fn embedding_digest(embedding_json: &str) -> i64 {
    let hash = blake3::hash(embedding_json.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash.as_bytes()[..8]);
    i64::from_le_bytes(bytes)
}

fn list_chunks_for_document_locked(conn: &Connection, document_id: &str) -> Result<Vec<Chunk>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, document_id, chunk_index, content, embedding_json, char_start, char_end, metadata_json
        FROM chunks
        WHERE document_id = ?
        ORDER BY chunk_index ASC, id ASC
        "#,
    )?;
    let chunks =
        crate::db::rows::query_vec(&mut stmt, params![document_id], rows::chunk)?;
    Ok(chunks)
}

fn same_chunk_snapshot(current: &[Chunk], expected: &[Chunk]) -> bool {
    if current.len() != expected.len() {
        return false;
    }
    let expected = expected
        .iter()
        .map(|chunk| (chunk.id.as_str(), chunk))
        .collect::<std::collections::HashMap<_, _>>();
    current.iter().all(|current| {
        expected.get(current.id.as_str()).is_some_and(|expected| {
            current.document_id == expected.document_id
                && current.chunk_index == expected.chunk_index
                && current.content == expected.content
                && current.embedding == expected.embedding
                && current.char_start == expected.char_start
                && current.char_end == expected.char_end
                && current.metadata_json == expected.metadata_json
        })
    })
}

pub(crate) fn set_embedding_manifest_locked(
    conn: &Connection,
    manifest: &EmbeddingManifest,
) -> Result<()> {
    conn.execute(
        r#"
        INSERT OR REPLACE INTO embedding_manifest
          (id, provider, model, dims, base_url, content_fingerprint, updated_at)
        VALUES
          (?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))
        "#,
        params![
            manifest.id,
            manifest.provider,
            manifest.model,
            manifest.dims,
            manifest.base_url,
            manifest.content_fingerprint,
            format_ts(manifest.updated_at),
        ],
    )?;
    Ok(())
}

fn refresh_document_graph_label_locked(
    conn: &Connection,
    doc: &Document,
) -> Result<Option<String>> {
    let node_id = {
        let mut stmt = conn.prepare("SELECT id FROM graph_nodes WHERE document_id = ? LIMIT 1")?;
        let mut rows = stmt.query(params![doc.id])?;
        rows.next()?.map(|row| row.get(0)).transpose()?
    };
    if let Some(ref node_id) = node_id {
        conn.execute(
            "UPDATE graph_nodes SET label = ?, label_key = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?",
            params![
                doc.title,
                crate::graph::normalize::label_key(&doc.title),
                format_ts(Utc::now()),
                node_id
            ],
        )?;
    }
    Ok(node_id)
}

#[cfg(test)]
thread_local! {
    static PRUNE_ORPHANS_FAIL_AFTER_NODES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Upper bound on one ops-log page for every reader (MCP, HTTP, services).
pub const MAX_OPS_LOG_PAGE: usize = 500;

/// True when `information_schema.tables` lists `name` (DuckDB).
fn table_exists_locked(conn: &Connection, name: &str) -> bool {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = ?",
            params![name],
            |row| row.get(0),
        )
        .unwrap_or(0);
    n > 0
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    parse_db_timestamp(s)
        .ok_or_else(|| AppError::db(format!("invalid timestamp value: {}", s.trim())))
}

mod chunks;
mod corpus_lane;
mod documents;
mod health;
mod manifest;
mod ops_log;
mod revisions;
mod schema_diary;
mod wiki_index;
mod wiki_meta;

use self::{ops_log::*, wiki_index::*, wiki_meta::*};
pub use self::manifest::{embedding_manifest_from_config, embedding_manifest_matches_config};
pub(crate) use self::chunks::ChunkBody;
pub(crate) use self::documents::{wiki_backlink_rows, DocumentRef};
pub(crate) use self::manifest::{embedding_identity_fingerprint, embedding_migration_manifest};

#[cfg(test)]
mod tests;
