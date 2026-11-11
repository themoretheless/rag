//! Document and chunk persistence over DuckDB.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use duckdb::{params, AccessMode, Config as DuckDbConfig, Connection};

use super::rows;
use super::schema;
use super::sql::ACTIVE_DOCUMENT_SQL;
use super::EmbeddingIdentity;
use crate::error::{AppError, Result};
use crate::models::{
    Chunk, DiaryEntry, Document, DocumentFilter, DocumentMetaApplyResult, DocumentMetaUpdate,
    DocumentRevisionPage, DocumentRevisionSummary, DuplicateCheckResult, DuplicateMatch,
    EmbeddingManifest, OpsLogEntry, PlacementUpdate, RoomCount, Taxonomy, TaxonomyRoom,
    TaxonomyWing, VacuumStoreReport, WikiIndexEntry, WingCount,
};
use crate::util::{
    content_hash, format_db_timestamp as format_ts, slugify as shared_slugify, wiki_slug_from_uri,
    SlugPolicy,
};

const EMBEDDING_MIGRATION_FINGERPRINT_PREFIX: &str = "migration-incomplete:";

#[cfg(any(test, feature = "testkit"))]
#[doc(hidden)]
pub const TEST_FAIL_WIKI_WRITE_AFTER_INDEX_OP: &str = "__test_fail_wiki_write_after_index__";

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
    // Schema extensions this database has had applied (shared by clones).
    schema_extensions: Arc<Mutex<std::collections::BTreeSet<&'static str>>>,
    read_only: bool,
    // Present only on an explicitly scoped clone used by a bulk workflow.
    // Clones keep the lease alive if nested work outlives its transport request.
    corpus_mutation_guard: Option<Arc<CorpusMutationGuard>>,
    // Vector search snapshots of this database (shared by clones).
    vector_cache: Arc<Mutex<super::search::VectorCache>>,
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
pub struct UncompiledShelfQueue {
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
    /// Open (or create) a DuckDB database at `path` and run schema migrations.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let conn = Connection::open(path)?;
        apply_performance_pragmas(&conn)?;
        let extensions = schema::registered_schema_extensions();
        schema::migrate_with(&conn, &extensions)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: path.to_path_buf(),
            schema_extensions: Arc::new(Mutex::new(
                extensions.iter().map(|extension| extension.name).collect(),
            )),
            read_only: false,
            corpus_mutation_lane: Arc::new(tokio::sync::RwLock::new(())),
            corpus_mutation_guard: None,
            vector_cache: Arc::default(),
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
        apply_performance_pragmas(&conn)?;
        let version = schema::current_schema_version(&conn).map_err(|error| {
            AppError::config(format!(
                "DuckDB '{}' is not a compatible rag-mcp database: {error}",
                path.display()
            ))
        })?;
        schema::refuse_newer_schema(version)?;
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
            schema_extensions: Arc::default(),
            read_only: true,
            corpus_mutation_lane: Arc::new(tokio::sync::RwLock::new(())),
            corpus_mutation_guard: None,
            vector_cache: Arc::default(),
        })
    }

    /// Vector search snapshots of this database.
    pub(crate) fn vector_cache(&self) -> &Mutex<super::search::VectorCache> {
        &self.vector_cache
    }

    /// Filesystem path of the opened database.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The raw DuckDB connection, for persistence code only.
    ///
    /// Private to `db`: code outside goes through Store methods or takes a
    /// connection from [`Self::with_conn`] / [`Self::with_tx`]. Test builds
    /// (`cfg(test)` or the `testkit` feature the crates above enable for their
    /// tests) make it public so tests can seed and inspect fixtures directly.
    #[cfg(not(any(test, feature = "testkit")))]
    pub(in crate::db) fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.lock_connection()
    }

    #[cfg(any(test, feature = "testkit"))]
    #[doc(hidden)]
    pub fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.lock_connection()
    }

    /// Run `work` with the connection under the Store lock.
    ///
    /// The connection handle for repository types outside `db` (code
    /// intelligence, analysis tasks, gate, calibration, external DB checks).
    pub fn with_conn<T>(&self, work: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut conn = self.lock_connection()?;
        work(&mut conn)
    }

    /// Run `work` in one transaction under the Store lock. Commits on `Ok`;
    /// an `Err` (or panic) drops the transaction, which rolls it back.
    pub(crate) fn with_tx<T>(
        &self,
        work: impl FnOnce(&duckdb::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let value = work(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    /// Register `extensions` process-wide and apply any this database has
    /// not had yet. A no-op on a read-only store, which never runs DDL.
    pub fn ensure_schema_extensions(&self, extensions: &[schema::SchemaExtension]) -> Result<()> {
        for extension in extensions {
            schema::register_schema_extension(*extension);
        }
        if self.read_only {
            return Ok(());
        }
        let mut applied = self
            .schema_extensions
            .lock()
            .map_err(|e| AppError::db(format!("schema extension lock poisoned: {e}")))?;
        for extension in extensions {
            if applied.contains(extension.name) {
                continue;
            }
            self.lock_connection()?.execute_batch(extension.ddl)?;
            applied.insert(extension.name);
        }
        Ok(())
    }

    fn lock_connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| AppError::db(format!("database lock poisoned: {e}")))
    }
}

