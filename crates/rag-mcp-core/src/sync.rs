//! Source roots, project inventory and source-sync progress.

use serde::{Deserialize, Serialize};

/// Lean source-root aggregate for project registry / status surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRootSummary {
    pub canonical_root: String,
    pub file_count: u64,
    pub last_seen: Option<String>,
    pub last_synced: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHome {
    pub project_id: String,
    pub documents: u64,
    pub chunks: u64,
    pub raw_documents: u64,
    pub wiki_documents: u64,
    pub diary_documents: u64,
    pub archived_documents: u64,
    pub rooms: u64,
    pub source_files: u64,
    pub graph_nodes: u64,
    pub graph_edges: u64,
    pub unresolved_links: u64,
    #[serde(default)]
    pub source_roots: Vec<SourceRootSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSyncError {
    pub path: String,
    pub error: String,
}

/// Structured terminal failure after source-sync mutations have already run.
///
/// This is additive to the existing per-path `errors` list so older clients
/// still classify the run as completed-with-errors, while newer clients can
/// distinguish a derived-index failure from a failed or rolled-back sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSyncFinalizationError {
    pub code: String,
    pub stage: String,
    pub durable_mutation_committed: bool,
    pub retryable: bool,
    pub fallback_dirty_marked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty_marker_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops_log_error: Option<String>,
    pub error: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceSyncCounters {
    pub preflight: u64,
    pub extracted: u64,
    pub embedded: u64,
}

/// Observable phases for a cooperative source synchronization run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSyncPhase {
    Scanning,
    Syncing,
    RemovingDeleted,
    RefreshingFts,
    Completed,
    Cancelled,
    /// A phase this build does not know (newer server).
    #[serde(other)]
    Unknown,
}

/// Progress snapshot emitted while a source tree is synchronized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSyncProgress {
    pub phase: SourceSyncPhase,
    pub total_files: usize,
    pub processed_files: usize,
    #[serde(default)]
    pub current_path: Option<String>,
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
    pub deleted: usize,
    pub errors: usize,
    #[serde(default)]
    pub counters: SourceSyncCounters,
}

impl SourceSyncPhase {
    /// Wire string (`scanning`, `syncing`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scanning => "scanning",
            Self::Syncing => "syncing",
            Self::RemovingDeleted => "removing_deleted",
            Self::RefreshingFts => "refreshing_fts",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

/// Body of `POST /v1/jobs/sync`: synchronize one allowlisted source root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncJobRequest {
    pub path: String,
    #[serde(default)]
    pub remove_deleted: bool,
    #[serde(default, alias = "project", skip_serializing_if = "Option::is_none")]
    pub wing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_bytes: Option<u64>,
}
