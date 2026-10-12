//! Background source-sync jobs: registry, lifecycle, retention and persistence.
//!
//! Transport-free: `http_api::jobs` maps these onto `/v1/jobs*`; another port
//! (e.g. MCP) can drive the same registry and single writer lane.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::Utc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub use rag_mcp_core::{JobSnapshot, JobStatus, SourceSyncJobReport};

use crate::config::Config;
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::source_sync::{
    sync_sources_nonblocking, SourceSyncCommand, SourceSyncControl, SourceSyncOutcome,
    SourceSyncReport,
};

pub(crate) const MAX_RETAINED_JOBS: usize = 100;
pub(crate) const MAX_ERROR_SAMPLES: usize = 20;

/// Honest resume limits for source_sync after process interrupt.
///
/// Requeue is safe because sync skips unchanged committed files (committed-prefix
/// awareness via source manifests). It does **not** resume an in-flight embedding
/// batch; any uncommitted batch from the interrupted run was never written.
pub const SOURCE_SYNC_REQUEUE_LIMITS: &str = "requeue_safe: source_sync skips unchanged committed files; does not resume mid-batch embeds or delete-phase mid-flight";

pub use rag_mcp_core::SyncJobRequest;

pub(crate) fn sync_command(request: &SyncJobRequest) -> SourceSyncCommand {
    SourceSyncCommand {
        path: PathBuf::from(&request.path),
        remove_deleted: request.remove_deleted,
        wing: request.wing.clone(),
        room: request.room.clone(),
        max_file_bytes: request.max_file_bytes,
    }
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


pub(crate) struct JobEntry {
    pub(crate) sequence: u64,
    pub(crate) snapshot: JobSnapshot,
    pub(crate) cancellation: CancellationToken,
}

pub(crate) struct JobRegistryInner {
    pub(crate) jobs: Mutex<HashMap<String, JobEntry>>,
    pub(crate) writer_lane: tokio::sync::Mutex<()>,
    pub(crate) next_sequence: AtomicU64,
    pub(crate) store: Option<Arc<Store>>,
}

/// Process-local job state with optional DuckDB snapshots. All write-side jobs
/// share one FIFO-ish mutex lane.
#[derive(Clone)]
pub struct JobRegistry {
    pub(crate) inner: Arc<JobRegistryInner>,
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self::new(None)
    }
}

impl JobRegistry {
    pub fn new(store: Option<Arc<Store>>) -> Self {
        Self {
            inner: Arc::new(JobRegistryInner {
                jobs: Mutex::new(HashMap::new()),
                writer_lane: tokio::sync::Mutex::new(()),
                next_sequence: AtomicU64::new(1),
                store,
            }),
        }
    }

    pub fn with_store(store: Arc<Store>) -> Self {
        Self::new(Some(store))
    }

    /// Load durable job history and mark any in-flight rows as failed.
    ///
    /// Interrupted `source_sync` jobs are marked `requeue_safe` so operators can
    /// `POST /v1/jobs/{id}/requeue` without inventing mid-batch resume. Requeue
    /// starts a fresh sync that skips already-committed unchanged files.
    pub fn restore_from_store(store: Arc<Store>) -> Result<Self, AppError> {
        let registry = Self::with_store(store.clone());
        let snapshots = crate::db::payload_kv::with_session(&store, |kv| {
            let table = crate::db::payload_kv::PayloadTable::BackgroundJobs;
            let pairs = kv.list_pairs(table)?;
            let mut out = Vec::with_capacity(pairs.len());
            for (id, payload) in pairs {
                let mut snapshot: JobSnapshot = serde_json::from_str(&payload)?;
                if !snapshot.status.is_terminal() {
                    mark_interrupted(&mut snapshot);
                    kv.upsert(table, &id, &snapshot)?;
                }
                out.push(snapshot);
            }
            Ok(out)
        })?;
        {
            let mut jobs = registry.jobs();
            for snapshot in snapshots {
                let sequence = registry.inner.next_sequence.fetch_add(1, Ordering::Relaxed);
                jobs.insert(
                    snapshot.id.clone(),
                    JobEntry {
                        sequence,
                        snapshot,
                        cancellation: CancellationToken::new(),
                    },
                );
            }
        }
        Ok(registry)
    }

