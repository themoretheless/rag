use std::time::Duration;

use super::*;
use crate::embeddings::MockEmbedder;

fn request(path: &std::path::Path) -> SyncJobRequest {
    SyncJobRequest {
        path: path.display().to_string(),
        remove_deleted: false,
        wing: Some("proj".into()),
        room: None,
        max_file_bytes: None,
    }
}

fn queued_snapshot(id: &str, status: JobStatus) -> JobSnapshot {
    JobSnapshot {
        id: id.into(),
        kind: "source_sync".into(),
        status,
        request: request(std::path::Path::new("/nowhere")),
        progress: None,
        report: None,
        error: None,
        cancel_requested: false,
        requeue_safe: false,
        resume_limits: None,
        requeued_from: None,
        created_at: Utc::now(),
        started_at: None,
        finished_at: None,
    }
}

async fn wait_terminal(registry: &JobRegistry, id: &str) -> JobSnapshot {
    for _ in 0..500 {
        let job = registry.get(id).expect("job retained");
        if job.status.is_terminal() {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("job {id} did not finish");
}

#[test]
fn sync_command_copies_every_request_field() {
    let command = sync_command(&SyncJobRequest {
        path: "/data/root".into(),
        remove_deleted: true,
        wing: Some("w".into()),
        room: Some("r".into()),
        max_file_bytes: Some(10),
    });
    assert_eq!(command.path, PathBuf::from("/data/root"));
    assert!(command.remove_deleted);
    assert_eq!(command.wing.as_deref(), Some("w"));
    assert_eq!(command.room.as_deref(), Some("r"));
    assert_eq!(command.max_file_bytes, Some(10));
}

#[test]
fn empty_registry_reports_unknown_jobs() {
    let registry = JobRegistry::default();
    assert!(registry.list().is_empty());
    assert!(registry.get("missing").is_none());
    assert!(registry.cancel("missing").is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_source_sync_rejects_zero_max_bytes_without_registering_a_job() {
    let store = Arc::new(crate::testkit::temp_store());
    let registry = JobRegistry::with_store(store.clone());
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    let mut bad = request(std::path::Path::new("/tmp"));
    bad.max_file_bytes = Some(0);
    assert!(registry
        .start_source_sync(bad, store, embedder, Config::for_tests())
        .is_err());
    assert!(registry.list().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_source_sync_runs_to_success_and_survives_restore() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.md"), "# A\nalpha body").unwrap();
    std::fs::write(root.path().join("b.md"), "# B\nbeta body").unwrap();
    let store = Arc::new(crate::testkit::temp_store());
    let config = Config {
        ingest_roots: vec![root.path().to_path_buf()],
        embedding_dims: 8,
        ..Config::for_tests()
    };
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    let registry = JobRegistry::with_store(store.clone());

    let started = registry
        .start_source_sync(
            request(root.path()),
            store.clone(),
            embedder.clone(),
            config.clone(),
        )
        .unwrap();
    assert_eq!(started.kind, "source_sync");
    assert_eq!(started.status, JobStatus::Queued);

    let finished = wait_terminal(&registry, &started.id).await;
    assert_eq!(finished.status, JobStatus::Succeeded, "{finished:?}");
    let report = finished.report.expect("report");
    assert_eq!(report.added_count, 2);
    assert_eq!(report.error_count, 0);
    assert!(finished.finished_at.is_some());
    assert!(!finished.requeue_safe);

    // A succeeded job cannot be requeued, and cancelling it is a no-op.
    assert!(registry
        .requeue_source_sync(&started.id, store.clone(), embedder.clone(), config.clone())
        .is_err());
    assert_eq!(
        registry.cancel(&started.id).unwrap().status,
        JobStatus::Succeeded
    );

    let restored = JobRegistry::restore_from_store(store.clone()).unwrap();
    let job = restored.get(&started.id).expect("persisted job");
    assert_eq!(job.status, JobStatus::Succeeded);
    assert_eq!(job.report.map(|r| r.added_count), Some(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_marks_in_flight_jobs_failed_and_requeue_safe() {
    let store = Arc::new(crate::testkit::temp_store());
    crate::db::payload_kv::upsert_payload(
        &store,
        crate::db::payload_kv::PayloadTable::BackgroundJobs,
        "running-job",
        &queued_snapshot("running-job", JobStatus::Running),
    )
    .unwrap();
    crate::db::payload_kv::upsert_payload(
        &store,
        crate::db::payload_kv::PayloadTable::BackgroundJobs,
        "done-job",
        &queued_snapshot("done-job", JobStatus::Succeeded),
    )
    .unwrap();

    let restored = JobRegistry::restore_from_store(store.clone()).unwrap();
    let interrupted = restored.get("running-job").unwrap();
    assert_eq!(interrupted.status, JobStatus::Failed);
    assert!(interrupted.requeue_safe);
    assert_eq!(
        interrupted.resume_limits.as_deref(),
        Some(SOURCE_SYNC_REQUEUE_LIMITS)
    );
    assert!(interrupted.error.unwrap().contains("interrupted"));
    assert_eq!(
        restored.get("done-job").unwrap().status,
        JobStatus::Succeeded
    );
    assert_eq!(restored.list().len(), 2);

    // The interrupted state is written back, so a second restart sees the same row.
    let again = JobRegistry::restore_from_store(store).unwrap();
    assert_eq!(again.get("running-job").unwrap().status, JobStatus::Failed);
}
