//! Local HTTP gateway for rag-mcp (same process as DuckDB).
//!
//! Bind with `RAG_HTTP_BIND=127.0.0.1:7432`.
//!
//! | Path | Role |
//! |------|------|
//! | `/health`, `/v1/graph`, `/v1/neighbors`, `/v1/find`, `/v1/document` | Graph + document UI |
//! | `GET /v1/wiki`, `POST /v1/wiki`, `PUT /v1/wiki`, `GET /v1/backlinks` | Wiki catalog, create-only, write (CAS), backlinks |
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
//! `POST /v1/wiki` only creates; an occupied canonical URI returns **409**.
//!
//! # Submodules
//!
//! | Module | Role |
//! |--------|------|
//! | `bind` | `parse_bind` + loopback gate |
//! | `error` | `api_ok` / `api_err` / `status_for` |
//! | `health` | `/health` |
//! | `graph` | Graph + document handlers |
//! | `wiki` | Wiki list/put/backlinks |

mod activity;
mod admin;
mod auth;
mod bind;
mod error;
mod eval_ops;
mod feedback;
mod graph;
mod health;
mod ingest;
mod jobs;
mod knowledge;
mod labels;
mod ops;
mod resources;
mod retrieval;
mod review;
mod sync;
mod traces;
mod wiki;
use crate::db::payload_kv;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::models::GraphView;
use crate::util::run_blocking;

pub use bind::parse_bind;

/// Validate security before opening the store or starting background writers.
pub fn validate_security(
    bind: SocketAddr,
    config: &crate::config::Config,
) -> Result<(), crate::AppError> {
    auth::HttpAuth::from_config(config)?.validate_bind(bind)
}

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
    /// Chat client built once from `RAG_LLM_*` (None when it cannot be built).
    pub(super) llm: Option<crate::llm::ChatClient>,
    /// Non-secret listener auth posture, reported by `/v1/capabilities`. Only
    /// [`serve`] can know it; state built without it stays in the default
    /// loopback-trusted description rather than claiming tokens it never read.
    auth: auth::AuthPosture,
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
        let llm = match crate::llm::ChatClient::from_config(&config) {
            Ok(client) => Some(client),
            Err(error) => {
                if config.llm_enabled {
                    tracing::warn!(%error, "LLM enabled but chat client config is invalid; LLM features disabled");
                }
                None
            }
        };
        Self::from_context(
            crate::context::AppContext {
                store,
                embedder,
                config,
                llm,
            },
            mcp_http,
        )
    }

    /// HTTP state over the process-wide [`crate::context::AppContext`].
    pub fn from_context(context: crate::context::AppContext, mcp_http: bool) -> Self {
        let store = context.store;
        let jobs = jobs::JobRegistry::restore_from_store(store.clone()).unwrap_or_else(|error| {
            tracing::warn!(%error, "failed to restore background jobs; starting empty registry");
            jobs::JobRegistry::with_store(store.clone())
        });
        Self {
            store,
            mcp_http,
            config: context.config,
            embedder: context.embedder,
            jobs,
            llm: context.llm,
            auth: auth::AuthPosture::default(),
        }
    }

    /// Retrieval use cases over this state's store, embedder and config.
    pub(crate) fn search_use_case(&self) -> crate::app::SearchUseCase {
        crate::app::SearchUseCase::new(
            self.store.clone(),
            self.embedder.clone(),
            Arc::new(self.config.clone()),
        )
    }

    /// Wiki write use case over this state's store, embedder and config.
    pub(crate) fn wiki_use_case(&self) -> crate::app::WikiUseCase {
        crate::app::WikiUseCase::new(
            self.store.clone(),
            self.embedder.clone(),
            Arc::new(self.config.clone()),
        )
    }

    /// Graph read use cases over this state's store.
    pub(crate) fn graph_use_case(&self) -> crate::app::GraphUseCase {
        crate::app::GraphUseCase::new(self.store.clone())
    }

    /// Attach the posture parsed from the listener's configured tokens.
    pub(crate) fn with_auth_posture(mut self, auth: auth::AuthPosture) -> Self {
        self.auth = auth;
        self
    }
}

/// Lenient boolean query flag: `0`, `false`, `no` and `off` (any case)
/// disable it; any other value, including a bare `?flag`, enables it.
fn query_flag<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let value = <String as serde::Deserialize>::deserialize(deserializer)?;
    Ok(!matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    ))
}

pub use crate::config::configured_http_allowed_hosts;

/// Start HTTP server (graph API + MCP `/mcp`); runs until process exit.
///
/// `mcp` is a router that already mounts `/mcp` (built by the composition
/// root from `mcp::mcp_http_router`), so this module never depends on `mcp`.
///
/// Uses the process [`crate::context::AppContext`] (config, embedder, chat client).
/// Route tables come from the private `health`, `graph`, and `wiki` route
/// builders and are combined here without duplicating handlers.
pub async fn serve(
    bind: SocketAddr,
    context: crate::context::AppContext,
    mcp: Option<Router>,
) -> Result<(), std::io::Error> {
    serve_with_shutdown(bind, context, mcp, CancellationToken::new()).await
}

