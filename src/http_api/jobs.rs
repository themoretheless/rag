//! `/v1/jobs*` routes over [`crate::jobs`].

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use super::error::api_err;
use super::HttpState;
use crate::error::AppError;
pub use crate::jobs::*;

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/jobs/sync", post(start_sync))
        .route("/v1/jobs", get(list_jobs))
        .route("/v1/jobs/{id}", get(get_job).delete(cancel_job))
        .route("/v1/jobs/{id}/requeue", post(requeue_job))
}

async fn start_sync(
    State(state): State<HttpState>,
    Json(request): Json<SyncJobRequest>,
) -> Response {
    if request.path.trim().is_empty() {
        return api_err(AppError::invalid("path must not be empty"));
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

async fn requeue_job(State(state): State<HttpState>, Path(id): Path<String>) -> Response {
    match state.jobs.requeue_source_sync(
        &id,
        state.store.clone(),
        state.embedder.clone(),
        state.config.clone(),
    ) {
        Ok(snapshot) => (
            StatusCode::ACCEPTED,
            Json(json!({
                "ok": true,
                "job": snapshot,
                "resume_limits": SOURCE_SYNC_REQUEUE_LIMITS,
            })),
        )
            .into_response(),
        Err(error) => api_err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::Store;
    use crate::embeddings::MockEmbedder;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use chrono::Utc;
    use duckdb::params;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;
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
            llm: None,
            auth: crate::http_api::auth::AuthPosture::default(),
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
                kind: "source_sync".into(),
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
                requeue_safe: false,
                resume_limits: None,
                requeued_from: None,
                created_at: Utc::now(),
                started_at: None,
                finished_at: status.is_terminal().then(Utc::now),
            },
            cancellation: CancellationToken::new(),
            progress_persisted_at: None,
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
    async fn code_index_failure_is_completed_with_errors_and_keeps_sources() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("main.rs"), "fn main() {} ").unwrap();
        let state = state(root.path());
        state
            .store
            .lock()
            .unwrap()
            .execute_batch("DROP TABLE code_index_documents")
            .unwrap();
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
        let report = finished.report.unwrap();
        assert_eq!(report.error_count, 1);
        assert_eq!(report.added_count, 1);
        assert!(report.finalization_error.is_none());
        assert!(report.code_index.is_none());
        let error = report.code_index_error.unwrap();
        assert_eq!(error.code, "CODE_INDEX_FINALIZATION_FAILED");
        assert!(error.durable_mutation_committed && error.retryable);
        assert_eq!(state.store.list_documents().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn final_fts_failure_is_completed_with_errors_and_retains_report() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "durable job mutation").unwrap();
        let mut state = state(root.path());
        state.store.ensure_fts(&state.config.fts_stemmer).unwrap();
        state.config.fts_stemmer = crate::db::fts::test_fault::fail("source-sync");
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

        let removed = prune_jobs(&mut jobs);

        assert_eq!(jobs.len(), MAX_RETAINED_JOBS);
        assert!(!jobs.contains_key("job-0"));
        assert!(!jobs.contains_key("job-1"));
        assert!(jobs.contains_key("job-2"));
        assert_eq!(removed, vec!["job-0".to_string(), "job-1".to_string()]);
    }

    #[test]
    fn deleting_a_job_row_waits_for_an_in_flight_persist() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&root.path().join("overlap.duckdb")).unwrap());
        let registry = JobRegistry::new(Some(store));
        // Stand in for a persist that has checked the map and not yet written.
        let in_flight = registry.inner.persist_order.lock().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let deleter = {
            let registry = registry.clone();
            std::thread::spawn(move || {
                registry.delete_persisted("job-1").unwrap();
                done_tx.send(()).unwrap();
            })
        };
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "delete must wait for the in-flight persist"
        );
        drop(in_flight);
        done_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("delete proceeds once the persist finishes");
        deleter.join().unwrap();
    }

    #[test]
    fn persisting_a_pruned_job_does_not_bring_its_row_back() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&root.path().join("pruned.duckdb")).unwrap());
        let registry = JobRegistry::new(Some(store.clone()));
        registry
            .jobs()
            .insert("job-1".into(), entry(1, JobStatus::Succeeded));
        let stale = registry.get("job-1").unwrap();
        registry.persist(&stale).unwrap();
        // Pruned: removed from the map and its row deleted.
        registry.jobs().remove("job-1");
        registry.delete_persisted("job-1").unwrap();
        registry.persist(&stale).unwrap();
        let rows: i64 = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM background_jobs WHERE id=?",
                ["job-1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn a_stale_snapshot_never_overwrites_a_newer_persisted_state() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&root.path().join("order.duckdb")).unwrap());
        let registry = JobRegistry::new(Some(store.clone()));
        registry
            .jobs()
            .insert("job-1".into(), entry(1, JobStatus::Running));
        // A progress writer cloned this before a concurrent cancel landed.
        let stale = registry.get("job-1").unwrap();
        registry.cancel("job-1").unwrap();
        registry.persist(&stale).unwrap();
        let payload: String = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM background_jobs WHERE id=?",
                ["job-1"],
                |r| r.get(0),
            )
            .unwrap();
        let stored: JobSnapshot = serde_json::from_str(&payload).unwrap();
        assert!(stored.cancel_requested);
    }

    #[test]
    fn progress_updates_persist_at_most_once_per_interval_but_status_at_once() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&root.path().join("progress.duckdb")).unwrap());
        let registry = JobRegistry::new(Some(store.clone()));
        registry
            .jobs()
            .insert("job-1".into(), entry(1, JobStatus::Running));
        let persisted = || -> JobSnapshot {
            let payload: String = store
                .lock()
                .unwrap()
                .query_row(
                    "SELECT payload FROM background_jobs WHERE id=?",
                    ["job-1"],
                    |r| r.get(0),
                )
                .unwrap();
            serde_json::from_str(&payload).unwrap()
        };

        for step in 0..50 {
            registry.update("job-1", |job| job.error = Some(format!("step {step}")));
        }
        assert_eq!(
            registry.get("job-1").unwrap().error.as_deref(),
            Some("step 49")
        );
        assert_eq!(persisted().error.as_deref(), Some("step 0"));

        registry.update("job-1", |job| job.status = JobStatus::Succeeded);
        let stored = persisted();
        assert_eq!(stored.status, JobStatus::Succeeded);
        assert_eq!(stored.error.as_deref(), Some("step 49"));
    }

    #[test]
    fn restore_marks_running_jobs_interrupted_and_persists() {
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            db_path: root.path().join("restore.duckdb"),
            embedding_dims: 16,
            ..Config::for_tests()
        };
        let store = Arc::new(Store::open(&config.db_path).unwrap());
        let snapshot = JobSnapshot {
            id: "job-running".into(),
            kind: "source_sync".into(),
            status: JobStatus::Running,
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
            requeue_safe: false,
            resume_limits: None,
            requeued_from: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: None,
        };
        {
            let conn = store.lock().unwrap();
            conn.execute(
                "INSERT INTO background_jobs VALUES (?, ?)",
                params!["job-running", serde_json::to_string(&snapshot).unwrap()],
            )
            .unwrap();
        }

        let registry = JobRegistry::restore_from_store(store.clone()).unwrap();
        let restored = registry.get("job-running").unwrap();
        assert_eq!(restored.status, JobStatus::Failed);
        assert_eq!(
            restored.error.as_deref(),
            Some("interrupted by process restart")
        );
        assert!(restored.finished_at.is_some());
        assert!(restored.requeue_safe);
        assert!(restored
            .resume_limits
            .as_deref()
            .is_some_and(|s| s.contains("committed")));

        let payload: String = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM background_jobs WHERE id=?",
                ["job-running"],
                |r| r.get(0),
            )
            .unwrap();
        let stored: JobSnapshot = serde_json::from_str(&payload).unwrap();
        assert_eq!(stored.status, JobStatus::Failed);
        assert_eq!(
            stored.error.as_deref(),
            Some("interrupted by process restart")
        );
        assert!(stored.finished_at.is_some());
        assert!(stored.requeue_safe);
    }

    #[tokio::test]
    async fn requeue_creates_new_job_from_interrupted() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "# requeue").unwrap();
        let state = state(root.path());
        let interrupted = JobSnapshot {
            id: "job-old".into(),
            kind: "source_sync".into(),
            status: JobStatus::Failed,
            request: SyncJobRequest {
                path: root.path().display().to_string(),
                remove_deleted: false,
                wing: Some("jobs-test".into()),
                room: None,
                max_file_bytes: None,
            },
            progress: Some(crate::source_sync::SourceSyncProgress {
                phase: crate::source_sync::SourceSyncPhase::Syncing,
                total_files: 10,
                processed_files: 3,
                current_path: None,
                added: 2,
                updated: 0,
                skipped: 1,
                deleted: 0,
                errors: 0,
                counters: crate::source_sync::SourceSyncCounters::default(),
            }),
            report: None,
            error: Some("interrupted by process restart".into()),
            cancel_requested: false,
            requeue_safe: true,
            resume_limits: Some(SOURCE_SYNC_REQUEUE_LIMITS.into()),
            requeued_from: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
        };
        {
            let mut jobs = state.jobs.jobs();
            jobs.insert(
                interrupted.id.clone(),
                JobEntry {
                    sequence: 1,
                    snapshot: interrupted.clone(),
                    cancellation: CancellationToken::new(),
                    progress_persisted_at: None,
                },
            );
        }

        let app = routes().with_state(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/jobs/job-old/requeue")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_ne!(body["job"]["id"], "job-old");
        assert_eq!(body["job"]["status"], "queued");
        assert_eq!(body["job"]["requeued_from"], "job-old");
        assert!(body["resume_limits"]
            .as_str()
            .unwrap()
            .contains("committed"));

        let finished = wait_for_terminal(&state.jobs, body["job"]["id"].as_str().unwrap()).await;
        assert_eq!(finished.status, JobStatus::Succeeded);
    }

    #[tokio::test]
    async fn requeue_refuses_active_job() {
        let root = tempfile::tempdir().unwrap();
        let state = state(root.path());
        {
            let mut jobs = state.jobs.jobs();
            jobs.insert("job-1".into(), entry(1, JobStatus::Running));
        }
        let err = state
            .jobs
            .requeue_source_sync(
                "job-1",
                state.store.clone(),
                state.embedder.clone(),
                state.config.clone(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Conflict(_)));
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
