//! Background jobs (`/v1/jobs`) and the gateway activity feed.

use serde::{Deserialize, Serialize};

use crate::index::IndexReport;
use crate::sync::{
    SourceSyncCounters, SourceSyncError, SourceSyncFinalizationError, SourceSyncProgress,
    SyncJobRequest,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    CompletedWithErrors,
    Failed,
    Cancelled,
    /// A status this build does not know (newer server or stored job).
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSyncJobReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_index: Option<IndexReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_index_error: Option<SourceSyncFinalizationError>,
    pub added_count: usize,
    pub updated_count: usize,
    pub skipped_count: usize,
    pub deleted_count: usize,
    pub error_count: usize,
    #[serde(default)]
    pub error_samples: Vec<SourceSyncError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finalization_error: Option<SourceSyncFinalizationError>,
    #[serde(default)]
    pub counters: SourceSyncCounters,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub id: String,
    pub kind: String,
    pub status: JobStatus,
    pub request: SyncJobRequest,
    #[serde(default)]
    pub progress: Option<SourceSyncProgress>,
    #[serde(default)]
    pub report: Option<SourceSyncJobReport>,
    #[serde(default)]
    pub error: Option<String>,
    pub cancel_requested: bool,
    /// True when a new sync with the same request is safe after interrupt/failure.
    /// Source sync is committed-prefix aware (unchanged files skip); mid-batch
    /// embeds are not resumed.
    #[serde(default)]
    pub requeue_safe: bool,
    /// Human-readable resume limits when `requeue_safe` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_limits: Option<String>,
    /// Job id this snapshot was requeued from, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requeued_from: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl JobStatus {
    /// True once the job can no longer change. Unknown statuses count as
    /// terminal so a client never offers to cancel what it cannot classify.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::CompletedWithErrors
                | Self::Failed
                | Self::Cancelled
                | Self::Unknown
        )
    }

    /// Wire string (`queued`, `running`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::CompletedWithErrors => "completed_with_errors",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

/// One `/v1/activity` feed entry (HTTP request or MCP call).
///
/// `kind` is a `Cow` so the server records static labels without allocating
/// while clients deserialize owned strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub seq: u64,
    pub at: String,
    /// `http` | `mcp` | ...
    pub kind: std::borrow::Cow<'static, str>,
    pub client: Option<String>,
    pub action: String,
    pub status: Option<u16>,
    pub elapsed_ms: Option<f64>,
    pub request_id: Option<String>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn job_snapshot_tolerates_newer_servers() {
        use super::{JobSnapshot, JobStatus};
        use crate::sync::SourceSyncPhase;
        let job: JobSnapshot = serde_json::from_value(serde_json::json!({
            "id": "j", "kind": "source_sync", "status": "paused_by_operator",
            "request": {"path": "/vault"},
            "progress": {"phase": "rebalancing", "total_files": 2, "processed_files": 1,
                         "added": 0, "updated": 0, "skipped": 0, "deleted": 0, "errors": 0},
            "cancel_requested": false,
            "created_at": "2026-10-01T12:00:00Z"
        }))
        .unwrap();
        assert_eq!(job.status, JobStatus::Unknown);
        assert!(job.status.is_terminal());
        assert_eq!(job.progress.unwrap().phase, SourceSyncPhase::Unknown);
        assert!(job.report.is_none() && job.started_at.is_none());
        assert_eq!(
            serde_json::to_value(JobStatus::CompletedWithErrors).unwrap(),
            JobStatus::CompletedWithErrors.as_str()
        );
    }
}
