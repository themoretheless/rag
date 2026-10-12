//! The UI keeps tolerant copies of some server DTOs (string timestamps, field
//! subsets). These tests serialize the real server types and require the UI
//! copies to read them, so a server rename or retype fails here, not at runtime.

use rag_mcp::db::{ProjectHome as ServerProjectHome, SourceRootSummary};
use rag_mcp::jobs::{JobSnapshot as ServerJobSnapshot, JobStatus, SourceSyncJobReport};
use rag_mcp::source_sync::{
    SourceSyncCounters, SourceSyncPhase, SourceSyncProgress, SourceSyncReport,
};

use crate::operations::JobSnapshot;
use crate::product::ProjectHome;

/// A server timestamp without adding chrono to the UI crate.
fn ts<T: serde::de::DeserializeOwned>() -> T {
    serde_json::from_value(serde_json::json!("2026-10-01T12:00:00Z")).unwrap()
}

#[test]
fn server_job_snapshot_reads_as_ui_job_snapshot() {
    let server = ServerJobSnapshot {
        id: "job-1".into(),
        kind: "source_sync".into(),
        status: JobStatus::CompletedWithErrors,
        request: rag_mcp_core::SyncJobRequest {
            path: "/vault".into(),
            remove_deleted: true,
            wing: Some("alpha".into()),
            room: None,
            max_file_bytes: None,
        },
        progress: Some(SourceSyncProgress {
            phase: SourceSyncPhase::Syncing,
            total_files: 4,
            processed_files: 2,
            current_path: Some("/vault/a.md".into()),
            added: 1,
            updated: 1,
            skipped: 0,
            deleted: 0,
            errors: 0,
            counters: SourceSyncCounters::default(),
        }),
        report: Some(SourceSyncJobReport::from(SourceSyncReport::default())),
        error: Some("one file failed".into()),
        cancel_requested: false,
        requeue_safe: true,
        resume_limits: None,
        requeued_from: None,
        created_at: ts(),
        started_at: Some(ts()),
        finished_at: None,
    };
    let wire = serde_json::to_value(&server).unwrap();
    let ui: JobSnapshot = serde_json::from_value(wire).expect("UI reads server JobSnapshot");
    assert_eq!(ui.id, "job-1");
    assert_eq!(ui.status, "completed_with_errors");
    assert_eq!(ui.request.wing.as_deref(), Some("alpha"));
    assert_eq!(ui.progress.expect("progress").processed_files, 2);
    assert!(ui.report.is_some());
    assert!(ui.started_at.is_some());
}

#[test]
fn server_project_home_reads_as_ui_project_home() {
    let server = ServerProjectHome {
        project_id: "alpha".into(),
        documents: 3,
        chunks: 9,
        raw_documents: 2,
        wiki_documents: 1,
        diary_documents: 0,
        archived_documents: 0,
        rooms: 1,
        source_files: 2,
        graph_nodes: 5,
        graph_edges: 4,
        unresolved_links: 1,
        source_roots: vec![SourceRootSummary {
            canonical_root: "/vault".into(),
            file_count: 2,
            last_seen: Some("2026-10-01T00:00:00Z".into()),
            last_synced: None,
        }],
        last_updated_at: Some(ts()),
    };
    let wire = serde_json::to_value(&server).unwrap();
    let ui: ProjectHome = serde_json::from_value(wire).expect("UI reads server ProjectHome");
    assert_eq!(ui.project_id, "alpha");
    assert_eq!(ui.unresolved_links, 1);
    assert_eq!(ui.source_roots[0].canonical_root, "/vault");
    assert!(ui.last_updated_at.is_some());
}
