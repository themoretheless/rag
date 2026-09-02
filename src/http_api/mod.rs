//! Local HTTP gateway for rag-mcp (same process as DuckDB).
//!
//! Bind with `RAG_HTTP_BIND=127.0.0.1:7432`.
//!
//! | Path | Role |
//! |------|------|
//! | `/health`, `/v1/graph`, `/v1/neighbors`, `/v1/find`, `/v1/document` | Graph + document UI |
//! | `GET /v1/wiki`, `PUT /v1/wiki`, `GET /v1/backlinks` | Wiki catalog, write (CAS), backlinks |
//! | `POST /v1/search`, `POST /v1/pack-context` | Retrieval lab: full `SearchParams`, packing |
//! | `/v1/status`, `/v1/doctor`, `/v1/runtime`, `/v1/calls`, `/v1/agents` | Console health + call log |
//! | `/v1/ops-log`, `/v1/taxonomy`, `/v1/diary`, `/v1/kg*`, `/v1/tunnels`, `/v1/llm-status` | Read-only console views |
//! | `/mcp` | Streamable HTTP MCP (Claude, ChatGPT, any remote MCP client) |
//!
//! Does **not** open a second DuckDB connection; shares the server's [`Store`].
//! UI: `rag-mcp-ui --http http://127.0.0.1:7432`.
//! MCP URL: `http://127.0.0.1:7432/mcp`.
//!
//! `PUT /v1/wiki` accepts optional `if_match_revision` / `if_match_etag` (same semantics as MCP
//! `write_wiki_page`). Stale CAS returns **409 Conflict**.
//!
//! # Submodules
//!
//! | Module | Role |
//! |--------|------|
//! | [`bind`] | `parse_bind` + loopback gate |
//! | [`error`] | `api_ok` / `api_err` / `status_for` |
//! | [`health`] | `/health` |
//! | [`graph`] | Graph + document handlers |
//! | [`wiki`] | Wiki list/put/backlinks |

mod activity;
mod admin;
mod bind;
mod error;
mod graph;
mod health;
mod jobs;
mod ops;
mod retrieval;
mod wiki;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager,
    tower::{StreamableHttpServerConfig, StreamableHttpService},
};
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::db::Store;
use crate::embeddings::{build_provider, EmbeddingProvider};
use crate::mcp::RagServer;
use crate::models::GraphView;

pub use bind::parse_bind;

/// Shared state for graph/wiki HTTP handlers (clone of MCP store).
#[derive(Clone)]
pub struct HttpState {
    pub store: Arc<Store>,
    /// True when streamable MCP is mounted at `/mcp`.
    pub mcp_http: bool,
    /// Config for wiki write (chunking / embed dims). Built in [`serve`] from env.
    pub config: Config,
    /// Embedder for wiki write re-chunk path.
    pub embedder: Arc<dyn EmbeddingProvider>,
    /// Process-local background job registry and single writer lane.
    pub(super) jobs: jobs::JobRegistry,
}

impl HttpState {
    /// Construct HTTP state with an isolated background-job registry. Prefer
    /// this over struct literals so internal route state can evolve without
    /// forcing downstream callers to know private implementation details.
    pub fn new(
        store: Arc<Store>,
        mcp_http: bool,
        config: Config,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self {
            store,
            mcp_http,
            config,
            embedder,
            jobs: jobs::JobRegistry::default(),
        }
    }
}

