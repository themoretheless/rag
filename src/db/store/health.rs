//! Counts, layer health, integrity, pruning, vacuum, schema and FTS state.

use super::*;
use crate::db::lin_engine::{
    cell_nonempty_string, cell_opt_string, cell_str, cell_string, lin_disk_bytes,
};

/// Largest uncompiled-raw sample or shelf queue read at once.
const MAX_UNCOMPILED_ROWS: usize = 200;

impl Store {
    /// Counts: `(documents, chunks, graph_nodes, graph_edges)`.
    pub fn stats(&self) -> Result<(u64, u64, u64, u64)> {
        let r = self.read()?;
        let docs = r.count_rows("documents", None)? as u64;
        let chunks = r.count_rows("chunks", None)? as u64;
        let nodes = r.count_rows("graph_nodes", None)? as u64;
        let edges = r.count_rows("graph_edges", None)? as u64;
        Ok((docs, chunks, nodes, edges))
    }

    /// Count raw/wiki compilation health without materializing document bodies.
    pub(crate) fn layer_health_counts(&self) -> Result<LayerHealthCounts> {
        let r = self.read()?;
        let mut raw_count = 0u64;
        let mut wiki_count = 0u64;
        let mut wiki_ids = std::collections::HashSet::new();
        r.scan_each_projected("documents", &["id", "layer"], |row| {
            match cell_str(row, "layer") {
                Some("raw") => raw_count += 1,
                Some("wiki") => {
                    wiki_count += 1;
                    if let Some(id) = cell_str(row, "id") {
                        wiki_ids.insert(id.to_string());
                    }
                }
                _ => {}
            }
            Ok(true)
        })?;

        let index_entry_count = r.count_rows("wiki_index", None)? as u64;
        let mut indexed_doc_ids = std::collections::HashSet::new();
        r.scan_each_projected("wiki_index", &["page_id", "document_id"], |row| {
            if let Some(pid) = cell_nonempty_string(row, "page_id")
                .or_else(|| cell_nonempty_string(row, "document_id"))
            {
                indexed_doc_ids.insert(pid);
            }
            Ok(true)
        })?;
        let indexed_pages = wiki_ids
            .iter()
            .filter(|id| indexed_doc_ids.contains(*id))
            .count() as u64;

        let uncompiled_raw_count = if wiki_count == 0 {
            raw_count
        } else {
            let compiled = compiled_document_ids(&r)?;
            let mut uncompiled = 0u64;
            r.scan_each_projected("documents", &["id", "layer"], |row| {
                if cell_str(row, "layer") == Some("raw")
                    && let Some(id) = cell_str(row, "id")
                    && !compiled.contains(id)
                {
                    uncompiled += 1;
                }
                Ok(true)
            })?;
            uncompiled
        };

        Ok(LayerHealthCounts {
            raw_count,
            wiki_count,
            index_entry_count,
            indexed_pages,
            uncompiled_raw_count,
        })
    }

    /// Bounded sample of uncompiled raw docs for lint guidance.
    pub fn list_uncompiled_raw_sample(&self, limit: usize) -> Result<Vec<UncompiledRawSample>> {
        let limit = limit.clamp(1, MAX_UNCOMPILED_ROWS);
        let r = self.read()?;
        let mut wiki_count = 0usize;
        r.for_each_by_eq_projected("documents", "layer", "wiki", Some(&["id"]), |_| {
            wiki_count += 1;
            Ok(true)
        })?;
        let compiled = if wiki_count == 0 {
            std::collections::HashSet::new()
        } else {
            compiled_document_ids(&r)?
        };

        let mut rows_out: Vec<UncompiledRawSample> = Vec::new();
        r.for_each_by_eq_projected(
            "documents",
            "layer",
            "raw",
            Some(&["id", "uri", "wing", "room"]),
            |row| {
                let Some(id) = cell_str(row, "id") else {
                    return Ok(true);
                };
                if wiki_count == 0 || !compiled.contains(id) {
                    rows_out.push(UncompiledRawSample {
                        document_id: id.to_string(),
                        uri: cell_string(row, "uri"),
                        wing: cell_opt_string(row, "wing"),
                        room: cell_opt_string(row, "room"),
                    });
                }
                Ok(true)
            },
        )?;

        rows_out.sort_by(|a, b| {
            opt_str_nulls_last(&a.wing, &b.wing)
                .then_with(|| opt_str_nulls_last(&a.room, &b.room))
                .then_with(|| a.uri.cmp(&b.uri))
        });
        rows_out.truncate(limit);
        Ok(rows_out)
    }