/// Repository accessors: each borrows the Store and shares its connection.
impl Store {
    /// The [`Documents`] repository.
    pub fn documents(&self) -> Documents<'_> {
        Documents { store: self }
    }

    /// The [`Chunks`] repository.
    pub fn chunks(&self) -> Chunks<'_> {
        Chunks { store: self }
    }

    /// The [`GraphRepo`] repository.
    pub fn graph(&self) -> GraphRepo<'_> {
        GraphRepo { store: self }
    }

    /// The [`KgRepo`] repository.
    pub fn kg(&self) -> KgRepo<'_> {
        KgRepo { store: self }
    }

    /// The [`OpsLog`] repository.
    pub fn ops_log(&self) -> OpsLog<'_> {
        OpsLog { store: self }
    }

    /// The [`EmbeddingManifestRepo`] repository.
    pub fn embedding_manifest(&self) -> EmbeddingManifestRepo<'_> {
        EmbeddingManifestRepo { store: self }
    }

    /// The [`WikiIndexRepo`] repository.
    pub fn wiki_index(&self) -> WikiIndexRepo<'_> {
        WikiIndexRepo { store: self }
    }

    /// The [`SyncRepo`] repository.
    pub fn sync(&self) -> SyncRepo<'_> {
        SyncRepo { store: self }
    }

    /// The [`CorpusLane`] repository.
    pub fn corpus_lane(&self) -> CorpusLane<'_> {
        CorpusLane { store: self }
    }

    /// The [`Collections`] repository.
    pub fn collections(&self) -> Collections<'_> {
        Collections { store: self }
    }

    /// The [`ReembedProgress`] repository.
    pub fn reembed_progress(&self) -> ReembedProgress<'_> {
        ReembedProgress { store: self }
    }

    /// The [`Maintenance`] repository.
    pub fn maintenance(&self) -> Maintenance<'_> {
        Maintenance { store: self }
    }

    /// The [`SourceManifestRepo`] repository.
    pub fn source_manifest(&self) -> SourceManifestRepo<'_> {
        SourceManifestRepo { store: self }
    }

    /// The [`Recovery`] repository.
    pub fn recovery(&self) -> Recovery<'_> {
        Recovery { store: self }
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
        rows.next()?
            .map(crate::db::rows::wiki_index_entry)
            .transpose()?
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
            replace_chunks_locked(conn, &doc.id, chunks, embedding_json)?;
            let (node_id, edge_count) =
                super::graph::resolve::rebuild_document_graph_locked(conn, doc)?;
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
    // GRAPH_DESIGN.md §6.3: never wipe incident edges here. The node is demoted to
    // an unresolved stub and edges from other notes survive.
    super::graph::demote_graph_for_document_locked(conn, id)?;
    replace_chunks_locked(conn, id, &[], &[])?;
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
    let normalized = super::write_hook::run_document_write_hooks(conn, doc)?;
    let doc = normalized.as_ref();

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

    if current_rev.is_some() {
        let columns = DocumentColumns::of(doc);
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
                columns.hash,
                doc.wing,
                doc.room,
                doc.source_file,
                columns.layer.as_ref(),
                columns.kind,
                columns.status,
                doc.pinned,
                columns.boost,
                new_rev,
                format_ts(doc.created_at),
                format_ts(doc.updated_at),
                doc.id,
            ],
        )?;
    } else {
        insert_document_row_locked(conn, doc, new_rev)?;
    }
    Ok(new_rev)
}

/// Document columns as they are stored: an empty hash is computed from the
/// content, the layer is canonical (`raw` when blank), a blank kind/status
/// becomes `document`/`active` and a non-positive or non-finite boost `1.0`.
struct DocumentColumns<'a> {
    hash: String,
    layer: std::borrow::Cow<'a, str>,
    kind: &'a str,
    status: &'a str,
    boost: f64,
}

impl<'a> DocumentColumns<'a> {
    fn of(doc: &'a Document) -> Self {
        let hash = match doc.content_hash.as_deref() {
            Some(h) if !h.is_empty() => h.to_string(),
            _ => content_hash(&doc.content),
        };
        let layer = if doc.layer.trim().is_empty() {
            std::borrow::Cow::Borrowed("raw")
        } else {
            crate::models::Layer::canonicalize(&doc.layer)
        };
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
        let boost = if doc.boost.is_finite() && doc.boost > 0.0 {
            doc.boost
        } else {
            1.0
        };
        Self {
            hash,
            layer,
            kind,
            status,
            boost,
        }
    }
}