/// Build streamable-HTTP MCP service that shares `store` / embedder / config.
///
/// Each MCP session gets a fresh [`RagServer`] clone of the same DuckDB `Store`.
pub fn mcp_http_service(
    store: Store,
    embedder: Arc<dyn EmbeddingProvider>,
    config: Config,
    cancellation_token: CancellationToken,
) -> StreamableHttpService<RagServer, LocalSessionManager> {
    let allowed_hosts = configured_http_allowed_hosts(
        &config,
        std::env::var("RAG_HTTP_ALLOWED_HOSTS").ok().as_deref(),
    );
    StreamableHttpService::new(
        move || {
            Ok(
                RagServer::new(store.clone(), embedder.clone(), config.clone())
                    .with_transport("http-mcp"),
            )
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_stateful_mode(true)
            .with_sse_keep_alive(Some(std::time::Duration::from_secs(15)))
            .with_cancellation_token(cancellation_token)
            .with_allowed_hosts(allowed_hosts),
    )
}

/// Exact Host authorities accepted by rmcp's DNS-rebinding guard.
///
/// Loopback is always available. A concrete bind address is safe to add
/// automatically; wildcard binds (`0.0.0.0` / `[::]`) require explicit remote
/// names or addresses in `RAG_HTTP_ALLOWED_HOSTS` (comma-separated).
fn configured_http_allowed_hosts(config: &Config, explicit: Option<&str>) -> Vec<String> {
    let mut hosts = BTreeSet::from([
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ]);
    if let Some(bind) = config
        .http_bind
        .as_deref()
        .and_then(|raw| raw.parse::<SocketAddr>().ok())
        .filter(|bind| !bind.ip().is_unspecified())
    {
        hosts.insert(bind.ip().to_string());
    }
    if let Some(explicit) = explicit {
        hosts.extend(
            explicit
                .split(',')
                .map(str::trim)
                .filter(|host| {
                    !host.is_empty()
                        && host.len() <= 255
                        && !host.contains('/')
                        && !host.chars().any(char::is_control)
                        && !host.chars().any(char::is_whitespace)
                })
                .map(str::to_owned),
        );
    }
    hosts.into_iter().collect()
}

/// Start HTTP server (graph API + MCP `/mcp`); runs until process exit.
///
/// Builds embedder/config from env for `PUT /v1/wiki` (same process env as MCP).
/// Route tables come from [`health::routes`], [`graph::routes`], [`wiki::routes`] (merge-only).
pub async fn serve(
    bind: SocketAddr,
    store: Arc<Store>,
    mcp: Option<StreamableHttpService<RagServer, LocalSessionManager>>,
) -> Result<(), std::io::Error> {
    let mcp_http = mcp.is_some();
    let config = Config::from_env().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("HTTP wiki write config: {e}"),
        )
    })?;
    let embedder = build_provider(&config)
        .map_err(|e| std::io::Error::other(format!("HTTP wiki write embedder: {e}")))?;
    let allowed_hosts = configured_http_allowed_hosts(
        &config,
        std::env::var("RAG_HTTP_ALLOWED_HOSTS").ok().as_deref(),
    );
    let api = api_router(HttpState::new(store, mcp_http, config, embedder));

    let app = if let Some(mcp_svc) = mcp {
        Router::new().merge(api).nest_service("/mcp", mcp_svc)
    } else {
        api
    };
    let app = gateway_layers(app, allowed_hosts);

    tracing::info!(%bind, mcp_http, mcp_path = if mcp_http { "/mcp" } else { "(disabled)" },
        "rag-mcp HTTP gateway listening (graph UI + optional streamable MCP)");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn gateway_layers(app: Router, allowed_hosts: Vec<String>) -> Router {
    app.layer(axum::middleware::from_fn(enforce_body_limit))
        .layer(axum::middleware::from_fn_with_state(
            HttpHostAllowlist::new(allowed_hosts),
            enforce_host_allowlist,
        ))
        .layer(axum::middleware::from_fn(http_metadata))
}

#[derive(Clone)]
struct HttpHostAllowlist {
    allowed: Arc<[AllowedAuthority]>,
}

impl HttpHostAllowlist {
    fn new(hosts: Vec<String>) -> Self {
        Self {
            allowed: hosts
                .into_iter()
                .filter_map(|host| AllowedAuthority::from_allowed(&host))
                .collect::<Vec<_>>()
                .into(),
        }
    }

