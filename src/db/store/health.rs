//! Counts, layer health, integrity, pruning, vacuum, schema and FTS state.

use super::*;

/// Largest uncompiled-raw sample or shelf queue read at once.
const MAX_UNCOMPILED_ROWS: usize = 200;

impl Store {
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
        let uncompiled = format!("NOT {}", compiled_link_exists("raw"));
        let counts = conn.query_row(
            &format!(
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
                    AND {uncompiled}
                )
              END AS uncompiled_raw_count
            FROM layer_counts
            "#
            ),
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

    /// Bounded sample of uncompiled raw docs for lint guidance.
    ///
    /// A sample only: paging a whole shelf is the catalog's job
    /// ([`Store::list_document_catalog`] with `layer=raw` and `compiled=false`).
    pub fn list_uncompiled_raw_sample(&self, limit: usize) -> Result<Vec<UncompiledRawSample>> {
        let limit = limit.clamp(1, MAX_UNCOMPILED_ROWS) as i64;
        let conn = self.lock()?;
        let uncompiled = format!("NOT {}", compiled_link_exists("raw"));
        let mut stmt = conn.prepare(&format!(
            r#"
            SELECT raw.id, raw.uri, raw.wing, raw.room
            FROM documents raw
            WHERE raw.layer = 'raw'
              AND (
                (SELECT COUNT(*) FROM documents wiki WHERE wiki.layer = 'wiki') = 0
                OR {uncompiled}
              )
            ORDER BY raw.wing ASC NULLS LAST, raw.room ASC NULLS LAST, raw.uri ASC
            LIMIT ?
            "#
        ))?;
        let rows = stmt
            .query_map([limit], |row| {
                Ok(UncompiledRawSample {
                    document_id: row.get(0)?,
                    uri: row.get(1)?,
                    wing: row.get(2)?,
                    room: row.get(3)?,
                })
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The uncompiled debt grouped by `(wing, room)` shelf, biggest first, plus how many
    /// shelves the debt spans.
    ///
    /// This is the compile queue: a 25-row sample ordered by wing always describes the
    /// same first shelves, which on the live corpus is 25 of 82 925 docs in one of 503
    /// shelves. Counting in SQL ranks every shelf instead.
    pub(crate) fn uncompiled_raw_shelves(&self, limit: usize) -> Result<UncompiledShelfQueue> {
        let limit = limit.clamp(1, MAX_UNCOMPILED_ROWS) as i64;
        let conn = self.lock()?;
        let uncompiled = format!("NOT {}", compiled_link_exists("raw"));
        // One predicate text for both the ranking and the queue width, so the two
        // numbers in the answer can't describe different corpora.
        let debt = format!(
            r#"FROM documents raw
            WHERE raw.layer = 'raw'
              AND (
                (SELECT COUNT(*) FROM documents wiki WHERE wiki.layer = 'wiki') = 0
                OR {uncompiled}
              )"#
        );
        let total_shelves: i64 = conn.query_row(
            &format!("SELECT COUNT(*)::BIGINT FROM (SELECT 1 {debt} GROUP BY raw.wing, raw.room)"),
            [],
            |row| row.get(0),
        )?;
        let mut stmt = conn.prepare(&format!(
            r#"
            SELECT raw.wing, raw.room, COUNT(*)::BIGINT AS docs
            {debt}
            GROUP BY raw.wing, raw.room
            ORDER BY docs DESC, raw.wing ASC NULLS LAST, raw.room ASC NULLS LAST
            LIMIT ?
            "#
        ))?;
        let shelves = stmt
            .query_map([limit], |row| {
                Ok(UncompiledRawShelf {
                    wing: row.get(0)?,
                    room: row.get(1)?,
                    docs: row.get::<_, i64>(2)?.max(0) as u64,
                })
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        Ok(UncompiledShelfQueue {
            total_shelves: total_shelves.max(0) as u64,
            shelves,
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
        // Find chunkless documents by id first (no body read), then check only
        // those for non-blank content: a healthy corpus never reads a body here,
        // which matters because `/health` is polled.
        let chunkless: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT d.id FROM documents d WHERE COALESCE(d.layer, '') <> 'schema' \
                 AND NOT EXISTS (SELECT 1 FROM chunks c WHERE c.document_id = d.id)",
            )?;
            crate::db::rows::query_vec(&mut stmt, [], |row| Ok(row.get(0)?))?
        };
        let mut documents_without_chunks = 0u64;
        for batch in chunkless.chunks(512) {
            let placeholders = vec!["?"; batch.len()].join(",");
            let count: i64 = conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM documents WHERE id IN ({placeholders}) \
                     AND length(trim(COALESCE(content, ''))) > 0"
                ),
                duckdb::params_from_iter(batch.iter()),
                |row| row.get(0),
            )?;
            documents_without_chunks += count.max(0) as u64;
        }
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
        let mut conn = self.lock()?;
        // A real transaction handle: any failing statement drops `tx` and rolls
        // back, so the shared connection never stays inside an open transaction.
        let tx = conn.transaction()?;
        if chunks > 0 {
            crate::db::fts::mark_fts_dirty(&tx)?;
        }
        tx.execute(
            "DELETE FROM graph_nodes WHERE document_id IS NOT NULL AND NOT EXISTS
               (SELECT 1 FROM documents d WHERE d.id = graph_nodes.document_id)",
            [],
        )?;
        #[cfg(test)]
        if PRUNE_ORPHANS_FAIL_AFTER_NODES.with(std::cell::Cell::get) {
            return Err(AppError::db(
                "injected prune_orphans failure after node delete",
            ));
        }
        tx.execute(
            "DELETE FROM graph_edges WHERE NOT EXISTS
               (SELECT 1 FROM graph_nodes n WHERE n.id = graph_edges.source_id)
               OR NOT EXISTS
               (SELECT 1 FROM graph_nodes n WHERE n.id = graph_edges.target_id)",
            [],
        )?;
        tx.execute(
            "DELETE FROM chunks WHERE NOT EXISTS
               (SELECT 1 FROM documents d WHERE d.id = chunks.document_id)",
            [],
        )?;
        tx.commit()?;
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
        Ok(crate::db::fts::fts_index_present(&conn))
    }

    /// Last-known FTS backend from `meta`, if `ensure_fts` has run.
    pub fn fts_status(&self) -> Result<Option<crate::db::fts::FtsState>> {
        let conn = self.lock()?;
        crate::db::fts::fts_status(&conn)
    }

    /// Rebuild the lexical index now; `None`/blank keeps the recorded stemmer.
    pub fn reindex_fts(&self, stemmer: Option<&str>) -> Result<crate::db::fts::FtsState> {
        let conn = self.lock()?;
        match stemmer.map(str::trim).filter(|s| !s.is_empty()) {
            Some(stemmer) => crate::db::fts::reindex_with_stemmer(&conn, stemmer),
            None => crate::db::fts::reindex(&conn),
        }
    }

    /// Generation counter bumped by every chunk text/row mutation.
    pub fn chunks_generation(&self) -> Result<u64> {
        let conn = self.lock()?;
        crate::db::fts::chunks_generation(&conn)
    }

    /// Ensure lexical search is ready (DuckDB BM25 or term-frequency fallback).
    ///
    /// Uses `stemmer` (e.g. `config.fts_stemmer` / `RAG_FTS_STEMMER`). Call after
    /// [`Store::open`] on server start so lex/hybrid search has a backend before
    /// the first MCP tool request.
    pub fn ensure_fts(&self, stemmer: &str) -> Result<crate::db::fts::FtsState> {
        let conn = self.lock()?;
        crate::db::fts::ensure_fts_with_stemmer(&conn, stemmer)
    }
}
