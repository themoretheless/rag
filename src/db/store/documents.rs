//! Document lookups, listings, catalog metadata, taxonomy and placement.

use super::*;

/// Identity of a document without its body or metadata.
#[derive(Debug, Clone)]
pub(crate) struct DocumentRef {
    pub id: String,
    pub uri: String,
    pub layer: String,
}

impl Store {
    /// Fetch a document by id.
    pub fn get_document(&self, id: &str) -> Result<Option<Document>> {
        let conn = self.lock()?;
        get_document_locked(&conn, id)
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

    /// Document by id, else by uri. The uri query runs only when the id misses.
    pub fn find_by_id_or_uri(&self, id: &str, uri: &str) -> Result<Option<Document>> {
        if let Some(document) = self.get_document(id)? {
            return Ok(Some(document));
        }
        if uri.is_empty() {
            return Ok(None);
        }
        self.find_by_uri(uri)
    }

    /// Stored `content_hash` for each id that exists, in one query.
    pub fn content_hashes(
        &self,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, Option<String>>> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT id, content_hash FROM documents WHERE id IN ({placeholders})"
        ))?;
        let rows = crate::db::rows::query_vec(
            &mut stmt,
            duckdb::params_from_iter(ids.iter()),
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )?;
        Ok(rows.into_iter().collect())
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

        let out = crate::db::rows::query_vec(&mut stmt, params![hash], rows::document)?;
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

