//! In-process background jobs for long-running, write-side HTTP operations.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::error::api_err;
use super::HttpState;
use crate::config::Config;
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::source_sync::{
    sync_sources_nonblocking, SourceSyncCommand, SourceSyncControl, SourceSyncCounters,
    SourceSyncError, SourceSyncFinalizationError, SourceSyncOutcome, SourceSyncProgress,
    SourceSyncReport,
};

const MAX_RETAINED_JOBS: usize = 100;
const MAX_ERROR_SAMPLES: usize = 20;

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/jobs/sync", post(start_sync))
        .route("/v1/jobs", get(list_jobs))
        .route("/v1/jobs/{id}", get(get_job).delete(cancel_job))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SyncJobRequest {
    pub path: String,
    #[serde(default)]
    pub remove_deleted: bool,
    #[serde(default, alias = "project")]
    pub wing: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
    #[serde(default)]
    pub max_file_bytes: Option<u64>,
}

impl SyncJobRequest {
    fn command(&self) -> SourceSyncCommand {
        SourceSyncCommand {
            path: PathBuf::from(&self.path),
            remove_deleted: self.remove_deleted,
            wing: self.wing.clone(),
            room: self.room.clone(),
            max_file_bytes: self.max_file_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    CompletedWithErrors,
    Failed,
    Cancelled,
}

impl JobStatus {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::CompletedWithErrors | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceSyncJobReport {
    pub added_count: usize,
    pub updated_count: usize,
    pub skipped_count: usize,
    pub deleted_count: usize,
    pub error_count: usize,
    pub error_samples: Vec<SourceSyncError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finalization_error: Option<SourceSyncFinalizationError>,
    pub counters: SourceSyncCounters,
}

impl From<SourceSyncReport> for SourceSyncJobReport {
    fn from(report: SourceSyncReport) -> Self {
        Self {
            added_count: report.added.len(),
            updated_count: report.updated.len(),
            skipped_count: report.skipped.len(),
            deleted_count: report.deleted.len(),
            error_count: report.errors.len(),
            error_samples: report.errors.into_iter().take(MAX_ERROR_SAMPLES).collect(),
            finalization_error: report.finalization_error,
            counters: report.counters,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JobSnapshot {
    pub id: String,
    pub kind: &'static str,
    pub status: JobStatus,
    pub request: SyncJobRequest,
    pub progress: Option<SourceSyncProgress>,
    pub report: Option<SourceSyncJobReport>,
    pub error: Option<String>,
    pub cancel_requested: bool,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

struct JobEntry {
    sequence: u64,
    snapshot: JobSnapshot,
    cancellation: CancellationToken,
}

struct JobRegistryInner {
    jobs: Mutex<HashMap<String, JobEntry>>,
    writer_lane: tokio::sync::Mutex<()>,
    next_sequence: AtomicU64,
}

/// Process-local job state. All write-side jobs share one FIFO-ish mutex lane.
#[derive(Clone)]
pub struct JobRegistry {
    inner: Arc<JobRegistryInner>,
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(JobRegistryInner {
                jobs: Mutex::new(HashMap::new()),
                writer_lane: tokio::sync::Mutex::new(()),
                next_sequence: AtomicU64::new(1),
            }),
        }
    }
}

impl JobRegistry {
    pub fn start_source_sync(
        &self,
        request: SyncJobRequest,
        store: Arc<Store>,
        embedder: Arc<dyn EmbeddingProvider>,
        config: Config,
    ) -> Result<JobSnapshot, AppError> {
        if request.max_file_bytes == Some(0) {
            return Err(AppError::config("max_file_bytes must be greater than zero"));
        }
        let id = Uuid::new_v4().to_string();
        let cancellation = CancellationToken::new();
        let snapshot = JobSnapshot {
            id: id.clone(),
            kind: "source_sync",
            status: JobStatus::Queued,
            request: request.clone(),
            progress: None,
            report: None,
            error: None,
            cancel_requested: false,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
        };
        let sequence = self.inner.next_sequence.fetch_add(1, Ordering::Relaxed);
        {
            let mut jobs = self.jobs();
            prune_jobs_to(&mut jobs, MAX_RETAINED_JOBS.saturating_sub(1));
            if jobs.len() >= MAX_RETAINED_JOBS {
                return Err(AppError::busy(format!(
                    "background job capacity reached ({MAX_RETAINED_JOBS} active jobs)"
                )));
            }
            jobs.insert(
                id.clone(),
                JobEntry {
                    sequence,
                    snapshot: snapshot.clone(),
                    cancellation: cancellation.clone(),
                },
            );
        }

        let registry = self.clone();
        tokio::spawn(async move {
            let _writer_guard = registry.inner.writer_lane.lock().await;
            if !registry.mark_running_if_queued(&id) {
                return;
            }

            let progress_registry = registry.clone();
            let progress_id = id.clone();
            let control = SourceSyncControl::new(cancellation.clone(), move |progress| {
                progress_registry.update(&progress_id, |job| {
                    job.progress = Some(progress);
                });
            });
            let result =
                sync_sources_nonblocking(store, embedder, config, request.command(), control).await;
            match result {
                Ok(SourceSyncOutcome::Completed(report)) => {
                    let status = if report.errors.is_empty() {
                        JobStatus::Succeeded
                    } else {
                        JobStatus::CompletedWithErrors
                    };
                    registry.finish(&id, status, Some(report), None);
                }
                Ok(SourceSyncOutcome::Cancelled(report)) => registry.finish_cancelled(&id, report),
                Err(_error) if cancellation.is_cancelled() => {
                    registry.finish_cancelled(&id, SourceSyncReport::default());
                }
                Err(error) => {
                    registry.finish(&id, JobStatus::Failed, None, Some(error.to_string()));
                }
            }
        });

        Ok(snapshot)
    }

    pub fn list(&self) -> Vec<JobSnapshot> {
        let jobs = self.jobs();
        let mut entries = jobs.values().collect::<Vec<_>>();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.sequence));
        entries
            .into_iter()
            .map(|entry| entry.snapshot.clone())
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<JobSnapshot> {
        self.jobs().get(id).map(|entry| entry.snapshot.clone())
    }

    pub fn cancel(&self, id: &str) -> Result<JobSnapshot, AppError> {
        let (snapshot, cancellation, should_cancel) = {
            let mut jobs = self.jobs();
            let entry = jobs
                .get_mut(id)
                .ok_or_else(|| AppError::not_found(format!("job not found: {id}")))?;

            let should_cancel = match entry.snapshot.status {
                JobStatus::Queued => {
                    entry.snapshot.cancel_requested = true;
                    mark_cancelled(&mut entry.snapshot, SourceSyncReport::default());
                    true
                }
                JobStatus::Running => {
                    entry.snapshot.cancel_requested = true;
                    true
                }
                _ => false,
            };
            (
                entry.snapshot.clone(),
                entry.cancellation.clone(),
                should_cancel,
            )
        };
        if should_cancel {
            cancellation.cancel();
        }
        Ok(snapshot)
    }

    /// Atomically claim a queued job for the writer lane.
    ///
    /// Cancellation uses the same registry mutex, so a queued job either
    /// becomes terminal `cancelled` or transitions to `running`; it can never
    /// be resurrected from `cancelled` by a worker that was waiting for the
    /// writer lane.
    fn mark_running_if_queued(&self, id: &str) -> bool {
        let mut jobs = self.jobs();
        let Some(entry) = jobs.get_mut(id) else {
            return false;
        };
        if entry.snapshot.status != JobStatus::Queued {
            return false;
        }
        entry.snapshot.status = JobStatus::Running;
        entry.snapshot.started_at = Some(Utc::now());
        true
    }

    fn update(&self, id: &str, update: impl FnOnce(&mut JobSnapshot)) {
        if let Some(entry) = self.jobs().get_mut(id) {
            update(&mut entry.snapshot);
        }
    }

    fn finish_cancelled(&self, id: &str, report: SourceSyncReport) {
        self.finish(id, JobStatus::Cancelled, Some(report), None);
    }

    fn finish(
        &self,
        id: &str,
        status: JobStatus,
        report: Option<SourceSyncReport>,
        error: Option<String>,
    ) {
        let mut jobs = self.jobs();
        if let Some(entry) = jobs.get_mut(id) {
            if entry.snapshot.status.is_terminal() {
                return;
            }
            entry.snapshot.status = status;
            entry.snapshot.report = report.map(SourceSyncJobReport::from);
            entry.snapshot.error = error;
            entry.snapshot.finished_at = Some(Utc::now());
        }
        prune_jobs(&mut jobs);
    }

    fn jobs(&self) -> MutexGuard<'_, HashMap<String, JobEntry>> {
        self.inner
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn mark_cancelled(snapshot: &mut JobSnapshot, report: SourceSyncReport) {
    snapshot.status = JobStatus::Cancelled;
    snapshot.report = Some(report.into());
    snapshot.error = None;
    snapshot.finished_at = Some(Utc::now());
}

fn prune_jobs(jobs: &mut HashMap<String, JobEntry>) {
    prune_jobs_to(jobs, MAX_RETAINED_JOBS);
}

fn prune_jobs_to(jobs: &mut HashMap<String, JobEntry>, maximum: usize) {
    while jobs.len() > maximum {
        let oldest_terminal = jobs
            .iter()
            .filter(|(_, entry)| entry.snapshot.status.is_terminal())
            .min_by_key(|(_, entry)| entry.sequence)
            .map(|(id, _)| id.clone());
        let Some(id) = oldest_terminal else {
            break;
        };
        jobs.remove(&id);
    }
}

async fn start_sync(
    State(state): State<HttpState>,
    Json(request): Json<SyncJobRequest>,
) -> Response {
    if request.path.trim().is_empty() {
        return api_err(AppError::config("path must not be empty"));
    }
    let snapshot = match state.jobs.start_source_sync(
        request,
        state.store.clone(),
        state.embedder.clone(),
        state.config.clone(),
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => return api_err(error),
    };
    (
        StatusCode::ACCEPTED,
        Json(json!({"ok": true, "job": snapshot})),
    )
        .into_response()
}

async fn list_jobs(State(state): State<HttpState>) -> impl IntoResponse {
    Json(json!({"ok": true, "items": state.jobs.list()}))
}

async fn get_job(State(state): State<HttpState>, Path(id): Path<String>) -> Response {
    match state.jobs.get(&id) {
        Some(job) => Json(json!({"ok": true, "job": job})).into_response(),
        None => api_err(AppError::not_found(format!("job not found: {id}"))),
    }
}

async fn cancel_job(State(state): State<HttpState>, Path(id): Path<String>) -> Response {
    match state.jobs.cancel(&id) {
        Ok(job) => {
            let status = if job.status.is_terminal() {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            };
            (status, Json(json!({"ok": true, "job": job}))).into_response()
        }
        Err(error) => api_err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbedder;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    fn state(root: &std::path::Path) -> HttpState {
        let config = Config {
            db_path: root.join("jobs.duckdb"),
            ingest_roots: vec![root.to_path_buf()],
            embedding_dims: 16,
            ..Config::for_tests()
        };
        HttpState {
            store: Arc::new(Store::open(&config.db_path).unwrap()),
            mcp_http: false,
            embedder: Arc::new(MockEmbedder::new(16)),
            config,
            jobs: JobRegistry::default(),
        }
    }

    async fn wait_for_terminal(registry: &JobRegistry, id: &str) -> JobSnapshot {
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let snapshot = registry.get(id).unwrap();
                if snapshot.status.is_terminal() {
                    return snapshot;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        terminal.unwrap_or_else(|_| {
            let last = registry.get(id).map(|snapshot| snapshot.status);
            panic!("job did not reach terminal state within 30s; last status: {last:?}")
        })
    }

    fn entry(sequence: u64, status: JobStatus) -> JobEntry {
        let id = format!("job-{sequence}");
        JobEntry {
            sequence,
            snapshot: JobSnapshot {
                id,
                kind: "source_sync",
                status,
                request: SyncJobRequest {
                    path: "/tmp/project".into(),
                    remove_deleted: false,
                    wing: None,
                    room: None,
                    max_file_bytes: None,
                },
                progress: None,
                report: None,
                error: None,
                cancel_requested: false,
                created_at: Utc::now(),
                started_at: None,
                finished_at: status.is_terminal().then(Utc::now),
            },
            cancellation: CancellationToken::new(),
        }
    }

    fn assert_immediately_cancelled(snapshot: &JobSnapshot) {
        assert_eq!(snapshot.status, JobStatus::Cancelled);
        assert!(snapshot.cancel_requested);
        assert!(snapshot.started_at.is_none());
        assert!(snapshot.finished_at.is_some());
        assert!(snapshot.error.is_none());
        let report = snapshot.report.as_ref().expect("cancelled job report");
        assert_eq!(report.added_count, 0);
        assert_eq!(report.updated_count, 0);
        assert_eq!(report.skipped_count, 0);
        assert_eq!(report.deleted_count, 0);
        assert_eq!(report.error_count, 0);
        assert!(report.error_samples.is_empty());
        assert_eq!(report.counters.preflight, 0);
        assert_eq!(report.counters.extracted, 0);
        assert_eq!(report.counters.embedded, 0);
    }

    #[tokio::test]
    async fn source_sync_job_reports_lifecycle_and_result() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "# indexed").unwrap();
        let state = state(root.path());
        let job = state
            .jobs
            .start_source_sync(
                SyncJobRequest {
                    path: root.path().display().to_string(),
                    remove_deleted: false,
                    wing: Some("jobs-test".into()),
                    room: None,
                    max_file_bytes: None,
                },
                state.store.clone(),
                state.embedder.clone(),
                state.config.clone(),
            )
            .unwrap();
        assert_eq!(job.status, JobStatus::Queued);

        let finished = wait_for_terminal(&state.jobs, &job.id).await;
        assert_eq!(finished.status, JobStatus::Succeeded);
        assert_eq!(finished.report.unwrap().added_count, 1);
        assert_eq!(finished.progress.unwrap().processed_files, 1);
        assert_eq!(state.jobs.list()[0].id, job.id);
    }

    #[tokio::test]
    async fn http_api_rejects_zero_max_file_bytes_without_enqueuing_a_job() {
        let root = tempfile::tempdir().unwrap();
        let state = state(root.path());
        let response = routes()
            .with_state(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/jobs/sync")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"path": root.path(), "max_file_bytes": 0}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("max_file_bytes"));
        assert!(state.jobs.list().is_empty());
    }

    #[tokio::test]
    async fn per_file_errors_are_terminal_but_not_reported_as_success() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("large.md"), "larger than one byte").unwrap();
        let state = state(root.path());
        let job = state
            .jobs
            .start_source_sync(
                SyncJobRequest {
                    path: root.path().display().to_string(),
                    remove_deleted: false,
                    wing: Some("jobs-test".into()),
                    room: None,
                    max_file_bytes: Some(1),
                },
                state.store.clone(),
                state.embedder.clone(),
                state.config.clone(),
            )
            .unwrap();

        let finished = wait_for_terminal(&state.jobs, &job.id).await;
        assert_eq!(finished.status, JobStatus::CompletedWithErrors);
        assert_eq!(finished.report.unwrap().error_count, 1);
    }

    #[tokio::test]
    async fn final_fts_failure_is_completed_with_errors_and_retains_report() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "durable job mutation").unwrap();
        let mut state = state(root.path());
        state.store.ensure_fts(&state.config.fts_stemmer).unwrap();
        state.config.fts_stemmer = crate::source_sync::TEST_FINALIZE_FTS_FAILURE_STEMMER.into();
        let job = state
            .jobs
            .start_source_sync(
                SyncJobRequest {
                    path: root.path().display().to_string(),
                    remove_deleted: false,
                    wing: Some("jobs-test".into()),
                    room: None,
                    max_file_bytes: None,
                },
                state.store.clone(),
                state.embedder.clone(),
                state.config.clone(),
            )
            .unwrap();

        let finished = wait_for_terminal(&state.jobs, &job.id).await;
        assert_eq!(finished.status, JobStatus::CompletedWithErrors);
        assert!(finished.error.is_none());
        let progress = finished.progress.as_ref().expect("terminal progress");
        assert_eq!(
            progress.phase,
            crate::source_sync::SourceSyncPhase::Completed
        );
        assert_eq!(progress.errors, 1);
        let report = finished.report.as_ref().expect("retained job report");
        assert_eq!(report.added_count, 1);
        assert_eq!(report.error_count, 1);
        assert_eq!(report.counters.preflight, 1);
        assert_eq!(report.counters.extracted, 1);
        assert_eq!(report.counters.embedded, 1);
        assert!(report.error_samples[0]
            .error
            .contains("FTS_FINALIZATION_FAILED"));
        let finalization = report
            .finalization_error
            .as_ref()
            .expect("structured finalization error");
        assert_eq!(finalization.code, "FTS_FINALIZATION_FAILED");
        assert_eq!(finalization.stage, "refresh_fts");
        assert!(finalization.durable_mutation_committed);
        assert!(finalization.retryable);
        assert!(finalization.fallback_dirty_marked);
        assert!(finalization.dirty_marker_error.is_none());
        assert_eq!(state.store.list_documents().unwrap().len(), 1);
        let generation = {
            let conn = state.store.lock().unwrap();
            crate::db::fts::fts_generation_state(&conn).unwrap()
        };
        assert!(generation.dirty, "next lexical read must retry FTS");
    }

    #[tokio::test]
    async fn queued_job_can_be_cancelled_before_it_enters_writer_lane() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "# not indexed").unwrap();
        let state = state(root.path());
        let writer_guard = state.jobs.inner.writer_lane.lock().await;
        let job = state
            .jobs
            .start_source_sync(
                SyncJobRequest {
                    path: root.path().display().to_string(),
                    remove_deleted: false,
                    wing: None,
                    room: None,
                    max_file_bytes: None,
                },
                state.store.clone(),
                state.embedder.clone(),
                state.config.clone(),
            )
            .unwrap();
        let cancelled = state.jobs.cancel(&job.id).unwrap();
        assert_immediately_cancelled(&cancelled);
        assert_immediately_cancelled(&state.jobs.get(&job.id).unwrap());
        assert!(!state.jobs.mark_running_if_queued(&job.id));

        drop(writer_guard);
        tokio::task::yield_now().await;

        assert_immediately_cancelled(&state.jobs.get(&job.id).unwrap());
        assert!(state.store.list_documents().unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrent_cancel_and_running_transition_never_resurrect_a_cancelled_job() {
        let registry = JobRegistry::default();
        let id = "job-1".to_string();
        registry
            .jobs()
            .insert(id.clone(), entry(1, JobStatus::Queued));
        let barrier = Arc::new(tokio::sync::Barrier::new(3));

        let cancel_task = tokio::spawn({
            let registry = registry.clone();
            let id = id.clone();
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                registry.cancel(&id).unwrap()
            }
        });
        let start_task = tokio::spawn({
            let registry = registry.clone();
            let id = id.clone();
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                registry.mark_running_if_queued(&id)
            }
        });

        barrier.wait().await;
        let cancelled = cancel_task.await.unwrap();
        let transitioned_to_running = start_task.await.unwrap();
        let final_snapshot = registry.get(&id).unwrap();
        let token_cancelled = registry
            .jobs()
            .get(&id)
            .expect("job remains retained")
            .cancellation
            .is_cancelled();

        assert!(token_cancelled);
        if transitioned_to_running {
            assert_eq!(cancelled.status, JobStatus::Running);
            assert_eq!(final_snapshot.status, JobStatus::Running);
            assert!(final_snapshot.cancel_requested);
            assert!(final_snapshot.started_at.is_some());
            assert!(final_snapshot.finished_at.is_none());
        } else {
            assert_immediately_cancelled(&cancelled);
            assert_immediately_cancelled(&final_snapshot);
        }
    }

