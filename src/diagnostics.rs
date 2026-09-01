//! Transport-independent status and integrity diagnostics.

use crate::config::Config;
use crate::db::schema::SCHEMA_VERSION;
use crate::db::Store;
use crate::error::AppError;
use crate::models::{DoctorReport, StatusReport};

pub struct DiagnosticsService<'a> {
    store: &'a Store,
    config: &'a Config,
}

impl<'a> DiagnosticsService<'a> {
    pub fn new(store: &'a Store, config: &'a Config) -> Self {
        Self { store, config }
    }

    pub fn status(&self) -> Result<StatusReport, AppError> {
        let schema_version = self.store.schema_version()?.unwrap_or(0);
        let fts_ready = self.store.fts_ready()?;
        let (document_count, chunk_count, node_count, edge_count) = self.store.stats()?;
        let wings = self.store.list_wings()?;
        let raw_documents = self.store.list_documents_by_layer("raw")?;
        let wiki_documents = self.store.list_documents_by_layer("wiki")?;
        let raw_count = raw_documents.len() as u64;
        let wiki_count = wiki_documents.len() as u64;
        let index = self.store.list_wiki_index()?;
        let index_entry_count = index.len() as u64;
        let indexed_pages = wiki_documents
            .iter()
            .filter(|document| {
                index
                    .iter()
                    .any(|entry| entry.page_id.as_deref() == Some(document.id.as_str()))
            })
            .count() as u64;
        let index_coverage = if wiki_count == 0 {
            0.0
        } else {
            indexed_pages as f64 / wiki_count as f64
        };
        let uncompiled_raw_count = self.uncompiled_raw_count(&raw_documents, wiki_count)?;
        let manifest = self.store.get_embedding_manifest()?;
        let embedding_manifest_match = manifest.as_ref().map_or(chunk_count == 0, |item| {
            item.dims as usize == self.config.embedding_dims
                && item.provider == self.config.embedding_provider.as_str()
                && item.model == self.config.embedding_model
        });

        Ok(StatusReport {
            backend: "duckdb".into(),
            storage_capabilities: crate::storage::duckdb_capability_names(),
            schema_version,
            fts_ready,
            document_count,
            chunk_count,
            node_count,
            edge_count,
            raw_count,
            wiki_count,
            index_entry_count,
            index_coverage,
            uncompiled_raw_count,
            embedding_manifest_match,
            embed_provider: self.config.embedding_provider.as_str().to_string(),
            embed_model: self.config.embedding_model.clone(),
            wings,
            embed_dims: self.config.embedding_dims,
            ready_for_search: chunk_count > 0 && schema_version >= SCHEMA_VERSION,
            ingest_roots_configured: !self.config.ingest_roots.is_empty(),
            db_path: self.store.path().display().to_string(),
        })
    }

    pub fn doctor(&self) -> Result<DoctorReport, AppError> {
        let schema_version = self.store.schema_version()?.unwrap_or(0);
        let schema_ok = schema_version >= SCHEMA_VERSION;
        let fts_ready = self.store.fts_ready()?;
        let (document_count, chunk_count, node_count, edge_count) = self.store.stats()?;
        let manifest_dims = self.store.get_embedding_manifest()?.map(|item| item.dims);
        let embed_ok = manifest_dims
            .map(|dims| dims as usize == self.config.embedding_dims)
            .unwrap_or(true);
        let (
            documents_without_chunks,
            orphan_chunks,
            orphan_document_nodes,
            orphan_edges,
            unscoped_documents,
        ) = self.store.integrity_counts()?;
        let relational_integrity_ok =
            orphan_chunks == 0 && orphan_document_nodes == 0 && orphan_edges == 0;
        let wal_bytes = self.store.wal_file_size_bytes();
        let wal_warn_bytes = crate::ops::wal_warn_bytes();
        let wal_too_large = wal_bytes >= wal_warn_bytes;
        let repair_hint = repair_hint(
            relational_integrity_ok,
            documents_without_chunks,
            wal_too_large,
        );
        Ok(DoctorReport {
            backend: "duckdb".into(),
            storage_capabilities: crate::storage::duckdb_capability_names(),
            schema_version,
            expected_schema_version: SCHEMA_VERSION,
            schema_ok,
            fts_ready,
            document_count,
            chunk_count,
            node_count,
            edge_count,
            embed_dims: self.config.embedding_dims,
            manifest_dims,
            embed_ok,
            ready_for_search: chunk_count > 0 && schema_ok && embed_ok,
            ingest_roots_configured: !self.config.ingest_roots.is_empty(),
            db_path: self.store.path().display().to_string(),
            wal_bytes,
            wal_warn_bytes,
            wal_too_large,
            documents_without_chunks,
            orphan_chunks,
            orphan_document_nodes,
            orphan_edges,
            unscoped_documents,
            relational_integrity_ok,
            repair_hint,
            ok: schema_ok && embed_ok && relational_integrity_ok && documents_without_chunks == 0,
        })
    }

    fn uncompiled_raw_count(
        &self,
        raw_documents: &[crate::models::Document],
        wiki_count: u64,
    ) -> Result<u64, AppError> {
        if wiki_count == 0 {
            return Ok(raw_documents.len() as u64);
        }
        let mut count = 0;
        for document in raw_documents {
            let Some(node) = self.store.find_node_by_document_id(&document.id)? else {
                count += 1;
                continue;
            };
            let referenced_by_wiki = self.store.backlinks(&node.id)?.nodes.iter().any(|source| {
                source.id != node.id
                    && source.resolved
                    && source
                        .uri
                        .as_deref()
                        .is_some_and(|uri| uri.starts_with("wiki://"))
            });
            if !referenced_by_wiki {
                count += 1;
            }
        }
        Ok(count)
    }
}

fn repair_hint(
    relational_integrity_ok: bool,
    documents_without_chunks: u64,
    wal_too_large: bool,
) -> Option<String> {
    if !relational_integrity_ok {
        Some("Create a backup, then run db_repair offline and maintain_refresh.".into())
    } else if documents_without_chunks > 0 {
        Some(
            "Reingest documents without chunks; sync_sources repairs unchanged file documents too."
                .into(),
        )
    } else if wal_too_large {
        Some("WAL exceeds the configured warning threshold; run a checkpointed backup or vacuum_store.".into())
    } else {
        None
    }
}
