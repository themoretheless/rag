//! Backup and portable recovery bundle MCP tools.

use std::path::Path;
use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::facade::retain_mutation_guard_while_blocking;
use super::facade::RagServer;
use super::tools::{BackupDbParams, ExportBundleParams, ExportVaultParams, ImportBundleParams};
use crate::config::Config;
use crate::db::recovery::{
    decode_recovery_bundle, encode_recovery_bundle, preflight_recovery_bundle,
    preflight_recovery_bundle_reembed, publish_recovery_artifact, read_recovery_bundle_file,
    BundleExportReport, ConflictPolicy, RecoveryBundle, BUNDLE_VERSION,
};
use crate::db::store::embedding_manifest_from_config;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::util::{
    refuse_live_database_target, resolve_allowlisted_output_file, run_blocking,
    validate_backup_output_paths,
};

#[tool_router(router = recovery_router, vis = "pub(super)")]
impl RagServer {
    #[tool(
        name = "backup_db",
        description = "Create and verify an MVCC-consistent DuckDB snapshot. CHECKPOINT and snapshot-connection cloning briefly hold the Store mutex; the potentially long database copy does not block normal Store queries. path must be under RAG_INGEST_ROOTS. dry_run=false writes; overwrite defaults false."
    )]
    pub(super) async fn backup_db(
        &self,
        Parameters(params): Parameters<BackupDbParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = resolve_allowlisted_output_file(&params.path, &self.config.ingest_roots)?;
        validate_backup_output_paths(&path, &self.config.ingest_roots, self.store.path())?;
        let store = self.store.clone();
        let dry_run = params.dry_run.unwrap_or(false);
        let overwrite = params.overwrite.unwrap_or(false);
        let report = run_blocking("backup", move || {
            store.backup_database(&path, dry_run, overwrite)
        })
        .await?;
        Self::json_result(&report)
    }

    #[tool(
        name = "export_bundle",
        description = "Export a bounded in-memory portable JSON/JSONL recovery bundle (maximum 64 MiB, 10,000 documents, 50,000 chunks). path must be under RAG_INGEST_ROOTS; never overwrites unless overwrite=true. For a larger corpus, use backup_db and verify the DuckDB backup."
    )]
    pub(super) async fn export_bundle(
        &self,
        Parameters(params): Parameters<ExportBundleParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = resolve_allowlisted_output_file(&params.path, &self.config.ingest_roots)?;
        refuse_live_database_target(&path, self.store.path())?;
        let format = recovery_format(params.format.as_deref(), &path)?;
        let overwrite = params.overwrite.unwrap_or(false);
        let dry_run = params.dry_run.unwrap_or(false);
        let store = self.store.clone();
        let report = run_blocking("export_bundle", move || {
            let existed = path.exists();
            if existed && !overwrite {
                return Err(AppError::conflict(format!(
                    "bundle destination '{}' already exists; set overwrite=true explicitly",
                    path.display()
                )));
            }
            store.portable_recovery_preflight()?;
            let bundle = store.recovery_bundle()?;
            let documents = bundle.documents.len() as u64;
            let chunks = bundle.documents.iter().map(|d| d.chunks.len() as u64).sum();
            let encoded = encode_recovery_bundle(&bundle, format)?;
            let overwritten = if dry_run {
                existed && overwrite
            } else {
                publish_recovery_artifact(&path, &encoded, overwrite)?
            };
            Ok(BundleExportReport {
                success: true,
                dry_run,
                path: path.display().to_string(),
                format: format.into(),
                overwritten,
                documents,
                chunks,
                bytes: Some(encoded.len() as u64),
                errors: Vec::new(),
            })
        })
        .await?;
        Self::json_result(&report)
    }

    #[tool(
        name = "export_vault",
        description = "Export a git-friendly Markdown vault partitioned by projects/<wing>/<room>/<layer>, with .rag graph, ops log, and embedding manifest JSON. path must be under RAG_INGEST_ROOTS. dry_run defaults true; overwrite retains the prior vault as a dated sibling."
    )]
    pub(super) async fn export_vault(
        &self,
        Parameters(params): Parameters<ExportVaultParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = resolve_allowlisted_output_file(&params.path, &self.config.ingest_roots)?;
        refuse_live_database_target(&path, self.store.path())?;
        let store = self.store.clone();
        let dry_run = params.dry_run.unwrap_or(true);
        let overwrite = params.overwrite.unwrap_or(false);
        let report = run_blocking("export_vault", move || {
            store.export_vault(&path, dry_run, overwrite)
        })
        .await?;
        Self::json_result(&report)
    }

    #[tool(
        name = "import_bundle",
        description = "Import a bounded in-memory portable JSON/JSONL recovery bundle (maximum 64 MiB, 10,000 documents, 50,000 chunks) from RAG_INGEST_ROOTS. dry_run defaults true. conflict_policy is error (default), skip, or overwrite; overwrite must be explicit. Legacy v1 chunks require reembed_legacy=true. For a larger corpus, use a verified DuckDB backup."
    )]
    pub(super) async fn import_bundle(
        &self,
        Parameters(params): Parameters<ImportBundleParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = resolve_allowlisted_output_file(&params.path, &self.config.ingest_roots)?;
        if !path.is_file() {
            return Err(McpError::from(AppError::not_found(format!(
                "bundle file '{}'",
                path.display()
            ))));
        }
        let format = recovery_format(params.format.as_deref(), &path)?;
        let policy = ConflictPolicy::parse(params.conflict_policy.as_deref())?;
        let read_path = path.clone();
        let bundle = run_blocking("read recovery bundle", move || {
            let input = read_recovery_bundle_file(&read_path)?;
            decode_recovery_bundle(&input, format)
        })
        .await?;
        let dry_run = params.dry_run.unwrap_or(true);
        let mut mutation_guard = if dry_run {
            None
        } else {
            Some(self.store.try_corpus_mutation_guard("bundle import")?)
        };
        let prepared = prepare_recovery_bundle_for_import(
            bundle,
            params.reembed_legacy.unwrap_or(false),
            dry_run,
            &self.embedder,
            &self.config,
        )
        .await?;
        let PreparedRecoveryBundle {
            bundle,
            legacy_bundle_version,
            embeddings_reembed_planned,
            embeddings_reembedded,
        } = prepared;
        let store = self.store.clone();
        let import_path = path.clone();
        let mut report = if let Some(guard) = mutation_guard.take() {
            let (guard, result) = retain_mutation_guard_while_blocking(guard, move || {
                store.import_recovery_bundle(&bundle, policy, dry_run, &import_path, format)
            })
            .await
            .map_err(|error| {
                McpError::from(AppError::db(format!("import_bundle task failed: {error}")))
            })?;
            mutation_guard = Some(guard);
            result?
        } else {
            run_blocking("import_bundle", move || {
                store.import_recovery_bundle(&bundle, policy, dry_run, &import_path, format)
            })
            .await?
        };
        report.legacy_bundle_version = legacy_bundle_version;
        report.legacy_reembed_requested = params.reembed_legacy.unwrap_or(false);
        report.embeddings_reembed_planned = embeddings_reembed_planned;
        report.embeddings_reembedded = embeddings_reembedded;
        if report.success
            && !report.dry_run
            && (report.documents_overwritten > 0 || report.chunks_inserted > 0)
        {
            let guard = mutation_guard
                .take()
                .expect("non-dry bundle import owns the mutation lane");
            let outcome = self
                .refresh_fts_after_mutation("import_bundle", guard)
                .await;
            mutation_guard = outcome.guard;
            if let Some(error) = outcome.error {
                report.success = false;
                report.errors.push(error);
            }
        }
        let _mutation_guard = mutation_guard;
        Self::json_result(&report)
    }
}