/// INSERT one `documents` row at `revision`, with the columns normalized by
/// [`DocumentColumns`]. Every document insert goes through here.
pub(super) fn insert_document_row_locked(
    conn: &Connection,
    doc: &Document,
    revision: i64,
) -> Result<()> {
    let columns = DocumentColumns::of(doc);
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
            columns.hash,
            doc.wing,
            doc.room,
            doc.source_file,
            columns.layer.as_ref(),
            columns.kind,
            columns.status,
            doc.pinned,
            columns.boost,
            revision,
            format_ts(doc.created_at),
            format_ts(doc.updated_at),
        ],
    )?;
    Ok(())
}

/// JSON-encode chunk embeddings for [`insert_chunks_locked`], before any lock
/// is taken so a serialization failure cannot leave a partial write. The text
/// is bound as `CAST(? AS FLOAT[])` (exact for every finite `f32`) and is
/// the input of [`embedding_digest`].
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

/// Replace every chunk of `document_id` with `chunks` (none clears them):
/// records the chunk change for FTS, deletes the old rows and inserts the new
/// ones with their prepared `embedding_json`.
pub(crate) fn replace_chunks_locked(
    conn: &Connection,
    document_id: &str,
    chunks: &[Chunk],
    embedding_json: &[String],
) -> Result<()> {
    super::fts::mark_chunks_changed_for(conn, &[document_id])?;
    conn.execute(
        "DELETE FROM chunks WHERE document_id = ?",
        params![document_id],
    )?;
    insert_chunks_locked(conn, chunks, embedding_json)
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
        "INSERT INTO chunks (id, document_id, chunk_index, content, embedding, \
         char_start, char_end, created_at, metadata_json, embedding_digest, parent_chunk_id) VALUES",
        "(?, ?, ?, ?, CAST(? AS FLOAT[]), ?, ?, CAST(? AS TIMESTAMP), ?, ?, NULLIF(json_extract_string(TRY_CAST(? AS JSON), '$.parent_chunk_id'), ''))",
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
                &chunk.metadata_json,
            ]);
        },
    )
}

/// 64-bit digest stored in `chunks.embedding_digest` (first 8 bytes of the
/// blake3 hash of the embedding's JSON encoding). Any digest change marks a changed vector.
pub(crate) fn embedding_digest(embedding_json: &str) -> i64 {
    let hash = blake3::hash(embedding_json.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash.as_bytes()[..8]);
    i64::from_le_bytes(bytes)
}

fn list_chunks_for_document_locked(conn: &Connection, document_id: &str) -> Result<Vec<Chunk>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, document_id, chunk_index, content, embedding, char_start, char_end, metadata_json
        FROM chunks
        WHERE document_id = ?
        ORDER BY chunk_index ASC, id ASC
        "#,
    )?;
    let chunks = crate::db::rows::query_vec(&mut stmt, params![document_id], rows::chunk)?;
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
                rag_mcp_markup::normalize::label_key(&doc.title),
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

mod chunks;
mod corpus_lane;
mod documents;
mod health;
mod manifest;
mod ops_log;
mod reembed;
mod revisions;
mod schema_diary;
mod ui_compat;
mod wiki_index;
mod wiki_meta;

pub(crate) use self::chunks::ChunkBody;
pub use self::chunks::Chunks;
pub use self::corpus_lane::CorpusLane;
pub use self::documents::Documents;
pub use self::documents::{wiki_backlink_rows, DocumentRef};
pub use self::health::Maintenance;
pub(crate) use self::manifest::embedding_identity_fingerprint;
pub use self::manifest::EmbeddingManifestRepo;
pub use self::manifest::{embedding_manifest_from_config, embedding_manifest_matches_config};
pub use self::manifest::{embedding_migration_manifest, get_embedding_manifest_locked};
pub use self::ops_log::OpsLog;
pub use self::reembed::ReembedProgress;
pub use self::wiki_index::WikiIndexRepo;
use self::{ops_log::*, wiki_index::*, wiki_meta::*};
pub use super::collections::Collections;
pub use super::graph::GraphRepo;
pub use super::kg::KgRepo;
pub use super::recovery::Recovery;
pub use super::source_manifest::SourceManifestRepo;
pub use super::sync::SyncRepo;

fn apply_performance_pragmas(conn: &Connection) -> Result<()> {
    if let Some(threads) = std::env::var("RAG_DB_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
    {
        conn.execute_batch(&format!("PRAGMA threads={threads}"))?;
    }
    if let Ok(limit) = std::env::var("RAG_DB_MEMORY_LIMIT") {
        let limit = limit.trim();
        // Only accept a tight shape; this string goes into a PRAGMA verbatim.
        if !limit.is_empty()
            && limit.len() <= 16
            && limit
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == ' ')
        {
            conn.execute_batch(&format!("PRAGMA memory_limit='{limit}'"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
