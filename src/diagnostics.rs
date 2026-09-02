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
        let layer_health = self.store.layer_health_counts()?;
        let raw_count = layer_health.raw_count;
        let wiki_count = layer_health.wiki_count;
        let index_entry_count = layer_health.index_entry_count;
        let indexed_pages = layer_health.indexed_pages;
        let index_coverage = if wiki_count == 0 {
            0.0
        } else {
            indexed_pages as f64 / wiki_count as f64
        };
        let uncompiled_raw_count = layer_health.uncompiled_raw_count;
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
    use crate::models::{Document, GraphEdge, GraphNode, WikiIndexEntry};

    #[test]
    fn status_layer_health_counts_match_legacy_semantics() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("status-layer-health.duckdb");
        let store = Store::open(&path).expect("open store");

        for (id, layer) in [
            ("raw-no-node", "raw"),
            ("raw-non-wiki-backlink", "raw"),
            ("raw-wiki-backlink", "raw"),
            ("raw-unresolved-wiki-backlink", "raw"),
        ] {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("file://{id}.md"),
                    title: id.into(),
                    layer: layer.into(),
                    ..Document::default()
                })
                .expect("insert raw document");
        }

        for node in [
            graph_node(
                "target-non-wiki",
                Some("raw-non-wiki-backlink"),
                Some("file://raw-non-wiki-backlink.md"),
                true,
            ),
            graph_node("source-non-wiki", None, Some("file://notes.md"), true),
            graph_node(
                "target-wiki",
                Some("raw-wiki-backlink"),
                Some("file://raw-wiki-backlink.md"),
                true,
            ),
            graph_node("source-wiki", None, Some("wiki://compiled"), true),
            graph_node(
                "target-unresolved",
                Some("raw-unresolved-wiki-backlink"),
                Some("file://raw-unresolved-wiki-backlink.md"),
                true,
            ),
            graph_node("source-unresolved-wiki", None, Some("wiki://draft"), false),
        ] {
            store.upsert_graph_node(&node).expect("insert graph node");
        }
        store
            .insert_graph_edges(&[
                graph_edge("edge-non-wiki", "source-non-wiki", "target-non-wiki"),
                graph_edge("edge-wiki", "source-wiki", "target-wiki"),
                graph_edge(
                    "edge-unresolved-wiki",
                    "source-unresolved-wiki",
                    "target-unresolved",
                ),
            ])
            .expect("insert graph edges");

        // Legacy behavior deliberately treats every raw as uncompiled when the
        // corpus contains no layer=wiki document, even if a wiki:// source node exists.
        let without_wiki = store.layer_health_counts().expect("layer health");
        assert_eq!(without_wiki.raw_count, 4);
        assert_eq!(without_wiki.wiki_count, 0);
        assert_eq!(without_wiki.index_entry_count, 0);
        assert_eq!(without_wiki.indexed_pages, 0);
        assert_eq!(without_wiki.uncompiled_raw_count, 4);

        for id in ["wiki-indexed", "wiki-unindexed"] {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("wiki://{id}"),
                    title: id.into(),
                    layer: "wiki".into(),
                    kind: "wiki".into(),
                    ..Document::default()
                })
                .expect("insert wiki document");
        }
        for (id, page_id) in [
            ("index-present", Some("wiki-indexed")),
            ("index-orphan", Some("missing-wiki")),
        ] {
            store
                .upsert_wiki_index_entry(&WikiIndexEntry {
                    id: id.into(),
                    slug: id.into(),
                    title: id.into(),
                    kind: "wiki".into(),
                    category: None,
                    summary: None,
                    page_id: page_id.map(str::to_string),
                    updated_at: chrono::Utc::now(),
                })
                .expect("insert wiki index entry");
        }

        let counts = store.layer_health_counts().expect("layer health");
        assert_eq!(counts.raw_count, 4);
        assert_eq!(counts.wiki_count, 2);
        assert_eq!(counts.index_entry_count, 2);
        assert_eq!(counts.indexed_pages, 1);
        assert_eq!(counts.uncompiled_raw_count, 3);

        let config = Config {
            db_path: path,
            ..Config::for_tests()
        };
        let report = DiagnosticsService::new(&store, &config)
            .status()
            .expect("status");
        assert_eq!(report.raw_count, counts.raw_count);
        assert_eq!(report.wiki_count, counts.wiki_count);
        assert_eq!(report.index_entry_count, counts.index_entry_count);
        assert_eq!(report.index_coverage, 0.5);
        assert_eq!(report.uncompiled_raw_count, counts.uncompiled_raw_count);
    }

    fn graph_node(
        id: &str,
        document_id: Option<&str>,
        uri: Option<&str>,
        resolved: bool,
    ) -> GraphNode {
        GraphNode {
            id: id.into(),
            kind: "document".into(),
            label: id.into(),
            document_id: document_id.map(str::to_string),
            uri: uri.map(str::to_string),
            resolved,
            metadata_json: "{}".into(),
        }
    }

    fn graph_edge(id: &str, source_id: &str, target_id: &str) -> GraphEdge {
        GraphEdge {
            id: id.into(),
            source_id: source_id.into(),
            target_id: target_id.into(),
            rel_type: "related".into(),
            weight: 1.0,
            context: None,
        }
    }

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