    fn allows(&self, authority: &AllowedAuthority) -> bool {
        self.allowed.iter().any(|allowed| {
            allowed.host == authority.host
                && allowed.port.is_none_or(|port| authority.port == Some(port))
        })
    }
}

#[derive(Clone)]
struct AllowedAuthority {
    host: String,
    port: Option<u16>,
}

impl AllowedAuthority {
    fn from_allowed(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        match raw.parse::<axum::http::uri::Authority>() {
            Ok(authority) => Some(Self::from_authority(&authority)),
            Err(_) => Some(Self {
                host: normalize_host(raw),
                port: None,
            }),
        }
    }

    fn from_request(raw: &str) -> Option<Self> {
        raw.parse::<axum::http::uri::Authority>()
            .ok()
            .map(|authority| Self::from_authority(&authority))
    }

    fn from_authority(authority: &axum::http::uri::Authority) -> Self {
        Self {
            host: normalize_host(authority.host()),
            port: authority.port_u16(),
        }
    }
}

fn normalize_host(host: &str) -> String {
    host.trim_matches('[')
        .trim_matches(']')
        .to_ascii_lowercase()
}

async fn enforce_host_allowlist(
    axum::extract::State(allowlist): axum::extract::State<HttpHostAllowlist>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|value| value.0.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let Some(raw_host) = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return host_rejected(
            axum::http::StatusCode::BAD_REQUEST,
            "HOST_REQUIRED",
            "missing or invalid Host header",
            request_id,
        );
    };
    let Some(authority) = AllowedAuthority::from_request(raw_host) else {
        return host_rejected(
            axum::http::StatusCode::BAD_REQUEST,
            "HOST_INVALID",
            "invalid Host header",
            request_id,
        );
    };
    if !allowlist.allows(&authority) {
        return host_rejected(
            axum::http::StatusCode::FORBIDDEN,
            "HOST_NOT_ALLOWED",
            "Host header is not allowed",
            request_id,
        );
    }
    next.run(request).await
}

fn host_rejected(
    status: axum::http::StatusCode,
    code: &'static str,
    message: &'static str,
    request_id: String,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        status,
        axum::Json(serde_json::json!({
            "ok": false,
            "code": code,
            "error": message,
            "request_id": request_id,
        })),
    )
        .into_response()
}

fn api_router(state: HttpState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(activity::routes())
        .merge(graph::routes())
        .merge(jobs::routes())
        .merge(ops::routes())
        .merge(retrieval::routes())
        .merge(wiki::routes())
        .merge(admin::routes())
        .with_state(state)
        .layer(axum::extract::DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
}

const MAX_HTTP_BODY_BYTES: usize = 1_048_576;

#[derive(Clone)]
struct RequestId(String);

async fn enforce_body_limit(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|value| value.0.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    if request
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|value| value > MAX_HTTP_BODY_BYTES as u64)
    {
        return body_too_large(request_id);
    }
    let (parts, body) = request.into_parts();
    match axum::body::to_bytes(body, MAX_HTTP_BODY_BYTES).await {
        Ok(body) => {
            next.run(axum::extract::Request::from_parts(
                parts,
                axum::body::Body::from(body),
            ))
            .await
        }
        Err(_) => body_too_large(request_id),
    }
}

fn body_too_large(request_id: String) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        axum::Json(serde_json::json!({
            "ok": false,
            "code": "BODY_TOO_LARGE",
            "error": "request body exceeds 1048576 bytes",
            "request_id": request_id,
        })),
    )
        .into_response()
}

