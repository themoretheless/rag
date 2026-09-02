//! Revision history HTTP client. Restore always uses gateway-side CAS.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::Duration;

use crate::gateway::{
    execute_request, format_http_error, format_json_parse_error, GatewayClient, Method, Request,
    ReqwestGatewayClient,
};
use crate::load::DocumentBody;

pub const REVISION_PAGE_SIZE: usize = 50;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RevisionItem {
    pub document_id: String,
    pub uri: String,
    pub title: String,
    #[serde(default)]
    pub wing: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
    pub layer: String,
    pub kind: String,
    pub status: String,
    pub updated_at: String,
    pub superseded_at: String,
    pub revision: i64,
    pub content_chars: u64,
    pub content_lines: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionPage {
    pub items: Vec<RevisionItem>,
    pub total: u64,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RevisionLineChange {
    pub kind: String,
    pub line: usize,
    pub content: String,
    pub content_truncated: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RevisionDiff {
    pub document_id: String,
    pub from_revision: i64,
    pub to_revision: i64,
    pub title_changed: bool,
    pub metadata_changed: bool,
    pub placement_changed: bool,
    pub added_lines: usize,
    pub removed_lines: usize,
    #[serde(default)]
    pub changes: Vec<RevisionLineChange>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RestoreRevisionResult {
    pub document_id: String,
    pub restored_from_revision: i64,
    pub revision: i64,
    pub etag: String,
    pub chunk_count: usize,
    pub node_id: String,
    pub edge_count: usize,
}

#[derive(Deserialize)]
struct RevisionsEnvelope {
    #[serde(default)]
    items: Vec<RevisionItem>,
    page: RevisionsPage,
}

#[derive(Deserialize)]
struct RevisionsPage {
    total: u64,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct DiffEnvelope {
    result: RevisionDiff,
}

#[derive(Deserialize)]
struct RestoreEnvelope {
    result: RestoreRevisionResult,
}

#[derive(Deserialize)]
struct SnapshotEnvelope {
    result: DocumentBody,
}

#[derive(Serialize)]
struct RestoreRequest<'a> {
    document_id: &'a str,
    revision: i64,
    if_match_revision: i64,
}

pub fn fetch_revisions_http(
    base: &str,
    document_id: &str,
    cursor: Option<&str>,
) -> Result<RevisionPage, String> {
    let client = client(20)?;
    fetch_revisions_with_client(&client, base, document_id, cursor)
}

fn fetch_revisions_with_client(
    client: &dyn GatewayClient,
    base: &str,
    document_id: &str,
    cursor: Option<&str>,
) -> Result<RevisionPage, String> {
    let mut path = format!(
        "v1/revisions?document_id={}&limit={REVISION_PAGE_SIZE}",
        encode_query(document_id)
    );
    if let Some(cursor) = cursor.filter(|cursor| !cursor.is_empty()) {
        path.push_str(&format!("&cursor={}", encode_query(cursor)));
    }
    get_json::<RevisionsEnvelope>(client, base, &path).map(|envelope| RevisionPage {
        items: envelope.items,
        total: envelope.page.total,
        next_cursor: envelope.page.next_cursor,
    })
}

pub fn fetch_revision_snapshot_http(
    base: &str,
    document_id: &str,
    revision: i64,
) -> Result<DocumentBody, String> {
    let client = client(20)?;
    fetch_revision_snapshot_with_client(&client, base, document_id, revision)
}

fn fetch_revision_snapshot_with_client(
    client: &dyn GatewayClient,
    base: &str,
    document_id: &str,
    revision: i64,
) -> Result<DocumentBody, String> {
    let path = format!(
        "v1/revisions/snapshot?document_id={}&revision={revision}",
        encode_query(document_id)
    );
    get_json::<SnapshotEnvelope>(client, base, &path).map(|envelope| envelope.result)
}

pub fn fetch_revision_diff_http(
    base: &str,
    document_id: &str,
    from_revision: i64,
    to_revision: Option<i64>,
) -> Result<RevisionDiff, String> {
    let client = client(20)?;
    let mut path = format!(
        "v1/revisions/diff?document_id={}&from_revision={from_revision}",
        encode_query(document_id)
    );
    if let Some(to_revision) = to_revision {
        path.push_str(&format!("&to_revision={to_revision}"));
    }
    get_json::<DiffEnvelope>(&client, base, &path).map(|envelope| envelope.result)
}

pub fn restore_revision_http(
    base: &str,
    document_id: &str,
    revision: i64,
    if_match_revision: i64,
) -> Result<RestoreRevisionResult, String> {
    let client = client(120)?;
    let body = serde_json::to_string(&RestoreRequest {
        document_id,
        revision,
        if_match_revision,
    })
    .map_err(|error| format!("serialize restore request: {error}"))?;
    send_json::<RestoreEnvelope>(
        &client,
        base,
        "v1/revisions/restore",
        Method::Post,
        Some(body),
    )
    .map(|envelope| envelope.result)
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
        return Err(format_http_error(&response, revision_context(path)));
    }
    serde_json::from_str(&response.body)
        .map_err(|error| format_json_parse_error(revision_context(path), &error))
}

fn join(base: &str, path: &str) -> Result<String, String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("http base URL is empty".to_string());
    }
    Ok(format!("{base}/{}", path.trim_start_matches('/')))
}

fn revision_context(path: &str) -> &'static str {
    if path.starts_with("v1/revisions/snapshot") {
        "Revision snapshot"
    } else if path.starts_with("v1/revisions/diff") {
        "Revision comparison"
    } else if path == "v1/revisions/restore" {
        "Revision restore"
    } else {
        "Revision history"
    }
}

