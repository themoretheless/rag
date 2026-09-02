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
//! | `/v1/ops-log`, `/v1/taxonomy`, `/v1/diary`, `/v1/kg*`, `/v1/tunnels`, `/v1/llm-status` | Console reads |
//! | `POST /v1/ingest/*`, `PATCH /v1/document`, `POST /v1/backup`, … | Console mutations (see [`admin`]) |
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

mod admin;
mod bind;
mod error;
mod graph;
mod health;
mod ops;
mod retrieval;
mod wiki;

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
    StreamableHttpService::new(
        move || {
            Ok(RagServer::new(store.clone(), embedder.clone(), config.clone())
                .with_transport("http-mcp"))
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig {
            stateful_mode: true,
            sse_keep_alive: Some(std::time::Duration::from_secs(15)),
            cancellation_token,
        },
    )
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
    let api = api_router(HttpState { store, mcp_http, config, embedder });

    let app = if let Some(mcp_svc) = mcp {
        Router::new().merge(api).nest_service("/mcp", mcp_svc)
    } else { api };

    tracing::info!(%bind, mcp_http, mcp_path = if mcp_http { "/mcp" } else { "(disabled)" },
        "rag-mcp HTTP gateway listening (graph UI + optional streamable MCP)");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn api_router(state: HttpState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(graph::routes())
        .merge(ops::routes())
        .merge(retrieval::routes())
        .merge(wiki::routes())
        .merge(admin::routes())
        .with_state(state)
        .layer(axum::middleware::from_fn(http_metadata))
}

const MAX_HTTP_BODY_BYTES: u64 = 1_048_576;

async fn http_metadata(request: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    use axum::response::IntoResponse;
    let request_id = request.headers().get("x-request-id").and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 128).map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    if request.headers().get(axum::http::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok()).is_some_and(|v| v > MAX_HTTP_BODY_BYTES) {
        return (axum::http::StatusCode::PAYLOAD_TOO_LARGE, axum::Json(serde_json::json!({
            "ok": false, "code": "BODY_TOO_LARGE", "error": "request body exceeds 1048576 bytes", "request_id": request_id
        }))).into_response();
    }
    let method = request.method().clone(); let path = request.uri().path().to_owned();
    let origin = request.headers().get(axum::http::header::ORIGIN).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let started = std::time::Instant::now();
    let mut response = next.run(request).await;
    response.headers_mut().insert("x-request-id", request_id.parse().unwrap());
    if origin.as_deref().is_some_and(loopback_origin) {
        response.headers_mut().insert(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.unwrap().parse().unwrap());
        response.headers_mut().insert(axum::http::header::VARY, "Origin".parse().unwrap());
    }
    tracing::info!(%request_id, %method, %path, status = response.status().as_u16(), elapsed_ms = started.elapsed().as_secs_f64() * 1000.0, "HTTP request");
    response
}

fn loopback_origin(origin: &str) -> bool {
    ["http://127.0.0.1", "https://127.0.0.1", "http://localhost", "https://localhost", "http://[::1]", "https://[::1]"]
        .iter().any(|prefix| origin == *prefix || origin.strip_prefix(prefix).is_some_and(|tail| tail.starts_with(':')))
}

