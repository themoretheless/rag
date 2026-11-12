//! Transport-independent status, integrity diagnostics, and doctor repair.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config::{Config, EmbeddingProviderKind};
use crate::db::Store;
use crate::db::schema::SCHEMA_VERSION;
use crate::db::store::embedding_manifest_matches_config;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::ingest::{IngestCommand, IngestService};
use crate::llm::{ChatClient, LlmApiDialect};
use crate::models::WingCount;
use crate::source_sync::SourceSyncError;

/// Extended health snapshot for the `status` MCP tool (MemPalace + vision §5.5).
///
/// Layer health (not only undifferentiated row counts): raw/wiki counts,
/// index coverage, uncompiled raw debt, embedding manifest match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    /// Storage backend kind (`duckdb` today).
    pub backend: String,
    #[serde(default)]
    pub storage_capabilities: Vec<String>,
    pub schema_version: i32,
    pub fts_ready: bool,
    pub document_count: u64,
    pub chunk_count: u64,
    pub node_count: u64,
    pub edge_count: u64,
    /// Documents with `layer=raw`.
    #[serde(default)]
    pub raw_count: u64,
    /// Documents with `layer=wiki`.
    #[serde(default)]
    pub wiki_count: u64,
    /// Rows in `wiki_index`.
    #[serde(default)]
    pub index_entry_count: u64,
    /// Wiki pages present in `wiki_index` / `wiki_count` (0 when no wiki).
    #[serde(default)]
    pub index_coverage: f64,
    /// Raw sources still treated as compile debt (raw without wiki corpus coverage).
    /// Currently equals `raw_count` when `wiki_count == 0`, else raw docs not yet
    /// referenced by any wiki graph edge (soft); falls back to `raw_count` if
    /// graph walk is empty.
    #[serde(default)]
    pub uncompiled_raw_count: u64,
    /// Live embedding identity matches stored `embedding_manifest`.
    /// Persisted canonical fingerprints are authoritative; legacy rows without
    /// one are unverifiable and remain fail-closed until a complete re-embed.
    #[serde(default)]
    pub embedding_manifest_match: bool,
    /// Configured embedding provider name.
    #[serde(default)]
    pub embed_provider: String,
    /// Configured embedding model id.
    #[serde(default)]
    pub embed_model: String,
    /// Distinct non-empty wings with document counts.
    pub wings: Vec<WingCount>,
    pub embed_dims: usize,
    pub ready_for_search: bool,
    pub ingest_roots_configured: bool,
    pub db_path: String,
    /// Gateway process id (single writer).
    #[serde(default)]
    pub pid: u32,
    /// Seconds since process start.
    #[serde(default)]
    pub uptime_seconds: u64,
    /// Main DuckDB file size in bytes when readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_file_bytes: Option<u64>,
    /// WAL file size in bytes (0 when absent).
    #[serde(default)]
    pub wal_bytes: u64,
    /// WAL warning threshold (`RAG_WAL_WARN_BYTES`).
    #[serde(default)]
    pub wal_warn_bytes: u64,
    /// Automatic snapshot footprint when `RAG_AUTO_BACKUP_DIR` is configured:
    /// retention, the bytes it holds on disk, and the volume's free space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_backup: Option<crate::ops::AutoBackupInventory>,
    /// FTS dirty marker (chunk mutations since last completed refresh).
    #[serde(default)]
    pub fts_dirty: bool,
    /// Gap at or above which the maintenance loop auto-reindexes
    /// (`RAG_FTS_STALE_GENERATION_THRESHOLD`; 0 disables).
    #[serde(default)]
    pub fts_stale_threshold: u64,
    /// Generations between source chunks and the completed FTS index
    /// (`None` when the index was never refreshed).
    #[serde(default)]
    pub fts_generation_gap: Option<u64>,
    /// Successful FTS refresh/reindex attempts (diagnostics).
    #[serde(default)]
    pub fts_rebuild_count: u64,
    /// True when `fts_dirty` and the gap reaches `fts_stale_threshold`.
    #[serde(default)]
    pub fts_stale: bool,
}

/// Row counts and identity only: the cheap view of [`StatusReport`] for
/// clients that poll (`GET /v1/status?summary=true`). Skips the layer-health
/// and wing scans, whose cost grows with the corpus and graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusSummary {
    pub backend: String,
    pub schema_version: i32,
    pub document_count: u64,
    pub chunk_count: u64,
    pub node_count: u64,
    pub edge_count: u64,
    pub db_path: String,
}