pub(super) const LEGACY_RECOVERY_BUNDLE_VERSION: u32 = 1;

const LEGACY_RECOVERY_REEMBED_BATCH_SIZE: usize = 64;

pub(super) struct PreparedRecoveryBundle {
    bundle: RecoveryBundle,
    legacy_bundle_version: Option<u32>,
    embeddings_reembed_planned: u64,
    embeddings_reembedded: u64,
}

fn recovery_format(requested: Option<&str>, path: &Path) -> Result<&'static str, AppError> {
    let value = requested
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .or_else(|| {
            path.extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
        })
        .unwrap_or_else(|| "json".into());
    match value.as_str() {
        "json" => Ok("json"),
        "jsonl" | "ndjson" => Ok("jsonl"),
        other => Err(AppError::invalid(format!(
            "invalid bundle format '{other}': expected json or jsonl"
        ))),
    }
}

pub(super) async fn prepare_recovery_bundle_for_import(
    mut bundle: RecoveryBundle,
    reembed_legacy: bool,
    dry_run: bool,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
) -> Result<PreparedRecoveryBundle, AppError> {
    preflight_recovery_bundle(&bundle)?;
    if bundle.format != "rag-recovery-bundle" {
        return Err(AppError::invalid(format!(
            "unsupported recovery bundle format/version: {}/{}",
            bundle.format, bundle.version
        )));
    }
    if bundle.version == BUNDLE_VERSION {
        return Ok(PreparedRecoveryBundle {
            bundle,
            legacy_bundle_version: None,
            embeddings_reembed_planned: 0,
            embeddings_reembedded: 0,
        });
    }
    if bundle.version != LEGACY_RECOVERY_BUNDLE_VERSION {
        return Err(AppError::invalid(format!(
            "unsupported recovery bundle format/version: {}/{}",
            bundle.format, bundle.version
        )));
    }

    let locations = bundle
        .documents
        .iter()
        .enumerate()
        .flat_map(|(document_index, document)| {
            (0..document.chunks.len()).map(move |chunk_index| (document_index, chunk_index))
        })
        .collect::<Vec<_>>();
    let planned = u64::try_from(locations.len())
        .map_err(|_| AppError::embeddings("legacy recovery bundle has too many chunks"))?;
    if !locations.is_empty() && !reembed_legacy {
        return Err(AppError::embeddings(format!(
            "legacy recovery bundle v1 contains {planned} chunks with unverifiable embedding identity; retry import_bundle with reembed_legacy=true to replace every vector with the live embedding provider"
        )));
    }

    let expected_dims = config.embedding_dims;
    preflight_recovery_bundle_reembed(&bundle, expected_dims)?;
    if !locations.is_empty() && embedder.dimensions() != expected_dims {
        return Err(AppError::embeddings(format!(
            "live embedder reports dims={}, but config requires dims={expected_dims}",
            embedder.dimensions()
        )));
    }
    let mut reembedded = 0u64;
    if dry_run {
        for &(document_index, chunk_index) in &locations {
            bundle.documents[document_index].chunks[chunk_index].embedding =
                vec![0.0; expected_dims];
        }
    } else {
        for batch in locations.chunks(LEGACY_RECOVERY_REEMBED_BATCH_SIZE) {
            let texts = batch
                .iter()
                .map(|&(document_index, chunk_index)| {
                    bundle.documents[document_index].chunks[chunk_index]
                        .content
                        .clone()
                })
                .collect::<Vec<_>>();
            let embeddings = embedder.embed(&texts).await?;
            if embeddings.len() != batch.len() {
                return Err(AppError::embeddings(format!(
                    "legacy bundle re-embed returned {} vectors for {} chunks",
                    embeddings.len(),
                    batch.len()
                )));
            }
            if let Some((index, embedding)) = embeddings
                .iter()
                .enumerate()
                .find(|(_, embedding)| embedding.len() != expected_dims)
            {
                return Err(AppError::embeddings(format!(
                    "legacy bundle re-embed vector {index} has dims={}, expected {expected_dims}",
                    embedding.len()
                )));
            }
            for (&(document_index, chunk_index), embedding) in batch.iter().zip(embeddings) {
                bundle.documents[document_index].chunks[chunk_index].embedding = embedding;
                reembedded += 1;
            }
        }
    }
    bundle.embedding_manifest = if locations.is_empty() {
        None
    } else {
        Some(embedding_manifest_from_config(config))
    };
    bundle.version = BUNDLE_VERSION;

    Ok(PreparedRecoveryBundle {
        bundle,
        legacy_bundle_version: Some(LEGACY_RECOVERY_BUNDLE_VERSION),
        embeddings_reembed_planned: planned,
        embeddings_reembedded: reembedded,
    })
}
