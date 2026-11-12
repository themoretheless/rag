//! Document and chunk persistence over `lin`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

static NEXT_STORE_ID: AtomicUsize = AtomicUsize::new(1);

use chrono::{DateTime, Utc};
use lin::SyncMode;

use super::EmbeddingIdentity;
use super::lin_engine::{
    LinBackend, LinReadGuard, LinReader, LinTx, LinWriteGuard, LinWriter, RecBuilder, cell_i64,
    cell_opt_string, cell_str, composite_id_2, read_lock, write_lock,
};
use super::rows;
use super::schema;
use crate::error::{AppError, Result};
use crate::models::{
    Chunk, DiaryEntry, Document, DocumentFilter, DocumentMetaApplyResult, DocumentMetaUpdate,
    DocumentRevisionPage, DocumentRevisionSummary, DuplicateCheckResult, DuplicateMatch,
    EmbeddingManifest, OpsLogEntry, PlacementUpdate, RoomCount, Taxonomy, TaxonomyRoom,
    TaxonomyWing, VacuumStoreReport, WikiIndexEntry, WingCount,
};
use crate::util::{
    SlugPolicy, content_hash, format_db_timestamp as format_ts, slugify as shared_slugify,
    wiki_slug_from_uri,
};

const EMBEDDING_MIGRATION_FINGERPRINT_PREFIX: &str = "migration-incomplete:";

#[cfg(any(test, feature = "testkit"))]
#[doc(hidden)]
pub const TEST_FAIL_WIKI_WRITE_AFTER_INDEX_OP: &str = "__test_fail_wiki_write_after_index__";

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

/// Thread-safe `lin`-backed document/chunk store.
///
/// Readers share concurrent `RwLockReadGuard` access with zero cloning;
/// writers take `RwLockWriteGuard` and commit atomically via [`LinTx`].
#[derive(Clone)]
pub struct Store {
    backend: Arc<RwLock<LinBackend>>,
    store_id: usize,
    path: PathBuf,
    corpus_mutation_lane: Arc<tokio::sync::RwLock<()>>,
    // Present only on an explicitly scoped clone used by a bulk workflow.
    // Clones keep the lease alive if nested work outlives its transport request.
    corpus_mutation_guard: Option<Arc<CorpusMutationGuard>>,
}

pub type CorpusMutationGuard = tokio::sync::OwnedRwLockWriteGuard<()>;

/// Keeps the embedding identity stable from preflight through document commit.
pub struct EmbeddingWriteGuard {
    _shared: Option<tokio::sync::OwnedRwLockReadGuard<()>>,
    _exclusive: Option<Arc<CorpusMutationGuard>>,
}

/// Persisted safety outcome for a failed derived-index finalization.
pub struct FtsFinalizationFailure {
    pub message: String,
    pub dirty_marker_written: bool,
    pub dirty_marker_error: Option<String>,
    pub ops_log_error: Option<String>,
}

/// Derived state that must stay consistent with a document write.
pub enum DocumentDerivedWrite<'a> {
    /// Keep chunks and graph unchanged (metadata-only update).
    Preserve,
    /// Keep chunks/edges, but refresh the existing document-node label.
    RefreshGraphLabel,
    /// Replace chunks and rebuild the document's outgoing graph slice.
    ReplaceChunksAndGraph(&'a [Chunk]),
}

/// Outcome of an atomic document + derived-state write.
#[derive(Debug)]
pub struct AtomicDocumentWriteResult {
    pub revision: i64,
    pub node_id: Option<String>,
    pub edge_count: usize,
}

/// Outcome of one exact-duplicate maintenance group.
///
/// `merged` contains every source that was archived or deleted. Raw sources
/// protected from a requested hard delete are also listed in `raw_protected`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactDuplicateMergeResult {
    pub merged: Vec<String>,
    pub raw_protected: Vec<String>,
}

/// Aggregate layer/index health used by diagnostics without loading document bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerHealthCounts {
    pub raw_count: u64,
    pub wiki_count: u64,
    pub index_entry_count: u64,
    pub indexed_pages: u64,
    pub uncompiled_raw_count: u64,
}

/// Lean uncompiled-raw sample row for lint guidance.
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
pub struct UncompiledShelfQueue {
    pub shelves: Vec<UncompiledRawShelf>,
    pub total_shelves: u64,
}