async fn http_metadata(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 128)
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let peer_ip = request
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip().to_string());
    let user_agent = request
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 160);
    // Activity is remotely readable whenever the explicitly enabled gateway is
    // remotely bound. Keep a stable per-client discriminator without retaining
    // or exposing raw IP addresses and user-agent strings.
    let client = anonymous_client(peer_ip.as_deref(), user_agent);
    let origin = request
        .headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let started = std::time::Instant::now();
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("x-request-id", request_id.parse().unwrap());
    if origin.as_deref().is_some_and(loopback_origin) {
        response.headers_mut().insert(
            axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
            origin.unwrap().parse().unwrap(),
        );
        response
            .headers_mut()
            .insert(axum::http::header::VARY, "Origin".parse().unwrap());
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    if path != "/v1/activity" {
        // `x-request-id` is caller-controlled. Keep it for the response header
        // and process log correlation, but never copy it into the cross-client
        // activity feed where it could smuggle paths, queries, or document ids.
        activity::record(
            "http",
            client.clone(),
            format!("{method} {path}"),
            Some(response.status().as_u16()),
            Some(elapsed_ms),
            None,
        );
    }
    tracing::info!(%request_id, %method, %path, client = client.as_deref().unwrap_or("unknown"), status = response.status().as_u16(), %elapsed_ms, "HTTP request");
    response
}

fn anonymous_client(peer_ip: Option<&str>, user_agent: Option<&str>) -> Option<String> {
    let material = match (peer_ip, user_agent) {
        (Some(ip), Some(agent)) => format!("{ip}\n{agent}"),
        (Some(ip), None) => ip.to_string(),
        (None, Some(agent)) => agent.to_string(),
        (None, None) => return None,
    };
    let digest = blake3::hash(material.as_bytes()).to_hex().to_string();
    Some(format!("client-{}", &digest[..12]))
}

pub(crate) fn record_mcp_tool(action: &str, status: u16, elapsed_ms: f64) {
    activity::record(
        "mcp_tool",
        None,
        action,
        Some(status),
        Some(elapsed_ms),
        None,
    );
}

pub(crate) fn record_rag_action(action: &str) {
    activity::record("rag", None, action, Some(200), None, None);
}

fn loopback_origin(origin: &str) -> bool {
    [
        "http://127.0.0.1",
        "https://127.0.0.1",
        "http://localhost",
        "https://localhost",
        "http://[::1]",
        "https://[::1]",
    ]
    .iter()
    .any(|prefix| {
        origin == *prefix
            || origin
                .strip_prefix(prefix)
                .is_some_and(|tail| tail.starts_with(':'))
    })
}

