//! Consistent DuckDB backup and portable document bundle recovery.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use std::time::SystemTime;

use chrono::{DateTime, NaiveDateTime, Utc};
use duckdb::params;
use serde::{Deserialize, Serialize};

use super::store::{embedding_identity_fingerprint, set_embedding_manifest_locked};
use super::Store;
use crate::error::{AppError, Result};
use crate::models::{Chunk, Document, EmbeddingManifest};
use crate::util::backup_artifact_paths;

pub const BUNDLE_VERSION: u32 = 2;

/// Portable JSON/JSONL recovery is intentionally bounded because both serde
/// decoding and the in-memory [`RecoveryBundle`] representation amplify the
/// on-disk payload. Larger corpora must use the verified DuckDB backup path.
pub const PORTABLE_RECOVERY_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const PORTABLE_RECOVERY_MAX_DOCUMENTS: u64 = 10_000;
pub const PORTABLE_RECOVERY_MAX_CHUNKS: u64 = 50_000;
pub const PORTABLE_RECOVERY_LIMIT_GUIDANCE: &str =
    "use backup_db (or `recovery backup`) and its verification sidecars to create a verified DuckDB backup for this large corpus";

const PORTABLE_DOCUMENT_OVERHEAD_BYTES: u64 = 1_024;
const PORTABLE_CHUNK_OVERHEAD_BYTES: u64 = 512;
const PORTABLE_MANIFEST_OVERHEAD_BYTES: u64 = 1_024;
const JSON_STRING_WORST_CASE_FACTOR: u64 = 6;
const JSON_FLOAT_WORST_CASE_BYTES: u64 = 64;

const DATABASE_ARTIFACT: usize = 0;
const SHA256_ARTIFACT: usize = 1;
const METADATA_ARTIFACT: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackupPublishStage {
    SnapshotConnectionReady,
    SidecarPublished,
    MainPublishedAndVerified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryBundle {
    pub format: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    /// Corpus-wide identity for every serialized chunk embedding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_manifest: Option<EmbeddingManifest>,
    pub documents: Vec<BundleDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleDocument {
    pub document: Document,
    #[serde(default)]
    pub chunks: Vec<Chunk>,
}

/// Allocation-safe preflight summary for portable recovery materialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PortableBundlePreflight {
    pub documents: u64,
    pub chunks: u64,
    pub embedding_values: u64,
    /// Conservative upper bound used before rows or JSON are materialized.
    pub estimated_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    Error,
    Skip,
    Overwrite,
}

impl ConflictPolicy {
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value
            .unwrap_or("error")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "error" => Ok(Self::Error),
            "skip" => Ok(Self::Skip),
            "overwrite" => Ok(Self::Overwrite),
            other => Err(AppError::config(format!(
                "invalid conflict_policy '{other}': expected error, skip, or overwrite"
            ))),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BackupReport {
    pub success: bool,
    pub dry_run: bool,
    pub source: String,
    pub path: String,
    pub overwritten: bool,
    pub bytes: Option<u64>,
    pub errors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<BackupVerification>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupVerification {
    pub ok: bool,
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub schema_version: i32,
    pub documents: u64,
    pub chunks: u64,
    pub nodes: u64,
    pub edges: u64,
    pub orphan_chunks: u64,
    pub orphan_document_nodes: u64,
    pub orphan_edges: u64,
    /// Every stored vector is covered by one canonical, complete manifest and
    /// has the manifest's declared dimensionality.
    pub embedding_contract_ok: bool,
    pub embedding_manifest: Option<crate::models::EmbeddingManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupSidecar {
    pub format: String,
    pub created_at: DateTime<Utc>,
    pub source: String,
    pub required_free_bytes: u64,
    pub verification: BackupVerification,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupInventoryItem {
    pub path: String,
    pub bytes: u64,
    pub modified_at: DateTime<Utc>,
    pub protected: bool,
    pub newest: bool,
}

#[derive(Debug, Serialize)]
pub struct BundleExportReport {
    pub success: bool,
    pub dry_run: bool,
    pub path: String,
    pub format: String,
    pub overwritten: bool,
    pub documents: u64,
    pub chunks: u64,
    pub bytes: Option<u64>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct BundleImportReport {
    pub success: bool,
    pub dry_run: bool,
    /// True only when this call committed document/chunk mutations.
    pub durable_mutation_committed: bool,
    /// Original legacy format version when the gateway upgraded it in memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_bundle_version: Option<u32>,
    /// Whether the caller explicitly authorized replacing legacy vectors.
    pub legacy_reembed_requested: bool,
    /// Legacy chunk vectors that would be replaced on apply.
    pub embeddings_reembed_planned: u64,
    /// Legacy chunk vectors actually replaced before this import attempt.
    pub embeddings_reembedded: u64,
    pub path: String,
    pub format: String,
    pub conflict_policy: String,
    pub documents_read: u64,
    pub documents_inserted: u64,
    pub documents_overwritten: u64,
    pub documents_skipped: u64,
    pub chunks_inserted: u64,
    pub conflicts: u64,
    pub errors: Vec<String>,
}

mod bundle;
mod import;
mod publish;

pub use self::bundle::*;
use self::import::*;
pub use self::publish::*;

fn parse_ts(value: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S%.f").map(|dt| dt.and_utc())
        })
        .unwrap_or_else(|_| Utc::now())
}

#[cfg(test)]
mod tests;
