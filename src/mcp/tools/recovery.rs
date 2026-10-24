//! Backup and recovery tool parameters.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

// --- Backup / recovery ---

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BackupDbParams {
    /// Destination DuckDB file under `RAG_INGEST_ROOTS`.
    pub path: String,
    /// Report the intended copy without writing it.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Replace an existing destination. Defaults to false.
    #[serde(default)]
    pub overwrite: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExportBundleParams {
    /// Destination `.json` or `.jsonl` file under `RAG_INGEST_ROOTS`.
    pub path: String,
    /// `json` or `jsonl`; inferred from the extension when omitted.
    #[serde(default)]
    pub format: Option<String>,
    /// Report counts without writing a file.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Replace an existing destination. Defaults to false.
    #[serde(default)]
    pub overwrite: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExportVaultParams {
    /// Destination directory under `RAG_INGEST_ROOTS`.
    pub path: String,
    /// Preview document/file counts without writing. Defaults to true.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Replace an existing vault, retaining it as a dated `.previous-*` sibling.
    #[serde(default)]
    pub overwrite: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImportBundleParams {
    /// Source `.json` or `.jsonl` file under `RAG_INGEST_ROOTS`.
    pub path: String,
    /// `json` or `jsonl`; inferred from the extension when omitted.
    #[serde(default)]
    pub format: Option<String>,
    /// `error` (default), `skip`, or `overwrite` for document id/URI conflicts.
    #[serde(default)]
    pub conflict_policy: Option<String>,
    /// Parse and report the import without changing DuckDB. Defaults to true.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Re-embed every chunk from a legacy v1 bundle with the live provider.
    /// Required for v1 bundles that contain vectors; defaults to false.
    #[serde(default)]
    pub reembed_legacy: Option<bool>,
}