        let out = crate::db::rows::query_vec(&mut stmt, [], rows::document)?;
        Ok(out)
    }

    /// Current `revision` of one document, without loading its body.
    pub(crate) fn document_revision(&self, id: &str) -> Result<Option<i64>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare_cached("SELECT revision FROM documents WHERE id = ?")?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// `(id, uri)` of one document, without loading its body.
    pub(crate) fn document_id_and_uri(&self, id: &str) -> Result<Option<(String, String)>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare_cached("SELECT id, uri FROM documents WHERE id = ?")?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some((row.get(0)?, row.get(1)?))),
            None => Ok(None),
        }
    }

    /// `(id, uri, layer)` of every document ordered by `created_at`, without bodies.
    pub(crate) fn list_document_refs(&self) -> Result<Vec<DocumentRef>> {
        let conn = self.lock()?;
        let mut stmt =
            conn.prepare("SELECT id, uri, layer FROM documents ORDER BY created_at ASC")?;
        crate::db::rows::query_vec(&mut stmt, [], |row| {
            Ok(DocumentRef {
                id: row.get(0)?,
                uri: row.get(1)?,
                layer: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            })
        })
    }

    /// Count documents without loading document bodies.
    pub(crate) fn count_documents(&self) -> Result<usize> {
        let conn = self.lock()?;
        let count: i64 = conn.query_row("SELECT COUNT(*)::BIGINT FROM documents", [], |row| {
            row.get(0)
        })?;
        usize::try_from(count).map_err(|_| AppError::db(format!("invalid document count: {count}")))
    }

    /// Count documents in one layer without loading their bodies.
    pub(crate) fn count_documents_by_layer(&self, layer: &str) -> Result<usize> {
        let conn = self.lock()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*)::BIGINT FROM documents WHERE layer = ?",
            params![layer],
            |row| row.get(0),
        )?;
        usize::try_from(count).map_err(|_| AppError::db(format!("invalid document count: {count}")))
    }

    /// Count documents eligible for a graph rebuild.
    pub(crate) fn count_graph_rebuild_candidates(&self, dirty_only: bool) -> Result<usize> {
        let conn = self.lock()?;
        let count: i64 = conn.query_row(
            r#"
            SELECT COUNT(*)::BIGINT
            FROM documents d
            WHERE NOT ? OR NOT EXISTS (
                SELECT 1 FROM graph_nodes n WHERE n.document_id = d.id
            )
            "#,
            params![dirty_only],
            |row| row.get(0),
        )?;
        usize::try_from(count)
            .map_err(|_| AppError::db(format!("invalid graph rebuild candidate count: {count}")))
    }

    /// Load one deterministic bounded page of graph rebuild candidates.
    pub(crate) fn list_graph_rebuild_candidates_after(
        &self,
        dirty_only: bool,
        after_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Document>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let conn = self.lock()?;
        let sql = format!(
            "SELECT {DOCUMENT_SELECT} FROM documents d \
             WHERE (NOT ? OR NOT EXISTS (SELECT 1 FROM graph_nodes n WHERE n.document_id = d.id)) \
               AND d.id > ? ORDER BY d.id ASC LIMIT ?"
        );
        let mut statement = conn.prepare(&sql)?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let documents = crate::db::rows::query_vec(
            &mut statement,
            params![dirty_only, after_id.unwrap_or(""), limit],
            rows::document,
        )?;
        Ok(documents)
    }

    /// Load one deterministic bounded page of document ids for full-corpus work.
    pub(crate) fn list_document_ids_after(
        &self,
        after_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let conn = self.lock()?;
        let mut statement =
            conn.prepare("SELECT id FROM documents WHERE id > ? ORDER BY id ASC LIMIT ?")?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let ids = crate::db::rows::query_vec(
            &mut statement,
            params![after_id.unwrap_or(""), limit],
            |row| Ok(row.get(0)?),
        )?;
        Ok(ids)
    }

    /// Count chunks attached to the first `limit` documents in keyset order.
    pub(crate) fn count_chunks_for_document_limit(&self, limit: usize) -> Result<usize> {
        if limit == 0 {
            return Ok(0);
        }
        let conn = self.lock()?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let count: i64 = conn.query_row(
            r#"
            SELECT COUNT(*)::BIGINT
            FROM chunks c
            INNER JOIN (
                SELECT id FROM documents ORDER BY id ASC LIMIT ?
            ) d ON d.id = c.document_id
            "#,
            params![limit],
            |row| row.get(0),
        )?;
        usize::try_from(count)
            .map_err(|_| AppError::db(format!("invalid bounded chunk count: {count}")))
    }

    /// Count and return a bounded deterministic prefix of persisted-hash matches.
    ///
    /// Callers must still validate actual body hashes before mutation.
    pub(crate) fn exact_duplicate_source_ids(
        &self,
        hash: &str,
        keep_id: &str,
        limit: usize,
    ) -> Result<(usize, Vec<String>)> {
        let conn = self.lock()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*)::BIGINT FROM documents WHERE content_hash = ? AND id <> ?",
            params![hash, keep_id],
            |row| row.get(0),
        )?;
        let count = usize::try_from(count)
            .map_err(|_| AppError::db(format!("invalid duplicate source count: {count}")))?;
        if limit == 0 {
            return Ok((count, Vec::new()));
        }
        let mut statement = conn.prepare(
            "SELECT id FROM documents WHERE content_hash = ? AND id <> ? \
             ORDER BY created_at ASC, id ASC LIMIT ?",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let ids =
            crate::db::rows::query_vec(&mut statement, params![hash, keep_id, limit], |row| {
                Ok(row.get(0)?)
            })?;
        Ok((count, ids))
    }

    /// Count and load only the bounded active, user-organizable unscoped set.
    /// Full document bodies are selected because the optional LLM workflow may
    /// inspect them later, but the SQL limit is applied before rows reach Rust.
    pub(crate) fn list_active_unscoped_documents(
        &self,
        limit: usize,
    ) -> Result<(usize, Vec<Document>)> {
        let conn = self.lock()?;
        let predicate = r#"
            COALESCE(status, 'active') NOT IN ('archived', 'tombstone')
            AND COALESCE(layer, '') NOT IN ('schema', 'index', 'log')
            AND (wing IS NULL OR TRIM(wing) = '')
        "#;
        let count: i64 = conn.query_row(
            &format!("SELECT COUNT(*)::BIGINT FROM documents WHERE {predicate}"),
            [],
            |row| row.get(0),
        )?;
        // Organize heuristics read titles, uris and paths only: skip bodies.
        let sql = format!(
            "SELECT {} FROM documents WHERE {predicate} \
             ORDER BY updated_at DESC, title ASC, id ASC LIMIT ?",
            document_select_without_body()
        );
        let mut statement = conn.prepare(&sql)?;
        let limit = i64::try_from(limit.max(1)).unwrap_or(i64::MAX);
        let documents = crate::db::rows::query_vec(&mut statement, params![limit], rows::document)?;
        Ok((count.max(0) as usize, documents))
    }

    /// Count and load a bounded batch of searchable documents whose derived
    /// chunks are missing.
    ///
    /// The anti-join and limit both run in DuckDB so doctor repair never
    /// materializes the whole corpus or performs one chunk query per document.
    pub(crate) fn list_documents_missing_chunks(
        &self,
        limit: usize,
    ) -> Result<(usize, Vec<Document>)> {
        let conn = self.lock()?;
        let predicate = r#"
            LOWER(COALESCE(NULLIF(TRIM(layer), ''), 'raw')) <> 'schema'
            AND TRIM(COALESCE(content, '')) <> ''
            AND NOT EXISTS (
                SELECT 1 FROM chunks WHERE chunks.document_id = documents.id
            )
        "#;
        let count: i64 = conn.query_row(
            &format!("SELECT COUNT(*)::BIGINT FROM documents WHERE {predicate}"),
            [],
            |row| row.get(0),
        )?;
        let sql = format!(
            "SELECT {DOCUMENT_SELECT} FROM documents WHERE {predicate} \
             ORDER BY updated_at DESC, id ASC LIMIT ?"
        );
        let mut statement = conn.prepare(&sql)?;
        let limit = i64::try_from(limit.max(1)).unwrap_or(i64::MAX);
        let documents = crate::db::rows::query_vec(&mut statement, params![limit], rows::document)?;
        Ok((count.max(0) as usize, documents))
    }

    /// List documents with the given `layer` (`raw`, `wiki`, …), ordered by `created_at`.
    pub fn list_documents_by_layer(&self, layer: &str) -> Result<Vec<Document>> {
        let conn = self.lock()?;
        let sql = format!(
            "SELECT {DOCUMENT_SELECT} FROM documents WHERE layer = ? ORDER BY created_at ASC"
        );
        let mut stmt = conn.prepare(&sql)?;

        let out = crate::db::rows::query_vec(&mut stmt, params![layer], rows::document)?;
        Ok(out)
    }

    /// Like [`Self::list_documents_by_layer`], with an empty `content`: for
    /// corpus-wide scans that only need ids, uris, titles and timestamps.
    pub(crate) fn list_documents_by_layer_without_body(
        &self,
        layer: &str,
    ) -> Result<Vec<Document>> {
        let conn = self.lock()?;
        let sql = format!(
            "SELECT {} FROM documents WHERE layer = ? ORDER BY created_at ASC",
            document_select_without_body()
        );
        let mut stmt = conn.prepare(&sql)?;
        crate::db::rows::query_vec(&mut stmt, params![layer], rows::document)
    }

    /// `content` of each of `ids` that exists, in one query.
    pub(crate) fn document_contents(
        &self,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, String>> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT id, content FROM documents WHERE id IN ({placeholders})"
        ))?;
        let rows =
            crate::db::rows::query_vec(&mut stmt, duckdb::params_from_iter(ids.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
        Ok(rows.into_iter().collect())
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
        fetch_wiki_page_meta_items(&conn, filter)
    }

    /// Incoming wikilink sources for a document (label, document_id or node id).
    pub fn wiki_backlinks_for_document(&self, document_id: &str) -> Result<Vec<(String, String)>> {
        Ok(self
            .wiki_backlink_view(document_id)?
            .map(|(node_id, view)| wiki_backlink_rows(&node_id, &view))
            .unwrap_or_default())
    }

    /// The document's graph node id and its incoming-edge view, if it has a node.
    pub fn wiki_backlink_view(
        &self,
        document_id: &str,
    ) -> Result<Option<(String, crate::models::GraphView)>> {
        let Some(node) = self.find_node_by_document_id(document_id)? else {
            return Ok(None);
        };
        let view = self.backlinks(&node.id)?;
        Ok(Some((node.id, view)))
    }

    /// Incoming wikilink sources constrained to the same project wing.
    ///
    /// Both the target and each returned source must be backed by a document
    /// in `wing`; unowned stubs are intentionally excluded because their
    /// project membership cannot be proven.
    pub fn wiki_backlinks_for_document_in_wing(
        &self,
        document_id: &str,
        wing: &str,
    ) -> Result<Vec<(String, String)>> {
        let document_id = document_id.trim();
        let wing = wing.trim();
        if document_id.is_empty() || wing.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT MIN(source_node.label), source_node.document_id
            FROM graph_nodes target_node
            JOIN documents target_document
              ON target_document.id = target_node.document_id
            JOIN graph_edges edge
              ON edge.target_id = target_node.id
             AND edge.rel_type = 'wikilink'
            JOIN graph_nodes source_node
              ON source_node.id = edge.source_id
            JOIN documents source_document
              ON source_document.id = source_node.document_id
            WHERE target_node.document_id = ?
              AND target_document.wing = ?
              AND source_document.wing = ?
            GROUP BY source_node.document_id
            ORDER BY MIN(source_node.label), source_node.document_id
            "#,
        )?;
        let out = crate::db::rows::query_vec(&mut stmt, params![document_id, wing, wing], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        Ok(out)
    }

    /// List documents in the given `wing`, ordered by `created_at`.
    pub fn list_documents_by_wing(&self, wing: &str) -> Result<Vec<Document>> {
        let conn = self.lock()?;
        let sql = format!(
            "SELECT {DOCUMENT_SELECT} FROM documents WHERE wing = ? ORDER BY created_at ASC"
        );
        let mut stmt = conn.prepare(&sql)?;

        let out = crate::db::rows::query_vec(&mut stmt, params![wing], rows::document)?;
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
        let out = crate::db::rows::query_vec(&mut stmt, params_dyn.as_slice(), rows::document)?;
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
        let out = crate::db::rows::query_vec(&mut stmt, [], |row| {
            let count: i64 = row.get(1)?;
            Ok(WingCount {
                wing: row.get(0)?,
                document_count: count as u64,
            })
        })?;
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
            let out = crate::db::rows::query_vec(&mut stmt, params![w], |row| {
                let count: i64 = row.get(2)?;
                let wing_name: String = row.get(0)?;
                Ok(RoomCount {
                    wing: Some(wing_name),
                    room: row.get(1)?,
                    document_count: count as u64,
                })
            })?;
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
            let out = crate::db::rows::query_vec(&mut stmt, [], |row| {
                let count: i64 = row.get(2)?;
                let wing: Option<String> = row.get(0)?;
                Ok(RoomCount {
                    wing: wing.filter(|s| !s.is_empty()),
                    room: row.get(1)?,
                    document_count: count as u64,
                })
            })?;
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
        self.update_document_meta_with_identity(id, update, None)
    }

    #[cfg(test)]
    pub(crate) fn update_document_meta_for_sync_test(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
        identity: &crate::db::sync::SyncIdentity,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        self.update_document_meta_with_identity(id, update, Some(identity))
    }

    pub(super) fn update_document_meta_with_identity(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
        identity: Option<&crate::db::sync::SyncIdentity>,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        let Some(mut applied) = self.prepare_document_meta_update(id, update)? else {
            return Ok(None);
        };
        let expected_revision = applied.document.revision;
        applied.document.revision = self
            .write_document_atomic_with_manifest(
                &applied.document,
                Some(expected_revision),
                DocumentDerivedWrite::Preserve,
                None,
                identity,
            )?
            .revision;
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
}

/// `(label, document key)` of each wikilink source in a backlinks `view` of
/// `target_id`, sorted by label with adjacent duplicate keys collapsed.
pub fn wiki_backlink_rows(target_id: &str, view: &crate::models::GraphView) -> Vec<(String, String)> {
    let nodes: std::collections::HashMap<&str, &crate::models::GraphNode> =
        view.nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    let mut out = Vec::new();
    for e in &view.edges {
        if e.target_id != target_id || e.rel_type != "wikilink" {
            continue;
        }
        let Some(src) = nodes.get(e.source_id.as_str()) else {
            continue;
        };
        let key = src.document_id.clone().unwrap_or_else(|| src.id.clone());
        out.push((src.label.clone(), key));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.1 == b.1);
    out
}

/// [`DOCUMENT_SELECT`] with an empty `content`, for listings that never read
/// document bodies.
pub(super) fn document_select_without_body() -> String {
    DOCUMENT_SELECT.replacen("content,", "'' AS content,", 1)
}