/// Compute the set of document IDs that have a compile link: an inbound edge
/// from a resolved `wiki://` graph node (`source.id != edge.target_id`) to a
/// graph node whose `document_id` owns that document.
///
/// Shared across `layer_health_counts`, `list_uncompiled_raw_sample`,
/// `uncompiled_raw_shelves`, and `catalog::list_document_catalog`.
pub(crate) fn compiled_document_ids(
    r: &impl LinReader,
) -> Result<std::collections::HashSet<String>> {
    let mut resolved_wiki_nodes = std::collections::HashSet::new();
    let mut node_to_doc: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    r.scan_each("graph_nodes", |row| {
        let Some(id) = cell_str(row, "id") else {
            return Ok(true);
        };
        if let Some(doc_id) = cell_str(row, "document_id").filter(|s| !s.is_empty()) {
            node_to_doc.insert(id.to_string(), doc_id.to_string());
        }
        let resolved = super::lin_engine::cell_bool(row, "resolved", false);
        if resolved && cell_str(row, "uri").is_some_and(|u| u.starts_with("wiki://")) {
            resolved_wiki_nodes.insert(id.to_string());
        }
        Ok(true)
    })?;

    let mut compiled_docs = std::collections::HashSet::new();
    if resolved_wiki_nodes.is_empty() || node_to_doc.is_empty() {
        return Ok(compiled_docs);
    }

    r.scan_each("graph_edges", |row| {
        let (Some(src), Some(tgt)) = (cell_str(row, "source_id"), cell_str(row, "target_id"))
        else {
            return Ok(true);
        };
        if src != tgt
            && resolved_wiki_nodes.contains(src)
            && let Some(doc_id) = node_to_doc.get(tgt)
        {
            compiled_docs.insert(doc_id.clone());
        }
        Ok(true)
    })?;

    Ok(compiled_docs)
}

/// Lean first-chunk vector sample for bounded maintenance comparisons.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentEmbeddingSample {
    pub document_id: String,
    pub title: String,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub embedding: Vec<f32>,
}

/// Inputs for [`Store::write_wiki_document_atomic`].
#[derive(Debug, Clone, Copy)]
pub struct WikiDocumentWrite<'a> {
    pub doc: &'a Document,
    pub if_match_revision: Option<i64>,
    pub chunks: &'a [Chunk],
    pub index_entry: &'a WikiIndexEntry,
    pub audit_entry: &'a OpsLogEntry,
    pub sync: &'a super::sync::WikiSyncWrite<'a>,
    pub create_only: bool,
}