/// Stop accepting connections on cancellation and let accepted requests drain.
/// The executable bounds that drain before its final database checkpoint.
pub async fn serve_with_shutdown(
    bind: SocketAddr,
    context: crate::context::AppContext,
    mcp: Option<Router>,
    shutdown: CancellationToken,
) -> Result<(), std::io::Error> {
    let auth = auth::HttpAuth::from_config(&context.config)
        .and_then(|auth| {
            auth.validate_bind(bind)?;
            Ok(auth)
        })
        .map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string())
        })?;
    let mcp_http = mcp.is_some();
    let allowed_hosts = configured_http_allowed_hosts(
        &context.config,
        context.config.http_allowed_hosts.as_deref(),
    );
    let posture = auth.posture();
    if !posture.tokens_required {
        tracing::warn!(
            %bind,
            "no RAG_HTTP_*_TOKEN configured: every local process that can reach this \
             listener is admin; set a role token before exposing the gateway beyond loopback"
        );
    }
    let state = HttpState::from_context(context, mcp_http).with_auth_posture(posture);
    sync::spawn_worker(state.clone());
    let api = api_router(state);

    let app = if let Some(mcp_router) = mcp {
        Router::new().merge(api).merge(mcp_router)
    } else {
        api
    };
    let app = authenticated_gateway_layers(app, allowed_hosts, auth);

    tracing::info!(%bind, mcp_http, mcp_path = if mcp_http { "/mcp" } else { "(disabled)" },
        "rag-mcp HTTP gateway listening (graph UI + optional streamable MCP)");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown.cancelled_owned())
    .await?;
    Ok(())
}

fn authenticated_gateway_layers(
    app: Router,
    allowed_hosts: Vec<String>,
    auth: auth::HttpAuth,
) -> Router {
    app.layer(axum::middleware::from_fn(enforce_body_limit))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            auth::enforce_auth,
        ))
        .layer(axum::middleware::from_fn_with_state(
            HttpHostAllowlist::new(allowed_hosts),
            enforce_host_allowlist,
        ))
        .layer(axum::middleware::from_fn(http_metadata))
}

#[cfg(test)]
fn gateway_layers(app: Router, allowed_hosts: Vec<String>) -> Router {
    authenticated_gateway_layers(app, allowed_hosts, auth::HttpAuth::local_for_tests())
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

/// The API routes without the listener's auth, host and request-id layers, for
/// integration tests that drive handlers with `tower::ServiceExt::oneshot`.
#[cfg(any(test, feature = "testkit"))]
pub fn router_for_tests(state: HttpState) -> Router {
    api_router(state)
}

fn api_router(state: HttpState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(ingest::routes())
        .merge(activity::routes())
        .merge(graph::routes())
        .merge(jobs::routes())
        .merge(ops::routes())
        .merge(retrieval::routes())
        .merge(resources::routes())
        .merge(sync::routes())
        .merge(wiki::routes())
        .merge(review::routes())
        .merge(feedback::routes())
        .merge(labels::routes())
        .merge(traces::routes())
        .merge(eval_ops::routes())
        .merge(knowledge::routes())
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
    let activity_path = activity_path(&path);
    let peer_ip = request
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip().to_string());
    let user_agent = request
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 160);
    let client_host = request
        .headers()
        .get("x-rag-client-host")
        .and_then(|value| value.to_str().ok());
    // Activity is remotely readable whenever the explicitly enabled gateway is
    // remotely bound. Prefer an explicit, tightly validated machine label and
    // otherwise keep a stable discriminator without retaining or exposing raw
    // IP addresses and user-agent strings.
    let client = client_label(client_host, peer_ip.as_deref(), user_agent);
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
        crate::activity::record(
            "http",
            client.clone(),
            format!("{method} {activity_path}"),
            Some(response.status().as_u16()),
            Some(elapsed_ms),
            None,
        );
    }
    tracing::info!(%request_id, %method, %path, client = client.as_deref().unwrap_or("unknown"), status = response.status().as_u16(), %elapsed_ms, "HTTP request");
    response
}

/// Stable route label retained by the cross-client activity feed.
///
/// Job ids are generated resource identifiers carried in the path rather than
/// the query string, so retain the route template instead. Static job routes
/// and unrelated paths keep their exact action names.
fn activity_path(path: &str) -> &str {
    const JOB_RESOURCE_PREFIX: &str = "/v1/jobs/";
    const JOB_RESOURCE_ROUTE: &str = "/v1/jobs/{id}";

    match path.strip_prefix(JOB_RESOURCE_PREFIX) {
        Some(resource_id)
            if !resource_id.is_empty() && !resource_id.contains('/') && resource_id != "sync" =>
        {
            JOB_RESOURCE_ROUTE
        }
        _ => path,
    }
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

fn client_label(
    client_host: Option<&str>,
    peer_ip: Option<&str>,
    user_agent: Option<&str>,
) -> Option<String> {
    client_host
        .and_then(sanitize_client_host)
        .map(|host| format!("host:{host}"))
        .or_else(|| anonymous_client(peer_ip, user_agent))
}

fn sanitize_client_host(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return None;
    }
    Some(value.to_ascii_lowercase())
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
mod tests;
