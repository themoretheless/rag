//! Product-facing models and HTTP loaders for Project Home and Unified Library.
//!
//! Keeping these lean responses separate from document bodies makes the native
//! shell cheap to refresh even for large workspaces. All functions in this
//! module are called from the background worker, never from an egui frame.

use serde::Deserialize;
use std::time::Duration;

use crate::gateway::{
    execute_request, format_http_error, GatewayClient, Method, Request, ReqwestGatewayClient,
    Response,
};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
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
    pub last_updated_at: Option<String>,
    #[serde(default)]
    pub source_roots: Vec<ProjectSourceRoot>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProjectSourceRoot {
    pub canonical_root: String,
    pub file_count: u64,
    #[serde(default)]
    pub last_seen: Option<String>,
    #[serde(default)]
    pub last_synced: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LibraryItem {
    pub id: String,
    pub uri: String,
    pub title: String,
    #[serde(default)]
    pub wing: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
    #[serde(default)]
    pub source_file: Option<String>,
    #[serde(default = "default_layer")]
    pub layer: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub content_hash: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default = "default_boost")]
    pub boost: f64,
    #[serde(default = "default_status")]
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

impl LibraryItem {
    pub fn scope_label(&self) -> String {
        match (self.wing.as_deref(), self.room.as_deref()) {
            (Some(wing), Some(room)) if !wing.is_empty() && !room.is_empty() => {
                format!("{wing} / {room}")
            }
            (Some(wing), _) if !wing.is_empty() => wing.to_string(),
            (_, Some(room)) if !room.is_empty() => room.to_string(),
            _ => "Unscoped".to_string(),
        }
    }

    pub fn is_wiki(&self) -> bool {
        self.layer == "wiki" || self.uri.starts_with("wiki://")
    }
}

fn default_layer() -> String {
    "raw".to_string()
}

fn default_kind() -> String {
    "document".to_string()
}

fn default_status() -> String {
    "active".to_string()
}