    pub(crate) fn persist(&self, snapshot: &JobSnapshot) -> Result<(), AppError> {
        let Some(store) = &self.inner.store else {
            return Ok(());
        };
        crate::db::payload_kv::upsert_payload(
            store,
            crate::db::payload_kv::PayloadTable::BackgroundJobs,
            &snapshot.id,
            snapshot,
        )?;
        Ok(())
    }

    pub(crate) fn delete_persisted(&self, id: &str) -> Result<(), AppError> {
        let Some(store) = &self.inner.store else {
            return Ok(());
        };
        crate::db::payload_kv::delete_payload(
            store,
            crate::db::payload_kv::PayloadTable::BackgroundJobs,
            id,
        )
    }

    /// Requeue a terminal `source_sync` job that is marked `requeue_safe`.
    ///
    /// Creates a **new** queued job with the same request. Does not resurrect the
    /// old id or invent mid-batch resume state.
    pub fn requeue_source_sync(
        &self,
        id: &str,
        store: Arc<Store>,
        embedder: Arc<dyn EmbeddingProvider>,
        config: Config,
    ) -> Result<JobSnapshot, AppError> {
        let prior = self
            .get(id)
            .ok_or_else(|| AppError::not_found(format!("job not found: {id}")))?;
        if !prior.status.is_terminal() {
            return Err(AppError::conflict(
                "job is still active; cancel or wait before requeue",
            ));
        }
        if prior.kind != "source_sync" || !prior.requeue_safe {
            return Err(AppError::invalid(
                "job is not requeue_safe; only interrupted/failed source_sync jobs with committed-prefix awareness may be requeued",
            ));
        }
        self.start_source_sync_from(
            prior.request.clone(),
            store,
            embedder,
            config,
            Some(prior.id),
        )
    }

    pub fn start_source_sync(
        &self,
        request: SyncJobRequest,
        store: Arc<Store>,
        embedder: Arc<dyn EmbeddingProvider>,
        config: Config,
    ) -> Result<JobSnapshot, AppError> {
        self.start_source_sync_from(request, store, embedder, config, None)
    }

