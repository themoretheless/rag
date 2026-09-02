//! One-writer HTTP operations: health, jobs, checkpoint and verified backup.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

use crate::gateway::{
    execute_request, format_http_error, GatewayClient, Method, Request, ReqwestGatewayClient,
};

#[derive(Debug, Clone, Deserialize)]
pub struct StatusSnapshot {
    pub backend: String,
    pub schema_version: i32,
    pub fts_ready: bool,
    pub document_count: u64,
    pub chunk_count: u64,
    pub node_count: u64,
    pub edge_count: u64,
    #[serde(default)]
    pub raw_count: u64,
    #[serde(default)]
    pub wiki_count: u64,
    #[serde(default)]
    pub index_coverage: f64,
    #[serde(default)]
    pub uncompiled_raw_count: u64,
    #[serde(default)]
    pub embedding_manifest_match: bool,
    pub ready_for_search: bool,
    pub ingest_roots_configured: bool,
    pub db_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DoctorSnapshot {
    pub schema_version: i32,
    pub expected_schema_version: i32,
    pub schema_ok: bool,
    pub fts_ready: bool,
    pub embed_ok: bool,
    pub ready_for_search: bool,
    pub wal_bytes: u64,
    pub wal_warn_bytes: u64,
    pub wal_too_large: bool,
    pub documents_without_chunks: u64,
    pub orphan_chunks: u64,
    pub orphan_document_nodes: u64,
    pub orphan_edges: u64,
    pub unscoped_documents: u64,
    pub relational_integrity_ok: bool,
    #[serde(default)]
    pub repair_hint: Option<String>,
    pub ok: bool,
}

#[derive(Debug, Clone)]
pub struct OperationsSnapshot {
    pub status: StatusSnapshot,
    pub doctor: DoctorSnapshot,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct JobCounters {
    pub preflight: u64,
    pub extracted: u64,
    pub embedded: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JobProgress {
    pub phase: String,
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
    pub counters: JobCounters,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JobErrorSample {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JobReport {
    pub added_count: usize,
    pub updated_count: usize,
    pub skipped_count: usize,
    pub deleted_count: usize,
    pub error_count: usize,
    #[serde(default)]
    pub error_samples: Vec<JobErrorSample>,
    #[serde(default)]
    pub counters: JobCounters,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SyncJobRequest {
    pub path: String,
    pub remove_deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_file_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JobSnapshot {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub request: SyncJobRequest,
    #[serde(default)]
    pub progress: Option<JobProgress>,
    #[serde(default)]
    pub report: Option<JobReport>,
    #[serde(default)]
    pub error: Option<String>,
    pub cancel_requested: bool,
    pub created_at: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

impl JobSnapshot {
    pub fn can_cancel(&self) -> bool {
        matches!(self.status.as_str(), "queued" | "running") && !self.cancel_requested
    }
}

#[derive(Debug, Clone, Deserialize)]
struct JobsEnvelope {
    #[serde(default)]
    items: Vec<JobSnapshot>,
}

#[derive(Debug, Clone, Deserialize)]
struct JobEnvelope {
    job: JobSnapshot,
}

#[derive(Debug, Clone)]
pub struct MaintenanceResult {
    pub operation: String,
    pub report: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BackupRequest {
    pub path: String,
    pub dry_run: bool,
    pub overwrite: bool,
}

#[derive(Deserialize)]
struct ReportEnvelope {
    report: Value,
}

pub fn fetch_operations_http(base: &str) -> Result<OperationsSnapshot, String> {
    let client = client(15)?;
    fetch_operations_with_client(&client, base)
}

fn fetch_operations_with_client(
    client: &dyn GatewayClient,
    base: &str,
) -> Result<OperationsSnapshot, String> {
    let status = get_json(client, base, "v1/status")?;
    let doctor = get_json(client, base, "v1/doctor")?;
    Ok(OperationsSnapshot { status, doctor })
}

pub fn fetch_jobs_http(base: &str) -> Result<Vec<JobSnapshot>, String> {
    let client = client(15)?;
    fetch_jobs_with_client(&client, base)
}

fn fetch_jobs_with_client(
    client: &dyn GatewayClient,
    base: &str,
) -> Result<Vec<JobSnapshot>, String> {
    get_json::<JobsEnvelope>(client, base, "v1/jobs").map(|envelope| envelope.items)
}

pub fn start_sync_job_http(base: &str, request: &SyncJobRequest) -> Result<JobSnapshot, String> {
    let client = client(20)?;
    let body = serde_json::to_string(request)
        .map_err(|error| format!("serialize source sync request: {error}"))?;
    send_json::<JobEnvelope>(&client, base, "v1/jobs/sync", Method::Post, Some(body))
        .map(|envelope| envelope.job)
}

pub fn cancel_job_http(base: &str, id: &str) -> Result<JobSnapshot, String> {
    let client = client(15)?;
    let path = format!("v1/jobs/{}", encode_segment(id));
    send_json::<JobEnvelope>(&client, base, &path, Method::Delete, None)
        .map(|envelope| envelope.job)
}

pub fn checkpoint_http(base: &str) -> Result<MaintenanceResult, String> {
    let client = client(60)?;
    send_json::<ReportEnvelope>(
        &client,
        base,
        "v1/operations/checkpoint",
        Method::Post,
        None,
    )
    .map(|envelope| MaintenanceResult {
        operation: "Checkpoint".to_string(),
        report: envelope.report,
    })
}

pub fn backup_http(base: &str, request: &BackupRequest) -> Result<MaintenanceResult, String> {
    let client = client(120)?;
    let body =
        serde_json::to_string(request).map_err(|error| format!("serialize backup: {error}"))?;
    send_json::<ReportEnvelope>(
        &client,
        base,
        "v1/operations/backup",
        Method::Post,
        Some(body),
    )
    .map(|envelope| MaintenanceResult {
        operation: if request.dry_run {
            "Backup dry run".to_string()
        } else {
            "Backup".to_string()
        },
        report: envelope.report,
    })
}

fn client(timeout_secs: u64) -> Result<ReqwestGatewayClient, String> {
    ReqwestGatewayClient::new(Duration::from_secs(timeout_secs))
}

fn get_json<T: DeserializeOwned>(
    client: &dyn GatewayClient,
    base: &str,
    path: &str,
) -> Result<T, String> {
    send_json(client, base, path, Method::Get, None)
}

fn send_json<T: DeserializeOwned>(
    client: &dyn GatewayClient,
    base: &str,
    path: &str,
    method: Method,
    body: Option<String>,
) -> Result<T, String> {
    let url = join(base, path)?;
    let response = execute_request(
        client,
        Request {
            method,
            url: url.clone(),
            body,
            headers: Vec::new(),
        },
    )?;
    if !response.is_success() {
        return Err(format_http_error(&response, operation_context(path)));
    }
    serde_json::from_str(&response.body)
        .map_err(|error| format!("parse response from {url}: {error}"))
}

fn join(base: &str, path: &str) -> Result<String, String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("http base URL is empty".to_string());
    }
    Ok(format!("{base}/{}", path.trim_start_matches('/')))
}

fn operation_context(path: &str) -> &'static str {
    if path == "v1/status" {
        "System status"
    } else if path == "v1/doctor" {
        "Diagnostics"
    } else if path == "v1/operations/checkpoint" {
        "Checkpoint"
    } else if path == "v1/operations/backup" {
        "Backup"
    } else {
        "Sync jobs"
    }
}

fn encode_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::Response;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct FakeGateway {
        responses: Mutex<VecDeque<Response>>,
        requests: Mutex<Vec<Request>>,
    }

    impl FakeGateway {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses: Mutex::new(
                    responses
                        .into_iter()
                        .map(|body| Response { status: 200, body })
                        .collect(),
                ),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl GatewayClient for FakeGateway {
        fn execute(&self, request: Request) -> Result<Response, String> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| "no fake response".to_string())
        }
    }

    #[test]
    fn operations_combines_status_and_doctor() {
        let gateway = FakeGateway::new(vec![
            r#"{"backend":"duckdb","schema_version":9,"fts_ready":true,"document_count":4,"chunk_count":8,"node_count":5,"edge_count":6,"raw_count":3,"wiki_count":1,"index_coverage":1.0,"uncompiled_raw_count":0,"embedding_manifest_match":true,"ready_for_search":true,"ingest_roots_configured":true,"db_path":"/db"}"#.to_string(),
            r#"{"schema_version":9,"expected_schema_version":9,"schema_ok":true,"fts_ready":true,"embed_ok":true,"ready_for_search":true,"wal_bytes":10,"wal_warn_bytes":100,"wal_too_large":false,"documents_without_chunks":0,"orphan_chunks":0,"orphan_document_nodes":0,"orphan_edges":0,"unscoped_documents":0,"relational_integrity_ok":true,"repair_hint":null,"ok":true}"#.to_string(),
        ]);
        let snapshot = fetch_operations_with_client(&gateway, "http://gateway/").unwrap();
        assert_eq!(snapshot.status.document_count, 4);
        assert!(snapshot.doctor.ok);
        let requests = gateway.requests.lock().unwrap();
        assert_eq!(requests[0].url, "http://gateway/v1/status");
        assert_eq!(requests[1].url, "http://gateway/v1/doctor");
    }

    #[test]
    fn jobs_parse_progress_and_cancel_uses_delete() {
        let job = r#"{"id":"j1","kind":"source_sync","status":"running","request":{"path":"/src","remove_deleted":false,"wing":"alpha","room":null,"max_file_bytes":null},"progress":{"phase":"syncing","total_files":10,"processed_files":3,"current_path":"/src/a.md","added":2,"updated":0,"skipped":1,"deleted":0,"errors":0,"counters":{"preflight":3,"extracted":2,"embedded":2}},"report":null,"error":null,"cancel_requested":false,"created_at":"2026-09-02T10:00:00Z","started_at":"2026-09-02T10:00:01Z","finished_at":null}"#;
        let gateway = FakeGateway::new(vec![
            format!(r#"{{"ok":true,"items":[{job}]}}"#),
            format!(r#"{{"ok":true,"job":{job}}}"#),
        ]);
        let jobs = fetch_jobs_with_client(&gateway, "http://gateway").unwrap();
        assert_eq!(jobs[0].progress.as_ref().unwrap().processed_files, 3);
        let cancelled = send_json::<JobEnvelope>(
            &gateway,
            "http://gateway",
            "v1/jobs/j1",
            Method::Delete,
            None,
        )
        .unwrap()
        .job;
        assert_eq!(cancelled.id, "j1");
        assert_eq!(gateway.requests.lock().unwrap()[1].method, Method::Delete);
    }

    #[test]
    fn operation_errors_use_specific_safe_context() {
        assert_eq!(operation_context("v1/status"), "System status");
        assert_eq!(operation_context("v1/doctor"), "Diagnostics");
        assert_eq!(operation_context("v1/jobs/private-job-id"), "Sync jobs");
        assert_eq!(operation_context("v1/operations/backup"), "Backup");
    }
}