    /// The uncompiled debt grouped by `(wing, room)` shelf, biggest first, plus how many
    /// shelves the debt spans.
    pub(crate) fn uncompiled_raw_shelves(&self, limit: usize) -> Result<UncompiledShelfQueue> {
        let limit = limit.clamp(1, MAX_UNCOMPILED_ROWS);
        let r = self.read()?;
        let mut wiki_count = 0usize;
        r.for_each_by_eq_projected("documents", "layer", "wiki", Some(&["id"]), |_| {
            wiki_count += 1;
            Ok(true)
        })?;
        let compiled = if wiki_count == 0 {
            std::collections::HashSet::new()
        } else {
            compiled_document_ids(&r)?
        };

        let mut grouped: std::collections::HashMap<(Option<String>, Option<String>), u64> =
            std::collections::HashMap::new();
        r.for_each_by_eq_projected(
            "documents",
            "layer",
            "raw",
            Some(&["id", "wing", "room"]),
            |row| {
                let Some(id) = cell_str(row, "id") else {
                    return Ok(true);
                };
                if wiki_count == 0 || !compiled.contains(id) {
                    let key = (cell_opt_string(row, "wing"), cell_opt_string(row, "room"));
                    *grouped.entry(key).or_insert(0) += 1;
                }
                Ok(true)
            },
        )?;

        let total_shelves = grouped.len() as u64;
        let mut shelves: Vec<UncompiledRawShelf> = grouped
            .into_iter()
            .map(|((wing, room), docs)| UncompiledRawShelf { wing, room, docs })
            .collect();
        shelves.sort_by(|a, b| {
            b.docs
                .cmp(&a.docs)
                .then_with(|| opt_str_nulls_last(&a.wing, &b.wing))
                .then_with(|| opt_str_nulls_last(&a.room, &b.room))
        });
        shelves.truncate(limit);
        Ok(UncompiledShelfQueue {
            total_shelves,
            shelves,
        })
    }

    /// Filesystem size of the `lin` directory (or portable backup file), when readable.
    pub fn db_file_size_bytes(&self) -> Option<u64> {
        lin_disk_bytes(&self.path)
    }

    /// Size in bytes of the `lin` WAL log file (`<path>/wal.v2.log` or legacy `<path>/log`), or 0 when absent.
    pub fn wal_file_size_bytes(&self) -> u64 {
        let clean = crate::db::lin_engine::normalize_lin_path(&self.path);
        if clean.is_dir() {
            if lin::disk_v2::is_disk_v2_dir(&clean) {
                std::fs::metadata(clean.join(lin::disk_v2::WAL_FILE))
                    .map(|m| m.len())
                    .unwrap_or(0)
            } else {
                std::fs::metadata(clean.join("log"))
                    .map(|m| m.len())
                    .unwrap_or(0)
            }
        } else {
            0
        }
    }

    /// Run a `lin` `checkpoint()` to compact the WAL into segments/snapshot.
    pub fn checkpoint(&self) -> Result<()> {
        self.write()?.checkpoint()
    }