    pub(crate) fn start_source_sync_from(
        &self,
        request: SyncJobRequest,
        store: Arc<Store>,
        embedder: Arc<dyn EmbeddingProvider>,
        config: Config,
        requeued_from: Option<String>,
    ) -> Result<JobSnapshot, AppError> {
        if request.max_file_bytes == Some(0) {
            return Err(AppError::invalid(
                "max_file_bytes must be greater than zero",
            ));
        }
        let id = Uuid::new_v4().to_string();
        let cancellation = CancellationToken::new();
        let snapshot = JobSnapshot {
            id: id.clone(),
            kind: "source_sync".into(),
            status: JobStatus::Queued,
            request: request.clone(),
            progress: None,
            report: None,
            error: None,
            cancel_requested: false,
            requeue_safe: false,
            resume_limits: None,
            requeued_from,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
        };
        let sequence = self.inner.next_sequence.fetch_add(1, Ordering::Relaxed);
        let (removed, accepted) = {
            let mut jobs = self.jobs();
            let removed = prune_jobs_to(&mut jobs, MAX_RETAINED_JOBS.saturating_sub(1));
            if jobs.len() >= MAX_RETAINED_JOBS {
                (removed, false)
            } else {
                jobs.insert(
                    id.clone(),
                    JobEntry {
                        sequence,
                        snapshot: snapshot.clone(),
                        cancellation: cancellation.clone(),
                    },
                );
                (removed, true)
            }
        };
        for removed_id in removed {
            self.delete_persisted(&removed_id)?;
        }
        if !accepted {
            return Err(AppError::busy(format!(
                "background job capacity reached ({MAX_RETAINED_JOBS} active jobs)"
            )));
        }
        self.persist(&snapshot)?;

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
                sync_sources_nonblocking(store, embedder, config, sync_command(&request), control)
                    .await;
            match result {
                Ok(SourceSyncOutcome::Completed(report)) => {
                    let status = if report.errors.is_empty() {
                        JobStatus::Succeeded
                    } else {
                        JobStatus::CompletedWithErrors
                    };
                    registry.finish(&id, status, Some(report), None, Some(false));
                }
                Ok(SourceSyncOutcome::Cancelled(report)) => registry.finish_cancelled(&id, report),
                Err(_error) if cancellation.is_cancelled() => {
                    registry.finish_cancelled(&id, SourceSyncReport::default());
                }
                Err(error) => {
                    registry.finish_failed_requeueable(&id, error.to_string());
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
        self.persist(&snapshot)?;
        Ok(snapshot)
    }

    /// Atomically claim a queued job for the writer lane.
    ///
    /// Cancellation uses the same registry mutex, so a queued job either
    /// becomes terminal `cancelled` or transitions to `running`; it can never
    /// be resurrected from `cancelled` by a worker that was waiting for the
    /// writer lane.
    pub(crate) fn mark_running_if_queued(&self, id: &str) -> bool {
        let snapshot = {
            let mut jobs = self.jobs();
            let Some(entry) = jobs.get_mut(id) else {
                return false;
            };
            if entry.snapshot.status != JobStatus::Queued {
                return false;
            }
            entry.snapshot.status = JobStatus::Running;
            entry.snapshot.started_at = Some(Utc::now());
            entry.snapshot.clone()
        };
        let _ = self.persist(&snapshot);
        true
    }

    pub(crate) fn update(&self, id: &str, update: impl FnOnce(&mut JobSnapshot)) {
        let snapshot = {
            let mut jobs = self.jobs();
            let Some(entry) = jobs.get_mut(id) else {
                return;
            };
            update(&mut entry.snapshot);
            entry.snapshot.clone()
        };
        let _ = self.persist(&snapshot);
    }

    pub(crate) fn finish_cancelled(&self, id: &str, report: SourceSyncReport) {
        // Cancelled runs may already have committed a prefix; requeue is safe.
        self.finish(id, JobStatus::Cancelled, Some(report), None, Some(true));
    }

    pub(crate) fn finish_failed_requeueable(&self, id: &str, error: String) {
        self.finish(id, JobStatus::Failed, None, Some(error), Some(true));
    }

    pub(crate) fn finish(
        &self,
        id: &str,
        status: JobStatus,
        report: Option<SourceSyncReport>,
        error: Option<String>,
        requeue_safe: Option<bool>,
    ) {
        let (snapshot, removed) = {
            let mut jobs = self.jobs();
            let snapshot = if let Some(entry) = jobs.get_mut(id) {
                if entry.snapshot.status.is_terminal() {
                    return;
                }
                entry.snapshot.status = status;
                entry.snapshot.report = report.map(SourceSyncJobReport::from);
                entry.snapshot.error = error;
                entry.snapshot.finished_at = Some(Utc::now());
                if let Some(safe) = requeue_safe {
                    entry.snapshot.requeue_safe = safe;
                    entry.snapshot.resume_limits = safe.then(|| SOURCE_SYNC_REQUEUE_LIMITS.into());
                }
                entry.snapshot.clone()
            } else {
                return;
            };
            let removed = prune_jobs(&mut jobs);
            (snapshot, removed)
        };
        let _ = self.persist(&snapshot);
        for removed_id in removed {
            let _ = self.delete_persisted(&removed_id);
        }
    }

    pub(crate) fn jobs(&self) -> MutexGuard<'_, HashMap<String, JobEntry>> {
        self.inner
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub(crate) fn mark_interrupted(snapshot: &mut JobSnapshot) {
    snapshot.status = JobStatus::Failed;
    snapshot.error = Some("interrupted by process restart".into());
    snapshot.finished_at = Some(Utc::now());
    if snapshot.kind == "source_sync" {
        snapshot.requeue_safe = true;
        snapshot.resume_limits = Some(SOURCE_SYNC_REQUEUE_LIMITS.into());
    }
}

pub(crate) fn mark_cancelled(snapshot: &mut JobSnapshot, report: SourceSyncReport) {
    snapshot.status = JobStatus::Cancelled;
    snapshot.report = Some(report.into());
    snapshot.error = None;
    snapshot.finished_at = Some(Utc::now());
    if snapshot.kind == "source_sync" {
        snapshot.requeue_safe = true;
        snapshot.resume_limits = Some(SOURCE_SYNC_REQUEUE_LIMITS.into());
    }
}

pub(crate) fn prune_jobs(jobs: &mut HashMap<String, JobEntry>) -> Vec<String> {
    prune_jobs_to(jobs, MAX_RETAINED_JOBS)
}

pub(crate) fn prune_jobs_to(jobs: &mut HashMap<String, JobEntry>, maximum: usize) -> Vec<String> {
    let mut removed = Vec::new();
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
        removed.push(id);
    }
    removed
}