/// Local chat LLM + embedding config snapshot for the `llm_status` MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmStatusReport {
    /// Whether chat tools are enabled (`RAG_LLM_ENABLED`).
    pub llm_enabled: bool,
    /// Provider preset (`ollama`|`openai`|`codex`|`claude`|`kimi`|`deepseek`|`custom`).
    #[serde(default)]
    pub provider: String,
    /// Wire dialect: `openai_compat` or `anthropic_messages`.
    #[serde(default)]
    pub dialect: String,
    /// Chat API base URL (`RAG_LLM_BASE_URL` or provider default).
    pub base_url: String,
    /// Chat model tag (`RAG_LLM_MODEL`).
    pub model: String,
    /// True when a short probe against the chat endpoint succeeds.
    pub reachable: bool,
    /// Probe error detail when `reachable` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Embedding provider name (`mock` | `openai` | …).
    pub embed_provider: String,
    /// Embedding model id.
    pub embed_model: String,
    /// Configured embedding dimensionality.
    pub embed_dims: usize,
    /// Embedding API base URL when relevant (openai / local compat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed_base_url: Option<String>,
}

/// Minimal integrity report for the `doctor` MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub backend: String,
    #[serde(default)]
    pub storage_capabilities: Vec<String>,
    pub schema_version: i32,
    pub expected_schema_version: i32,
    pub schema_ok: bool,
    pub fts_ready: bool,
    pub document_count: u64,
    pub chunk_count: u64,
    pub node_count: u64,
    pub edge_count: u64,
    pub embed_dims: usize,
    /// Dims from `embedding_manifest` when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_dims: Option<i32>,
    /// True when the stored embedding identity matches live config (or when
    /// both the manifest and corpus chunks are absent).
    pub embed_ok: bool,
    pub ready_for_search: bool,
    pub ingest_roots_configured: bool,
    pub db_path: String,
    pub wal_bytes: u64,
    pub wal_warn_bytes: u64,
    pub wal_too_large: bool,
    pub documents_without_chunks: u64,
    pub orphan_chunks: u64,
    pub orphan_document_nodes: u64,
    pub orphan_edges: u64,
    pub unscoped_documents: u64,
    pub relational_integrity_ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair_hint: Option<String>,
    /// Critical integrity gate: schema current and embed dims consistent.
    pub ok: bool,
}

/// Outcome of `doctor_repair` (MCP) / `POST /v1/doctor/repair` (HTTP).
#[derive(Debug, Serialize)]
pub struct DoctorRepairReport {
    pub dry_run: bool,
    /// Terminal derived-index failures after already-durable repair work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
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
    /// Bind the service to a store and config.
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
        let mut mutation_guard = if dry_run {
            None
        } else {
            Some(
                self.store
                    .corpus_lane()
                    .try_corpus_mutation_guard("doctor repair")?,
            )
        };
        let cap = self.config.maint_max_docs.max(1);
        let max_docs = max_docs.unwrap_or(cap).clamp(1, cap);
        let before = crate::util::blocking_section(|| self.doctor())?;
        // Intentionally empty source placeholders are valid manifest entries,
        // not corrupt searchable documents. Keep this aligned with
        // Store::integrity_counts so doctor and doctor_repair agree. The SQL
        // anti-join applies the cap before document bodies enter Rust.
        let (_missing_total, missing) = crate::util::blocking_section(|| {
            self.store
                .documents()
                .list_documents_missing_chunks(max_docs)
        })?;