#[allow(dead_code)]
pub type GraphJson = GraphView;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;

    #[test]
    fn mcp_host_allowlist_is_secure_for_wildcard_and_extensible() {
        let wildcard = Config {
            http_bind: Some("0.0.0.0:7432".into()),
            ..Config::default()
        };
        let defaults = configured_http_allowed_hosts(&wildcard, None);
        assert!(defaults.iter().any(|host| host == "localhost"));
        assert!(defaults.iter().any(|host| host == "127.0.0.1"));
        assert!(!defaults.iter().any(|host| host == "0.0.0.0"));

        let explicit = configured_http_allowed_hosts(
            &wildcard,
            Some("192.168.50.205, rag.local:7432, bad host, /invalid"),
        );
        assert!(explicit.iter().any(|host| host == "192.168.50.205"));
        assert!(explicit.iter().any(|host| host == "rag.local:7432"));
        assert!(!explicit.iter().any(|host| host == "bad host"));
        assert!(!explicit.iter().any(|host| host == "/invalid"));

        let concrete = Config {
            http_bind: Some("192.168.50.205:7432".into()),
            ..Config::default()
        };
        assert!(configured_http_allowed_hosts(&concrete, None)
            .iter()
            .any(|host| host == "192.168.50.205"));
    }

    fn app() -> Router {
        let root = tempfile::tempdir().unwrap().keep();
        let config = Config {
            db_path: root.join("http.duckdb"),
            ..Config::default()
        };
        let store = Arc::new(Store::open(&config.db_path).unwrap());
        let embedder = build_provider(&config).unwrap();
        let allowed_hosts = configured_http_allowed_hosts(&config, None);
        gateway_layers(
            api_router(HttpState {
                store,
                mcp_http: false,
                config,
                embedder,
                jobs: jobs::JobRegistry::default(),
            }),
            allowed_hosts,
        )
    }

    #[tokio::test]
    async fn outer_gateway_rejects_rebound_v1_host_and_keeps_loopback_health() {
        let denied = app()
            .oneshot(
                Request::builder()
                    .uri("/v1/status")
                    .header("host", "attacker.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), axum::http::StatusCode::FORBIDDEN);
        let body = to_bytes(denied.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "HOST_NOT_ALLOWED"
        );

        let health = app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("host", "127.0.0.1:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn metadata_version_cors_and_body_limit_are_json() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/v1/version")
                    .header("host", "localhost:7432")
                    .header("origin", "http://localhost:3000")
                    .header("x-request-id", "test-request-7")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(response.headers()["x-request-id"], "test-request-7");
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            "http://localhost:3000"
        );
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/search")
                    .header("host", "localhost:7432")
                    .header("content-length", MAX_HTTP_BODY_BYTES + 1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "BODY_TOO_LARGE"
        );

        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/search")
                    .header("host", "localhost:7432")
                    .header("content-type", "application/json")
                    .body(Body::from(vec![b'x'; MAX_HTTP_BODY_BYTES + 1]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn caller_request_id_is_returned_but_never_persisted_in_activity() {
        let app = app();
        let baseline = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/activity")
                    .header("host", "localhost:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let baseline = to_bytes(baseline.into_body(), 256 * 1024).await.unwrap();
        let baseline: serde_json::Value = serde_json::from_slice(&baseline).unwrap();
        let after = baseline["latest_seq"].as_u64().unwrap_or(0);
        let malicious = "/private/path?query=secret-document&project=alpha";

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/status")
                    .header("host", "localhost:7432")
                    .header("x-request-id", malicious)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()["x-request-id"], malicious);

        let activity = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/activity?after={after}"))
                    .header("host", "localhost:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let activity = to_bytes(activity.into_body(), 256 * 1024).await.unwrap();
        let activity_json: serde_json::Value = serde_json::from_slice(&activity).unwrap();
        assert!(!String::from_utf8_lossy(&activity).contains(malicious));
        assert!(activity_json["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["action"] == "GET /v1/status" && event["request_id"].is_null()));
    }

    #[tokio::test]
    async fn mounted_mcp_rejects_streamed_oversized_bodies() {
        let root = tempfile::tempdir().unwrap().keep();
        let config = Config {
            db_path: root.join("mcp-http.duckdb"),
            ..Config::default()
        };
        let store = Store::open(&config.db_path).unwrap();
        let embedder = build_provider(&config).unwrap();
        let mcp = mcp_http_service(
            store.clone(),
            embedder.clone(),
            config.clone(),
            CancellationToken::new(),
        );
        let allowed_hosts = configured_http_allowed_hosts(&config, None);
        let api = api_router(HttpState {
            store: Arc::new(store),
            mcp_http: true,
            config,
            embedder,
            jobs: jobs::JobRegistry::default(),
        });
        let response = gateway_layers(
            Router::new().merge(api).nest_service("/mcp", mcp),
            allowed_hosts,
        )
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("host", "localhost:7432")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(Body::from(vec![b'x'; MAX_HTTP_BODY_BYTES + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn mounted_mcp_enforces_dns_rebinding_host_allowlist() {
        let root = tempfile::tempdir().unwrap().keep();
        let config = Config {
            db_path: root.join("mcp-host-guard.duckdb"),
            http_bind: Some("192.168.50.205:7432".into()),
            ..Config::default()
        };
        let store = Store::open(&config.db_path).unwrap();
        let embedder = build_provider(&config).unwrap();
        let service = |store: Store| {
            Router::new().nest_service(
                "/mcp",
                mcp_http_service(
                    store,
                    embedder.clone(),
                    config.clone(),
                    CancellationToken::new(),
                ),
            )
        };

        let denied = service(store.clone())
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/mcp")
                    .header("host", "attacker.example")
                    .header("accept", "text/event-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), axum::http::StatusCode::FORBIDDEN);

        let allowed = service(store)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/mcp")
                    .header("host", "192.168.50.205:7432")
                    .header("accept", "text/event-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(allowed.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn revision_timeline_is_lean_paginated_and_snapshot_is_lazy() {
        let root = tempfile::tempdir().unwrap().keep();
        let config = Config {
            db_path: root.join("revisions-http.duckdb"),
            ..Config::default()
        };
        let store = Arc::new(Store::open(&config.db_path).unwrap());
        let now = chrono::Utc::now();
        let mut document = crate::models::Document {
            id: "revision-doc".into(),
            uri: "wiki://revision-doc".into(),
            title: "Revision doc".into(),
            content: "first body".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            layer: "wiki".into(),
            kind: "wiki".into(),
            ..Default::default()
        };
        store.upsert_document_cas(&document, None).unwrap();
        document.content = "second body".into();
        store.upsert_document_cas(&document, Some(1)).unwrap();
        document.content = "third body".into();
        store.upsert_document_cas(&document, Some(2)).unwrap();
        let embedder = build_provider(&config).unwrap();
        let allowed_hosts = configured_http_allowed_hosts(&config, None);
        let app = gateway_layers(
            api_router(HttpState {
                store,
                mcp_http: false,
                config,
                embedder,
                jobs: jobs::JobRegistry::default(),
            }),
            allowed_hosts,
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/revisions?document_id=revision-doc&limit=1")
                    .header("host", "localhost:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["page"]["total"], 2);
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["items"][0]["revision"], 2);
        assert!(body["items"][0].get("content").is_none());
        assert!(body["items"][0].get("metadata_json").is_none());
        assert!(body["page"]["next_cursor"].as_str().is_some());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/revisions/snapshot?document_id=revision-doc&revision=2")
                    .header("host", "localhost:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["result"]["content"], "second body");
    }

    #[tokio::test]
    async fn operational_routes_pagination_errors_and_conditional_get_work() {
        let app = app();
        for uri in [
            "/v1/status",
            "/v1/doctor",
            "/v1/activity",
            "/v1/projects",
            "/v1/project-home?project=alpha",
            "/v1/jobs",
            "/v1/capabilities",
            "/v1/routes",
            "/v1/documents?limit=1&q=readme&layer=raw",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .header("host", "localhost:7432")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK, "{uri}");
        }
        let routes = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/routes")
                    .header("host", "localhost:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let routes = to_bytes(routes.into_body(), 64 * 1024).await.unwrap();
        let routes: serde_json::Value = serde_json::from_slice(&routes).unwrap();
        for (method, path) in [
            ("GET", "/v1/activity"),
            ("POST", "/v1/jobs/sync"),
            ("DELETE", "/v1/jobs/{id}"),
            ("GET", "/v1/revisions/snapshot"),
            ("GET", "/v1/revisions/diff"),
            ("POST", "/v1/revisions/restore"),
            ("POST", "/v1/operations/checkpoint"),
            ("POST", "/v1/operations/backup"),
        ] {
            assert!(
                routes["routes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|route| { route["method"] == method && route["path"] == path }),
                "missing {method} {path}"
            );
        }
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/wiki?limit=1")
                    .header("host", "localhost:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let etag = first.headers()[axum::http::header::ETAG].clone();
        let cached = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/wiki?limit=1")
                    .header("host", "localhost:7432")
                    .header(axum::http::header::IF_NONE_MATCH, etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cached.status(), axum::http::StatusCode::NOT_MODIFIED);
        let invalid = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/search")
                    .header("host", "localhost:7432")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"query":"","mode":"lex"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = to_bytes(invalid.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            "INVALID_REQUEST"
        );

        let busy = error::api_err(crate::error::AppError::busy("test"));
        assert_eq!(busy.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(busy.headers()["retry-after"], "1");
    }

    #[test]
    fn cors_policy_and_cursor_are_strict() {
        assert!(loopback_origin("http://127.0.0.1:7432"));
        assert!(loopback_origin("http://localhost:3000"));
        assert!(!loopback_origin("https://example.com"));
        assert_eq!(
            retrieval::decode_cursor(Some(&retrieval::encode_cursor(42))).unwrap(),
            42
        );
        assert!(retrieval::decode_cursor(Some("42")).is_err());
        let client = anonymous_client(Some("192.0.2.10"), Some("Secret Browser/1"))
            .expect("anonymous client id");
        assert_eq!(
            client,
            anonymous_client(Some("192.0.2.10"), Some("Secret Browser/1")).unwrap()
        );
        assert!(!client.contains("192.0.2.10"));
        assert!(!client.contains("Secret Browser"));
    }

    async fn get_json(app: &Router, uri: &str) -> (axum::http::StatusCode, serde_json::Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("host", "localhost:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (
            status,
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
        )
    }

    async fn send_json(
        app: &Router,
        method: &str,
        uri: &str,
        body: serde_json::Value,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("host", "localhost:7432")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (
            status,
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
        )
    }

    #[tokio::test]
    async fn console_read_routes_respond_on_empty_store() {
        let app = app();
        for uri in [
            "/v1/runtime",
            "/v1/calls",
            "/v1/agents",
            "/v1/ops-log",
            "/v1/taxonomy",
            "/v1/wings",
            "/v1/rooms",
            "/v1/embedding-manifest",
            "/v1/diary",
            "/v1/kg",
            "/v1/kg/stats",
            "/v1/tunnels",
            "/v1/lint-wiki",
            "/v1/eval/history",
            "/v1/kg/timeline?subject=rag-mcp",
        ] {
            let (status, body) = get_json(&app, uri).await;
            assert_eq!(status, axum::http::StatusCode::OK, "{uri}: {body}");
            assert_eq!(body["ok"], true, "{uri}: {body}");
        }
        let (status, body) = get_json(&app, "/v1/status").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(body["pid"], std::process::id());
        assert!(body["wal_warn_bytes"].as_u64().unwrap() > 0);
        let (status, _) = get_json(&app, "/v1/ops-log?seq=999999").await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        let (status, _) = send_json(
            &app,
            "POST",
            "/v1/search",
            serde_json::json!({"query": "x", "mode": "lex", "rrf_k": 0}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn legacy_console_mutations_are_not_mounted() {
        let app = app();
        for (method, uri) in [
            ("POST", "/v1/ingest/text"),
            ("POST", "/v1/ingest/file"),
            ("POST", "/v1/sync-sources"),
            ("PATCH", "/v1/document"),
            ("DELETE", "/v1/document?id=missing"),
            ("POST", "/v1/reembed"),
            ("POST", "/v1/backup"),
            ("POST", "/v1/vacuum"),
            ("POST", "/v1/doctor/repair"),
        ] {
            let (status, _) = send_json(&app, method, uri, serde_json::json!({})).await;
            assert!(
                matches!(
                    status,
                    axum::http::StatusCode::NOT_FOUND | axum::http::StatusCode::METHOD_NOT_ALLOWED
                ),
                "unsafe legacy mutation route was mounted: {method} {uri} -> {status}"
            );
        }

        let (status, _) = send_json(
            &app,
            "POST",
            "/v1/pack-context",
            serde_json::json!({"hits": [], "max_tokens": 400}),
        )
        .await;
        assert_ne!(status, axum::http::StatusCode::NOT_FOUND);
        assert_ne!(status, axum::http::StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn cancellation_token_wakes_http_shutdown_future() {
        let token = CancellationToken::new();
        let waiter = token.clone();
        let task = tokio::spawn(async move {
            waiter.cancelled().await;
        });
        token.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }
}