impl Store {
    /// Open (or create) a `lin` database at `path` and run schema migrations.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_extensions(path, &schema::registered_schema_extensions())
    }

    pub(crate) fn open_with_extensions(
        path: &Path,
        extensions: &[schema::SchemaExtension],
    ) -> Result<Self> {
        let is_mem = path.as_os_str().is_empty() || path.as_os_str() == ":memory:";
        let backend = if is_mem {
            LinBackend::open_memory()
        } else {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            LinBackend::open_durable(path, SyncMode::Full)?
        };

        let backend = Arc::new(RwLock::new(backend));
        {
            let mut w = write_lock(&backend)?;
            let already_migrated = w.disk_v2().is_some()
                && schema::current_schema_version(&w).ok().flatten()
                    == Some(schema::SCHEMA_VERSION);
            schema::migrate(&mut w)?;
            if !already_migrated {
                // Flush initial catalog/manifest so concurrent `Store::open_read_only`
                // sees all collections and schema_version immediately on disk.
                let _ = w.checkpoint();
            }
        }

        let store = Self {
            backend,
            store_id: NEXT_STORE_ID.fetch_add(1, Ordering::Relaxed),
            path: path.to_path_buf(),
            corpus_mutation_lane: Arc::new(tokio::sync::RwLock::new(())),
            corpus_mutation_guard: None,
        };
        store.ensure_schema_extensions(extensions)?;
        Ok(store)
    }

    /// Open an in-memory `lin` store with migrated schema (fast unit-test convenience).
    pub fn open_in_memory() -> Result<Self> {
        Self::open(Path::new(":memory:"))
    }

    /// Open an existing, current-schema `rag-mcp` `lin` database without any writes.
    pub fn open_read_only(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AppError::not_found(format!(
                    "read-only lin database '{}' does not exist",
                    path.display()
                ))
            } else {
                AppError::Io(error)
            }
        })?;
        if !metadata.is_dir() && !super::lin_engine::is_lin_portable_backup_file(path) {
            return Err(AppError::config(format!(
                "read-only lin path '{}' is neither a lin directory nor a LIN\\x04 backup file",
                path.display()
            )));
        }

        let backend = LinBackend::open_read_only(path).map_err(|error| {
            AppError::db(format!(
                "cannot open lin database '{}' read-only: {error}",
                path.display()
            ))
        })?;
        let backend = Arc::new(RwLock::new(backend));
        {
            let r = read_lock(&backend)?;
            for required in ["documents", "chunks", "graph_nodes", "graph_edges"] {
                if !r.has_collection(required) {
                    return Err(AppError::config(format!(
                        "lin database '{}' is not a compatible rag-mcp database: missing collection '{required}'",
                        path.display()
                    )));
                }
            }
            let version = schema::current_schema_version(&r).map_err(|error| {
                AppError::config(format!(
                    "lin database '{}' is not a compatible rag-mcp database: {error}",
                    path.display()
                ))
            })?;
            if version != Some(schema::SCHEMA_VERSION) {
                return Err(AppError::config(format!(
                    "lin database '{}' has incompatible schema version {}; expected {}. Upgrade a writable copy through the gateway before inspecting it",
                    path.display(),
                    version
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "missing".into()),
                    schema::SCHEMA_VERSION
                )));
            }
        }

        Ok(Self {
            backend,
            store_id: NEXT_STORE_ID.fetch_add(1, Ordering::Relaxed),
            path: path.to_path_buf(),
            corpus_mutation_lane: Arc::new(tokio::sync::RwLock::new(())),
            corpus_mutation_guard: None,
        })
    }

    /// Filesystem path of the opened database.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Shared read lock over the `lin` backend (concurrent zero-copy readers).
    pub fn read(&self) -> Result<LinReadGuard<'_>> {
        read_lock(&self.backend)
    }

    /// Exclusive write lock over the `lin` backend.
    pub fn write(&self) -> Result<LinWriteGuard<'_>> {
        write_lock(&self.backend)
    }

    /// Export a self-contained portable `LIN\x04` snapshot file without sidecars.
    pub fn export_portable_backup(&self, destination: &Path) -> Result<()> {
        let mut w = self.write()?;
        w.export_backup(destination)
    }

    /// Toggle full hardware barrier (`F_FULLFSYNC` / `fsync`) vs OS WAL buffer flush (`SyncMode::Normal`).
    /// Production defaults to `true` (`SyncMode::Full`); benchmarks or bulk pipelines can test `false` (`SyncMode::Normal`).
    pub fn set_sync_full(&self, sync_full: bool) -> Result<()> {
        let mut w = self.write()?;
        if let Some(eng) = w.disk_v2_mut() {
            eng.set_sync_full(sync_full);
        }
        Ok(())
    }

    /// Explicitly `fsync` the active WAL file to physical media.
    pub fn flush_wal(&self) -> Result<()> {
        let mut w = self.write()?;
        if let Some(eng) = w.disk_v2_mut() {
            eng.flush_wal()?;
        }
        Ok(())
    }

    /// Monotonic process-unique identity for snapshot caches (immune to ABA pointer reuse).
    pub(in crate::db) fn backend_key(&self) -> usize {
        self.store_id
    }

    /// Run `work` in one ACID `lin::Transaction` under the Store write lock.
    /// Commits on `Ok`; an `Err` (or panic) rolls back automatically.
    pub fn with_tx<T>(&self, work: impl FnOnce(&mut LinTx<'_>) -> Result<T>) -> Result<T> {
        let mut w = self.write()?;
        w.with_tx(work)
    }

    /// Register `extensions` process-wide. `lin`'s [`schema::migrate`] ensures
    /// every collection up front, so no runtime SQL DDL is needed.
    pub fn ensure_schema_extensions(&self, extensions: &[schema::SchemaExtension]) -> Result<()> {
        for extension in extensions {
            schema::register_schema_extension(*extension);
        }
        Ok(())
    }

    /// Fault-injection helper for tests simulating a missing internal table/collection.
    #[cfg(any(test, feature = "testkit"))]
    #[doc(hidden)]
    pub fn drop_collection_for_test(&self, name: &str) -> Result<()> {
        use super::lin_engine::LinWriter;
        let mut w = self.write()?;
        if let Some(eng) = w.disk_v2_mut() {
            eng.drop_table(name)?;
            eng.drop_table("meta")?;
            w.ensure_col(
                "meta",
                false,
                &[("id", "id"), ("key", "var"), ("value", "var")],
            )?;
            let _ = w.checkpoint();
            return Ok(());
        }
        let db = w.db_mut();
        db.catalog.collections.remove(name);
        db.catalog.indexes.retain(|_, idx| idx.collection != name);
        db.store.extra_collections.remove(name);
        db.store
            .extra_indexes
            .retain(|_, idx| idx.collection != name);
        db.store.collections.remove(name);
        db.store.indexes.retain(|_, idx| idx.def.collection != name);
        db.catalog.collections.remove("meta");
        db.store.extra_collections.remove("meta");
        db.catalog.refresh_hash();
        let mut tx = db.transaction()?;
        tx.execute("col meta { id: id, key: var, value: var }")?;
        tx.commit()?;
        let _ = db.checkpoint();
        Ok(())
    }

    /// Cross-thread Store-scoped fault injection for derived duplicate graph edges.
    #[cfg(any(test, feature = "testkit"))]
    #[doc(hidden)]
    pub fn set_fail_derived_edge_duplicate_for_test(&self, fail: bool) {
        use super::lin_engine::LinWriter;
        if let Ok(w) = self.write() {
            let key = std::sync::Arc::as_ptr(w.bridge_embedder()) as usize;
            crate::db::graph::set_fail_derived_edge_duplicate_key(key, fail);
        }
    }

    /// Insert or replace a document row by primary key `id` (last-write-wins).
    pub fn upsert_document(&self, doc: &Document) -> Result<()> {
        self.upsert_document_cas(doc, None).map(|_| ())
    }

    /// CAS upsert: if `if_match_revision` is `Some`, the existing row (by `id`)
    /// must have that revision or [`AppError::Conflict`] is returned.
    pub fn upsert_document_cas(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
    ) -> Result<i64> {
        self.with_tx(|tx| upsert_document_cas_locked(tx, doc, if_match_revision))
    }

    /// Persist a document and its derived state under one transaction.
    pub fn write_document_atomic(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
        derived: DocumentDerivedWrite<'_>,
    ) -> Result<AtomicDocumentWriteResult> {
        self.write_document_atomic_with_manifest(doc, if_match_revision, derived, None, None)
    }

    /// Persist an unchunked document (e.g. `schema://agents`), rebuild its
    /// outgoing graph slice and append its audit row in one transaction.
    pub fn write_unchunked_document_with_audit(
        &self,
        doc: &Document,
        audit: &OpsLogEntry,
    ) -> Result<AtomicDocumentWriteResult> {
        self.with_tx(|tx| {
            let revision = upsert_document_cas_locked(tx, doc, None)?;
            let (node_id, edge_count) =
                super::graph::resolve::rebuild_document_graph_locked(tx, doc)?;
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
    pub fn write_source_document_atomic(
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

    /// Persist a wiki document and its chunks, graph, catalog row, audit event,
    /// replication journal and applied cursor under one transaction.
    pub fn write_wiki_document_atomic(
        &self,
        write: WikiDocumentWrite<'_>,
    ) -> Result<(Option<AtomicDocumentWriteResult>, i64)> {
        let WikiDocumentWrite {
            doc,
            if_match_revision,
            chunks,
            index_entry,
            audit_entry,
            sync,
            create_only,
        } = write;
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

        self.with_tx(|tx| {
            if create_only {
                let uri_exists = !tx
                    .find_by_eq("documents", "uri", doc.uri.as_str())?
                    .is_empty();
                let id_exists = tx.get_by_id("documents", &doc.id)?.is_some();
                if uri_exists || id_exists {
                    return Err(AppError::conflict("wiki page already exists"));
                }
            }
            if let Some(sequence) = super::sync::sync_duplicate_locked(tx, sync)? {
                return Ok((None, sequence));
            }
            if super::sync::defer_pulled_document_locked(tx, sync, &doc.uri)? {
                let sequence = super::sync::commit_sync_write_locked(tx, sync, &local_event)?;
                return Ok((None, sequence));
            }
            super::review::accept_locked(tx, doc, audit_entry)?;
            let result = write_document_locked(
                tx,
                doc,
                if_match_revision,
                DocumentDerivedWrite::ReplaceChunksAndGraph(chunks),
                Some(&prepared_embeddings),
                None,
            )?;
            upsert_wiki_index_entry_locked(tx, index_entry)?;
            #[cfg(any(test, feature = "testkit"))]
            if audit_entry.op == TEST_FAIL_WIKI_WRITE_AFTER_INDEX_OP {
                return Err(AppError::db(
                    "injected wiki write failure after catalog update",
                ));
            }
            append_ops_log_locked(tx, audit_entry)?;
            let sequence = super::sync::commit_sync_write_locked(tx, sync, &local_event)?;
            #[cfg(any(test, feature = "testkit"))]
            if audit_entry.op == "__test_fail_wiki_after_sync_journal__" {
                return Err(AppError::db("injected wiki failure after sync journal"));
            }
            Ok((Some(result), sequence))
        })
    }

    /// Insert chunk rows with binary `Cell::Vec` embeddings via `BridgeEmbedder`.
    pub fn insert_chunks(&self, chunks: &[Chunk]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }

        let embedding_json = embedding_json(chunks)?;
        let documents: std::collections::BTreeSet<&str> = chunks
            .iter()
            .map(|chunk| chunk.document_id.as_str())
            .collect();
        let documents: Vec<&str> = documents.into_iter().collect();
        self.with_tx(|tx| {
            super::fts::mark_chunks_changed_for(tx, &documents)?;
            insert_chunks_locked(tx, chunks, &embedding_json)
        })
    }

    /// Delete a document, its chunks, and demote its graph node.
    pub fn delete_document(&self, id: &str) -> Result<bool> {
        self.with_tx(|tx| delete_document_locked(tx, id))
    }

    /// Apply a maintenance batch of hard deletes and tombstones atomically.
    pub fn apply_document_dispositions_atomic(
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
                let chunk_count = tx.find_by_eq("chunks", "document_id", id.as_str())?.len() as u64;
                if !delete_document_locked(tx, id)? {
                    return Err(AppError::not_found(format!(
                        "document not found for delete: {id}"
                    )));
                }
                deleted += 1;
                chunks_deleted = chunks_deleted.saturating_add(chunk_count);
            }
            Ok((deleted, tombstoned, chunks_deleted))
        })
    }

    /// Validate and apply one exact-content duplicate group atomically.
    pub fn merge_exact_duplicates_atomic(
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
    pub fn count_chunks_for_document(&self, document_id: &str) -> Result<u64> {
        let r = self.read()?;
        Ok(r.find_by_eq("chunks", "document_id", document_id)?.len() as u64)
    }

    /// Delete only chunks for a document (used on uri re-ingest to keep graph node stable).
    pub fn delete_chunks_for_document(&self, document_id: &str) -> Result<()> {
        self.with_tx(|tx| {
            super::fts::mark_chunks_changed_for(tx, &[document_id])?;
            tx.delete_where("chunks", lin::query::pred::eq("document_id", document_id))?;
            Ok(())
        })
    }

    /// Delete all documents whose `source_file` equals `source`, plus their chunks.
    pub fn delete_by_source(&self, source: &str) -> Result<u64> {
        self.delete_by_source_ids(source)
            .map(|document_ids| document_ids.len() as u64)
    }

    /// Delete by exact source and return the ids selected inside the same transaction.
    pub fn delete_by_source_ids(&self, source: &str) -> Result<Vec<String>> {
        self.delete_source_state_ids_with(source, |_| Ok(()))
            .map(|(document_ids, _)| document_ids)
    }

    /// Atomically remove every document/derived row and manifest ownership for one filesystem source.
    pub fn delete_source_state(&self, source: &str) -> Result<(u64, bool)> {
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
            let manifest_present = tx.get_by_id("source_manifest", source)?.is_some()
                || !tx
                    .find_by_eq("source_manifest", "canonical_path", source)?
                    .is_empty();
            let doc_rows = tx.find_by_eq("documents", "source_file", source)?;
            let ids: Vec<String> = doc_rows
                .iter()
                .filter_map(|r| cell_opt_string(r, "id"))
                .collect();
            for (index, id) in ids.iter().enumerate() {
                delete_document_locked(tx, id)?;
                after_document(index + 1)?;
            }
            let _ = tx.delete_by_id("source_manifest", source)?;
            tx.delete_where(
                "source_manifest",
                lin::query::pred::eq("canonical_path", source),
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

fn journal_generic_wiki_change_locked(
    w: &mut impl LinWriter,
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

    let existing_index = w
        .find_by_eq("wiki_index", "slug", slug)?
        .first()
        .map(rows::wiki_index_entry)
        .transpose()?;
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
        w,
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
    super::sync::local_change_locked(w, identity, &event)?;
    Ok(())
}

fn write_document_locked(
    w: &mut impl LinWriter,
    doc: &Document,
    if_match_revision: Option<i64>,
    derived: DocumentDerivedWrite<'_>,
    prepared_embeddings: Option<&[String]>,
    manifest: Option<crate::db::SourceManifestWrite<'_>>,
) -> Result<AtomicDocumentWriteResult> {
    let revision = upsert_document_cas_locked(w, doc, if_match_revision)?;
    let (node_id, edge_count) = match (derived, prepared_embeddings) {
        (DocumentDerivedWrite::Preserve, None) => (None, 0),
        (DocumentDerivedWrite::RefreshGraphLabel, None) => {
            (refresh_document_graph_label_locked(w, doc)?, 0)
        }
        (DocumentDerivedWrite::ReplaceChunksAndGraph(chunks), Some(embedding_json)) => {
            super::fts::mark_chunks_changed_for(w, &[doc.id.as_str()])?;
            w.delete_where(
                "chunks",
                lin::query::pred::eq("document_id", doc.id.as_str()),
            )?;
            insert_chunks_locked(w, chunks, embedding_json)?;
            let (node_id, edge_count) =
                super::graph::resolve::rebuild_document_graph_locked(w, doc)?;
            (Some(node_id), edge_count)
        }
        _ => unreachable!("derived write and prepared embedding state must match"),
    };
    if let Some(manifest) = manifest {
        super::source_manifest::upsert_source_manifest_locked(w, manifest)?;
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
            doc.status = status.into();
        }
    }
    if let Some(ref layer) = update.layer {
        let layer = layer.trim();
        if !layer.is_empty() {
            doc.layer = crate::models::Layer::canonicalize(layer).as_ref().into();
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

pub(crate) fn get_document_locked(r: &impl LinReader, id: &str) -> Result<Option<Document>> {
    r.with_by_id("documents", id, rows::document)
}

pub(super) fn delete_document_locked(w: &mut impl LinWriter, id: &str) -> Result<bool> {
    if !w.exists_by_id("documents", id)? {
        return Ok(false);
    }
    super::fts::mark_chunks_changed_for(w, &[id])?;
    super::graph::demote_graph_for_document_locked(w, id)?;
    w.delete_where("chunks", lin::query::pred::eq("document_id", id))?;

    // Delete wiki_index entries matching document_id or page_id via secondary indexes
    let mut wiki_ids = Vec::new();
    for field in ["document_id", "page_id"] {
        w.for_each_by_eq_str3("wiki_index", field, id, "id", field, None, |wid, _, _| {
            if let Some(wid) = wid.filter(|s| !s.is_empty()) {
                if !wiki_ids.iter().any(|existing| existing == wid) {
                    wiki_ids.push(wid.to_string());
                }
            }
            Ok(true)
        })?;
    }
    w.delete_by_ids("wiki_index", &wiki_ids)?;

    // Delete collection_dependencies referencing id via secondary indexes
    let mut dep_ids = Vec::new();
    for field in ["document_id", "depends_on_document_id"] {
        w.for_each_by_eq_str3(
            "collection_dependencies",
            field,
            id,
            "id",
            field,
            None,
            |did, _, _| {
                if let Some(did) = did.filter(|s| !s.is_empty()) {
                    if !dep_ids.iter().any(|existing| existing == did) {
                        dep_ids.push(did.to_string());
                    }
                }
                Ok(true)
            },
        )?;
    }
    w.delete_by_ids("collection_dependencies", &dep_ids)?;

    // Nullify parent_document_id on collection_entries
    w.update_where(
        "collection_entries",
        lin::query::pred::eq("parent_document_id", id),
        RecBuilder::new()
            .opt_str("parent_document_id", None)
            .build(),
    )?;
    w.delete_where(
        "collection_entries",
        lin::query::pred::eq("document_id", id),
    )?;
    w.delete_where("source_manifest", lin::query::pred::eq("document_id", id))?;
    w.delete_where(
        "document_revisions",
        lin::query::pred::eq("document_id", id),
    )?;
    w.delete_by_id("documents", id)
}

pub(crate) fn upsert_document_cas_locked(
    w: &mut impl LinWriter,
    doc: &Document,
    if_match_revision: Option<i64>,
) -> Result<i64> {
    let normalized = super::write_hook::run_document_write_hooks(w, doc)?;
    let doc = normalized.as_ref();
    let hash = match doc.content_hash.as_deref() {
        Some(h) if !h.is_empty() => h.to_string(),
        _ => content_hash(&doc.content),
    };
    let layer = if doc.layer.as_str().trim().is_empty() {
        std::borrow::Cow::Borrowed("raw")
    } else {
        crate::models::Layer::canonicalize(doc.layer.as_str())
    };
    let layer = layer.as_ref();
    let kind = if doc.kind.is_empty() {
        "document"
    } else {
        doc.kind.as_str()
    };
    let status = if doc.status.as_str().is_empty() {
        "active"
    } else {
        doc.status.as_str()
    };

    let existing_row = w.get_by_id("documents", &doc.id)?;
    let uri_unchanged = existing_row
        .as_ref()
        .is_some_and(|row| cell_str(row, "uri") == Some(doc.uri.as_str()));

    // Enforce URI uniqueness across distinct document IDs (skip index probe when updating same URI)
    if !uri_unchanged {
        let mut conflicting_owner: Option<String> = None;
        w.for_each_by_eq_str3(
            "documents",
            "uri",
            doc.uri.as_str(),
            "id",
            "uri",
            None,
            |owner_id, _, _| {
                if let Some(owner) = owner_id.filter(|id| *id != doc.id.as_str()) {
                    conflicting_owner = Some(owner.to_string());
                    return Ok(false);
                }
                Ok(true)
            },
        )?;
        if let Some(owner) = conflicting_owner {
            return Err(AppError::conflict(format!(
                "document uri '{}' is already owned by document {owner}",
                doc.uri
            )));
        }
    }

    let current_rev = existing_row
        .as_ref()
        .map(|row| cell_i64(row, "revision", 1).max(1));

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

    if let Some(ref prev_row) = existing_row {
        let prev_doc = rows::document(prev_row)?;
        let rev_id = composite_id_2(&prev_doc.id, &prev_doc.revision.to_string());
        if !w.exists_by_id("document_revisions", &rev_id)? {
            w.insert_one(
                "document_revisions",
                RecBuilder::new()
                    .str("id", rev_id)
                    .str("document_id", &prev_doc.id)
                    .i64("revision", prev_doc.revision)
                    .str("uri", &prev_doc.uri)
                    .str("title", &prev_doc.title)
                    .str("content", &prev_doc.content)
                    .str("metadata_json", &prev_doc.metadata_json)
                    .opt_string("content_hash", &prev_doc.content_hash)
                    .opt_string("wing", &prev_doc.wing)
                    .opt_string("room", &prev_doc.room)
                    .opt_string("source_file", &prev_doc.source_file)
                    .str("layer", prev_doc.layer.as_str())
                    .str("kind", prev_doc.kind.as_str())
                    .str("status", prev_doc.status.as_str())
                    .bool("pinned", prev_doc.pinned)
                    .f64("boost", prev_doc.boost)
                    .str("created_at", format_ts(prev_doc.created_at))
                    .str("updated_at", format_ts(prev_doc.updated_at))
                    .str("superseded_at", format_ts(Utc::now()))
                    .build(),
            )?;
        }

        w.update_where(
            "documents",
            lin::query::pred::eq("id", doc.id.as_str()),
            RecBuilder::new()
                .str("uri", &doc.uri)
                .str("title", &doc.title)
                .str("content", &doc.content)
                .str("metadata_json", &doc.metadata_json)
                .str("content_hash", hash)
                .opt_string("wing", &doc.wing)
                .opt_string("room", &doc.room)
                .opt_string("source_file", &doc.source_file)
                .str("layer", layer)
                .str("kind", kind)
                .str("status", status)
                .bool("pinned", doc.pinned)
                .f64("boost", boost)
                .i64("revision", new_rev)
                .str("created_at", format_ts(doc.created_at))
                .str("updated_at", format_ts(doc.updated_at))
                .build(),
        )?;
    } else {
        w.insert_one(
            "documents",
            RecBuilder::new()
                .str("id", &doc.id)
                .str("uri", &doc.uri)
                .str("title", &doc.title)
                .str("content", &doc.content)
                .str("metadata_json", &doc.metadata_json)
                .str("content_hash", hash)
                .opt_string("wing", &doc.wing)
                .opt_string("room", &doc.room)
                .opt_string("source_file", &doc.source_file)
                .str("layer", layer)
                .str("kind", kind)
                .str("status", status)
                .bool("pinned", doc.pinned)
                .f64("boost", boost)
                .i64("revision", new_rev)
                .str("created_at", format_ts(doc.created_at))
                .str("updated_at", format_ts(doc.updated_at))
                .build(),
        )?;
    }
    Ok(new_rev)
}

/// JSON-encode chunk embeddings (kept for digest computation and validation).
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

pub(crate) fn extract_parent_chunk_id(metadata_json: &str) -> Option<String> {
    if !metadata_json.contains("parent_chunk_id") {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(metadata_json)
        .ok()?
        .get("parent_chunk_id")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub(crate) fn insert_chunks_locked(
    w: &mut impl LinWriter,
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
    let bridge = w.bridge_embedder().clone();
    let mut records = Vec::with_capacity(chunks.len());
    for (chunk, json) in chunks.iter().zip(embedding_json) {
        let digest = embedding_digest(json);
        let parent_chunk_id = extract_parent_chunk_id(&chunk.metadata_json);
        let has_vec = !chunk.embedding.is_empty();
        if has_vec {
            bridge.stage(chunk.id.clone(), Arc::from(chunk.embedding.as_slice()));
        }
        records.push(
            RecBuilder::new()
                .str("id", &chunk.id)
                .str("document_id", &chunk.document_id)
                .i64("chunk_index", i64::from(chunk.chunk_index))
                .str("content", &chunk.content)
                .opt_str("snippet", has_vec.then_some("1"))
                .i64("char_start", i64::from(chunk.char_start))
                .i64("char_end", i64::from(chunk.char_end))
                .str("created_at", &now)
                .str("metadata_json", &chunk.metadata_json)
                .i64("embedding_digest", digest)
                .opt_string("parent_chunk_id", &parent_chunk_id)
                .build(),
        );
    }
    if let Err(err) = w.insert_records("chunks", records) {
        bridge.clear_staged();
        return Err(err);
    }
    Ok(())
}

/// 64-bit digest stored in `chunks.embedding_digest` (first 8 bytes of the
/// blake3 hash of the stored JSON). Any digest change marks a changed vector.
pub(crate) fn embedding_digest(embedding_json: &str) -> i64 {
    let hash = blake3::hash(embedding_json.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash.as_bytes()[..8]);
    i64::from_le_bytes(bytes)
}

pub(crate) fn embedding_digest_vec(vec: &[f32]) -> i64 {
    if let Ok(json) = serde_json::to_string(vec) {
        embedding_digest(&json)
    } else {
        let mut hasher = blake3::Hasher::new();
        for &v in vec {
            hasher.update(&v.to_le_bytes());
        }
        let hash = hasher.finalize();
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&hash.as_bytes()[..8]);
        i64::from_le_bytes(bytes)
    }
}

pub(crate) fn list_chunks_for_document_locked(
    r: &impl LinReader,
    document_id: &str,
) -> Result<Vec<Chunk>> {
    let chunk_rows = r.find_by_eq("chunks", "document_id", document_id)?;
    let mut chunks: Vec<Chunk> = chunk_rows
        .iter()
        .map(rows::chunk)
        .collect::<Result<Vec<_>>>()?;
    chunks.sort_by(|a, b| {
        a.chunk_index
            .cmp(&b.chunk_index)
            .then_with(|| a.id.cmp(&b.id))
    });
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
    w: &mut impl LinWriter,
    manifest: &EmbeddingManifest,
) -> Result<()> {
    w.upsert_by_id(
        "embedding_manifest",
        &manifest.id,
        RecBuilder::new()
            .str("id", &manifest.id)
            .str("provider", &manifest.provider)
            .str("model", &manifest.model)
            .i64("dims", i64::from(manifest.dims))
            .opt_string("base_url", &manifest.base_url)
            .opt_string("content_fingerprint", &manifest.content_fingerprint)
            .str("updated_at", format_ts(manifest.updated_at))
            .build(),
    )
}

fn refresh_document_graph_label_locked(
    w: &mut impl LinWriter,
    doc: &Document,
) -> Result<Option<String>> {
    let matches = w.find_by_eq("graph_nodes", "document_id", doc.id.as_str())?;
    let node_id = matches.first().and_then(|row| cell_opt_string(row, "id"));
    if let Some(ref nid) = node_id {
        w.update_where(
            "graph_nodes",
            lin::query::pred::eq("id", nid.as_str()),
            RecBuilder::new()
                .str("label", &doc.title)
                .str("label_key", crate::graph::normalize::label_key(&doc.title))
                .str("updated_at", format_ts(Utc::now()))
                .build(),
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

/// True when the collection exists in the `lin` catalog.
fn table_exists_locked(r: &impl LinReader, name: &str) -> bool {
    r.has_collection(name)
}

mod chunks;
mod corpus_lane;
mod documents;
mod health;
mod manifest;
pub mod ops_log;
mod revisions;
mod schema_diary;
mod wiki_index;
mod wiki_meta;

pub(crate) use self::chunks::ChunkBody;
pub use self::documents::{DocumentRef, wiki_backlink_rows};
pub(crate) use self::manifest::embedding_identity_fingerprint;
pub use self::manifest::{
    embedding_manifest_from_config, embedding_manifest_matches_config,
    embedding_migration_manifest, get_embedding_manifest_locked,
};
use self::{ops_log::*, wiki_index::*, wiki_meta::*};
pub use super::repos::{
    Chunks, Collections, CorpusLane, Documents, EmbeddingManifestRepo, GraphRepo, KgRepo,
    Maintenance, OpsLog, Recovery, ReembedProgress, SourceManifestRepo, SyncRepo, WikiIndexRepo,
};

impl Store {
    pub fn fts_generation_state(&self) -> Result<super::fts::FtsGenerationState> {
        let r = self.read()?;
        super::fts::fts_generation_state(&r)
    }
}

impl Store {
    pub fn list_reembed_pending_document_ids(
        &self,
        target_fingerprint: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let r = self.read()?;
        let done_rows =
            r.find_by_eq("reembed_progress", "target_fingerprint", target_fingerprint)?;
        let done_ids: std::collections::HashSet<String> = done_rows
            .iter()
            .filter_map(|row| cell_opt_string(row, "document_id"))
            .collect();
        let mut ids: Vec<String> = Vec::new();
        r.scan_each("documents", |row| {
            if let Some(id) = cell_str(row, "id") {
                if after.is_none_or(|a| id > a) && !done_ids.contains(id) {
                    ids.push(id.to_string());
                }
            }
            Ok(true)
        })?;
        ids.sort();
        ids.truncate(limit);
        Ok(ids)
    }
}

impl Store {
    pub fn count_reembed_pending(&self, target_fingerprint: &str) -> Result<usize> {
        let r = self.read()?;
        let done_rows =
            r.find_by_eq("reembed_progress", "target_fingerprint", target_fingerprint)?;
        let done_ids: std::collections::HashSet<String> = done_rows
            .iter()
            .filter_map(|row| cell_opt_string(row, "document_id"))
            .collect();
        let mut pending = 0usize;
        r.scan_each("documents", |row| {
            if let Some(id) = cell_str(row, "id")
                && !done_ids.contains(id)
            {
                pending += 1;
            }
            Ok(true)
        })?;
        Ok(pending)
    }
}

impl Store {
    pub fn count_reembed_done(&self, target_fingerprint: &str) -> Result<usize> {
        let r = self.read()?;
        Ok(
            r.find_by_eq("reembed_progress", "target_fingerprint", target_fingerprint)?
                .len(),
        )
    }
}

impl Store {
    pub fn mark_reembed_done(
        &self,
        target_fingerprint: &str,
        document_id: &str,
        chunks_reembedded: usize,
    ) -> Result<()> {
        let mut w = self.write()?;
        let id = composite_id_2(target_fingerprint, document_id);
        w.upsert_by_id(
            "reembed_progress",
            &id,
            RecBuilder::new()
                .str("id", &id)
                .str("target_fingerprint", target_fingerprint)
                .str("document_id", document_id)
                .i64("chunks_reembedded", chunks_reembedded as i64)
                .str("completed_at", format_ts(Utc::now()))
                .build(),
        )
    }
}

impl Store {
    pub fn clear_reembed_progress(&self, target_fingerprint: &str) -> Result<usize> {
        let mut w = self.write()?;
        w.delete_where(
            "reembed_progress",
            lin::query::pred::eq("target_fingerprint", target_fingerprint),
        )
    }
}

#[cfg(test)]
mod tests;