    #[tokio::test]
    async fn http_api_creates_lists_reads_and_cancels_a_job() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "# queued").unwrap();
        let state = state(root.path());
        let writer_guard = state.jobs.inner.writer_lane.lock().await;
        let app = routes().with_state(state.clone());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/jobs/sync")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"path": root.path(), "project": "jobs-http"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = body["job"]["id"].as_str().unwrap().to_string();

        for uri in ["/v1/jobs".to_string(), format!("/v1/jobs/{id}")] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/v1/jobs/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["job"]["status"], "cancelled");
        assert_eq!(body["job"]["cancel_requested"], true);
        assert!(body["job"]["started_at"].is_null());
        assert!(body["job"]["finished_at"].is_string());
        assert!(body["job"]["report"].is_object());
        assert_eq!(body["job"]["report"]["error_count"], 0);

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/jobs/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["job"]["status"], "cancelled");
        assert!(body["job"]["finished_at"].is_string());
        assert!(body["job"]["report"].is_object());

        drop(writer_guard);
        tokio::task::yield_now().await;
        assert_immediately_cancelled(&state.jobs.get(&id).unwrap());
        assert!(state.store.list_documents().unwrap().is_empty());
    }

    #[test]
    fn retention_keeps_the_newest_terminal_jobs() {
        let mut jobs = HashMap::new();
        for sequence in 0..(MAX_RETAINED_JOBS as u64 + 2) {
            jobs.insert(
                format!("job-{sequence}"),
                entry(sequence, JobStatus::Succeeded),
            );
        }

        prune_jobs(&mut jobs);

        assert_eq!(jobs.len(), MAX_RETAINED_JOBS);
        assert!(!jobs.contains_key("job-0"));
        assert!(!jobs.contains_key("job-1"));
        assert!(jobs.contains_key("job-2"));
    }

    #[tokio::test]
    async fn admission_refuses_a_burst_when_all_slots_are_active() {
        let root = tempfile::tempdir().unwrap();
        let state = state(root.path());
        {
            let mut jobs = state.jobs.jobs();
            for sequence in 0..MAX_RETAINED_JOBS as u64 {
                jobs.insert(
                    format!("job-{sequence}"),
                    entry(sequence, JobStatus::Queued),
                );
            }
        }

        let error = state
            .jobs
            .start_source_sync(
                SyncJobRequest {
                    path: root.path().display().to_string(),
                    remove_deleted: false,
                    wing: None,
                    room: None,
                    max_file_bytes: None,
                },
                state.store.clone(),
                state.embedder.clone(),
                state.config.clone(),
            )
            .unwrap_err();

        assert!(matches!(error, AppError::Busy(_)));
        assert_eq!(state.jobs.list().len(), MAX_RETAINED_JOBS);
    }
}