#[allow(dead_code)]
pub type GraphJson = GraphView;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::{to_bytes, Body}, http::Request};
    use tower::ServiceExt;

    fn app() -> Router {
        let root = tempfile::tempdir().unwrap().keep();
        let config = Config { db_path: root.join("http.duckdb"), ..Config::default() };
        let store = Arc::new(Store::open(&config.db_path).unwrap());
        let embedder = build_provider(&config).unwrap();
        api_router(HttpState { store, mcp_http: false, config, embedder })
    }

    #[tokio::test]
    async fn metadata_version_cors_and_body_limit_are_json() {
        let response = app().oneshot(Request::builder().uri("/v1/version").header("origin", "http://localhost:3000").header("x-request-id", "test-request-7").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(response.headers()["x-request-id"], "test-request-7");
        assert_eq!(response.headers()["access-control-allow-origin"], "http://localhost:3000");
        let response = app().oneshot(Request::builder().method("POST").uri("/v1/search").header("content-length", MAX_HTTP_BODY_BYTES + 1).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"], "BODY_TOO_LARGE");
    }

    #[tokio::test]
    async fn operational_routes_pagination_errors_and_conditional_get_work() {
        let app = app();
        for uri in ["/v1/status", "/v1/doctor", "/v1/projects", "/v1/capabilities", "/v1/routes", "/v1/documents?limit=1"] {
            let response = app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK, "{uri}");
        }
        let first = app.clone().oneshot(Request::builder().uri("/v1/wiki?limit=1").body(Body::empty()).unwrap()).await.unwrap();
        let etag = first.headers()[axum::http::header::ETAG].clone();
        let cached = app.clone().oneshot(Request::builder().uri("/v1/wiki?limit=1").header(axum::http::header::IF_NONE_MATCH, etag).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(cached.status(), axum::http::StatusCode::NOT_MODIFIED);
        let invalid = app.oneshot(Request::builder().method("POST").uri("/v1/search").header("content-type", "application/json")
            .body(Body::from(r#"{"query":"","mode":"lex"}"#)).unwrap()).await.unwrap();
        assert_eq!(invalid.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = to_bytes(invalid.into_body(), 4096).await.unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"], "INVALID_REQUEST");

        let busy = error::api_err(crate::error::AppError::busy("test"));
        assert_eq!(busy.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(busy.headers()["retry-after"], "1");
    }

    #[test]
    fn cors_policy_and_cursor_are_strict() {
        assert!(loopback_origin("http://127.0.0.1:7432")); assert!(loopback_origin("http://localhost:3000"));
        assert!(!loopback_origin("https://example.com"));
        assert_eq!(retrieval::decode_cursor(Some(&retrieval::encode_cursor(42))).unwrap(), 42);
        assert!(retrieval::decode_cursor(Some("42")).is_err());
    }

    async fn get_json(app: &Router, uri: &str) -> (axum::http::StatusCode, serde_json::Value) {
        let response = app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null))
    }

    async fn send_json(app: &Router, method: &str, uri: &str, body: serde_json::Value) -> (axum::http::StatusCode, serde_json::Value) {
        let response = app.clone().oneshot(Request::builder().method(method).uri(uri).header("content-type", "application/json")
            .body(Body::from(body.to_string())).unwrap()).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null))
    }

    #[tokio::test]
    async fn console_read_routes_respond_on_empty_store() {
        let app = app();
        for uri in ["/v1/runtime", "/v1/calls", "/v1/agents", "/v1/ops-log", "/v1/taxonomy", "/v1/wings", "/v1/rooms",
            "/v1/embedding-manifest", "/v1/diary", "/v1/kg", "/v1/kg/stats", "/v1/tunnels", "/v1/lint-wiki", "/v1/eval/history",
            "/v1/kg/timeline?subject=rag-mcp"] {
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
        let (status, _) = send_json(&app, "POST", "/v1/search", serde_json::json!({"query": "x", "mode": "lex", "rrf_k": 0})).await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn console_mutations_search_lab_and_call_log_roundtrip() {
        let app = app();
        let (status, body) = send_json(&app, "POST", "/v1/ingest/text", serde_json::json!({
            "text": "# Manifest\n\nThe embedding manifest makes vec and hybrid search fail closed on a dims mismatch.",
            "title": "manifest-note", "uri": "raw://manifest-note", "wing": "rag", "room": "docs"
        })).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{body}");
        let document_id = body["result"]["document_id"].as_str().unwrap().to_string();
        assert!(body["result"]["chunk_count"].as_u64().unwrap() >= 1);

        let (status, body) = send_json(&app, "POST", "/v1/search", serde_json::json!({
            "query": "manifest mismatch", "mode": "hybrid", "top_k": 5, "rrf_k": 30, "wing": "rag"
        })).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{body}");
        assert_eq!(body["mode"], "hybrid");
        assert!(body["count"].as_u64().unwrap() >= 1);
        let hit = &body["items"][0];
        assert!(hit["rank_lex"].is_number() || hit["rank_vec"].is_number(), "{hit}");
        assert_eq!(hit["explanation"]["rrf_k"], 30.0);
        assert!(body["timings"]["embed_ms"].is_number(), "{body}");
        assert!(body["timings"]["vec_ms"].is_number(), "{body}");
        assert!(body["timings"]["lex_ms"].is_number(), "{body}");

        let (status, packed) = send_json(&app, "POST", "/v1/pack-context", serde_json::json!({
            "hits": body["items"], "max_tokens": 400, "context_expansion": "neighbors"
        })).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{packed}");
        assert!(packed["total_tokens"].as_u64().unwrap() > 0);
        assert!(packed["context_text"].as_str().unwrap().contains("manifest"));

        let (status, body) = send_json(&app, "PATCH", "/v1/document", serde_json::json!({
            "document_id": document_id, "pinned": true, "room": "wiki", "boost": 1.5
        })).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{body}");
        assert_eq!(body["result"]["pinned"], true);
        assert_eq!(body["result"]["room"], "wiki");
        let (_, docs) = get_json(&app, "/v1/documents?room=wiki").await;
        assert_eq!(docs["page"]["total"], 1);
        let (_, tax) = get_json(&app, "/v1/taxonomy").await;
        assert_eq!(tax["taxonomy"]["wings"][0]["wing"], "rag");

        let (status, body) = send_json(&app, "POST", "/v1/doctor/repair", serde_json::json!({})).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{body}");
        assert_eq!(body["result"]["dry_run"], true);
        let (status, body) = send_json(&app, "POST", "/v1/vacuum", serde_json::json!({})).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{body}");
        assert_eq!(body["result"]["checkpointed"], true);
        let (status, body) = send_json(&app, "POST", "/v1/reembed", serde_json::json!({"document_id": document_id})).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{body}");

        let (_, calls) = get_json(&app, "/v1/calls?agent=http&limit=100").await;
        let tools: Vec<&str> = calls["items"].as_array().unwrap().iter().map(|c| c["tool"].as_str().unwrap()).collect();
        for expected in ["ingest_text", "search", "update_document_meta", "doctor_repair", "vacuum_store", "reembed_document"] {
            assert!(tools.contains(&expected), "missing {expected} in {tools:?}");
        }
        assert!(calls["summary"]["buffered"].as_u64().unwrap() >= 6);
        let (_, agents) = get_json(&app, "/v1/agents").await;
        let http = agents["items"].as_array().unwrap().iter().find(|a| a["agent"] == "http").expect("http agent row");
        assert_eq!(http["online"], true);
        assert!(http["calls_total"].as_u64().unwrap() >= 6, "{http}");

        let (status, _) = send_json(&app, "DELETE", &format!("/v1/document?id={document_id}"), serde_json::json!({})).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        let (status, _) = send_json(&app, "DELETE", &format!("/v1/document?id={document_id}"), serde_json::json!({})).await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        let (status, _) = send_json(&app, "POST", "/v1/ingest/file", serde_json::json!({"path": "/nowhere/x.md"})).await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn cancellation_token_wakes_http_shutdown_future() {
        let token = CancellationToken::new(); let waiter = token.clone();
        let task = tokio::spawn(async move { waiter.cancelled().await; }); token.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), task).await.unwrap().unwrap();
    }
}