        let mut documents_repaired = Vec::new();
        let mut documents_failed = Vec::new();
        let mut errors = Vec::new();
        if !dry_run {
            let ingest = IngestService::new(self.store, embedder, self.config);
            // Embed the missing documents in shared provider calls; each one
            // still commits on its own.
            let commands: Vec<IngestCommand> = missing
                .iter()
                .map(|document| IngestCommand {
                    text: document.content.clone(),
                    title: Some(document.title.clone()),
                    uri: Some(document.uri.clone()),
                    metadata_json: Some(document.metadata_json.clone()),
                    wing: document.wing.clone(),
                    room: document.room.clone(),
                    source_file: document.source_file.clone(),
                    layer: document.layer.to_string(),
                    kind: document.kind.to_string(),
                    immutable: false,
                })
                .collect();
            for (document, result) in missing.iter().zip(ingest.ingest_many(commands).await) {
                match result {
                    Ok(_) => documents_repaired.push(document.id.clone()),
                    Err(error) => documents_failed.push(SourceSyncError {
                        path: document.uri.clone(),
                        error: error.to_string(),
                    }),
                }
            }
        }
        let (orphan_chunks_pruned, orphan_document_nodes_pruned, orphan_edges_pruned) =
            crate::util::blocking_section(|| self.store.maintenance().prune_orphans(dry_run))?;
        if !dry_run {
            if !documents_repaired.is_empty() || orphan_chunks_pruned > 0 {
                // Repair is a bounded corpus operation. Publish its derived
                // lexical state before the tool reports a healthy terminal
                // result instead of charging the next search for the rebuild.
                let store = self.store.clone();
                let stemmer = self.config.fts_stemmer.clone();
                let guard = mutation_guard
                    .take()
                    .expect("non-dry doctor repair owns the mutation lane");
                match tokio::task::spawn_blocking(move || {
                    let refresh_result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            #[cfg(test)]
                            crate::db::fts::test_fault::check("doctor", &stemmer)?;
                            store.maintenance().ensure_fts(&stemmer).map(|_| ())
                        }))
                        .unwrap_or_else(|_| {
                            Err(AppError::fts("doctor repair FTS finalization panicked"))
                        });
                    let refresh_result = refresh_result.map_err(|error| {
                        store
                            .corpus_lane()
                            .record_fts_finalization_failure(
                                "doctor_repair",
                                true,
                                &error.to_string(),
                            )
                            .message
                    });
                    (guard, refresh_result)
                })
                .await
                {
                    Ok((guard, Ok(()))) => mutation_guard = Some(guard),
                    Ok((guard, Err(error))) => {
                        mutation_guard = Some(guard);
                        errors.push(error);
                    }
                    Err(error) => {
                        mutation_guard = None;
                        errors.push(
                            self.store
                                .corpus_lane()
                                .record_fts_finalization_failure(
                                    "doctor_repair",
                                    true,
                                    &format!("doctor repair FTS refresh task failed: {error}"),
                                )
                                .message,
                        );
                    }
                }
            }
            crate::util::blocking_section(|| self.store.maintenance().checkpoint())?;
        }
        let after = if dry_run {
            before.clone()
        } else {
            crate::util::blocking_section(|| self.doctor())?
        };
        let report = DoctorRepairReport {
            dry_run,
            errors,
            documents_considered: missing.len(),
            documents_repaired,
            documents_failed,
            orphan_chunks_pruned,
            orphan_document_nodes_pruned,
            orphan_edges_pruned,
            before,
            after,
        };
        // Keep the guard through checkpoint and the terminal doctor snapshot.
        let _mutation_guard = mutation_guard;
        Ok(report)
    }

    /// Full status report: schema, counts, wings, layer health and embedding state.
    pub fn status(&self) -> Result<StatusReport, AppError> {
        let schema_version = self.store.maintenance().schema_version()?.unwrap_or(0);
        let fts_ready = self.store.maintenance().fts_ready()?;
        let fts_state = self.store.maintenance().fts_generation_state()?;
        let fts_generation_gap = crate::db::fts::fts_generation_gap(&fts_state);
        let fts_stale = crate::db::fts::fts_stale_needs_reindex(
            &fts_state,
            self.config.fts_stale_generation_threshold,
        );
        let (document_count, chunk_count, node_count, edge_count) =
            self.store.maintenance().stats()?;
        let wings = self.store.documents().list_wings()?;
        let layer_health = self.store.maintenance().layer_health_counts()?;
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
        let manifest = self.store.embedding_manifest().get_embedding_manifest()?;
        let embedding_manifest_match = manifest.as_ref().map_or(chunk_count == 0, |item| {
            embedding_manifest_matches_config(item, self.config)
        });

        Ok(StatusReport {
            backend: "duckdb".into(),
            storage_capabilities: crate::storage::duckdb_capability_names(),
            schema_version,
            fts_ready,
            fts_dirty: fts_state.dirty,
            fts_generation_gap,
            fts_rebuild_count: fts_state.rebuild_count,
            fts_stale_threshold: self.config.fts_stale_generation_threshold,
            fts_stale,
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
            ready_for_search: chunk_count > 0
                && schema_version >= SCHEMA_VERSION
                && embedding_manifest_match,
            ingest_roots_configured: !self.config.ingest_roots.is_empty(),
            db_path: self.store.path().display().to_string(),
            pid: runtime.pid,
            uptime_seconds: runtime.uptime_seconds,
            db_file_bytes: self.store.maintenance().db_file_size_bytes(),
            wal_bytes: self.store.maintenance().wal_file_size_bytes(),
            wal_warn_bytes: self.config.wal_warn_bytes,
            auto_backup: crate::ops::auto_backup_inventory(self.config),
        })
    }

    /// Lightweight status: backend, schema version and row counts.
    pub fn status_summary(&self) -> Result<StatusSummary, AppError> {
        let schema_version = self.store.maintenance().schema_version()?.unwrap_or(0);
        let (document_count, chunk_count, node_count, edge_count) =
            self.store.maintenance().stats()?;
        Ok(StatusSummary {
            backend: "duckdb".into(),
            schema_version,
            document_count,
            chunk_count,
            node_count,
            edge_count,
            db_path: self.store.path().display().to_string(),
        })
    }

    /// Run health checks (schema, FTS, embeddings, integrity) and report findings.
    pub fn doctor(&self) -> Result<DoctorReport, AppError> {
        let schema_version = self.store.maintenance().schema_version()?.unwrap_or(0);
        let schema_ok = schema_version >= SCHEMA_VERSION;
        let fts_ready = self.store.maintenance().fts_ready()?;
        let (document_count, chunk_count, node_count, edge_count) =
            self.store.maintenance().stats()?;
        let manifest = self.store.embedding_manifest().get_embedding_manifest()?;
        let manifest_dims = manifest.as_ref().map(|item| item.dims);
        let embed_ok = manifest.as_ref().map_or(chunk_count == 0, |item| {
            embedding_manifest_matches_config(item, self.config)
        });
        let (
            documents_without_chunks,
            orphan_chunks,
            orphan_document_nodes,
            orphan_edges,
            unscoped_documents,
        ) = self.store.maintenance().integrity_counts()?;
        let relational_integrity_ok =
            orphan_chunks == 0 && orphan_document_nodes == 0 && orphan_edges == 0;
        let wal_bytes = self.store.maintenance().wal_file_size_bytes();
        let wal_warn_bytes = self.config.wal_warn_bytes;
        let wal_too_large = wal_bytes >= wal_warn_bytes;
        let repair_hint = if !embed_ok {
            Some(
                "Embedding identity differs from runtime config; run a complete uncapped reembed_all before vector search."
                    .into(),
            )
        } else {
            repair_hint(
                relational_integrity_ok,
                documents_without_chunks,
                wal_too_large,
            )
        };
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

/// Periodic FTS staleness check (the maintenance loop for the derived lexical
/// index). When the dirty-generation gap reaches
/// `config.fts_stale_generation_threshold` the loop rebuilds the FTS index via
/// `ensure_fts` and records an ops-log entry (`op = "fts_auto_reindex"`).
/// `threshold == 0` disables auto-reindex entirely; staleness remains visible
/// in `/v1/status` either way. Interval: `config.fts_maint_interval_secs`
/// (0 disables the loop).
pub fn spawn_fts_maintenance(
    tasks: &crate::background::BackgroundTasks,
    store: Store,
    config: &Config,
) {
    if config.fts_stale_generation_threshold == 0 || config.fts_maint_interval_secs == 0 {
        return;
    }
    let threshold = config.fts_stale_generation_threshold;
    let stemmer = config.fts_stemmer.clone();
    let interval = std::time::Duration::from_secs(config.fts_maint_interval_secs);
    let loop_tasks = tasks.clone();
    tasks.spawn("fts-maintenance", async move {
        loop {
            if !loop_tasks.pause(interval).await {
                break;
            }
            let check_store = store.clone();
            let stemmer = stemmer.clone();
            let outcome = tokio::task::spawn_blocking(move || -> Result<Option<u64>, AppError> {
                let state = check_store.maintenance().fts_generation_state()?;
                if !crate::db::fts::fts_stale_needs_reindex(&state, threshold) {
                    return Ok(None);
                }
                let gap = crate::db::fts::fts_generation_gap(&state);
                check_store.maintenance().ensure_fts(&stemmer)?;
                Ok(Some(gap.unwrap_or(state.chunks_generation)))
            })
            .await;
            match outcome {
                Ok(Ok(Some(gap))) => {
                    tracing::warn!(
                        gap,
                        threshold,
                        "FTS stale beyond threshold; auto-reindex completed"
                    );
                    store.ops_log().record(
                        &crate::models::OpsLogEntry::new(
                            "fts_auto_reindex",
                            "FTS",
                            format!(
                                "FTS generation gap {gap} reached threshold {threshold}; reindexed"
                            ),
                        )
                        .entity(None, "fts")
                        .payload(
                            serde_json::json!({
                                "generation_gap": gap,
                                "threshold": threshold,
                            })
                            .to_string(),
                        ),
                    );
                }
                Ok(Ok(None)) => {}
                Ok(Err(error)) => {
                    tracing::error!(error = %error, "FTS maintenance check failed")
                }
                Err(error) => {
                    tracing::error!(error = %error, "FTS maintenance task failed")
                }
            }
        }
    });
}

#[cfg(test)]
mod tests;