fn default_boost() -> f64 {
    1.0
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryRequest {
    pub q: String,
    pub wing: String,
    pub room: String,
    pub layer: String,
    pub kind: String,
    pub status: String,
    pub include_archived: bool,
    pub limit: usize,
    pub cursor: Option<String>,
}

impl LibraryRequest {
    pub fn clear_filters(&mut self) {
        self.q.clear();
        self.room.clear();
        self.layer.clear();
        self.kind.clear();
        self.status.clear();
        self.include_archived = false;
        self.cursor = None;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LibraryPage {
    pub items: Vec<LibraryItem>,
    pub total: u64,
    pub next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct ProjectHomeEnvelope {
    project: ProjectHome,
}

#[derive(Deserialize)]
struct LibraryEnvelope {
    #[serde(default)]
    items: Vec<LibraryItem>,
    page: LibraryPageMeta,
}

#[derive(Deserialize)]
struct LibraryPageMeta {
    #[serde(default)]
    next_cursor: Option<String>,
    total: u64,
}

pub fn fetch_project_home_http(base: &str, project: &str) -> Result<ProjectHome, String> {
    let client = ReqwestGatewayClient::new(Duration::from_secs(20))?;
    fetch_project_home_with_client(&client, base, project)
}

fn fetch_project_home_with_client(
    client: &dyn GatewayClient,
    base: &str,
    project: &str,
) -> Result<ProjectHome, String> {
    let project = project.trim();
    if project.is_empty() {
        return Err("select a project to open Project Home".to_string());
    }
    let url = join_url(
        base,
        &format!("v1/project-home?project={}", encode_query(project)),
    )?;
    let response = get(client, url.clone())?;
    ensure_success(response, "Project Home").and_then(|body| {
        serde_json::from_str::<ProjectHomeEnvelope>(&body)
            .map(|envelope| envelope.project)
            .map_err(|error| format!("parse Project Home from {url}: {error}"))
    })
}

pub fn fetch_library_http(base: &str, request: &LibraryRequest) -> Result<LibraryPage, String> {
    let client = ReqwestGatewayClient::new(Duration::from_secs(30))?;
    fetch_library_with_client(&client, base, request)
}

fn fetch_library_with_client(
    client: &dyn GatewayClient,
    base: &str,
    request: &LibraryRequest,
) -> Result<LibraryPage, String> {
    let url = join_url(base, &library_path(request))?;
    let response = get(client, url.clone())?;
    ensure_success(response, "Library").and_then(|body| {
        serde_json::from_str::<LibraryEnvelope>(&body)
            .map(|envelope| LibraryPage {
                items: envelope.items,
                total: envelope.page.total,
                next_cursor: envelope.page.next_cursor,
            })
            .map_err(|error| format!("parse Unified Library from {url}: {error}"))
    })
}

fn library_path(request: &LibraryRequest) -> String {
    let mut fields = vec![("limit", request.limit.clamp(1, 200).to_string())];
    for (name, value) in [
        ("q", request.q.as_str()),
        ("wing", request.wing.as_str()),
        ("room", request.room.as_str()),
        ("layer", request.layer.as_str()),
        ("kind", request.kind.as_str()),
        ("status", request.status.as_str()),
    ] {
        let value = value.trim();
        if !value.is_empty() {
            fields.push((name, encode_query(value)));
        }
    }
    if request.include_archived {
        fields.push(("include_archived", "true".to_string()));
    }
    if let Some(cursor) = request
        .cursor
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        fields.push(("cursor", encode_query(cursor)));
    }
    let query = fields
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("v1/documents?{query}")
}

fn get(client: &dyn GatewayClient, url: String) -> Result<Response, String> {
    execute_request(
        client,
        Request {
            method: Method::Get,
            url,
            body: None,
            headers: Vec::new(),
        },
    )
}

fn ensure_success(response: Response, context: &str) -> Result<String, String> {
    if response.is_success() {
        Ok(response.body)
    } else {
        Err(format_http_error(&response, context))
    }
}

fn join_url(base: &str, path: &str) -> Result<String, String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("http base URL is empty".to_string());
    }
    Ok(format!("{}/{}", base, path.trim_start_matches('/')))
}

fn encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeGateway {
        response: Response,
        requests: Mutex<Vec<Request>>,
    }

    impl FakeGateway {
        fn ok(body: &str) -> Self {
            Self {
                response: Response {
                    status: 200,
                    body: body.to_string(),
                },
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl GatewayClient for FakeGateway {
        fn execute(&self, request: Request) -> Result<Response, String> {
            self.requests.lock().unwrap().push(request);
            Ok(self.response.clone())
        }
    }

    #[test]
    fn project_home_is_scoped_and_decoded() {
        let gateway = FakeGateway::ok(
            r#"{"ok":true,"project":{"project_id":"Project A/B","documents":12,"chunks":34,"raw_documents":8,"wiki_documents":3,"diary_documents":1,"archived_documents":2,"rooms":4,"source_files":9,"graph_nodes":10,"graph_edges":11,"unresolved_links":1,"last_updated_at":"2026-09-02T12:00:00Z","source_roots":[{"canonical_root":"/sources/project-a","file_count":9,"last_seen":"2026-09-02T11:59:00Z","last_synced":"2026-09-02T12:00:00Z"}]}}"#,
        );
        let home =
            fetch_project_home_with_client(&gateway, "http://gateway/", "Project A/B").unwrap();
        assert_eq!(home.documents, 12);
        assert_eq!(home.unresolved_links, 1);
        assert_eq!(home.source_roots[0].file_count, 9);
        let requests = gateway.requests.lock().unwrap();
        assert_eq!(
            requests[0].url,
            "http://gateway/v1/project-home?project=Project%20A%2FB"
        );
    }

    #[test]
    fn library_encodes_filters_and_cursor_and_stays_lean() {
        let gateway = FakeGateway::ok(
            r#"{"ok":true,"items":[{"id":"a","uri":"file:///a.md","title":"Alpha","wing":"Project A/B","room":"docs","source_file":"/a.md","layer":"raw","kind":"document","pinned":true,"boost":1.0,"status":"active","created_at":"2026-09-01T10:00:00Z","updated_at":"2026-09-02T10:00:00Z"}],"page":{"limit":50,"next_cursor":"v1:50","total":120}}"#,
        );
        let page = fetch_library_with_client(
            &gateway,
            "http://gateway",
            &LibraryRequest {
                q: "design notes".to_string(),
                wing: "Project A/B".to_string(),
                room: "docs".to_string(),
                include_archived: true,
                limit: 50,
                cursor: Some("v1:0".to_string()),
                ..LibraryRequest::default()
            },
        )
        .unwrap();
        assert_eq!(page.total, 120);
        assert_eq!(page.items[0].scope_label(), "Project A/B / docs");
        assert_eq!(page.next_cursor.as_deref(), Some("v1:50"));
        let request = &gateway.requests.lock().unwrap()[0];
        assert!(request.url.contains("q=design%20notes"));
        assert!(request.url.contains("wing=Project%20A%2FB"));
        assert!(request.url.contains("include_archived=true"));
        assert!(request.url.contains("cursor=v1%3A0"));
    }

    #[test]
    fn clear_filters_preserves_project_scope() {
        let mut request = LibraryRequest {
            q: "needle".to_string(),
            wing: "alpha".to_string(),
            room: "docs".to_string(),
            layer: "wiki".to_string(),
            include_archived: true,
            cursor: Some("v1:50".to_string()),
            ..LibraryRequest::default()
        };
        request.clear_filters();
        assert_eq!(request.wing, "alpha");
        assert!(request.q.is_empty());
        assert!(request.cursor.is_none());
        assert!(!request.include_archived);
    }

    #[test]
    fn failed_gateway_response_keeps_status_and_context() {
        let gateway = FakeGateway {
            response: Response {
                status: 503,
                body: "temporarily unavailable".to_string(),
            },
            requests: Mutex::new(Vec::new()),
        };
        let error =
            fetch_project_home_with_client(&gateway, "http://gateway", "alpha").unwrap_err();
        assert_eq!(error, "HTTP 503 · Project Home: temporarily unavailable");
    }
}
