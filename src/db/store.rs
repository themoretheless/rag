//! Document and chunk persistence over DuckDB.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use duckdb::{params, Connection};

use super::rows;
use super::schema;
use crate::config::Config;
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
    source_sync_lane: Arc<tokio::sync::RwLock<()>>,
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

/// Aggregate layer/index health used by diagnostics without loading document bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LayerHealthCounts {
    pub raw_count: u64,
    pub wiki_count: u64,
    pub index_entry_count: u64,
    pub indexed_pages: u64,
    pub uncompiled_raw_count: u64,
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
            source_sync_lane: Arc::new(tokio::sync::RwLock::new(())),
        })
    }

    /// Filesystem path of the opened database.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Shared connection handle for sibling modules that prefer raw access.
    #[allow(dead_code)]
    pub(crate) fn conn(&self) -> &Arc<Mutex<Connection>> {
        &self.conn
    }

    pub(crate) fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| AppError::db(format!("database lock poisoned: {e}")))
    }

    /// Process-local writer lane shared by every clone that synchronizes source trees.
    ///
    /// Source synchronization owns the write side for its full run. Hybrid
    /// searches briefly own the read side, so concurrent searches remain
    /// possible while a newly-starting sync cannot race past their idle check.
    pub(crate) fn source_sync_lane(&self) -> Arc<tokio::sync::RwLock<()>> {
        self.source_sync_lane.clone()
    }

    /// Acquire the non-exclusive side of the source-sync lane without waiting.
    ///
    /// `None` means a source synchronization run is active. Keeping the guard
    /// alive prevents a new run from starting until the guarded search ends.
    pub(crate) fn try_source_sync_idle_guard(
        &self,
    ) -> Option<tokio::sync::OwnedRwLockReadGuard<()>> {
        self.source_sync_lane.clone().try_read_owned().ok()
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
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let new_rev = upsert_document_cas_locked(&tx, doc, if_match_revision)?;
        tx.commit()?;
        Ok(new_rev)
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
        self.write_document_atomic_with_manifest(doc, if_match_revision, derived, None)
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
        )
    }

    fn write_document_atomic_with_manifest(
        &self,
        doc: &Document,
        if_match_revision: Option<i64>,
        derived: DocumentDerivedWrite<'_>,
        manifest: Option<crate::db::SourceManifestWrite<'_>>,
    ) -> Result<AtomicDocumentWriteResult> {
        let prepared_embeddings = match &derived {
            DocumentDerivedWrite::ReplaceChunksAndGraph(chunks) => {
                if let Some(chunk) = chunks.iter().find(|chunk| chunk.document_id != doc.id) {
                    return Err(AppError::config(format!(
                        "chunk {} belongs to document {}, expected {}",
                        chunk.id, chunk.document_id, doc.id
                    )));
                }
                Some(
                    chunks
                        .iter()
                        .map(|chunk| serde_json::to_string(&chunk.embedding))
                        .collect::<std::result::Result<Vec<_>, _>>()?,
                )
            }
            DocumentDerivedWrite::Preserve | DocumentDerivedWrite::RefreshGraphLabel => None,
        };

        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let revision = upsert_document_cas_locked(&tx, doc, if_match_revision)?;
        let (node_id, edge_count) = match (derived, prepared_embeddings) {
            (DocumentDerivedWrite::Preserve, None) => (None, 0),
            (DocumentDerivedWrite::RefreshGraphLabel, None) => {
                (refresh_document_graph_label_locked(&tx, doc)?, 0)
            }
            (DocumentDerivedWrite::ReplaceChunksAndGraph(chunks), Some(embedding_json)) => {
                super::fts::mark_fts_dirty(&tx)?;
                tx.execute("DELETE FROM chunks WHERE document_id = ?", params![doc.id])?;
                insert_chunks_locked(&tx, chunks, &embedding_json)?;
                let (node_id, edge_count) =
                    crate::graph::resolve::rebuild_document_graph_locked(&tx, doc)?;
                (Some(node_id), edge_count)
            }
            _ => unreachable!("derived write and prepared embedding state must match"),
        };
        if let Some(manifest) = manifest {
            super::source_manifest::upsert_source_manifest_locked(&tx, manifest)?;
        }
        tx.commit()?;
        Ok(AtomicDocumentWriteResult {
            revision,
            node_id,
            edge_count,
        })
    }

    /// Historical document snapshots, newest revision first.
    pub fn list_document_revisions(&self, document_id: &str) -> Result<Vec<Document>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT document_id AS id, uri, title, content, metadata_json, content_hash, wing, room, source_file, layer, kind, CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR), COALESCE(status, 'active'), COALESCE(pinned, false), COALESCE(boost, 1.0), revision FROM document_revisions WHERE document_id = ? ORDER BY revision DESC"
        )?;
        let mut rows = stmt.query(params![document_id.trim()])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(rows::document(row)?);
        }
        Ok(out)
    }

    /// Paginated revision timeline metadata without materializing historical
    /// bodies. Use [`Self::get_document_revision`] for one selected snapshot.
    pub fn list_document_revision_summaries(
        &self,
        document_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<DocumentRevisionPage> {
        let document_id = document_id.trim();
        if document_id.is_empty() {
            return Err(AppError::config("document_id must be non-empty"));
        }
        let limit = limit.clamp(1, 200);
        let conn = self.lock()?;
        let total: i64 = conn.query_row(
            "SELECT COUNT(*)::BIGINT FROM document_revisions WHERE document_id = ?",
            params![document_id],
            |row| row.get(0),
        )?;
        let sql = format!(
            r#"
            SELECT document_id, uri, title, wing, room,
                   COALESCE(NULLIF(layer, ''), 'raw'),
                   COALESCE(NULLIF(kind, ''), 'document'),
                   COALESCE(NULLIF(status, ''), 'active'),
                   CAST(updated_at AS VARCHAR), CAST(superseded_at AS VARCHAR), revision,
                   LENGTH(content)::BIGINT,
                   CASE WHEN content = '' THEN 0
                        ELSE 1 + LENGTH(content) - LENGTH(REPLACE(content, '\n', '')) END
            FROM document_revisions
            WHERE document_id = ?
            ORDER BY revision DESC
            LIMIT {limit} OFFSET {offset}
            "#
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params![document_id])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            items.push(DocumentRevisionSummary {
                document_id: row.get(0)?,
                uri: row.get(1)?,
                title: row.get(2)?,
                wing: row.get(3)?,
                room: row.get(4)?,
                layer: row.get(5)?,
                kind: row.get(6)?,
                status: row.get(7)?,
                updated_at: row.get(8)?,
                superseded_at: row.get(9)?,
                revision: row.get(10)?,
                content_chars: row.get::<_, i64>(11)?.max(0) as u64,
                content_lines: row.get::<_, i64>(12)?.max(0) as u64,
            });
        }
        Ok(DocumentRevisionPage {
            items,
            total: total.max(0) as u64,
        })
    }

    /// Load one historical document snapshot by its original revision number.
    pub fn get_document_revision(
        &self,
        document_id: &str,
        revision: i64,
    ) -> Result<Option<Document>> {
        if revision < 1 {
            return Err(AppError::config("revision must be >= 1"));
        }
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT document_id AS id, uri, title, content, metadata_json, content_hash, wing, room, source_file, layer, kind, CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR), COALESCE(status, 'active'), COALESCE(pinned, false), COALESCE(boost, 1.0), revision FROM document_revisions WHERE document_id = ? AND revision = ?",
        )?;
        let mut rows = stmt.query(params![document_id.trim(), revision])?;
        match rows.next()? {
            Some(row) => Ok(Some(rows::document(row)?)),
            None => Ok(None),
        }
    }

    /// Insert chunk rows. Embeddings are stored as JSON float arrays in `embedding_json`.
    pub fn insert_chunks(&self, chunks: &[Chunk]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }

        let embedding_json = chunks
            .iter()
            .map(|chunk| serde_json::to_string(&chunk.embedding))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let conn = self.lock()?;
        super::fts::mark_fts_dirty(&conn)?;
        insert_chunks_locked(&conn, chunks, &embedding_json)
    }

    /// Delete a document, its chunks, and its graph node + incident edges.
    ///
    /// Returns `true` if a document row was removed.
    pub fn delete_document(&self, id: &str) -> Result<bool> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let deleted = delete_document_locked(&tx, id)?;
        tx.commit()?;
        Ok(deleted)
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
        self.delete_source_state(source)
            .map(|(documents, _)| documents)
    }

    /// Atomically remove every document/derived row and manifest ownership for
    /// one filesystem source. Returns `(documents_deleted, manifest_deleted)`.
    pub(crate) fn delete_source_state(&self, source: &str) -> Result<(u64, bool)> {
        self.delete_source_state_with(source, |_| Ok(()))
    }

    fn delete_source_state_with(
        &self,
        source: &str,
        mut after_document: impl FnMut(usize) -> Result<()>,
    ) -> Result<(u64, bool)> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let manifest_present: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM source_manifest WHERE canonical_path = ?)",
            params![source],
            |row| row.get(0),
        )?;
        let mut ids = Vec::new();
        {
            let mut stmt = tx.prepare("SELECT id FROM documents WHERE source_file = ?")?;
            let mut rows = stmt.query(params![source])?;
            while let Some(row) = rows.next()? {
                ids.push(row.get::<_, String>(0)?);
            }
        }
        for (index, id) in ids.iter().enumerate() {
            delete_document_locked(&tx, id)?;
            after_document(index + 1)?;
        }
        tx.execute(
            "DELETE FROM source_manifest WHERE canonical_path = ?",
            params![source],
        )?;
        tx.commit()?;
        Ok((ids.len() as u64, manifest_present))
    }

    /// Fetch a document by id.
    pub fn get_document(&self, id: &str) -> Result<Option<Document>> {
        let conn = self.lock()?;
        let sql = format!("SELECT {DOCUMENT_SELECT} FROM documents WHERE id = ?");
        let mut stmt = conn.prepare(&sql)?;

        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(rows::document(row)?)),
            None => Ok(None),
        }
    }

    /// Fetch a document by stable URI (re-ingest lookup).
    pub fn find_by_uri(&self, uri: &str) -> Result<Option<Document>> {
        let conn = self.lock()?;
        let sql = format!("SELECT {DOCUMENT_SELECT} FROM documents WHERE uri = ? LIMIT 1");
        let mut stmt = conn.prepare(&sql)?;

        let mut rows = stmt.query(params![uri])?;
        match rows.next()? {
            Some(row) => Ok(Some(rows::document(row)?)),
            None => Ok(None),
        }
    }

    /// Fetch the first document matching `content_hash` (dedupe probe).
    pub fn find_by_content_hash(&self, hash: &str) -> Result<Option<Document>> {
        Ok(self.list_by_content_hash(hash)?.into_iter().next())
    }

    /// All documents with the given `content_hash`, ordered by `created_at` ascending.
    pub fn list_by_content_hash(&self, hash: &str) -> Result<Vec<Document>> {
        let conn = self.lock()?;
        let sql = format!(
            "SELECT {DOCUMENT_SELECT} FROM documents WHERE content_hash = ? ORDER BY created_at ASC"
        );
        let mut stmt = conn.prepare(&sql)?;

        let mut rows = stmt.query(params![hash])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(rows::document(row)?);
        }
        Ok(out)
    }

    /// Alias of [`Self::list_by_content_hash`].
    pub fn find_all_by_content_hash(&self, hash: &str) -> Result<Vec<Document>> {
        self.list_by_content_hash(hash)
    }

    /// Content-hash / URI idempotency probe for `check_duplicate`.
    ///
    /// Provide at least one of `content`, `content_hash`, or `uri`:
    /// - `content` is hashed with [`content_hash`] when `content_hash` is empty.
    /// - Hash matches come from `documents.content_hash`.
    /// - URI match comes from exact `documents.uri` (when `uri` is non-empty).
    pub fn check_duplicate(
        &self,
        content: Option<&str>,
        content_hash_in: Option<&str>,
        uri: Option<&str>,
    ) -> Result<DuplicateCheckResult> {
        let hash = match content_hash_in.map(str::trim).filter(|s| !s.is_empty()) {
            Some(h) => Some(h.to_string()),
            None => content
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(content_hash),
        };
        let uri = uri.map(str::trim).filter(|s| !s.is_empty());

        if hash.is_none() && uri.is_none() {
            return Err(AppError::config(
                "check_duplicate requires content, content_hash, or uri",
            ));
        }

        // id -> (doc, reasons)
        let mut by_id: std::collections::BTreeMap<String, (Document, Vec<&'static str>)> =
            std::collections::BTreeMap::new();

        if let Some(ref h) = hash {
            for doc in self.list_by_content_hash(h)? {
                by_id
                    .entry(doc.id.clone())
                    .or_insert_with(|| (doc, Vec::new()))
                    .1
                    .push("content_hash");
            }
        }

        if let Some(u) = uri {
            if let Some(doc) = self.find_by_uri(u)? {
                by_id
                    .entry(doc.id.clone())
                    .or_insert_with(|| (doc, Vec::new()))
                    .1
                    .push("uri");
            }
        }

        let matches: Vec<DuplicateMatch> = by_id
            .into_values()
            .map(|(doc, reasons)| {
                let match_reason = if reasons.contains(&"content_hash") && reasons.contains(&"uri")
                {
                    "content_hash+uri".to_string()
                } else if reasons.contains(&"uri") {
                    "uri".to_string()
                } else {
                    "content_hash".to_string()
                };
                DuplicateMatch {
                    id: doc.id,
                    uri: doc.uri,
                    title: doc.title,
                    content_hash: doc.content_hash,
                    wing: doc.wing,
                    room: doc.room,
                    source_file: doc.source_file,
                    match_reason,
                }
            })
            .collect();

        Ok(DuplicateCheckResult {
            is_duplicate: !matches.is_empty(),
            content_hash: hash,
            matches,
        })
    }

    /// List all documents ordered by `created_at` ascending.
    pub fn list_documents(&self) -> Result<Vec<Document>> {
        let conn = self.lock()?;
        let sql = format!("SELECT {DOCUMENT_SELECT} FROM documents ORDER BY created_at ASC");
        let mut stmt = conn.prepare(&sql)?;

        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(rows::document(row)?);
        }
        Ok(out)
    }

    /// List documents with the given `layer` (`raw`, `wiki`, …), ordered by `created_at`.
    pub fn list_documents_by_layer(&self, layer: &str) -> Result<Vec<Document>> {
        let conn = self.lock()?;
        let sql = format!(
            "SELECT {DOCUMENT_SELECT} FROM documents WHERE layer = ? ORDER BY created_at ASC"
        );
        let mut stmt = conn.prepare(&sql)?;

        let mut rows = stmt.query(params![layer])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(rows::document(row)?);
        }
        Ok(out)
    }

    /// Wiki/catalog listing **without** loading `content` (sidebar / full catalog).
    ///
    /// Equivalent to [`Self::list_wiki_page_metas_filtered`] with empty filters
    /// (no `q` / kind / category / wing / room, no pagination).
    pub fn list_wiki_page_metas(&self) -> Result<Vec<crate::models::WikiPageListItem>> {
        self.list_wiki_page_metas_filtered(&WikiPageMetaFilter::default())
            .map(|(items, _total)| items)
    }

    /// Wiki catalog with optional text query, placement/kind filters, and pagination.
    ///
    /// Used by HTTP `GET /v1/wiki?q=&limit=&offset=&kind=&category=&wing=&room=`.
    ///
    /// - `q`: case-insensitive substring on title, slug, uri, summary, category, kind
    /// - `kind` / `category` / `wing` / `room`: exact match when non-empty (category from metadata)
    /// - `offset` / `limit`: applied after filters; `limit` omitted returns all matches
    ///
    /// Returns `(page_items, total)` where `total` is the filtered count before pagination.
    ///
    /// Orchestrates: SQL fetch + DTO map, then post-filter, then page (see free helpers below).
    pub fn list_wiki_page_metas_filtered(
        &self,
        filter: &WikiPageMetaFilter,
    ) -> Result<(Vec<crate::models::WikiPageListItem>, usize)> {
        let conn = self.lock()?;
        let mut items = fetch_wiki_page_meta_items(&conn, filter)?;
        drop(conn);
        post_filter_wiki_page_metas(&mut items, filter);
        Ok(page_wiki_page_metas(items, filter.offset, filter.limit))
    }

    /// Incoming wikilink sources for a document (label, document_id or node id).
    pub fn wiki_backlinks_for_document(&self, document_id: &str) -> Result<Vec<(String, String)>> {
        let node = match self.find_node_by_document_id(document_id)? {
            Some(n) => n,
            None => return Ok(Vec::new()),
        };
        let bl = self.backlinks(&node.id)?;
        let mut out = Vec::new();
        for e in &bl.edges {
            if e.target_id != node.id || e.rel_type != "wikilink" {
                continue;
            }
            let Some(src) = bl.nodes.iter().find(|n| n.id == e.source_id) else {
                continue;
            };
            let key = src.document_id.clone().unwrap_or_else(|| src.id.clone());
            out.push((src.label.clone(), key));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.dedup_by(|a, b| a.1 == b.1);
        Ok(out)
    }

    /// List documents in the given `wing`, ordered by `created_at`.
    pub fn list_documents_by_wing(&self, wing: &str) -> Result<Vec<Document>> {
        let conn = self.lock()?;
        let sql = format!(
            "SELECT {DOCUMENT_SELECT} FROM documents WHERE wing = ? ORDER BY created_at ASC"
        );
        let mut stmt = conn.prepare(&sql)?;

        let mut rows = stmt.query(params![wing])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(rows::document(row)?);
        }
        Ok(out)
    }

    /// List documents matching optional wing / room / status / layer / kind filters.
    ///
    /// When `status` is unset and `include_archived` is not true, rows with
    /// status `archived` or `tombstone` are excluded.
    ///
    /// Order: pinned DESC, boost DESC, updated_at DESC, title ASC.
    pub fn list_documents_filtered(&self, filter: &DocumentFilter) -> Result<Vec<Document>> {
        let mut sql = format!("SELECT {DOCUMENT_SELECT} FROM documents WHERE 1=1");
        let mut binds: Vec<String> = Vec::new();

        if let Some(ref w) = filter.wing {
            if !w.is_empty() {
                sql.push_str(" AND wing = ?");
                binds.push(w.clone());
            }
        }
        if let Some(ref r) = filter.room {
            if !r.is_empty() {
                sql.push_str(" AND room = ?");
                binds.push(r.clone());
            }
        }
        if let Some(ref s) = filter.status {
            if !s.is_empty() {
                sql.push_str(" AND COALESCE(status, 'active') = ?");
                binds.push(s.clone());
            }
        } else if !filter.include_archived.unwrap_or(false) {
            sql.push_str(" AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone')");
        }
        if let Some(ref layer) = filter.layer {
            if !layer.is_empty() {
                sql.push_str(" AND layer = ?");
                binds.push(layer.clone());
            }
        }
        if let Some(ref kind) = filter.kind {
            if !kind.is_empty() {
                sql.push_str(" AND kind = ?");
                binds.push(kind.clone());
            }
        }
        if let Some(ref src) = filter.source_file {
            if !src.is_empty() {
                sql.push_str(" AND source_file = ?");
                binds.push(src.clone());
            }
        }

        sql.push_str(
            " ORDER BY COALESCE(pinned, false) DESC, COALESCE(boost, 1.0) DESC, updated_at DESC, title ASC",
        );

        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {}", limit.max(1)));
        }

        let conn = self.lock()?;
        let mut stmt = conn.prepare(&sql)?;
        let params_dyn: Vec<&dyn duckdb::types::ToSql> = binds
            .iter()
            .map(|s| s as &dyn duckdb::types::ToSql)
            .collect();
        let mut rows = stmt.query(params_dyn.as_slice())?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(rows::document(row)?);
        }
        Ok(out)
    }

    /// Distinct wings with document counts (non-null, non-empty wing), sorted by name.
    pub fn list_wings(&self) -> Result<Vec<WingCount>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT wing, COUNT(*)::BIGINT
            FROM documents
            WHERE wing IS NOT NULL AND TRIM(wing) <> ''
            GROUP BY wing
            ORDER BY wing ASC
            "#,
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let count: i64 = row.get(1)?;
            out.push(WingCount {
                wing: row.get(0)?,
                document_count: count as u64,
            });
        }
        Ok(out)
    }

    /// First-class project catalog backed by the compatible `wing` storage key.
    pub fn list_projects(&self) -> Result<Vec<crate::models::ProjectSummary>> {
        self.get_taxonomy()?
            .wings
            .into_iter()
            .map(|wing| {
                Ok(crate::models::ProjectSummary {
                    project_id: crate::models::ProjectId::parse(wing.wing)?,
                    document_count: wing.document_count,
                    rooms: wing.rooms,
                })
            })
            .collect()
    }

    /// Distinct rooms with document counts.
    ///
    /// When `wing` is `Some`, only rooms under that wing are returned (with wing set).
    /// When `None`, returns all rooms globally (wing column may vary per row).
    pub fn list_rooms(&self, wing: Option<&str>) -> Result<Vec<RoomCount>> {
        let conn = self.lock()?;
        if let Some(w) = wing {
            let mut stmt = conn.prepare(
                r#"
                SELECT wing, room, COUNT(*)::BIGINT
                FROM documents
                WHERE wing = ?
                  AND room IS NOT NULL AND TRIM(room) <> ''
                GROUP BY wing, room
                ORDER BY room ASC
                "#,
            )?;
            let mut rows = stmt.query(params![w])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                let count: i64 = row.get(2)?;
                let wing_name: String = row.get(0)?;
                out.push(RoomCount {
                    wing: Some(wing_name),
                    room: row.get(1)?,
                    document_count: count as u64,
                });
            }
            Ok(out)
        } else {
            let mut stmt = conn.prepare(
                r#"
                SELECT wing, room, COUNT(*)::BIGINT
                FROM documents
                WHERE room IS NOT NULL AND TRIM(room) <> ''
                GROUP BY wing, room
                ORDER BY wing ASC NULLS LAST, room ASC
                "#,
            )?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                let count: i64 = row.get(2)?;
                let wing: Option<String> = row.get(0)?;
                out.push(RoomCount {
                    wing: wing.filter(|s| !s.is_empty()),
                    room: row.get(1)?,
                    document_count: count as u64,
                });
            }
            Ok(out)
        }
    }

    /// Full taxonomy tree: wings → rooms with document counts, plus unscoped totals.
    pub fn get_taxonomy(&self) -> Result<Taxonomy> {
        let wings_flat = self.list_wings()?;
        let rooms_flat = self.list_rooms(None)?;

        let mut by_wing: std::collections::BTreeMap<String, Vec<TaxonomyRoom>> =
            std::collections::BTreeMap::new();
        for r in rooms_flat {
            if let Some(w) = r.wing.as_ref().filter(|s| !s.is_empty()) {
                by_wing.entry(w.clone()).or_default().push(TaxonomyRoom {
                    room: r.room,
                    document_count: r.document_count,
                });
            }
        }

        let mut wings = Vec::with_capacity(wings_flat.len());
        for w in wings_flat {
            let rooms = by_wing.remove(&w.wing).unwrap_or_default();
            wings.push(TaxonomyWing {
                wing: w.wing,
                document_count: w.document_count,
                rooms,
            });
        }

        let conn = self.lock()?;
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))?;
        let unscoped: i64 = conn.query_row(
            r#"
            SELECT COUNT(*) FROM documents
            WHERE wing IS NULL OR TRIM(wing) = ''
            "#,
            [],
            |r| r.get(0),
        )?;

        Ok(Taxonomy {
            wings,
            unscoped_count: unscoped as u64,
            total_documents: total as u64,
        })
    }

    /// Apply a partial metadata update without re-embedding unless body text changes.
    ///
    /// Fields that are `None` in `update` are left unchanged. Empty string for
    /// `wing` / `room` / `source_file` clears the column (NULL).
    ///
    /// When `content` is set and differs from the stored body, the text and
    /// `content_hash` are updated and `content_changed` is true — the **caller**
    /// must re-chunk and re-embed. Meta-only updates leave chunks untouched.
    ///
    /// Returns `None` if `id` is missing.
    pub fn update_document_meta(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        let Some(mut applied) = self.prepare_document_meta_update(id, update)? else {
            return Ok(None);
        };
        let expected_revision = applied.document.revision;
        applied.document.revision =
            self.upsert_document_cas(&applied.document, Some(expected_revision))?;
        Ok(Some(applied))
    }

    /// Prepare a metadata/body update without mutating storage.
    ///
    /// Ingestion uses this seam to finish any required chunking/embedding before
    /// passing the prepared document to [`Self::write_document_atomic`].
    pub(crate) fn prepare_document_meta_update(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        self.get_document(id)?
            .map(|doc| apply_document_meta_update(doc, update))
            .transpose()
    }

    /// Placement-only alias for [`Self::update_document_meta`] (returns document only).
    pub fn update_document_placement(
        &self,
        id: &str,
        update: &PlacementUpdate,
    ) -> Result<Option<Document>> {
        Ok(self.update_document_meta(id, update)?.map(|r| r.document))
    }

    /// List chunks for a document ordered by `chunk_index`.
    pub fn list_chunks_for_document(&self, doc_id: &str) -> Result<Vec<Chunk>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, document_id, chunk_index, content, embedding_json, char_start, char_end, metadata_json
            FROM chunks
            WHERE document_id = ?
            ORDER BY chunk_index ASC
            "#,
        )?;

        let mut rows = stmt.query(params![doc_id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(rows::chunk(row)?);
        }
        Ok(out)
    }

    /// Load every chunk with its embedding (for in-process vector search).
    pub fn all_chunks_with_embeddings(&self) -> Result<Vec<Chunk>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, document_id, chunk_index, content, embedding_json, char_start, char_end, metadata_json
            FROM chunks
            ORDER BY document_id ASC, chunk_index ASC
            "#,
        )?;

        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(rows::chunk(row)?);
        }
        Ok(out)
    }

    /// Counts: `(documents, chunks, graph_nodes, graph_edges)`.
    pub fn stats(&self) -> Result<(u64, u64, u64, u64)> {
        let conn = self.lock()?;
        let docs: i64 = conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))?;
        let chunks: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
        let nodes: i64 = conn.query_row("SELECT COUNT(*) FROM graph_nodes", [], |r| r.get(0))?;
        let edges: i64 = conn.query_row("SELECT COUNT(*) FROM graph_edges", [], |r| r.get(0))?;
        Ok((docs as u64, chunks as u64, nodes as u64, edges as u64))
    }

    /// Count raw/wiki compilation health in SQL without materializing document bodies.
    pub(crate) fn layer_health_counts(&self) -> Result<LayerHealthCounts> {
        let conn = self.lock()?;
        let counts = conn.query_row(
            r#"
            WITH layer_counts AS (
              SELECT
                COALESCE(SUM(CASE WHEN layer = 'raw' THEN 1 ELSE 0 END), 0)::BIGINT AS raw_count,
                COALESCE(SUM(CASE WHEN layer = 'wiki' THEN 1 ELSE 0 END), 0)::BIGINT AS wiki_count
              FROM documents
            )
            SELECT
              layer_counts.raw_count,
              layer_counts.wiki_count,
              (SELECT COUNT(*)::BIGINT FROM wiki_index) AS index_entry_count,
              (
                SELECT COUNT(*)::BIGINT
                FROM documents wiki
                WHERE wiki.layer = 'wiki'
                  AND EXISTS (
                    SELECT 1
                    FROM wiki_index entry
                    WHERE COALESCE(entry.page_id, entry.document_id) = wiki.id
                  )
              ) AS indexed_pages,
              CASE
                WHEN layer_counts.wiki_count = 0 THEN layer_counts.raw_count
                ELSE (
                  SELECT COUNT(*)::BIGINT
                  FROM documents raw
                  WHERE raw.layer = 'raw'
                    AND NOT EXISTS (
                      SELECT 1
                      FROM graph_edges edge
                      JOIN graph_nodes source ON source.id = edge.source_id
                      WHERE edge.target_id = (
                        SELECT target.id
                        FROM graph_nodes target
                        WHERE target.document_id = raw.id
                        LIMIT 1
                      )
                        AND source.id <> edge.target_id
                        AND source.resolved
                        AND starts_with(source.uri, 'wiki://')
                    )
                )
              END AS uncompiled_raw_count
            FROM layer_counts
            "#,
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;
        let count = |value: i64| value.max(0) as u64;
        Ok(LayerHealthCounts {
            raw_count: count(counts.0),
            wiki_count: count(counts.1),
            index_entry_count: count(counts.2),
            indexed_pages: count(counts.3),
            uncompiled_raw_count: count(counts.4),
        })
    }

    /// Filesystem size of the main DuckDB file, when readable.
    pub fn db_file_size_bytes(&self) -> Option<u64> {
        std::fs::metadata(&self.path).ok().map(|m| m.len())
    }

    pub fn wal_file_size_bytes(&self) -> u64 {
        let wal = PathBuf::from(format!("{}.wal", self.path.display()));
        std::fs::metadata(wal).map(|m| m.len()).unwrap_or(0)
    }

    pub fn checkpoint(&self) -> Result<()> {
        self.lock()?.execute_batch("CHECKPOINT")?;
        Ok(())
    }

    /// Counts for relationships that DuckDB does not enforce with foreign keys.
    pub fn integrity_counts(&self) -> Result<(u64, u64, u64, u64, u64)> {
        let conn = self.lock()?;
        let scalar = |sql: &str| -> Result<u64> {
            let count: i64 = conn.query_row(sql, [], |row| row.get(0))?;
            Ok(count.max(0) as u64)
        };
        let documents_without_chunks = scalar(
            "SELECT COUNT(*) FROM documents d WHERE COALESCE(d.layer, '') <> 'schema' \
             AND length(trim(COALESCE(d.content, ''))) > 0 AND NOT EXISTS \
             (SELECT 1 FROM chunks c WHERE c.document_id = d.id)",
        )?;
        let orphan_chunks = scalar(
            "SELECT COUNT(*) FROM chunks c WHERE NOT EXISTS \
             (SELECT 1 FROM documents d WHERE d.id = c.document_id)",
        )?;
        let orphan_document_nodes = scalar(
            "SELECT COUNT(*) FROM graph_nodes n WHERE n.document_id IS NOT NULL AND NOT EXISTS \
             (SELECT 1 FROM documents d WHERE d.id = n.document_id)",
        )?;
        let orphan_edges = scalar(
            "SELECT COUNT(*) FROM graph_edges e WHERE NOT EXISTS \
             (SELECT 1 FROM graph_nodes n WHERE n.id = e.source_id) OR NOT EXISTS \
             (SELECT 1 FROM graph_nodes n WHERE n.id = e.target_id)",
        )?;
        let unscoped_documents =
            scalar("SELECT COUNT(*) FROM documents WHERE wing IS NULL OR trim(wing) = ''")?;
        Ok((
            documents_without_chunks,
            orphan_chunks,
            orphan_document_nodes,
            orphan_edges,
            unscoped_documents,
        ))
    }

    /// Remove rows whose referenced parent/endpoints no longer exist.
    /// Returns the counts observed before pruning: chunks, document nodes, edges.
    pub fn prune_orphans(&self, dry_run: bool) -> Result<(u64, u64, u64)> {
        let (_, chunks, nodes, edges, _) = self.integrity_counts()?;
        if dry_run || (chunks == 0 && nodes == 0 && edges == 0) {
            return Ok((chunks, nodes, edges));
        }
        let conn = self.lock()?;
        if chunks > 0 {
            super::fts::mark_fts_dirty(&conn)?;
        }
        conn.execute_batch(
            "BEGIN TRANSACTION;
             DELETE FROM graph_edges WHERE NOT EXISTS
               (SELECT 1 FROM graph_nodes n WHERE n.id = graph_edges.source_id)
               OR NOT EXISTS
               (SELECT 1 FROM graph_nodes n WHERE n.id = graph_edges.target_id);
             DELETE FROM graph_nodes WHERE document_id IS NOT NULL AND NOT EXISTS
               (SELECT 1 FROM documents d WHERE d.id = graph_nodes.document_id);
             DELETE FROM chunks WHERE NOT EXISTS
               (SELECT 1 FROM documents d WHERE d.id = chunks.document_id);
             COMMIT;",
        )?;
        Ok((chunks, nodes, edges))
    }

    /// Run DuckDB `CHECKPOINT` and report main-file size before/after when possible.
    ///
    /// Safe L0 maintenance: flushes the WAL into the main database file. Does not
    /// delete rows. Appends an `ops_log` entry (`op=vacuum_store`).
    pub fn vacuum_store(&self) -> Result<VacuumStoreReport> {
        let db_path = self.path.display().to_string();
        let bytes_before = self.db_file_size_bytes();

        {
            let conn = self.lock()?;
            conn.execute_batch("CHECKPOINT")?;
        }

        let bytes_after = self.db_file_size_bytes();
        let bytes_delta = match (bytes_before, bytes_after) {
            (Some(b), Some(a)) => Some(a as i64 - b as i64),
            _ => None,
        };

        let report = VacuumStoreReport {
            db_path,
            bytes_before,
            bytes_after,
            bytes_delta,
            checkpointed: true,
        };

        let payload = serde_json::json!({
            "bytes_before": report.bytes_before,
            "bytes_after": report.bytes_after,
            "bytes_delta": report.bytes_delta,
            "checkpointed": report.checkpointed,
        });
        let _ = self.append_ops_log(&OpsLogEntry {
            id: String::new(),
            seq: 0,
            ts: Utc::now(),
            op: "vacuum_store".into(),
            prefix: Some("MAINT".into()),
            message: "DuckDB CHECKPOINT".into(),
            entity_id: None,
            entity_kind: Some("store".into()),
            payload_json: payload.to_string(),
            agent_name: None,
        });

        Ok(report)
    }

    /// Highest applied schema version, if recorded.
    pub fn schema_version(&self) -> Result<Option<i32>> {
        let conn = self.lock()?;
        schema::current_schema_version(&conn)
    }

    /// Whether DuckDB FTS index schema `fts_main_chunks` is present.
    ///
    /// False under term-frequency fallback (lex still works via Rust TF).
    /// Does not INSTALL/LOAD the extension; observational only for status/doctor.
    pub fn fts_ready(&self) -> Result<bool> {
        let conn = self.lock()?;
        Ok(super::fts::fts_index_present(&conn))
    }

    /// Last-known FTS backend from `meta`, if `ensure_fts` has run.
    pub fn fts_status(&self) -> Result<Option<super::fts::FtsState>> {
        let conn = self.lock()?;
        super::fts::fts_status(&conn)
    }

    /// Ensure lexical search is ready (DuckDB BM25 or term-frequency fallback).
    ///
    /// Uses `stemmer` (e.g. `config.fts_stemmer` / `RAG_FTS_STEMMER`). Call after
    /// [`Store::open`] on server start so lex/hybrid search has a backend before
    /// the first MCP tool request.
    pub fn ensure_fts(&self, stemmer: &str) -> Result<super::fts::FtsState> {
        let conn = self.lock()?;
        super::fts::ensure_fts_with_stemmer(&conn, stemmer)
    }

    /// Load the embedding corpus fingerprint (typically id `"default"`).
    pub fn get_embedding_manifest(&self) -> Result<Option<EmbeddingManifest>> {
        self.get_embedding_manifest_by_id("default")
    }

    /// Load embedding manifest by primary key.
    pub fn get_embedding_manifest_by_id(&self, id: &str) -> Result<Option<EmbeddingManifest>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, provider, model, dims, base_url, content_fingerprint,
                   CAST(updated_at AS VARCHAR)
            FROM embedding_manifest
            WHERE id = ?
            "#,
        )?;

        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(rows::embedding_manifest(row)?)),
            None => Ok(None),
        }
    }

    /// Insert or replace the embedding corpus fingerprint.
    pub fn set_embedding_manifest(&self, manifest: &EmbeddingManifest) -> Result<()> {
        let conn = self.lock()?;
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

    /// Record the default embedding manifest from config if none is stored yet.
    ///
    /// Does **not** overwrite an existing row (preserves corpus fingerprint so
    /// dim/model drift can be detected). Call on server start and before first ingest.
    pub fn ensure_embedding_manifest(&self, config: &Config) -> Result<EmbeddingManifest> {
        if let Some(existing) = self.get_embedding_manifest()? {
            return Ok(existing);
        }
        let manifest = embedding_manifest_from_config(config);
        self.set_embedding_manifest(&manifest)?;
        tracing::info!(
            provider = %manifest.provider,
            model = %manifest.model,
            dims = manifest.dims,
            "recorded embedding_manifest"
        );
        Ok(manifest)
    }

    /// Overwrite the default embedding manifest to match live config (e.g. after reembed).
    pub fn write_embedding_manifest_from_config(
        &self,
        config: &Config,
    ) -> Result<EmbeddingManifest> {
        let manifest = embedding_manifest_from_config(config);
        self.set_embedding_manifest(&manifest)?;
        Ok(manifest)
    }

    /// Refuse vec/hybrid search (and ingest of new vectors) when corpus dims differ
    /// from the live config. No stored manifest means no check (empty or pre-manifest DB).
    pub fn require_embedding_dims_match(&self, config_dims: usize) -> Result<()> {
        let Some(manifest) = self.get_embedding_manifest()? else {
            return Ok(());
        };
        let expected = config_dims as i32;
        if manifest.dims == expected {
            return Ok(());
        }
        Err(AppError::embeddings(format!(
            "embedding dimension mismatch: corpus has dims={} (provider={}, model={}), \
             config has dims={}. Call reembed_document for each document after changing \
             RAG_EMBEDDING_DIMS (or embedding model), then verify with get_embedding_manifest.",
            manifest.dims, manifest.provider, manifest.model, expected
        )))
    }

    /// Atomically replace all chunks for a document, preserving caller-supplied ids.
    pub fn replace_chunks_for_document(&self, document_id: &str, chunks: &[Chunk]) -> Result<()> {
        let embedding_json = chunks
            .iter()
            .map(|chunk| serde_json::to_string(&chunk.embedding))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        super::fts::mark_fts_dirty(&tx)?;
        tx.execute(
            "DELETE FROM chunks WHERE document_id = ?",
            params![document_id],
        )?;
        insert_chunks_locked(&tx, chunks, &embedding_json)?;
        tx.commit()?;
        Ok(())
    }

    // --- Karpathy wiki: ops_log + wiki_index + layer helpers ---

    /// Next ops_log sequence number (max+1, starting at 1).
    ///
    /// Prefer letting [`Self::append_ops_log`] assign `seq` under the same lock.
    pub fn next_ops_seq(&self) -> Result<i64> {
        let conn = self.lock()?;
        next_ops_seq_locked(&conn)
    }

    /// Append an ops_log row (append-only). Fills `id`, `seq`, `ts`, and
    /// `payload_json` defaults when missing. Requires non-empty `op`.
    pub fn append_ops_log(&self, entry: &OpsLogEntry) -> Result<OpsLogEntry> {
        let op = entry.op.trim();
        if op.is_empty() {
            return Err(AppError::config("ops_log op must be non-empty"));
        }
        let mut out = entry.clone();
        out.op = op.to_string();
        if out.id.is_empty() {
            out.id = uuid::Uuid::new_v4().to_string();
        }
        if out.ts.timestamp() == 0 {
            out.ts = Utc::now();
        }
        if out.payload_json.trim().is_empty() {
            out.payload_json = "{}".into();
        }
        if let Some(ref p) = out.prefix {
            let t = p.trim();
            out.prefix = if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            };
        }

        let conn = self.lock()?;
        if out.seq <= 0 {
            out.seq = next_ops_seq_locked(&conn)?;
        }
        conn.execute(
            r#"
            INSERT INTO ops_log
              (id, seq, ts, op, prefix, message, entity_id, entity_kind, payload_json, agent_name)
            VALUES
              (?, ?, CAST(? AS TIMESTAMP), ?, ?, ?, ?, ?, ?, ?)
            "#,
            params![
                out.id,
                out.seq,
                format_ts(out.ts),
                out.op,
                out.prefix,
                out.message,
                out.entity_id,
                out.entity_kind,
                out.payload_json,
                out.agent_name,
            ],
        )?;
        Ok(out)
    }

    /// Whether the `ops_log` table exists (false on pre-migration / partial DBs).
    pub fn ops_log_exists(&self) -> Result<bool> {
        let conn = self.lock()?;
        Ok(table_exists_locked(&conn, "ops_log"))
    }

    /// Recent ops_log rows, newest first (`seq` DESC, then `ts` DESC).
    ///
    /// Returns an empty list when `ops_log` is missing (no error).
    pub fn list_ops_log(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        self.list_recent_ops(limit)
    }

    /// Recent ops_log rows, newest first. Alias of [`Self::list_ops_log`].
    ///
    /// Returns an empty list when `ops_log` is missing (no error).
    pub fn list_recent_ops(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        let limit = limit.max(1) as i64;
        let conn = self.lock()?;
        if !table_exists_locked(&conn, "ops_log") {
            return Ok(Vec::new());
        }
        let mut stmt = conn.prepare(
            r#"
            SELECT id, COALESCE(seq, 0), CAST(ts AS VARCHAR), op,
                   prefix, COALESCE(message, ''), entity_id, entity_kind,
                   COALESCE(payload_json, '{}'), agent_name
            FROM ops_log
            ORDER BY COALESCE(seq, 0) DESC, ts DESC
            LIMIT ?
            "#,
        )?;
        let mut rows = stmt.query(params![limit])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_ops_log(row)?);
        }
        Ok(out)
    }

    /// Recent memory-filing ops (ingest / drawer / wiki / diary / checkpoint).
    ///
    /// Used by MCP `memories_filed_away`. Empty when `ops_log` is missing or no
    /// matching rows. Newest first.
    pub fn memories_filed_away(&self, limit: usize) -> Result<Vec<OpsLogEntry>> {
        let limit = limit.max(1) as i64;
        let conn = self.lock()?;
        if !table_exists_locked(&conn, "ops_log") {
            return Ok(Vec::new());
        }
        // Filing-related ops: ingest family, drawers, wiki compile, diary, checkpoint.
        let mut stmt = conn.prepare(
            r#"
            SELECT id, COALESCE(seq, 0), CAST(ts AS VARCHAR), op,
                   prefix, COALESCE(message, ''), entity_id, entity_kind,
                   COALESCE(payload_json, '{}'), agent_name
            FROM ops_log
            WHERE lower(op) IN (
                'ingest', 'ingest_text', 'ingest_file', 'ingest_raw', 'add_drawer',
                'file_answer', 'wiki_write', 'write_wiki_page', 'update_wiki_page',
                'diary_write', 'checkpoint', 'compile_source'
            )
            OR lower(COALESCE(prefix, '')) IN (
                'ingest', 'drawer', 'wiki', 'diary', 'checkpoint', 'raw'
            )
            ORDER BY COALESCE(seq, 0) DESC, ts DESC
            LIMIT ?
            "#,
        )?;
        let mut rows = stmt.query(params![limit])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_ops_log(row)?);
        }
        Ok(out)
    }

    /// Read one ops_log row by primary key `id`.
    pub fn get_ops_log(&self, id: &str) -> Result<Option<OpsLogEntry>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, COALESCE(seq, 0), CAST(ts AS VARCHAR), op,
                   prefix, COALESCE(message, ''), entity_id, entity_kind,
                   COALESCE(payload_json, '{}'), agent_name
            FROM ops_log WHERE id = ?
            "#,
        )?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_ops_log(row)?)),
            None => Ok(None),
        }
    }

    /// Read one ops_log row by monotonic `seq`.
    pub fn get_ops_log_by_seq(&self, seq: i64) -> Result<Option<OpsLogEntry>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, COALESCE(seq, 0), CAST(ts AS VARCHAR), op,
                   prefix, COALESCE(message, ''), entity_id, entity_kind,
                   COALESCE(payload_json, '{}'), agent_name
            FROM ops_log WHERE seq = ?
            LIMIT 1
            "#,
        )?;
        let mut rows = stmt.query(params![seq])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_ops_log(row)?)),
            None => Ok(None),
        }
    }

    /// Read ops_log: by `id`, by `seq`, or recent list when both absent.
    ///
    /// When listing, `limit` defaults to 50 and rows are newest-first.
    pub fn read_ops_log(
        &self,
        id: Option<&str>,
        seq: Option<i64>,
        limit: Option<usize>,
    ) -> Result<Vec<OpsLogEntry>> {
        if let Some(id) = id.map(str::trim).filter(|s| !s.is_empty()) {
            return Ok(self.get_ops_log(id)?.into_iter().collect());
        }
        if let Some(seq) = seq {
            return Ok(self.get_ops_log_by_seq(seq)?.into_iter().collect());
        }
        self.list_recent_ops(limit.unwrap_or(50))
    }

    /// Upsert a wiki index catalog entry (by primary key `id` or by `slug`).
    pub fn upsert_wiki_index_entry(&self, entry: &WikiIndexEntry) -> Result<()> {
        let title = if entry.title.is_empty() {
            entry.slug.clone()
        } else {
            entry.title.clone()
        };
        let label = title.clone();
        let page_id = entry.page_id.clone();
        let document_id = page_id.clone();
        let conn = self.lock()?;
        conn.execute(
            r#"
            INSERT OR REPLACE INTO wiki_index
              (id, slug, title, label, kind, summary, category, document_id, page_id, updated_at)
            VALUES
              (?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))
            "#,
            params![
                entry.id,
                entry.slug,
                title,
                label,
                entry.kind,
                entry.summary,
                entry.category,
                document_id,
                page_id,
                format_ts(entry.updated_at),
            ],
        )?;
        Ok(())
    }

    /// List all wiki index entries (catalog).
    pub fn list_wiki_index(&self) -> Result<Vec<WikiIndexEntry>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id,
                   COALESCE(slug, COALESCE(label, id)),
                   COALESCE(title, COALESCE(label, id)),
                   COALESCE(kind, 'wiki'),
                   category, summary,
                   COALESCE(page_id, document_id),
                   CAST(updated_at AS VARCHAR)
            FROM wiki_index
            ORDER BY COALESCE(title, label, id) ASC
            "#,
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_wiki_index(row)?);
        }
        Ok(out)
    }

    /// Find wiki index entry by slug.
    pub fn get_wiki_index_by_slug(&self, slug: &str) -> Result<Option<WikiIndexEntry>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id,
                   COALESCE(slug, COALESCE(label, id)),
                   COALESCE(title, COALESCE(label, id)),
                   COALESCE(kind, 'wiki'),
                   category, summary,
                   COALESCE(page_id, document_id),
                   CAST(updated_at AS VARCHAR)
            FROM wiki_index
            WHERE slug = ? OR label = ? OR id = ?
            LIMIT 1
            "#,
        )?;
        let mut rows = stmt.query(params![slug, slug, slug])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_wiki_index(row)?)),
            None => Ok(None),
        }
    }

    /// Delete wiki index entry by id.
    pub fn delete_wiki_index_entry(&self, id: &str) -> Result<bool> {
        let conn = self.lock()?;
        let n = conn.execute("DELETE FROM wiki_index WHERE id = ?", params![id])?;
        Ok(n > 0)
    }

    /// Clear and rebuild wiki_index from all `layer=wiki` documents.
    ///
    /// One catalog row per wiki page with a one-line summary taken from content
    /// (or `metadata_json.summary` when present). Category from
    /// `metadata_json.category` when set.
    pub fn rebuild_wiki_index_from_docs(&self) -> Result<usize> {
        let docs = self.list_documents_by_layer("wiki")?;
        {
            let conn = self.lock()?;
            conn.execute("DELETE FROM wiki_index", [])?;
        }
        let now = Utc::now();
        let mut n = 0usize;
        for doc in docs {
            let slug = wiki_slug_from_uri(&doc.uri).unwrap_or_else(|| slugify(&doc.title));
            let (meta_summary, meta_category) = meta_summary_category(&doc.metadata_json);
            let summary = meta_summary.unwrap_or_else(|| first_line_summary(&doc.content, 240));
            let entry = WikiIndexEntry {
                id: format!("idx-{}", doc.id),
                slug: slug.clone(),
                title: doc.title.clone(),
                kind: if doc.kind.is_empty() {
                    "wiki".into()
                } else {
                    doc.kind.clone()
                },
                category: meta_category,
                summary: Some(summary),
                page_id: Some(doc.id.clone()),
                updated_at: now,
            };
            self.upsert_wiki_index_entry(&entry)?;
            n += 1;
        }
        Ok(n)
    }

    /// Create or merge-update a catalog entry keyed by `slug`.
    ///
    /// Empty optional fields leave existing values unchanged. On insert, missing
    /// title defaults to slug; kind defaults to `wiki`.
    pub fn update_wiki_index_entry_fields(
        &self,
        slug: &str,
        title: Option<String>,
        summary: Option<String>,
        kind: Option<String>,
        category: Option<String>,
        page_id: Option<String>,
    ) -> Result<WikiIndexEntry> {
        let slug_clean = slug.trim();
        if slug_clean.is_empty() {
            return Err(AppError::config(
                "update_index_entry: slug must be non-empty",
            ));
        }
        let slug_key = slugify(slug_clean);
        if slug_key.is_empty() {
            return Err(AppError::config(
                "update_index_entry: slug is empty after normalize",
            ));
        }

        let now = Utc::now();
        let mut entry = if let Some(e) = self.get_wiki_index_by_slug(&slug_key)? {
            e
        } else if let Some(e) = self.get_wiki_index_by_slug(slug_clean)? {
            e
        } else {
            WikiIndexEntry {
                id: format!("idx-slug-{}", slug_key),
                slug: slug_key.clone(),
                title: slug_key.clone(),
                kind: "wiki".into(),
                category: None,
                summary: None,
                page_id: None,
                updated_at: now,
            }
        };

        entry.slug = slug_key;
        if let Some(t) = title {
            let t = t.trim();
            if !t.is_empty() {
                entry.title = t.to_string();
            }
        }
        if let Some(s) = summary {
            entry.summary = Some(s);
        }
        if let Some(k) = kind {
            let k = k.trim();
            if !k.is_empty() {
                entry.kind = k.to_string();
            }
        }
        if let Some(c) = category {
            let c = c.trim();
            entry.category = if c.is_empty() {
                None
            } else {
                Some(c.to_string())
            };
        }
        if let Some(p) = page_id {
            let p = p.trim();
            entry.page_id = if p.is_empty() {
                None
            } else {
                Some(p.to_string())
            };
        }
        entry.updated_at = now;
        self.upsert_wiki_index_entry(&entry)?;
        Ok(entry)
    }

    /// Render the wiki index catalog as markdown (Karpathy `index.md` analogue).
    pub fn render_wiki_index_markdown(&self) -> Result<String> {
        let entries = self.list_wiki_index()?;
        let mut out = String::from("# Wiki index\n\n");
        if entries.is_empty() {
            out.push_str("_empty catalog_\n");
            return Ok(out);
        }
        let mut by_cat: std::collections::BTreeMap<String, Vec<&WikiIndexEntry>> =
            std::collections::BTreeMap::new();
        for e in &entries {
            let cat = e.category.clone().unwrap_or_else(|| "uncategorized".into());
            by_cat.entry(cat).or_default().push(e);
        }
        for (cat, rows) in by_cat {
            out.push_str(&format!("## {cat}\n\n"));
            for e in rows {
                let sum = e.summary.as_deref().unwrap_or("");
                out.push_str(&format!(
                    "- [[{}]] (`{}`, {}) — {}\n",
                    e.title, e.slug, e.kind, sum
                ));
            }
            out.push('\n');
        }
        Ok(out)
    }

    /// Schema document (`uri=schema://agents`) if present.
    ///
    /// Prefer exact URI match; fall back to the first `layer=schema` document.
    pub fn get_schema_document(&self) -> Result<Option<Document>> {
        if let Some(d) = self.find_by_uri("schema://agents")? {
            return Ok(Some(d));
        }
        // Fallback: first layer=schema (legacy / non-canonical uri)
        let list = self.list_documents_by_layer("schema")?;
        Ok(list.into_iter().next())
    }

    /// Default wing for an agent diary: `agents/<agent_name>`.
    pub fn diary_wing_for_agent(agent_name: &str) -> String {
        format!("agents/{}", agent_name.trim())
    }

    /// List recent diary documents (`kind=diary`) for an agent, newest first.
    ///
    /// Matches rows where `room = agent_name` or `wing = agents/<agent_name>`.
    /// When `agent_name` is empty/`None`, returns recent diary across all agents.
    pub fn list_diary_entries(
        &self,
        agent_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>> {
        let limit = limit.max(1);
        let agent = agent_name.map(str::trim).filter(|s| !s.is_empty());

        let conn = self.lock()?;
        let mut out = Vec::new();
        if let Some(name) = agent {
            let wing = Self::diary_wing_for_agent(name);
            let sql = format!(
                "SELECT {DOCUMENT_SELECT} FROM documents \
                 WHERE kind = 'diary' \
                   AND (room = ? OR wing = ?) \
                   AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone') \
                 ORDER BY created_at DESC \
                 LIMIT {limit}"
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(params![name, wing])?;
            while let Some(row) = rows.next()? {
                let doc = rows::document(row)?;
                out.push(DiaryEntry::from_document(&doc));
            }
        } else {
            let sql = format!(
                "SELECT {DOCUMENT_SELECT} FROM documents \
                 WHERE kind = 'diary' \
                   AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone') \
                 ORDER BY created_at DESC \
                 LIMIT {limit}"
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let doc = rows::document(row)?;
                out.push(DiaryEntry::from_document(&doc));
            }
        }
        Ok(out)
    }

    /// List pinned active documents (for `wake_up`), newest / highest boost first.
    ///
    /// Order: boost DESC, updated_at DESC, title ASC. Excludes archived/tombstone.
    pub fn list_pinned_documents(&self, limit: usize) -> Result<Vec<Document>> {
        let limit = limit.max(1);
        let conn = self.lock()?;
        let sql = format!(
            "SELECT {DOCUMENT_SELECT} FROM documents \
             WHERE COALESCE(pinned, false) = true \
               AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone') \
             ORDER BY COALESCE(boost, 1.0) DESC, updated_at DESC, title ASC \
             LIMIT {limit}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(rows::document(row)?);
        }
        Ok(out)
    }

    /// Index-first catalog lookup: rank `wiki_index` rows by simple term overlap
    /// against slug / title / summary / category / kind (case-insensitive).
    ///
    /// Empty `query` returns catalog entries in title order (score 1.0), limited
    /// by `limit`. Used by the `query_with_index` MCP tool.
    pub fn query_wiki_index(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::models::IndexQueryMatch>> {
        let limit = limit.max(1);
        let entries = self.list_wiki_index()?;
        let terms = tokenize_index_query(query);
        if terms.is_empty() {
            return Ok(entries
                .into_iter()
                .take(limit)
                .map(|entry| crate::models::IndexQueryMatch { entry, score: 1.0 })
                .collect());
        }

        let mut scored: Vec<(f32, WikiIndexEntry)> = entries
            .into_iter()
            .filter_map(|entry| {
                let score = score_index_entry(&entry, &terms);
                if score > 0.0 {
                    Some((score, entry))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.title.cmp(&b.1.title))
        });
        Ok(scored
            .into_iter()
            .take(limit)
            .map(|(score, entry)| crate::models::IndexQueryMatch { entry, score })
            .collect())
    }
}

fn apply_document_meta_update(
    mut doc: Document,
    update: &DocumentMetaUpdate,
) -> Result<DocumentMetaApplyResult> {
    let mut content_changed = false;
    let mut title_changed = false;
    let persisted_raw = doc.layer.eq_ignore_ascii_case("raw");

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
            .is_some_and(|layer| !layer.eq_ignore_ascii_case("raw"))
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
            doc.layer = if layer.eq_ignore_ascii_case("raw") {
                "raw".to_string()
            } else {
                layer.to_string()
            };
        }
    }
    if let Some(ref kind) = update.kind {
        let kind = kind.trim();
        if !kind.is_empty() {
            doc.kind = kind.to_string();
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
    conn.execute(
        r#"
        DELETE FROM graph_edges
        WHERE source_id IN (SELECT id FROM graph_nodes WHERE document_id = ?)
           OR target_id IN (SELECT id FROM graph_nodes WHERE document_id = ?)
        "#,
        params![id, id],
    )?;
    conn.execute("DELETE FROM graph_nodes WHERE document_id = ?", params![id])?;
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
    let layer = if doc.layer.is_empty() {
        "raw"
    } else {
        doc.layer.as_str()
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

fn insert_chunks_locked(
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
    let mut stmt = conn.prepare(
        r#"
        INSERT INTO chunks
          (id, document_id, chunk_index, content, embedding_json, char_start, char_end, created_at, metadata_json)
        VALUES
          (?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), ?)
        "#,
    )?;
    for (chunk, embedding_json) in chunks.iter().zip(embedding_json) {
        stmt.execute(params![
            chunk.id,
            chunk.document_id,
            chunk.chunk_index,
            chunk.content,
            embedding_json,
            chunk.char_start,
            chunk.char_end,
            now.as_str(),
            chunk.metadata_json,
        ])?;
    }
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
            "UPDATE graph_nodes SET label = ?, updated_at = CAST(? AS TIMESTAMP) WHERE id = ?",
            params![doc.title, format_ts(Utc::now()), node_id],
        )?;
    }
    Ok(node_id)
}

/// Whitespace / punctuation tokenization for index query matching.
fn tokenize_index_query(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

/// Field-weighted catalog score so top_k discriminates (slug > title > summary > kind).
///
/// Whole-token match (same tokenizer as query). Exact phrase on slug/title boosts hard.
fn score_index_entry(entry: &WikiIndexEntry, terms: &[String]) -> f32 {
    if terms.is_empty() {
        return 1.0;
    }
    let slug_tokens = tokenize_index_query(&entry.slug);
    let title_tokens = tokenize_index_query(&entry.title);
    let summary_tokens = tokenize_index_query(entry.summary.as_deref().unwrap_or(""));
    let kind_tokens = tokenize_index_query(&entry.kind);
    let cat_tokens = tokenize_index_query(entry.category.as_deref().unwrap_or(""));

    let slug_lc = entry.slug.to_ascii_lowercase();
    let title_lc = entry.title.to_ascii_lowercase();
    let summary_lc = entry.summary.as_deref().unwrap_or("").to_ascii_lowercase();

    let mut score = 0.0f32;
    let mut terms_hit = 0u32;
    for t in terms {
        let mut hit = false;
        if slug_tokens.iter().any(|h| h == t) {
            score += 4.0;
            hit = true;
        }
        if title_tokens.iter().any(|h| h == t) {
            score += 3.0;
            hit = true;
        }
        if summary_tokens.iter().any(|h| h == t) {
            score += 1.5;
            hit = true;
        }
        if kind_tokens.iter().any(|h| h == t) || cat_tokens.iter().any(|h| h == t) {
            score += 0.5;
            hit = true;
        }
        // Substring on slug/title only (e.g. "spine" in "spine-tools")
        if !hit {
            if slug_lc.contains(t.as_str()) {
                score += 2.0;
                hit = true;
            } else if title_lc.contains(t.as_str()) {
                score += 1.5;
                hit = true;
            } else if summary_lc.contains(t.as_str()) {
                score += 0.75;
                hit = true;
            }
        }
        if hit {
            terms_hit += 1;
        }
    }
    if terms_hit == 0 {
        return 0.0;
    }

    // Average per query term so longer queries do not dominate by raw sum alone.
    score /= terms.len() as f32;

    // Full-phrase identity / containment boosts ranking head.
    let phrase = terms.join(" ");
    let slug_phrase = slug_tokens.join(" ");
    let title_phrase = title_tokens.join(" ");
    if slug_phrase == phrase || title_phrase == phrase {
        score += 2.0;
    } else if slug_lc.contains(&phrase) || title_lc.contains(&phrase) {
        score += 1.0;
    }
    // Prefer covering all query terms.
    if terms_hit as usize == terms.len() {
        score += 0.5;
    }
    score
}

fn slugify(s: &str) -> String {
    shared_slugify(s, SlugPolicy::IndexLookup)
}

fn first_line_summary(content: &str, max: usize) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .or_else(|| content.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("");
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let mut s: String = line.chars().take(max.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

// ---------------------------------------------------------------------------
// Wiki catalog listing: SQL fetch + DTO map + post-filter / page
// (kept free of search ranking and CAS upsert paths)
// ---------------------------------------------------------------------------

/// SQL equality filters (kind/wing/room) + row map to [`crate::models::WikiPageListItem`].
///
/// SELECT intentionally omits `content` so list stays lean (ISP vs full [`Document`]).
fn fetch_wiki_page_meta_items(
    conn: &Connection,
    filter: &WikiPageMetaFilter,
) -> Result<Vec<crate::models::WikiPageListItem>> {
    let mut sql = String::from(
        r#"
            SELECT id, uri, title, kind,
                   metadata_json,
                   COALESCE(revision, 1),
                   CAST(updated_at AS VARCHAR)
            FROM documents
            WHERE layer = 'wiki'
            "#,
    );
    let mut binds: Vec<String> = Vec::new();

    if let Some(ref kind) = filter.kind {
        let kind = kind.trim();
        if !kind.is_empty() {
            // Match stored kind; empty DB kind is projected as "wiki" in the DTO map.
            if kind == "wiki" {
                sql.push_str(" AND (COALESCE(kind, '') = '' OR kind = ?)");
            } else {
                sql.push_str(" AND kind = ?");
            }
            binds.push(kind.to_string());
        }
    }
    if let Some(ref wing) = filter.wing {
        let wing = wing.trim();
        if !wing.is_empty() {
            sql.push_str(" AND wing = ?");
            binds.push(wing.to_string());
        }
    }
    if let Some(ref room) = filter.room {
        let room = room.trim();
        if !room.is_empty() {
            sql.push_str(" AND room = ?");
            binds.push(room.to_string());
        }
    }

    sql.push_str(" ORDER BY title ASC");

    let mut stmt = conn.prepare(&sql)?;
    let params_dyn: Vec<&dyn duckdb::types::ToSql> = binds
        .iter()
        .map(|s| s as &dyn duckdb::types::ToSql)
        .collect();
    let mut rows = stmt.query(params_dyn.as_slice())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(map_wiki_page_meta_row(row)?);
    }
    Ok(out)
}

/// Project one catalog SELECT row into lean [`crate::models::WikiPageListItem`] (no body).
fn map_wiki_page_meta_row(row: &duckdb::Row<'_>) -> Result<crate::models::WikiPageListItem> {
    let id: String = row.get(0)?;
    let uri: String = row.get(1)?;
    let title: String = row.get(2)?;
    let kind: String = row
        .get::<_, Option<String>>(3)?
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "wiki".into());
    let metadata_json: String = row.get(4)?;
    let revision: i64 = row.get::<_, Option<i64>>(5)?.unwrap_or(1);
    let updated_raw: String = row.get(6)?;
    let updated_at = parse_ts(&updated_raw)?;
    let slug = uri
        .strip_prefix("wiki://")
        .unwrap_or(uri.as_str())
        .to_string();
    let (summary, category) = meta_summary_category(&metadata_json);
    Ok(crate::models::WikiPageListItem {
        id,
        uri,
        slug,
        title,
        kind,
        summary,
        category,
        revision,
        etag: crate::models::format_document_etag(revision),
        updated_at: updated_at.to_rfc3339(),
    })
}

/// Category (metadata) and free-text `q` filters applied after DTO projection.
fn post_filter_wiki_page_metas(
    items: &mut Vec<crate::models::WikiPageListItem>,
    filter: &WikiPageMetaFilter,
) {
    if let Some(ref cat) = filter.category {
        let cat = cat.trim();
        if !cat.is_empty() {
            let cat_lc = cat.to_ascii_lowercase();
            items.retain(|item| {
                item.category
                    .as_deref()
                    .map(|c| c.eq_ignore_ascii_case(&cat_lc))
                    .unwrap_or(false)
            });
        }
    }

    if let Some(ref q) = filter.q {
        let q = q.trim();
        if !q.is_empty() {
            let q_lc = q.to_ascii_lowercase();
            items.retain(|item| wiki_meta_matches_q(item, &q_lc));
        }
    }
}

/// Slice filtered catalog items; returns `(page, total_before_pagination)`.
fn page_wiki_page_metas(
    items: Vec<crate::models::WikiPageListItem>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> (Vec<crate::models::WikiPageListItem>, usize) {
    let total = items.len();
    let offset = offset.unwrap_or(0).min(total);
    let mut page: Vec<_> = items.into_iter().skip(offset).collect();
    if let Some(limit) = limit {
        let limit = limit.max(1);
        if page.len() > limit {
            page.truncate(limit);
        }
    }
    (page, total)
}

/// Case-insensitive substring match for wiki catalog free-text `q`.
fn wiki_meta_matches_q(item: &crate::models::WikiPageListItem, q_lc: &str) -> bool {
    let hit = |s: &str| s.to_ascii_lowercase().contains(q_lc);
    hit(&item.title)
        || hit(&item.slug)
        || hit(&item.uri)
        || hit(&item.kind)
        || item.summary.as_deref().map(hit).unwrap_or(false)
        || item.category.as_deref().map(hit).unwrap_or(false)
}

/// Pull optional `summary` / `category` strings from document metadata JSON.
fn meta_summary_category(metadata_json: &str) -> (Option<String>, Option<String>) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(metadata_json) else {
        return (None, None);
    };
    let summary = v
        .get("summary")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let category = v
        .get("category")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    (summary, category)
}

fn next_ops_seq_locked(conn: &Connection) -> Result<i64> {
    let max: Option<i64> = conn
        .query_row("SELECT MAX(seq) FROM ops_log", [], |r| r.get(0))
        .unwrap_or(None);
    Ok(max.unwrap_or(0) + 1)
}

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

fn row_to_ops_log(row: &duckdb::Row<'_>) -> Result<OpsLogEntry> {
    let ts_raw: String = row.get(2)?;
    Ok(OpsLogEntry {
        id: row.get(0)?,
        seq: row.get(1)?,
        ts: parse_ts(&ts_raw)?,
        op: row.get(3)?,
        prefix: row.get(4)?,
        message: row.get(5)?,
        entity_id: row.get(6)?,
        entity_kind: row.get(7)?,
        payload_json: row.get(8)?,
        agent_name: row.get(9)?,
    })
}

fn row_to_wiki_index(row: &duckdb::Row<'_>) -> Result<WikiIndexEntry> {
    let updated_raw: String = row.get(7)?;
    Ok(WikiIndexEntry {
        id: row.get(0)?,
        slug: row.get(1)?,
        title: row.get(2)?,
        kind: row.get(3)?,
        category: row.get(4)?,
        summary: row.get(5)?,
        page_id: row.get(6)?,
        updated_at: parse_ts(&updated_raw)?,
    })
}

/// Build a default-id [`EmbeddingManifest`] snapshot from runtime config.
pub fn embedding_manifest_from_config(config: &Config) -> EmbeddingManifest {
    let provider = config.embedding_provider.as_str().to_string();
    let fingerprint = content_hash(&format!(
        "{}|{}|{}|{}",
        provider, config.embedding_model, config.embedding_dims, config.embedding_base_url
    ));
    EmbeddingManifest {
        id: "default".into(),
        provider,
        model: config.embedding_model.clone(),
        dims: config.embedding_dims as i32,
        base_url: Some(config.embedding_base_url.clone()),
        content_fingerprint: Some(fingerprint),
        updated_at: Utc::now(),
    }
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    parse_db_timestamp(s)
        .ok_or_else(|| AppError::db(format!("invalid timestamp value: {}", s.trim())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn open_temp() -> Store {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.duckdb");
        // Keep tempdir alive by leaking for test process lifetime.
        std::mem::forget(dir);
        Store::open(&path).expect("open store")
    }

    fn sample_doc(id: &str, uri: &str) -> Document {
        let now = Utc::now();
        Document {
            id: id.into(),
            uri: uri.into(),
            title: "Title".into(),
            content: "Hello world".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        }
    }

    #[test]
    fn upsert_get_find_list_delete_and_stats() {
        let store = open_temp();
        let doc = sample_doc("d1", "file://a.md");
        store.upsert_document(&doc).unwrap();

        let got = store.get_document("d1").unwrap().expect("doc");
        assert_eq!(got.uri, "file://a.md");
        assert_eq!(got.content, "Hello world");
        assert_eq!(got.layer, "raw");
        assert_eq!(got.kind, "document");
        assert_eq!(
            got.content_hash.as_deref(),
            Some(content_hash("Hello world").as_str())
        );

        let by_uri = store.find_by_uri("file://a.md").unwrap().expect("by uri");
        assert_eq!(by_uri.id, "d1");

        let listed = store.list_documents().unwrap();
        assert_eq!(listed.len(), 1);

        let chunk = Chunk {
            id: "c1".into(),
            document_id: "d1".into(),
            chunk_index: 0,
            content: "Hello".into(),
            embedding: vec![0.1, 0.2, 0.3],
            char_start: 0,
            char_end: 5,
            metadata_json: "{}".into(),
        };
        store.insert_chunks(&[chunk]).unwrap();

        let chunks = store.list_chunks_for_document("d1").unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].embedding, vec![0.1, 0.2, 0.3]);

        let all = store.all_chunks_with_embeddings().unwrap();
        assert_eq!(all.len(), 1);

        let (docs, ch, nodes, edges) = store.stats().unwrap();
        assert_eq!(docs, 1);
        assert_eq!(ch, 1);
        assert_eq!(nodes, 0);
        assert_eq!(edges, 0);

        let mut revised = got;
        revised.content = "Hello revision two".into();
        revised.updated_at = Utc::now();
        assert_eq!(store.upsert_document_cas(&revised, Some(1)).unwrap(), 2);
        assert_eq!(store.list_document_revisions("d1").unwrap().len(), 1);

        assert!(store.delete_document("d1").unwrap());
        assert!(store.get_document("d1").unwrap().is_none());
        assert!(store.list_chunks_for_document("d1").unwrap().is_empty());
        assert!(store.list_document_revisions("d1").unwrap().is_empty());
        assert!(!store.delete_document("d1").unwrap());
    }

    #[test]
    fn repeated_indexed_document_updates_survive_checkpoint_and_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("art-regression.duckdb");
        {
            let store = Store::open(&path).expect("open store");
            let mut doc = sample_doc("stable-id", "file://project/README.md");
            for revision in 0..50 {
                doc.title = format!("Title {revision}");
                doc.content = format!("content revision {revision}");
                doc.content_hash = Some(content_hash(&doc.content));
                doc.wing = Some(format!("project-{}", revision % 3));
                doc.room = Some(format!("room-{}", revision % 5));
                doc.updated_at = Utc::now();
                store.upsert_document(&doc).expect("indexed UPDATE");
            }
            store.checkpoint().expect("checkpoint");
        }

        let reopened = Store::open(&path).expect("reopen after repeated updates");
        let doc = reopened
            .get_document("stable-id")
            .unwrap()
            .expect("document");
        assert_eq!(doc.title, "Title 49");
        assert_eq!(doc.content, "content revision 49");
        assert_eq!(doc.revision, 50);
        assert_eq!(reopened.integrity_counts().unwrap().1, 0);
    }

    #[test]
    fn atomic_document_write_rolls_back_document_chunks_and_graph_on_graph_fault() {
        let store = open_temp();
        let mut original = sample_doc("atomic", "wiki://atomic");
        original.title = "Original title".into();
        original.content = "Original body links to [[Old target]].".into();
        original.layer = "wiki".into();
        original.kind = "wiki".into();
        original.content_hash = Some(content_hash(&original.content));
        let original_chunk = Chunk {
            id: "old-chunk".into(),
            document_id: original.id.clone(),
            chunk_index: 0,
            content: original.content.clone(),
            embedding: vec![0.1, 0.2],
            char_start: 0,
            char_end: original.content.chars().count() as i32,
            metadata_json: "{}".into(),
        };
        let initial = store
            .write_document_atomic(
                &original,
                None,
                DocumentDerivedWrite::ReplaceChunksAndGraph(std::slice::from_ref(&original_chunk)),
            )
            .expect("initial atomic write");
        assert_eq!(initial.revision, 1);
        let generation_after_initial = {
            let conn = store.lock().unwrap();
            crate::db::fts::chunks_generation(&conn).unwrap()
        };
        assert_eq!(generation_after_initial, 1);
        let node_id = initial.node_id.expect("document node");
        let original_edges: Vec<_> = store
            .list_graph_edges()
            .unwrap()
            .into_iter()
            .filter(|edge| edge.source_id == node_id)
            .collect();
        assert_eq!(original_edges.len(), 1);

        // The unique key is a test-only fault injector: the rebuilt graph gets
        // as far as its second identical edge, after document/chunk mutations.
        store
            .lock()
            .unwrap()
            .execute(
                "CREATE UNIQUE INDEX rollback_edge_key ON graph_edges(source_id, target_id, rel_type)",
                [],
            )
            .expect("fault index");

        let mut updated = original.clone();
        updated.title = "Updated title".into();
        updated.content = "Updated body [[Repeated target]] and [[Repeated target]].".into();
        updated.content_hash = Some(content_hash(&updated.content));
        updated.updated_at = Utc::now();
        let replacement_chunk = Chunk {
            id: "new-chunk".into(),
            document_id: updated.id.clone(),
            chunk_index: 0,
            content: updated.content.clone(),
            embedding: vec![0.3, 0.4],
            char_start: 0,
            char_end: updated.content.chars().count() as i32,
            metadata_json: "{}".into(),
        };
        let error = store
            .write_document_atomic(
                &updated,
                Some(initial.revision),
                DocumentDerivedWrite::ReplaceChunksAndGraph(std::slice::from_ref(
                    &replacement_chunk,
                )),
            )
            .expect_err("duplicate graph edge must fail the transaction");
        assert!(
            error.to_string().contains("duplicate")
                || error.to_string().contains("constraint")
                || error.to_string().contains("unique"),
            "unexpected fault: {error}"
        );

        let persisted = store.get_document(&original.id).unwrap().expect("document");
        assert_eq!(persisted.revision, initial.revision);
        assert_eq!(persisted.title, original.title);
        assert_eq!(persisted.content, original.content);
        let chunks = store.list_chunks_for_document(&original.id).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].id, original_chunk.id);
        assert_eq!(chunks[0].content, original_chunk.content);
        let node = store
            .find_node_by_document_id(&original.id)
            .unwrap()
            .expect("document node after rollback");
        assert_eq!(node.id, node_id);
        assert_eq!(node.label, original.title);
        let edges: Vec<_> = store
            .list_graph_edges()
            .unwrap()
            .into_iter()
            .filter(|edge| edge.source_id == node_id)
            .collect();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].id, original_edges[0].id);
        assert_eq!(edges[0].target_id, original_edges[0].target_id);
        assert_eq!(edges[0].rel_type, original_edges[0].rel_type);
        assert!(store
            .find_nodes_by_label("Repeated target")
            .unwrap()
            .is_empty());
        assert!(store
            .list_document_revisions(&original.id)
            .unwrap()
            .is_empty());
        let generation_after_rollback = {
            let conn = store.lock().unwrap();
            crate::db::fts::chunks_generation(&conn).unwrap()
        };
        assert_eq!(generation_after_rollback, generation_after_initial);
    }

    #[test]
    fn content_hash_find_and_scope_list_delete_by_source() {
        let store = open_temp();
        let now = Utc::now();
        let text = "scoped body";
        let hash = content_hash(text);

        let doc = Document {
            id: "d1".into(),
            uri: "file://vault/a.md".into(),
            title: "A".into(),
            content: text.into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            wing: Some("projects".into()),
            room: Some("alpha".into()),
            source_file: Some("/vault/a.md".into()),
            layer: "raw".into(),
            kind: "document".into(),
            content_hash: None,
            ..Default::default()
        };
        store.upsert_document(&doc).unwrap();

        let wiki = Document {
            id: "d2".into(),
            uri: "wiki://page".into(),
            title: "Wiki".into(),
            content: "compiled".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            wing: Some("projects".into()),
            room: Some("beta".into()),
            source_file: Some("/vault/a.md".into()),
            layer: "wiki".into(),
            kind: "wiki".into(),
            content_hash: None,
            ..Default::default()
        };
        store.upsert_document(&wiki).unwrap();

        let other = Document {
            id: "d3".into(),
            uri: "file://other.md".into(),
            title: "Other".into(),
            content: "other body".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            wing: Some("archive".into()),
            source_file: Some("/vault/other.md".into()),
            ..Default::default()
        };
        store.upsert_document(&other).unwrap();

        let by_hash = store
            .find_by_content_hash(&hash)
            .unwrap()
            .expect("hash hit");
        assert_eq!(by_hash.id, "d1");

        let dup = store.check_duplicate(Some(text), None, None).unwrap();
        assert!(dup.is_duplicate);
        assert_eq!(dup.content_hash.as_deref(), Some(hash.as_str()));
        assert_eq!(dup.matches.len(), 1);
        assert_eq!(dup.matches[0].id, "d1");
        assert_eq!(dup.matches[0].match_reason, "content_hash");

        let dup_uri = store
            .check_duplicate(None, None, Some("file://vault/a.md"))
            .unwrap();
        assert!(dup_uri.is_duplicate);
        assert_eq!(dup_uri.matches[0].match_reason, "uri");

        let dup_both = store
            .check_duplicate(Some(text), None, Some("file://vault/a.md"))
            .unwrap();
        assert!(dup_both.is_duplicate);
        assert_eq!(dup_both.matches[0].match_reason, "content_hash+uri");

        let no_dup = store
            .check_duplicate(Some("totally new text"), None, None)
            .unwrap();
        assert!(!no_dup.is_duplicate);
        assert!(no_dup.matches.is_empty());

        assert!(store.check_duplicate(None, None, None).is_err());

        let raw = store.list_documents_by_layer("raw").unwrap();
        assert_eq!(raw.len(), 2);
        assert!(raw.iter().all(|d| d.layer == "raw"));

        let wiki_list = store.list_documents_by_layer("wiki").unwrap();
        assert_eq!(wiki_list.len(), 1);
        assert_eq!(wiki_list[0].id, "d2");

        let wing = store.list_documents_by_wing("projects").unwrap();
        assert_eq!(wing.len(), 2);

        for id in ["d1", "d2"] {
            store
                .insert_chunks(&[Chunk {
                    id: format!("chunk-{id}"),
                    document_id: id.into(),
                    chunk_index: 0,
                    content: "derived".into(),
                    embedding: vec![1.0, 0.0],
                    char_start: 0,
                    char_end: 7,
                    metadata_json: "{}".into(),
                }])
                .unwrap();
        }
        store
            .upsert_source_manifest(crate::db::SourceManifestWrite {
                canonical_path: "/vault/a.md",
                canonical_root: "/vault",
                size_bytes: 11,
                mtime_ns: 7,
                content_hash: &hash,
                document_id: "d1",
            })
            .unwrap();
        store
            .upsert_wiki_index_entry(&WikiIndexEntry {
                id: "idx-d2".into(),
                slug: "page".into(),
                title: "Wiki".into(),
                kind: "wiki".into(),
                category: None,
                summary: None,
                page_id: Some("d2".into()),
                updated_at: now,
            })
            .unwrap();

        let deleted = store.delete_by_source("/vault/a.md").unwrap();
        assert_eq!(deleted, 2);
        assert!(store.get_document("d1").unwrap().is_none());
        assert!(store.get_document("d2").unwrap().is_none());
        assert!(store.get_document("d3").unwrap().is_some());
        assert!(store.list_chunks_for_document("d1").unwrap().is_empty());
        assert!(store.list_chunks_for_document("d2").unwrap().is_empty());
        assert!(store.get_wiki_index_by_slug("page").unwrap().is_none());
        let remaining = store
            .load_source_manifest_root(Path::new("/vault"))
            .unwrap();
        assert!(!remaining.contains_key("/vault/a.md"));
        assert!(remaining.contains_key("/vault/other.md"));
        assert_eq!(store.delete_by_source("/vault/a.md").unwrap(), 0);
    }

    #[test]
    fn source_delete_fault_rolls_back_documents_chunks_graph_and_manifest() {
        let store = open_temp();
        let source = "/vault/atomic.md";
        for id in ["atomic-a", "atomic-b"] {
            let mut document = sample_doc(id, &format!("file:///{id}.md"));
            document.title = format!("Document {id}");
            document.source_file = Some(source.into());
            document.content = format!("body for {id}");
            let chunk = Chunk {
                id: format!("chunk-{id}"),
                document_id: id.into(),
                chunk_index: 0,
                content: document.content.clone(),
                embedding: vec![1.0, 0.0],
                char_start: 0,
                char_end: document.content.chars().count() as i32,
                metadata_json: "{}".into(),
            };
            store
                .write_document_atomic(
                    &document,
                    None,
                    DocumentDerivedWrite::ReplaceChunksAndGraph(std::slice::from_ref(&chunk)),
                )
                .unwrap();
        }
        store
            .upsert_source_manifest(crate::db::SourceManifestWrite {
                canonical_path: source,
                canonical_root: "/vault",
                size_bytes: 20,
                mtime_ns: 9,
                content_hash: "manifest-hash",
                document_id: "atomic-a",
            })
            .unwrap();

        let error = store
            .delete_source_state_with(source, |deleted| {
                if deleted == 1 {
                    Err(AppError::db("injected source delete failure"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(error.to_string().contains("injected source delete failure"));
        for id in ["atomic-a", "atomic-b"] {
            assert!(store.get_document(id).unwrap().is_some());
            assert_eq!(store.list_chunks_for_document(id).unwrap().len(), 1);
            assert!(store.find_node_by_document_id(id).unwrap().is_some());
        }
        assert!(store
            .load_source_manifest_root(Path::new("/vault"))
            .unwrap()
            .contains_key(source));

        assert_eq!(store.delete_source_state(source).unwrap(), (2, true));
        for id in ["atomic-a", "atomic-b"] {
            assert!(store.get_document(id).unwrap().is_none());
            assert!(store.list_chunks_for_document(id).unwrap().is_empty());
            assert!(store.find_node_by_document_id(id).unwrap().is_none());
        }
        assert!(!store
            .load_source_manifest_root(Path::new("/vault"))
            .unwrap()
            .contains_key(source));
    }

    #[test]
    fn taxonomy_placement_update_and_filtered_list() {
        let store = open_temp();
        let now = Utc::now();

        for (id, wing, room, status, pinned, boost) in [
            ("a", "research", "rag", "active", true, 2.0),
            ("b", "research", "rag", "active", false, 1.0),
            ("c", "research", "eval", "archived", false, 1.0),
            ("d", "ops", "runbooks", "active", false, 1.5),
            ("e", "", "", "active", false, 1.0),
        ] {
            let wing = if wing.is_empty() {
                None
            } else {
                Some(wing.into())
            };
            let room = if room.is_empty() {
                None
            } else {
                Some(room.into())
            };
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("doc://{id}"),
                    title: id.into(),
                    content: format!("body {id}"),
                    metadata_json: "{}".into(),
                    created_at: now,
                    updated_at: now,
                    wing,
                    room,
                    status: status.into(),
                    pinned,
                    boost,
                    ..Default::default()
                })
                .unwrap();
        }

        let wings = store.list_wings().unwrap();
        assert_eq!(wings.len(), 2);
        assert_eq!(wings[0].wing, "ops");
        assert_eq!(wings[0].document_count, 1);
        assert_eq!(wings[1].wing, "research");
        assert_eq!(wings[1].document_count, 3);

        let rooms = store.list_rooms(Some("research")).unwrap();
        assert_eq!(rooms.len(), 2);
        assert!(rooms
            .iter()
            .any(|r| r.room == "rag" && r.document_count == 2));
        assert!(rooms
            .iter()
            .any(|r| r.room == "eval" && r.document_count == 1));

        let tax = store.get_taxonomy().unwrap();
        assert_eq!(tax.total_documents, 5);
        assert_eq!(tax.unscoped_count, 1);
        assert_eq!(tax.wings.len(), 2);
        let research = tax.wings.iter().find(|w| w.wing == "research").unwrap();
        assert_eq!(research.document_count, 3);
        assert_eq!(research.rooms.len(), 2);

        let filtered = store
            .list_documents_filtered(&DocumentFilter {
                wing: Some("research".into()),
                room: Some("rag".into()),
                status: Some("active".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].id, "a"); // pinned first
        assert_eq!(filtered[1].id, "b");

        let archived = store
            .list_documents_filtered(&DocumentFilter {
                status: Some("archived".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, "c");

        // Default list excludes archived (include_archived unset / false).
        let default_list = store
            .list_documents_filtered(&DocumentFilter {
                wing: Some("research".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(default_list.len(), 2);
        assert!(default_list.iter().all(|d| d.id == "a" || d.id == "b"));

        let with_arch = store
            .list_documents_filtered(&DocumentFilter {
                wing: Some("research".into()),
                include_archived: Some(true),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(with_arch.len(), 3);

        let updated = store
            .update_document_placement(
                "b",
                &PlacementUpdate {
                    wing: Some("ops".into()),
                    room: Some("runbooks".into()),
                    status: Some("draft".into()),
                    ..Default::default()
                },
            )
            .unwrap()
            .expect("updated");
        assert_eq!(updated.wing.as_deref(), Some("ops"));
        assert_eq!(updated.room.as_deref(), Some("runbooks"));
        assert_eq!(updated.status, "draft");
        assert_eq!(updated.content, "body b"); // body preserved

        let cleared = store
            .update_document_placement(
                "b",
                &PlacementUpdate {
                    wing: Some(String::new()),
                    room: Some(String::new()),
                    ..Default::default()
                },
            )
            .unwrap()
            .expect("cleared");
        assert!(cleared.wing.is_none());
        assert!(cleared.room.is_none());
        assert_eq!(cleared.status, "draft");

        assert!(store
            .update_document_placement("missing", &PlacementUpdate::default())
            .unwrap()
            .is_none());

        // Meta-only update: title / metadata / pin / boost — no content_changed.
        let meta = store
            .update_document_meta(
                "a",
                &DocumentMetaUpdate {
                    title: Some("Pinned research note".into()),
                    metadata_json: Some(r#"{"topic":"rag"}"#.into()),
                    pinned: Some(true),
                    boost: Some(3.5),
                    status: Some("active".into()),
                    ..Default::default()
                },
            )
            .unwrap()
            .expect("meta");
        assert!(!meta.content_changed);
        assert!(meta.title_changed);
        assert_eq!(meta.document.title, "Pinned research note");
        assert_eq!(meta.document.metadata_json, r#"{"topic":"rag"}"#);
        assert!(meta.document.pinned);
        assert!((meta.document.boost - 3.5).abs() < f64::EPSILON);
        assert_eq!(meta.document.content, "body a");

        // Content change on non-raw (status draft from earlier was "b"; use "d" wiki-like).
        store
            .upsert_document(&Document {
                id: "wiki1".into(),
                uri: "wiki://w1".into(),
                title: "W".into(),
                content: "old wiki body".into(),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                layer: "wiki".into(),
                kind: "wiki".into(),
                ..Default::default()
            })
            .unwrap();
        let body_upd = store
            .update_document_meta(
                "wiki1",
                &DocumentMetaUpdate {
                    content: Some("new wiki body".into()),
                    title: Some("W2".into()),
                    ..Default::default()
                },
            )
            .unwrap()
            .expect("body");
        assert!(body_upd.content_changed);
        assert!(body_upd.title_changed);
        assert_eq!(body_upd.document.content, "new wiki body");
        assert_eq!(body_upd.document.title, "W2");
        assert!(body_upd.document.content_hash.is_some());

        // layer=raw refuses content rewrite.
        let err = store
            .update_document_meta(
                "a",
                &DocumentMetaUpdate {
                    content: Some("mutated raw".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("immutable") || err.to_string().contains("layer=raw"),
            "unexpected err: {err}"
        );

        let bad_boost = store.update_document_meta(
            "a",
            &DocumentMetaUpdate {
                boost: Some(0.0),
                ..Default::default()
            },
        );
        assert!(bad_boost.is_err());
    }

    #[test]
    fn raw_meta_update_cannot_escape_immutability_by_relabelling() {
        let store = open_temp();
        let mut raw = sample_doc("raw-policy", "file://raw-policy.md");
        raw.source_file = Some("/vault/raw-policy.md".into());
        store.upsert_document(&raw).unwrap();
        let before = store.get_document(&raw.id).unwrap().unwrap();

        let combined = store
            .update_document_meta(
                &raw.id,
                &DocumentMetaUpdate {
                    layer: Some("wiki".into()),
                    content: Some("rewritten in the same patch".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(combined, AppError::Conflict(_)));

        let transition = store
            .update_document_meta(
                &raw.id,
                &DocumentMetaUpdate {
                    layer: Some("wiki".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(transition, AppError::Conflict(_)));
        let after = store.get_document(&raw.id).unwrap().unwrap();
        assert_eq!(after.layer, "raw");
        assert_eq!(after.content, before.content);
        assert_eq!(after.revision, before.revision);

        let mut wiki = sample_doc("freeze-policy", "wiki://freeze-policy");
        wiki.layer = "wiki".into();
        wiki.kind = "wiki".into();
        store.upsert_document(&wiki).unwrap();
        let frozen = store
            .update_document_meta(
                &wiki.id,
                &DocumentMetaUpdate {
                    layer: Some("RAW".into()),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(frozen.document.layer, "raw");
        assert!(store
            .update_document_meta(
                &wiki.id,
                &DocumentMetaUpdate {
                    content: Some("cannot rewrite after freezing".into()),
                    ..Default::default()
                },
            )
            .is_err());
    }

    #[test]
    fn embedding_manifest_get_set() {
        let store = open_temp();
        assert!(store.get_embedding_manifest().unwrap().is_none());

        let m = EmbeddingManifest {
            id: "default".into(),
            provider: "mock".into(),
            model: "mock-embed".into(),
            dims: 8,
            base_url: Some("http://localhost".into()),
            content_fingerprint: Some("fp1".into()),
            updated_at: Utc::now(),
        };
        store.set_embedding_manifest(&m).unwrap();

        let got = store.get_embedding_manifest().unwrap().expect("manifest");
        assert_eq!(got.provider, "mock");
        assert_eq!(got.model, "mock-embed");
        assert_eq!(got.dims, 8);
        assert_eq!(got.base_url.as_deref(), Some("http://localhost"));
        assert_eq!(got.content_fingerprint.as_deref(), Some("fp1"));

        let m2 = EmbeddingManifest {
            id: "default".into(),
            provider: "openai".into(),
            model: "text-embedding-3-small".into(),
            dims: 1536,
            base_url: None,
            content_fingerprint: None,
            updated_at: Utc::now(),
        };
        store.set_embedding_manifest(&m2).unwrap();
        let got2 = store.get_embedding_manifest().unwrap().expect("manifest");
        assert_eq!(got2.provider, "openai");
        assert_eq!(got2.dims, 1536);
        assert!(got2.base_url.is_none());
    }

    #[test]
    fn schema_version_and_fts_probe_do_not_fail() {
        let store = open_temp();
        let ver = store.schema_version().expect("schema_version");
        assert_eq!(ver, Some(schema::SCHEMA_VERSION));
        // May be false when FTS extension is unavailable offline; must not error.
        let _ = store.fts_ready().expect("fts_ready");
    }

    fn sample_config(dims: usize) -> Config {
        Config {
            db_path: PathBuf::from("./rag.duckdb"),
            embedding_provider: crate::config::EmbeddingProviderKind::Mock,
            embedding_base_url: "https://api.openai.com/v1".to_string(),
            embedding_api_key: String::new(),
            embedding_model: "mock-embed".to_string(),
            embedding_dims: dims,
            chunk_size: 800,
            chunk_overlap: 120,
            default_top_k: 5,
            ingest_roots: Vec::new(),
            max_context_tokens: 4096,
            max_chunks_per_doc: 3,
            fts_stemmer: "porter".to_string(),
            default_search_mode: crate::models::SearchMode::Vec,
            llm_base_url: "http://127.0.0.1:11434/v1".to_string(),
            llm_provider: crate::llm::LlmProviderKind::Ollama,
            llm_model: "llama3.2".to_string(),
            llm_api_key: "ollama".to_string(),
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

    #[test]
    fn ensure_embedding_manifest_records_once() {
        let store = open_temp();
        let cfg = sample_config(32);
        assert!(store.get_embedding_manifest().unwrap().is_none());

        let m1 = store.ensure_embedding_manifest(&cfg).unwrap();
        assert_eq!(m1.dims, 32);
        assert_eq!(m1.provider, "mock");

        // Second call must not overwrite when config dims change.
        let cfg2 = sample_config(64);
        let m2 = store.ensure_embedding_manifest(&cfg2).unwrap();
        assert_eq!(m2.dims, 32);

        store.require_embedding_dims_match(32).unwrap();
        let err = store.require_embedding_dims_match(64).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("dimension mismatch") || msg.contains("dims="),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn write_manifest_and_replace_chunks() {
        let store = open_temp();
        let cfg = sample_config(4);
        let m = store.write_embedding_manifest_from_config(&cfg).unwrap();
        assert_eq!(m.dims, 4);

        let now = Utc::now();
        let doc = Document {
            id: "d1".into(),
            uri: "u1".into(),
            title: "t".into(),
            content: "hello world".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        store.upsert_document(&doc).unwrap();
        store
            .insert_chunks(&[Chunk {
                id: "c1".into(),
                document_id: "d1".into(),
                chunk_index: 0,
                content: "hello world".into(),
                embedding: vec![1.0, 0.0, 0.0, 0.0],
                char_start: 0,
                char_end: 11,
                metadata_json: "{}".into(),
            }])
            .unwrap();

        store
            .replace_chunks_for_document(
                "d1",
                &[Chunk {
                    id: "c1".into(),
                    document_id: "d1".into(),
                    chunk_index: 0,
                    content: "hello world".into(),
                    embedding: vec![0.0, 1.0, 0.0, 0.0],
                    char_start: 0,
                    char_end: 11,
                    metadata_json: "{}".into(),
                }],
            )
            .unwrap();
        let chunks = store.list_chunks_for_document("d1").unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].embedding, vec![0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn ops_log_append_read_list_recent() {
        let store = open_temp();

        let empty_op = OpsLogEntry {
            id: String::new(),
            seq: 0,
            ts: Utc::now(),
            op: "  ".into(),
            prefix: None,
            message: "x".into(),
            entity_id: None,
            entity_kind: None,
            payload_json: "{}".into(),
            agent_name: None,
        };
        assert!(store.append_ops_log(&empty_op).is_err());

        let e1 = store
            .append_ops_log(&OpsLogEntry {
                id: String::new(),
                seq: 0,
                ts: Utc::now(),
                op: "ingest".into(),
                prefix: Some("INGEST".into()),
                message: "ingested doc".into(),
                entity_id: Some("d1".into()),
                entity_kind: Some("document".into()),
                payload_json: r#"{"chunks":3}"#.into(),
                agent_name: Some("tester".into()),
            })
            .unwrap();
        assert!(!e1.id.is_empty());
        assert_eq!(e1.seq, 1);
        assert_eq!(e1.op, "ingest");
        assert_eq!(e1.prefix.as_deref(), Some("INGEST"));

        let e2 = store
            .append_ops_log(&OpsLogEntry {
                id: String::new(),
                seq: 0,
                ts: Utc::now(),
                op: "wiki_write".into(),
                prefix: Some("WIKI".into()),
                message: "wrote page".into(),
                entity_id: Some("w1".into()),
                entity_kind: Some("wiki".into()),
                payload_json: String::new(),
                agent_name: None,
            })
            .unwrap();
        assert_eq!(e2.seq, 2);
        assert_eq!(e2.payload_json, "{}");

        let by_id = store.get_ops_log(&e1.id).unwrap().expect("by id");
        assert_eq!(by_id.seq, 1);
        assert_eq!(by_id.message, "ingested doc");
        assert_eq!(by_id.entity_id.as_deref(), Some("d1"));
        assert_eq!(by_id.agent_name.as_deref(), Some("tester"));

        let by_seq = store.get_ops_log_by_seq(2).unwrap().expect("by seq");
        assert_eq!(by_seq.id, e2.id);
        assert_eq!(by_seq.op, "wiki_write");

        let recent = store.list_recent_ops(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].seq, 2);
        assert_eq!(recent[1].seq, 1);

        let limited = store.list_ops_log(1).unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].seq, 2);

        let read_one = store.read_ops_log(Some(&e1.id), None, None).unwrap();
        assert_eq!(read_one.len(), 1);
        assert_eq!(read_one[0].id, e1.id);

        let read_seq = store.read_ops_log(None, Some(2), None).unwrap();
        assert_eq!(read_seq.len(), 1);
        assert_eq!(read_seq[0].id, e2.id);

        let read_list = store.read_ops_log(None, None, Some(10)).unwrap();
        assert_eq!(read_list.len(), 2);

        assert!(store.get_ops_log("missing").unwrap().is_none());
        assert!(store.get_ops_log_by_seq(99).unwrap().is_none());

        assert!(store.ops_log_exists().unwrap());

        // Non-filing ops are excluded; ingest / wiki_write match the filter.
        let filed = store.memories_filed_away(10).unwrap();
        assert_eq!(filed.len(), 2);
        assert_eq!(filed[0].op, "wiki_write");
        assert_eq!(filed[1].op, "ingest");

        store
            .append_ops_log(&OpsLogEntry {
                id: String::new(),
                seq: 0,
                ts: Utc::now(),
                op: "analyze_corpus".into(),
                prefix: Some("MAINT".into()),
                message: "not a memory filing".into(),
                entity_id: None,
                entity_kind: None,
                payload_json: "{}".into(),
                agent_name: None,
            })
            .unwrap();
        let filed2 = store.memories_filed_away(10).unwrap();
        assert_eq!(filed2.len(), 2, "non-filing ops must not appear");
        assert!(filed2.iter().all(|e| e.op != "analyze_corpus"));
    }

    #[test]
    fn vacuum_store_checkpoint_and_size_stats() {
        let store = open_temp();
        store
            .upsert_document(&sample_doc("d1", "file://vac.md"))
            .unwrap();

        let report = store.vacuum_store().expect("vacuum_store");
        assert!(report.checkpointed);
        assert_eq!(report.db_path, store.path().display().to_string());
        assert!(report.bytes_before.is_some());
        assert!(report.bytes_after.is_some());
        assert!(report.bytes_delta.is_some());
        assert!(report.bytes_after.unwrap() > 0);

        let ops = store.list_ops_log(5).unwrap();
        assert!(
            ops.iter().any(|e| e.op == "vacuum_store"),
            "expected ops_log vacuum_store entry, got: {:?}",
            ops.iter().map(|e| e.op.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn document_cas_rejects_stale_revision() {
        let store = open_temp();
        let now = Utc::now();
        let mut doc = Document {
            id: "d1".into(),
            uri: "wiki://t".into(),
            title: "T".into(),
            content: "v1".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            layer: "wiki".into(),
            kind: "wiki".into(),
            ..Default::default()
        };
        let r1 = store.upsert_document_cas(&doc, None).unwrap();
        assert_eq!(r1, 1);
        doc.content = "v2".into();
        let r2 = store.upsert_document_cas(&doc, Some(1)).unwrap();
        assert_eq!(r2, 2);
        doc.content = "v3".into();
        let err = store.upsert_document_cas(&doc, Some(1)).unwrap_err();
        assert!(
            matches!(err, AppError::Conflict(_)),
            "expected conflict, got {err:?}"
        );
        let ok = store.upsert_document_cas(&doc, Some(2)).unwrap();
        assert_eq!(ok, 3);
        let loaded = store.get_document("d1").unwrap().unwrap();
        assert_eq!(loaded.revision, 3);
        assert_eq!(loaded.content, "v3");
        let history = store.list_document_revisions("d1").unwrap();
        assert_eq!(
            history.iter().map(|doc| doc.revision).collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(history[0].content, "v2");
        assert_eq!(history[1].content, "v1");
        let first_page = store.list_document_revision_summaries("d1", 1, 0).unwrap();
        assert_eq!(first_page.total, 2);
        assert_eq!(first_page.items.len(), 1);
        assert_eq!(first_page.items[0].revision, 2);
        assert_eq!(first_page.items[0].content_chars, 2);
        assert_eq!(first_page.items[0].content_lines, 1);
        let serialized = serde_json::to_value(&first_page.items[0]).unwrap();
        assert!(serialized.get("content").is_none());
        assert!(serialized.get("metadata_json").is_none());
        let second_page = store.list_document_revision_summaries("d1", 1, 1).unwrap();
        assert_eq!(second_page.items[0].revision, 1);
    }

    #[test]
    fn query_wiki_index_term_match_and_limit() {
        let store = open_temp();
        let now = Utc::now();
        store
            .upsert_wiki_index_entry(&WikiIndexEntry {
                id: "idx-1".into(),
                slug: "rust-ownership".into(),
                title: "Rust Ownership".into(),
                kind: "concept".into(),
                category: Some("languages".into()),
                summary: Some("How ownership and borrowing work in Rust".into()),
                page_id: Some("page-1".into()),
                updated_at: now,
            })
            .unwrap();
        store
            .upsert_wiki_index_entry(&WikiIndexEntry {
                id: "idx-2".into(),
                slug: "python-gil".into(),
                title: "Python GIL".into(),
                kind: "concept".into(),
                category: Some("languages".into()),
                summary: Some("Global Interpreter Lock notes".into()),
                page_id: Some("page-2".into()),
                updated_at: now,
            })
            .unwrap();

        let hits = store.query_wiki_index("rust ownership", 10).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].entry.slug, "rust-ownership");
        assert!(hits[0].score > 0.0);
        assert!(!hits.iter().any(|h| h.entry.slug == "python-gil"));

        // Slug/title hits outrank weak summary overlap (discrimination).
        store
            .upsert_wiki_index_entry(&WikiIndexEntry {
                id: "idx-3".into(),
                slug: "architecture-spine".into(),
                title: "Architecture and Spine Tools".into(),
                kind: "concept".into(),
                category: Some("system".into()),
                summary: Some("Layers L0-L4 and index-first cascade".into()),
                page_id: Some("page-3".into()),
                updated_at: now,
            })
            .unwrap();
        store
            .upsert_wiki_index_entry(&WikiIndexEntry {
                id: "idx-4".into(),
                slug: "connect-clients".into(),
                title: "Connect Clients".into(),
                kind: "howto".into(),
                category: Some("ops".into()),
                summary: Some("Mentions architecture only in passing".into()),
                page_id: Some("page-4".into()),
                updated_at: now,
            })
            .unwrap();
        let ranked = store.query_wiki_index("architecture spine", 10).unwrap();
        assert!(ranked.len() >= 2);
        assert_eq!(ranked[0].entry.slug, "architecture-spine");
        assert!(
            ranked[0].score > ranked[1].score,
            "expected discrimination: {} vs {}",
            ranked[0].score,
            ranked[1].score
        );

        let limited = store.query_wiki_index("", 1).unwrap();
        assert_eq!(limited.len(), 1);

        let none = store.query_wiki_index("zzzzqqqxxxyyy", 5).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn wiki_index_rebuild_read_update_entry() {
        let store = open_temp();
        let now = Utc::now();

        let w1 = Document {
            id: "w1".into(),
            uri: "wiki://alpha".into(),
            title: "Alpha".into(),
            content: "# Alpha\n\nFirst line about alpha systems.\n\nMore body.".into(),
            metadata_json: r#"{"category":"entities","summary":"Alpha entity summary"}"#.into(),
            created_at: now,
            updated_at: now,
            layer: "wiki".into(),
            kind: "entity".into(),
            ..Default::default()
        };
        let w2 = Document {
            id: "w2".into(),
            uri: "wiki://beta-page".into(),
            title: "Beta Page".into(),
            content: "Beta intro without heading.\nSecond.".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            layer: "wiki".into(),
            kind: "wiki".into(),
            ..Default::default()
        };
        let raw = Document {
            id: "r1".into(),
            uri: "raw://src".into(),
            title: "Raw".into(),
            content: "raw only".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            layer: "raw".into(),
            kind: "document".into(),
            ..Default::default()
        };
        store.upsert_document(&w1).unwrap();
        store.upsert_document(&w2).unwrap();
        store.upsert_document(&raw).unwrap();

        let n = store.rebuild_wiki_index_from_docs().unwrap();
        assert_eq!(n, 2);
        assert_eq!(store.list_wiki_index().unwrap().len(), 2);

        let alpha = store
            .get_wiki_index_by_slug("alpha")
            .unwrap()
            .expect("alpha");
        assert_eq!(alpha.title, "Alpha");
        assert_eq!(alpha.kind, "entity");
        assert_eq!(alpha.category.as_deref(), Some("entities"));
        assert_eq!(alpha.summary.as_deref(), Some("Alpha entity summary"));
        assert_eq!(alpha.page_id.as_deref(), Some("w1"));

        let beta = store
            .get_wiki_index_by_slug("beta-page")
            .unwrap()
            .expect("beta");
        assert_eq!(beta.title, "Beta Page");
        assert_eq!(beta.summary.as_deref(), Some("Beta intro without heading."));

        let updated = store
            .update_wiki_index_entry_fields(
                "beta-page",
                None,
                Some("Updated one-line summary".into()),
                None,
                Some("concepts".into()),
                None,
            )
            .unwrap();
        assert_eq!(updated.title, "Beta Page");
        assert_eq!(updated.summary.as_deref(), Some("Updated one-line summary"));
        assert_eq!(updated.category.as_deref(), Some("concepts"));

        let md = store.render_wiki_index_markdown().unwrap();
        assert!(md.contains("# Wiki index"));
        assert!(md.contains("[[Alpha]]"));
        assert!(md.contains("Updated one-line summary"));

        let manual = store
            .update_wiki_index_entry_fields(
                "manual-note",
                Some("Manual Note".into()),
                Some("Hand-written catalog row".into()),
                Some("concept".into()),
                Some("notes".into()),
                None,
            )
            .unwrap();
        assert_eq!(manual.slug, "manual-note");
        assert_eq!(store.list_wiki_index().unwrap().len(), 3);

        let n2 = store.rebuild_wiki_index_from_docs().unwrap();
        assert_eq!(n2, 2);
        assert!(store
            .get_wiki_index_by_slug("manual-note")
            .unwrap()
            .is_none());
        assert!(store.get_wiki_index_by_slug("alpha").unwrap().is_some());
    }

    #[test]
    fn diary_entries_and_pinned_list_for_wake_up() {
        let store = open_temp();
        let now = Utc::now();
        let earlier = now - chrono::Duration::hours(2);

        let d1 = Document {
            id: "diary-1".into(),
            uri: "diary://agents/alice/1".into(),
            title: "Alice note 1".into(),
            content: "first diary body".into(),
            metadata_json: r#"{"agent_name":"alice"}"#.into(),
            created_at: earlier,
            updated_at: earlier,
            wing: Some("agents/alice".into()),
            room: Some("alice".into()),
            layer: "diary".into(),
            kind: "diary".into(),
            ..Default::default()
        };
        let d2 = Document {
            id: "diary-2".into(),
            uri: "diary://agents/alice/2".into(),
            title: "Alice note 2".into(),
            content: "second diary body".into(),
            metadata_json: r#"{"agent_name":"alice"}"#.into(),
            created_at: now,
            updated_at: now,
            wing: Some("agents/alice".into()),
            room: Some("alice".into()),
            layer: "diary".into(),
            kind: "diary".into(),
            ..Default::default()
        };
        let d_bob = Document {
            id: "diary-bob".into(),
            uri: "diary://agents/bob/1".into(),
            title: "Bob note".into(),
            content: "bob body".into(),
            metadata_json: r#"{"agent_name":"bob"}"#.into(),
            created_at: now,
            updated_at: now,
            wing: Some("agents/bob".into()),
            room: Some("bob".into()),
            layer: "diary".into(),
            kind: "diary".into(),
            ..Default::default()
        };
        let pinned = Document {
            id: "pin-1".into(),
            uri: "wiki://evergreen".into(),
            title: "Evergreen".into(),
            content: "pinned page".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            layer: "wiki".into(),
            kind: "wiki".into(),
            pinned: true,
            boost: 2.0,
            ..Default::default()
        };
        let unpinned = Document {
            id: "plain-1".into(),
            uri: "raw://plain".into(),
            title: "Plain".into(),
            content: "not pinned".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            layer: "raw".into(),
            kind: "document".into(),
            pinned: false,
            ..Default::default()
        };
        for d in [&d1, &d2, &d_bob, &pinned, &unpinned] {
            store.upsert_document(d).unwrap();
        }

        let alice = store
            .list_diary_entries(Some("alice"), 10)
            .expect("alice diary");
        assert_eq!(alice.len(), 2);
        assert_eq!(alice[0].id, "diary-2"); // newest first
        assert_eq!(alice[0].agent_name, "alice");
        assert_eq!(alice[0].content, "second diary body");
        assert_eq!(alice[0].wing, "agents/alice");
        assert_eq!(alice[1].id, "diary-1");

        let limited = store.list_diary_entries(Some("alice"), 1).expect("limit 1");
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].id, "diary-2");

        let all = store.list_diary_entries(None, 10).expect("all diary");
        assert_eq!(all.len(), 3);

        let pins = store.list_pinned_documents(10).expect("pinned");
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].id, "pin-1");
        assert!(pins[0].pinned);
    }

    #[test]
    fn list_wiki_page_metas_filters_order_metadata_and_revision() {
        let store = open_temp();
        let now = Utc::now();

        let alpha = Document {
            id: "w-alpha".into(),
            uri: "wiki://alpha".into(),
            title: "Alpha".into(),
            content: "# Alpha\n\nFirst line of body content.\n".into(),
            metadata_json: r#"{"summary":"Alpha summary","category":"entities"}"#.into(),
            created_at: now,
            updated_at: now,
            layer: "wiki".into(),
            kind: "entity".into(),
            ..Default::default()
        };
        let zeta = Document {
            id: "w-zeta".into(),
            uri: "wiki://zeta-page".into(),
            title: "Zeta".into(),
            content: "Body only; no summary in metadata.".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            layer: "wiki".into(),
            kind: "wiki".into(),
            ..Default::default()
        };
        // Kind will be cleared in-DB after upsert so metas empty/null kind -> "wiki".
        let beta = Document {
            id: "w-beta".into(),
            uri: "wiki://beta".into(),
            title: "Beta".into(),
            content: "beta body".into(),
            metadata_json: r#"{"category":"  concepts  "}"#.into(),
            created_at: now,
            updated_at: now,
            layer: "wiki".into(),
            kind: "concept".into(),
            ..Default::default()
        };
        let raw = Document {
            id: "r1".into(),
            uri: "file://raw.md".into(),
            title: "Raw Should Not Appear".into(),
            content: "raw".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            layer: "raw".into(),
            kind: "document".into(),
            ..Default::default()
        };
        for d in [&alpha, &zeta, &beta, &raw] {
            store.upsert_document(d).unwrap();
        }
        // upsert_document coerces empty kind to "document"; force empty for fallback path.
        {
            let conn = store.lock().unwrap();
            conn.execute(
                "UPDATE documents SET kind = '' WHERE id = ?",
                params!["w-beta"],
            )
            .unwrap();
        }

        // Bump revision on alpha so metas surface revision/etag > 1.
        let mut alpha_v2 = alpha.clone();
        alpha_v2.content = "# Alpha\n\nRevised body.\n".into();
        let rev = store.upsert_document_cas(&alpha_v2, Some(1)).unwrap();
        assert_eq!(rev, 2);

        let empty = open_temp().list_wiki_page_metas().unwrap();
        assert!(empty.is_empty());

        let metas = store.list_wiki_page_metas().unwrap();
        assert_eq!(metas.len(), 3, "raw layer excluded");
        // Ordered by title ASC: Alpha, Beta, Zeta
        assert_eq!(
            metas.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(),
            vec!["Alpha", "Beta", "Zeta"]
        );

        let a = &metas[0];
        assert_eq!(a.id, "w-alpha");
        assert_eq!(a.uri, "wiki://alpha");
        assert_eq!(a.slug, "alpha");
        assert_eq!(a.kind, "entity");
        assert_eq!(a.summary.as_deref(), Some("Alpha summary"));
        assert_eq!(a.category.as_deref(), Some("entities"));
        assert_eq!(a.revision, 2);
        assert_eq!(a.etag, crate::models::format_document_etag(2));
        assert!(!a.updated_at.is_empty());

        let b = &metas[1];
        assert_eq!(b.id, "w-beta");
        assert_eq!(b.slug, "beta");
        assert_eq!(b.kind, "wiki", "empty kind defaults to wiki");
        assert!(b.summary.is_none());
        assert_eq!(b.category.as_deref(), Some("concepts"));
        assert_eq!(b.revision, 1);
        assert_eq!(b.etag, crate::models::format_document_etag(1));

        let z = &metas[2];
        assert_eq!(z.id, "w-zeta");
        assert_eq!(z.slug, "zeta-page");
        // Unlike rebuild_wiki_index, metas do not fall back to first content line.
        assert!(
            z.summary.is_none(),
            "summary must come from metadata only, not content"
        );
        assert!(z.category.is_none());

        // q / kind / category / limit / offset filters
        let (by_q, total_q) = store
            .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
                q: Some("alp".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(total_q, 1);
        assert_eq!(by_q.len(), 1);
        assert_eq!(by_q[0].id, "w-alpha");

        let (by_kind, total_kind) = store
            .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
                kind: Some("wiki".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(total_kind, 2, "empty kind + explicit wiki");
        assert_eq!(
            by_kind.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["w-beta", "w-zeta"]
        );

        let (by_cat, total_cat) = store
            .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
                category: Some("ENTITIES".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(total_cat, 1);
        assert_eq!(by_cat[0].id, "w-alpha");

        let (page, total_all) = store
            .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
                limit: Some(1),
                offset: Some(1),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(total_all, 3);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].title, "Beta");

        let (none, total_none) = store
            .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
                q: Some("zzzz-no-match".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(total_none, 0);
        assert!(none.is_empty());
    }

    #[test]
    fn list_wiki_page_metas_never_leaks_body_content() {
        let store = open_temp();
        let now = Utc::now();
        let secret = "SECRET_BODY_MARKER_xyz_do_not_list";
        store
            .upsert_document(&Document {
                id: "w-body".into(),
                uri: "wiki://body-page".into(),
                title: "Body Page".into(),
                content: format!("# Body\n\n{secret}\n"),
                metadata_json: r#"{"summary":"catalog blurb only"}"#.into(),
                created_at: now,
                updated_at: now,
                layer: "wiki".into(),
                kind: "wiki".into(),
                ..Default::default()
            })
            .unwrap();

        let items = store.list_wiki_page_metas().unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.summary.as_deref(), Some("catalog blurb only"));
        let wire = serde_json::to_string(item).unwrap();
        assert!(
            !wire.contains(secret),
            "list DTO must not include document body: {wire}"
        );
        assert!(
            !wire.contains("\"content\""),
            "list DTO JSON must not have content key: {wire}"
        );
    }

    #[test]
    fn list_wiki_page_metas_filtered_wing_room() {
        let store = open_temp();
        let now = Utc::now();
        for (id, title, wing, room) in [
            ("w1", "One", Some("projects"), Some("rag")),
            ("w2", "Two", Some("projects"), Some("ui")),
            ("w3", "Three", Some("personal"), None),
        ] {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("wiki://{id}"),
                    title: title.into(),
                    content: title.into(),
                    metadata_json: "{}".into(),
                    created_at: now,
                    updated_at: now,
                    layer: "wiki".into(),
                    kind: "wiki".into(),
                    wing: wing.map(str::to_string),
                    room: room.map(str::to_string),
                    ..Default::default()
                })
                .unwrap();
        }

        let (rows, total) = store
            .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
                wing: Some("projects".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(total, 2);
        assert_eq!(
            rows.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["w1", "w2"]
        );

        let (room_rows, room_total) = store
            .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
                wing: Some("projects".into()),
                room: Some("rag".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(room_total, 1);
        assert_eq!(room_rows[0].id, "w1");
    }

    #[test]
    fn wiki_backlinks_for_document_wikilink_only_sorted_and_deduped() {
        let store = open_temp();
        let now = Utc::now();

        // Missing document / no graph node -> empty, not an error.
        assert!(store
            .wiki_backlinks_for_document("missing-doc")
            .unwrap()
            .is_empty());

        for (id, uri, title) in [
            ("doc-target", "wiki://target", "Target"),
            ("doc-src-a", "wiki://src-a", "Source A"),
            ("doc-src-b", "wiki://src-b", "Source B"),
        ] {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: uri.into(),
                    title: title.into(),
                    content: title.into(),
                    metadata_json: "{}".into(),
                    created_at: now,
                    updated_at: now,
                    layer: "wiki".into(),
                    kind: "wiki".into(),
                    ..Default::default()
                })
                .unwrap();
        }

        let gnode = |id: &str, label: &str, doc_id: Option<&str>| crate::models::GraphNode {
            id: id.into(),
            kind: if doc_id.is_some() {
                "document".into()
            } else {
                "stub".into()
            },
            label: label.into(),
            document_id: doc_id.map(str::to_string),
            uri: None,
            resolved: doc_id.is_some(),
            metadata_json: "{}".into(),
        };

        store
            .upsert_graph_node(&gnode("n-target", "Target", Some("doc-target")))
            .unwrap();
        store
            .upsert_graph_node(&gnode("n-src-a", "Source A", Some("doc-src-a")))
            .unwrap();
        store
            .upsert_graph_node(&gnode("n-src-b", "Source B", Some("doc-src-b")))
            .unwrap();
        // Stub without document_id: key falls back to node id.
        store
            .upsert_graph_node(&gnode("n-stub", "Stub Link", None))
            .unwrap();

        store
            .link_nodes("n-src-b", "n-target", "wikilink", 1.0)
            .unwrap();
        store
            .link_nodes("n-src-a", "n-target", "wikilink", 1.0)
            .unwrap();
        store
            .link_nodes("n-stub", "n-target", "wikilink", 1.0)
            .unwrap();
        // Non-wikilink inbound edges must be ignored.
        store
            .link_nodes("n-src-a", "n-target", "related", 1.0)
            .unwrap();
        store
            .link_nodes("n-src-a", "n-target", "tagged", 1.0)
            .unwrap();
        // Duplicate wikilink from same source (same label + key) should dedupe.
        store
            .link_nodes("n-src-a", "n-target", "wikilink", 0.5)
            .unwrap();

        let bl = store.wiki_backlinks_for_document("doc-target").unwrap();
        assert_eq!(
            bl.len(),
            3,
            "expected 3 unique wikilink sources, got {bl:?}"
        );
        // Sorted by label ASC.
        assert_eq!(
            bl.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(),
            vec!["Source A", "Source B", "Stub Link"]
        );
        assert_eq!(bl[0].1, "doc-src-a");
        assert_eq!(bl[1].1, "doc-src-b");
        assert_eq!(bl[2].1, "n-stub", "stub without document_id uses node id");

        // Document with a node but no inbound wikilinks.
        assert!(store
            .wiki_backlinks_for_document("doc-src-a")
            .unwrap()
            .is_empty());
    }
}