fn encode_query(value: &str) -> String {
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
    fn revision_history_is_lean_paginated_and_scoped() {
        let item = r#"{"document_id":"d1","uri":"wiki://a","title":"A","wing":"alpha","room":null,"layer":"wiki","kind":"wiki","status":"active","updated_at":"2026-09-01T00:00:00Z","superseded_at":"2026-09-02T00:00:00Z","revision":2,"content_chars":3,"content_lines":1}"#;
        let gateway = FakeGateway::new(vec![format!(
            r#"{{"ok":true,"items":[{item}],"page":{{"limit":50,"next_cursor":"v1:50","total":77}}}}"#
        )]);
        let page =
            fetch_revisions_with_client(&gateway, "http://gateway", "d1/a", Some("v1:0")).unwrap();
        assert_eq!(page.items[0].revision, 2);
        assert_eq!(page.items[0].content_chars, 3);
        assert_eq!(page.total, 77);
        assert_eq!(page.next_cursor.as_deref(), Some("v1:50"));
        let requests = gateway.requests.lock().unwrap();
        assert!(requests[0].url.contains("document_id=d1%2Fa"));
        assert!(requests[0].url.contains("limit=50"));
        assert!(requests[0].url.contains("cursor=v1%3A0"));
        assert!(!requests[0].url.contains("content"));
    }

    #[test]
    fn snapshot_body_is_loaded_separately_for_one_revision() {
        let gateway = FakeGateway::new(vec![
            r#"{"ok":true,"result":{"id":"d1","uri":"wiki://a","title":"A","content":"historical body","metadata_json":"{}","wing":"alpha","room":null,"source_file":null,"layer":"wiki","kind":"wiki","content_hash":"h","pinned":false,"boost":1.0,"status":"active","created_at":"2026-09-01T00:00:00Z","updated_at":"2026-09-02T00:00:00Z","revision":2}}"#.to_string(),
        ]);
        let body =
            fetch_revision_snapshot_with_client(&gateway, "http://gateway", "d1/a", 2).unwrap();
        assert_eq!(body.id, "d1");
        assert_eq!(body.content, "historical body");
        let requests = gateway.requests.lock().unwrap();
        assert!(requests[0]
            .url
            .ends_with("v1/revisions/snapshot?document_id=d1%2Fa&revision=2"));
    }

    #[test]
    fn revision_diff_is_scoped_and_parsed() {
        let gateway = FakeGateway::new(vec![
            r#"{"ok":true,"result":{"document_id":"d1","from_revision":2,"to_revision":3,"title_changed":false,"metadata_changed":false,"placement_changed":false,"added_lines":1,"removed_lines":1,"changes":[{"kind":"removed","line":1,"content":"old","content_truncated":false},{"kind":"added","line":1,"content":"new","content_truncated":false}],"truncated":false}}"#.to_string(),
        ]);
        let diff_path = "v1/revisions/diff?document_id=d1%2Fa&from_revision=2";
        let diff = get_json::<DiffEnvelope>(&gateway, "http://gateway", diff_path)
            .unwrap()
            .result;
        assert_eq!((diff.added_lines, diff.removed_lines), (1, 1));
    }

    #[test]
    fn restore_posts_required_cas_revision() {
        let gateway = FakeGateway::new(vec![
            r#"{"ok":true,"result":{"document_id":"d1","restored_from_revision":2,"revision":4,"etag":"W/\"4\"","chunk_count":3,"node_id":"n1","edge_count":1}}"#.to_string(),
        ]);
        let body = serde_json::to_string(&RestoreRequest {
            document_id: "d1",
            revision: 2,
            if_match_revision: 3,
        })
        .unwrap();
        let result = send_json::<RestoreEnvelope>(
            &gateway,
            "http://gateway",
            "v1/revisions/restore",
            Method::Post,
            Some(body),
        )
        .unwrap()
        .result;
        assert_eq!(result.revision, 4);
        let requests = gateway.requests.lock().unwrap();
        let body = requests[0].body.as_deref().unwrap();
        assert!(body.contains(r#""revision":2"#));
        assert!(body.contains(r#""if_match_revision":3"#));
    }

    #[test]
    fn revision_errors_use_specific_safe_context() {
        assert_eq!(
            revision_context("v1/revisions?document_id=private-id"),
            "Revision history"
        );
        assert_eq!(
            revision_context("v1/revisions/snapshot?document_id=private-id"),
            "Revision snapshot"
        );
        assert_eq!(
            revision_context("v1/revisions/diff?document_id=private-id"),
            "Revision comparison"
        );
        assert_eq!(revision_context("v1/revisions/restore"), "Revision restore");
    }

    #[test]
    fn malformed_revision_success_does_not_echo_private_query() {
        let gateway = FakeGateway::new(vec![r#"{"items":["private-value""#.to_string()]);
        let error = get_json::<RevisionsEnvelope>(
            &gateway,
            "http://user:secret@gateway",
            "v1/revisions?document_id=private-id&token=private-token",
        )
        .err()
        .expect("malformed response is rejected");

        assert!(error.starts_with("Could not parse Revision history response"));
        for secret in [
            "user",
            "secret",
            "private-id",
            "private-token",
            "private-value",
        ] {
            assert!(!error.contains(secret));
        }
    }
}
