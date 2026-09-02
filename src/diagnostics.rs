//! Transport-independent status, integrity diagnostics, and doctor repair.

use std::sync::Arc;

use serde::Serialize;

use crate::config::{Config, EmbeddingProviderKind};
use crate::db::schema::SCHEMA_VERSION;
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::ingest::{IngestCommand, IngestService};
use crate::llm::{ChatClient, LlmApiDialect};
use crate::models::{DoctorReport, LlmStatusReport, StatusReport};
use crate::source_sync::SourceSyncError;

/// Outcome of `doctor_repair` (MCP) / `POST /v1/doctor/repair` (HTTP).
#[derive(Debug, Serialize)]
pub struct DoctorRepairReport {
    pub dry_run: bool,
    pub documents_considered: usize,
    pub documents_repaired: Vec<String>,
    pub documents_failed: Vec<SourceSyncError>,
    pub orphan_chunks_pruned: u64,
    pub orphan_document_nodes_pruned: u64,
    pub orphan_edges_pruned: u64,
    pub before: DoctorReport,
    pub after: DoctorReport,
}

/// Probe the configured chat LLM and describe embedding config (no corpus mutation).
///
/// Shared by MCP `llm_status` and `GET /v1/llm-status`.
pub async fn llm_status(config: &Config, llm: Option<&ChatClient>) -> LlmStatusReport {
    let (reachable, error) = match llm {
        Some(client) => {
            let probe = client.llm_status().await;
            let error = if probe.ok { None } else { Some(probe.detail) };
            (probe.ok, error)
        }
        None => (
            false,
            Some("ChatClient not configured; check RAG_LLM_BASE_URL and RAG_LLM_MODEL".to_string()),
        ),
    };
    let embed_base_url = match config.embedding_provider {
        EmbeddingProviderKind::Mock => None,
        EmbeddingProviderKind::OpenAi | EmbeddingProviderKind::Ollama => {
            Some(config.embedding_base_url.clone())
        }
    };
    let dialect = match config.llm_provider.dialect() {
        LlmApiDialect::OpenAiCompat => "openai_compat",
        LlmApiDialect::AnthropicMessages => "anthropic_messages",
    };
    LlmStatusReport {
        llm_enabled: config.llm_enabled,
        provider: config.llm_provider.as_str().to_string(),
        dialect: dialect.to_string(),
        base_url: config.llm_base_url.clone(),
        model: config.llm_model.clone(),
        reachable,
        error,
        embed_provider: config.embedding_provider.as_str().to_string(),
        embed_model: config.embedding_model.clone(),
        embed_dims: config.embedding_dims,
        embed_base_url,
    }
}

pub struct DiagnosticsService<'a> {
    store: &'a Store,
    config: &'a Config,
}

impl<'a> DiagnosticsService<'a> {
    pub fn new(store: &'a Store, config: &'a Config) -> Self {
        Self { store, config }
    }

    /// Preview (`dry_run=true`) or apply doctor findings: reingest non-schema documents
    /// that lost their chunks, prune orphan chunks / nodes / edges, then checkpoint.
    ///
    /// `max_docs` is clamped to `1..=RAG_MAINT_MAX_DOCS`.
    pub async fn repair(
        &self,
        embedder: &Arc<dyn EmbeddingProvider>,
        dry_run: bool,
        max_docs: Option<usize>,
    ) -> Result<DoctorRepairReport, AppError> {
        let cap = self.config.maint_max_docs.max(1);
        let max_docs = max_docs.unwrap_or(cap).clamp(1, cap);
        let before = self.doctor()?;
        let mut missing = Vec::new();
        for document in self.store.list_documents()? {
            // Intentionally empty source placeholders are valid manifest entries,
            // not corrupt searchable documents. Keep this aligned with
            // Store::integrity_counts so doctor and doctor_repair agree.
            if document.layer == "schema" || document.content.trim().is_empty() {
                continue;
            }
            if self
                .store
                .list_chunks_for_document(&document.id)?
                .is_empty()
            {
                missing.push(document);
                if missing.len() >= max_docs {
                    break;
                }
            }
        }

        let mut documents_repaired = Vec::new();
        let mut documents_failed = Vec::new();
        if !dry_run {
            let ingest = IngestService::new(self.store, embedder, self.config);
            for document in &missing {
                let command = IngestCommand {
                    text: document.content.clone(),
                    title: Some(document.title.clone()),
                    uri: Some(document.uri.clone()),
                    metadata_json: Some(document.metadata_json.clone()),
                    wing: document.wing.clone(),
                    room: document.room.clone(),
                    source_file: document.source_file.clone(),
                    layer: document.layer.clone(),
                    kind: document.kind.clone(),
                    immutable: false,
                };
                match ingest.ingest(command).await {
                    Ok(_) => documents_repaired.push(document.id.clone()),
                    Err(error) => documents_failed.push(SourceSyncError {
                        path: document.uri.clone(),
                        error: error.to_string(),
                    }),
                }
            }
        }
        let (orphan_chunks_pruned, orphan_document_nodes_pruned, orphan_edges_pruned) =
            self.store.prune_orphans(dry_run)?;
        if !dry_run {
            self.store.checkpoint()?;
        }
        let after = if dry_run {
            before.clone()
        } else {
            self.doctor()?
        };
        Ok(DoctorRepairReport {
            dry_run,
            documents_considered: missing.len(),
            documents_repaired,
            documents_failed,
            orphan_chunks_pruned,
            orphan_document_nodes_pruned,
            orphan_edges_pruned,
            before,
            after,
        })
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
        let runtime = crate::ops::runtime_snapshot();
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
            pid: runtime.pid,
            uptime_seconds: runtime.uptime_seconds,
            db_file_bytes: self.store.db_file_size_bytes(),
            wal_bytes: self.store.wal_file_size_bytes(),
            wal_warn_bytes: crate::ops::wal_warn_bytes(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbedder;
    use crate::models::Document;

    #[tokio::test]
    async fn repair_ignores_intentionally_empty_documents() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("empty-document.duckdb");
        let store = Store::open(&path).expect("open store");
        store
            .upsert_document(&Document {
                id: "empty-source".into(),
                uri: "file://empty.md".into(),
                title: "Empty source".into(),
                content: "   ".into(),
                ..Document::default()
            })
            .expect("insert empty document");

        let config = Config {
            db_path: path,
            embedding_dims: 8,
            ..Config::for_tests()
        };
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
        let report = DiagnosticsService::new(&store, &config)
            .repair(&embedder, true, Some(10))
            .await
            .expect("dry-run repair");

        assert_eq!(report.before.documents_without_chunks, 0);
        assert_eq!(report.documents_considered, 0);
        assert!(report.documents_repaired.is_empty());
    }
}