    /// Counts for relationships not enforced with foreign keys.
    pub fn integrity_counts(&self) -> Result<(u64, u64, u64, u64, u64)> {
        let r = self.read()?;
        let mut doc_ids = std::collections::HashSet::new();
        let mut non_schema_non_empty_docs = std::collections::HashSet::new();
        let mut unscoped_documents = 0u64;

        r.scan_each_projected(
            "documents",
            &["id", "layer", "content", "wing"],
            |row| {
                if let Some(id) = cell_str(row, "id") {
                    doc_ids.insert(id.to_string());
                    let layer = cell_str(row, "layer").unwrap_or("");
                    if layer != "schema" {
                        let content = cell_str(row, "content").map(str::trim).unwrap_or("");
                        if !content.is_empty() {
                            non_schema_non_empty_docs.insert(id.to_string());
                        }
                    }
                }
                if cell_str(row, "wing").map(str::trim).unwrap_or("").is_empty() {
                    unscoped_documents += 1;
                }
                Ok(true)
            },
        )?;

        let mut docs_with_chunks = std::collections::HashSet::new();
        let mut orphan_chunks = 0u64;
        r.scan_each_projected("chunks", &["document_id"], |row| {
            let doc_id = cell_str(row, "document_id").unwrap_or("");
            if doc_ids.contains(doc_id) {
                docs_with_chunks.insert(doc_id.to_string());
            } else {
                orphan_chunks += 1;
            }
            Ok(true)
        })?;

        let documents_without_chunks = non_schema_non_empty_docs
            .iter()
            .filter(|id| !docs_with_chunks.contains(*id))
            .count() as u64;

        let mut node_ids = std::collections::HashSet::new();
        let mut orphan_document_nodes = 0u64;
        r.scan_each_projected("graph_nodes", &["id", "document_id"], |row| {
            if let Some(id) = cell_str(row, "id") {
                node_ids.insert(id.to_string());
            }
            if let Some(doc_id) = cell_str(row, "document_id")
                && !doc_ids.contains(doc_id)
            {
                orphan_document_nodes += 1;
            }
            Ok(true)
        })?;

        let mut orphan_edges = 0u64;
        r.scan_each_projected("graph_edges", &["source_id", "target_id"], |row| {
            let src = cell_str(row, "source_id").unwrap_or("");
            let tgt = cell_str(row, "target_id").unwrap_or("");
            if !node_ids.contains(src) || !node_ids.contains(tgt) {
                orphan_edges += 1;
            }
            Ok(true)
        })?;

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

        self.with_tx(|tx| {
            if chunks > 0 {
                crate::db::fts::mark_fts_dirty(tx)?;
            }

            let mut doc_ids = std::collections::HashSet::new();
            tx.scan_each_projected("documents", &["id"], |row| {
                if let Some(id) = cell_str(row, "id") {
                    doc_ids.insert(id.to_string());
                }
                Ok(true)
            })?;

            let mut dead_node_ids = Vec::new();
            tx.scan_each_projected("graph_nodes", &["id", "document_id"], |row| {
                if let Some(doc_id) = cell_str(row, "document_id")
                    && !doc_ids.contains(doc_id)
                    && let Some(id) = cell_str(row, "id")
                {
                    dead_node_ids.push(id.to_string());
                }
                Ok(true)
            })?;
            tx.delete_by_ids("graph_nodes", &dead_node_ids)?;

            #[cfg(test)]
            if PRUNE_ORPHANS_FAIL_AFTER_NODES.with(std::cell::Cell::get) {
                return Err(AppError::db(
                    "injected prune_orphans failure after node delete",
                ));
            }

            let mut live_node_ids = std::collections::HashSet::new();
            tx.scan_each_projected("graph_nodes", &["id"], |row| {
                if let Some(id) = cell_str(row, "id") {
                    live_node_ids.insert(id.to_string());
                }
                Ok(true)
            })?;

            let mut dead_edge_ids = Vec::new();
            tx.scan_each_projected("graph_edges", &["id", "source_id", "target_id"], |row| {
                let src = cell_str(row, "source_id").unwrap_or("");
                let tgt = cell_str(row, "target_id").unwrap_or("");
                if (!live_node_ids.contains(src) || !live_node_ids.contains(tgt))
                    && let Some(id) = cell_str(row, "id")
                {
                    dead_edge_ids.push(id.to_string());
                }
                Ok(true)
            })?;
            tx.delete_by_ids("graph_edges", &dead_edge_ids)?;

            let mut dead_chunk_ids = Vec::new();
            tx.scan_each_projected("chunks", &["id", "document_id"], |row| {
                let doc_id = cell_str(row, "document_id").unwrap_or("");
                if !doc_ids.contains(doc_id)
                    && let Some(id) = cell_str(row, "id")
                {
                    dead_chunk_ids.push(id.to_string());
                }
                Ok(true)
            })?;
            tx.delete_by_ids("chunks", &dead_chunk_ids)?;

            Ok((chunks, nodes, edges))
        })
    }

    /// Run out-of-core segment compaction (`DiskEngineV2::vacuum`) + `MmapBm25Index` refresh
    /// and report total directory size before/after when possible.
    pub fn vacuum_store(&self) -> Result<VacuumStoreReport> {
        let db_path = self.path.display().to_string();
        let bytes_before = self.db_file_size_bytes();

        let compaction = self.write()?.vacuum()?;
        if let Ok(Some(st)) = self.fts_status()
            && st.backend == crate::db::fts::FtsBackend::DuckDbBm25
        {
            let _ = crate::db::fts::persist_bm25_disk_cache(self, &st.stemmer);
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
            "tables_compacted": compaction.tables_compacted,
            "old_segments_removed": compaction.old_segments_removed,
            "new_segments_written": compaction.new_segments_written,
            "dead_rows_reclaimed": compaction.dead_rows_reclaimed,
        });
        let _ = self.append_ops_log(&OpsLogEntry {
            id: String::new(),
            seq: 0,
            ts: Utc::now(),
            op: "vacuum_store".into(),
            prefix: Some("MAINT".into()),
            message: "lin VACUUM + COMPACT".into(),
            entity_id: None,
            entity_kind: Some("store".into()),
            payload_json: payload.to_string(),
            agent_name: None,
        });

        Ok(report)
    }

    /// Highest applied schema version, if recorded.
    pub fn schema_version(&self) -> Result<Option<i32>> {
        let r = self.read()?;
        schema::current_schema_version(&r)
    }

    /// Whether the BM25 FTS index is active and ready.
    pub fn fts_ready(&self) -> Result<bool> {
        let r = self.read()?;
        Ok(crate::db::fts::fts_index_present(&r))
    }

    /// Last-known FTS backend from `meta`, if `ensure_fts` has run.
    pub fn fts_status(&self) -> Result<Option<crate::db::fts::FtsState>> {
        let r = self.read()?;
        crate::db::fts::fts_status(&r)
    }

    /// Rebuild the lexical index now; `None`/blank keeps the recorded stemmer.
    pub fn reindex_fts(&self, stemmer: Option<&str>) -> Result<crate::db::fts::FtsState> {
        let mut w = self.write()?;
        match stemmer.map(str::trim).filter(|s| !s.is_empty()) {
            Some(stemmer) => crate::db::fts::reindex_with_stemmer(&mut w, stemmer),
            None => crate::db::fts::reindex(&mut w),
        }
    }

    /// Generation counter bumped by every chunk text/row mutation.
    pub fn chunks_generation(&self) -> Result<u64> {
        let r = self.read()?;
        crate::db::fts::chunks_generation(&r)
    }

    /// Ensure lexical search is ready (BM25 or term-frequency fallback).
    pub fn ensure_fts(&self, stemmer: &str) -> Result<crate::db::fts::FtsState> {
        let mut w = self.write()?;
        crate::db::fts::ensure_fts_with_stemmer(&mut w, stemmer)
    }
}

fn opt_str_nulls_last(a: &Option<String>, b: &Option<String>) -> std::cmp::Ordering {
    match (a.as_deref(), b.as_deref()) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}
