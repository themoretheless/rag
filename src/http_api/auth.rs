//! One authentication boundary for REST and mounted MCP. Credentials never enter logs.

use std::net::SocketAddr;

use axum::{
    extract::{Request, State},
    http::{header, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use ring::hmac;

use crate::{access::AccessRole, AppError};

#[derive(Clone)]
pub(crate) struct HttpAuth {
    key: hmac::Key,
    credentials: Vec<(AccessRole, hmac::Tag)>,
}

impl HttpAuth {
    #[cfg(test)]
    pub(super) fn local_for_tests() -> Self {
        Self::from_tokens(None, None, None).unwrap()
    }
    pub(crate) fn from_env() -> Result<Self, AppError> {
        let read = token_env("RAG_HTTP_READ_TOKEN")?;
        let write = token_env("RAG_HTTP_WRITE_TOKEN")?;
        let admin = token_env("RAG_HTTP_ADMIN_TOKEN")?;
        Self::from_tokens(read.as_deref(), write.as_deref(), admin.as_deref())
    }

    fn from_tokens(
        read: Option<&str>,
        write: Option<&str>,
        admin: Option<&str>,
    ) -> Result<Self, AppError> {
        let key = hmac::Key::new(hmac::HMAC_SHA256, b"rag-mcp HTTP credential fingerprint v1");
        let mut credentials: Vec<(AccessRole, hmac::Tag)> = Vec::new();
        for (role, name, token) in [
            (AccessRole::Read, "RAG_HTTP_READ_TOKEN", read),
            (AccessRole::Write, "RAG_HTTP_WRITE_TOKEN", write),
            (AccessRole::Admin, "RAG_HTTP_ADMIN_TOKEN", admin),
        ] {
            if let Some(token) = token {
                let unpadded = token.trim_end_matches('=');
                if !(32..=4096).contains(&token.len())
                    || unpadded.is_empty()
                    || !unpadded
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-._~+/".contains(&c))
                {
                    return Err(AppError::config(format!(
                        "{name} must contain 32..4096 valid Bearer token characters"
                    )));
                }
                let tag = hmac::sign(&key, token.as_bytes());
                if credentials
                    .iter()
                    .any(|(_, existing)| existing.as_ref() == tag.as_ref())
                {
                    return Err(AppError::config(
                        "HTTP tokens for different roles must be distinct",
                    ));
                }
                credentials.push((role, tag));
            }
        }
        Ok(Self { key, credentials })
    }

    pub(crate) fn validate_bind(&self, bind: SocketAddr) -> Result<(), AppError> {
        if !bind.ip().is_loopback() && self.credentials.is_empty() {
            return Err(AppError::config("non-loopback HTTP/MCP requires RAG_HTTP_READ_TOKEN, RAG_HTTP_WRITE_TOKEN or RAG_HTTP_ADMIN_TOKEN; configure credentials before remote startup"));
        }
        Ok(())
    }

    fn authenticate(&self, headers: &axum::http::HeaderMap) -> Option<AccessRole> {
        if self.credentials.is_empty() {
            // Only a loopback listener may use this mode (validated before bind).
            return Some(AccessRole::Admin);
        }
        let mut values = headers.get_all(header::AUTHORIZATION).iter();
        let value = values.next()?.to_str().ok()?;
        if values.next().is_some() {
            return None;
        }
        let (scheme, token) = value.split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("bearer") || token.len() > 4096 {
            return None;
        }
        self.credentials.iter().find_map(|(role, expected)| {
            hmac::verify(&self.key, token.as_bytes(), expected.as_ref())
                .is_ok()
                .then_some(*role)
        })
    }
}

fn token_env(name: &str) -> Result<Option<String>, AppError> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(AppError::config(format!("{name} must be valid UTF-8"))),
    }
}

/// Unknown/new routes require admin until explicitly classified.
fn required_http_role(method: &Method, path: &str) -> AccessRole {
    if path == "/mcp" || path.starts_with("/mcp/") {
        // The MCP dispatcher checks the actual tool using this authenticated role.
        return AccessRole::Read;
    }
    if matches!(*method, Method::GET | Method::HEAD) {
        if path == "/v1/sync/pull" {
            return AccessRole::Admin;
        }
        if matches!(
            path,
            "/health"
                | "/live"
                | "/ready"
                | "/v1/status"
                | "/v1/doctor"
                | "/v1/activity"
                | "/v1/runtime"
                | "/v1/calls"
                | "/v1/agents"
                | "/v1/version"
                | "/v1/capabilities"
                | "/v1/routes"
                | "/v1/projects"
                | "/v1/project-home"
                | "/v1/graph"
                | "/v1/neighbors"
                | "/v1/find"
                | "/v1/document"
                | "/v1/documents"
                | "/v1/source-file"
                | "/v1/wiki"
                | "/v1/backlinks"
                | "/v1/wiki-review"
                | "/v1/wiki-proposals"
                | "/v1/expand-chunks"
                | "/v1/find-similar"
                | "/v1/revisions"
                | "/v1/revisions/snapshot"
                | "/v1/revisions/diff"
                | "/v1/ops-log"
                | "/v1/taxonomy"
                | "/v1/wings"
                | "/v1/rooms"
                | "/v1/llm-status"
                | "/v1/embedding-manifest"
                | "/v1/diary"
                | "/v1/kg"
                | "/v1/kg/timeline"
                | "/v1/kg/stats"
                | "/v1/tunnels"
                | "/v1/lint-wiki"
                | "/v1/eval/history"
                | "/v1/eval/feedback"
                | "/v1/eval/runs"
                | "/v1/eval/labels"
                | "/v1/eval/traces"
                | "/v1/eval/prompts"
                | "/v1/eval/export"
                | "/v1/knowledge"
                | "/v1/knowledge/views"
                | "/v1/sync/status"
                | "/v1/jobs"
        ) || path
            .strip_prefix("/v1/jobs/")
            .is_some_and(|id| !id.is_empty() && !id.contains('/'))
            || path
                .strip_prefix("/v1/eval/labels/")
                .is_some_and(|id| !id.is_empty() && id != "claim")
            || path
                .strip_prefix("/v1/eval/traces/")
                .is_some_and(|id| !id.is_empty())
            || path
                .strip_prefix("/v1/eval/prompts/")
                .is_some_and(|id| !id.is_empty())
        {
            return AccessRole::Read;
        }
    }
    match (method.as_str(), path) {
        ("POST", "/v1/search" | "/v1/pack-context" | "/v1/multi-get") => AccessRole::Read,
        ("PUT" | "POST", "/v1/wiki")
        | (
            "POST",
            "/v1/revisions/restore"
                | "/v1/wiki-proposals"
                | "/v1/eval/feedback"
                | "/v1/eval/feedback/run"
                | "/v1/eval/runs/compare"
                | "/v1/eval/labels"
                | "/v1/eval/labels/claim"
                | "/v1/eval/traces"
                | "/v1/eval/judge"
                | "/v1/eval/online"
                | "/v1/eval/replay"
                | "/v1/eval/prompts",
        ) => AccessRole::Write,
        ("PUT", path) if path.starts_with("/v1/eval/labels/") => AccessRole::Write,
        ("PUT", "/v1/knowledge") | ("POST", "/v1/knowledge/views") => AccessRole::Write,
        _ => AccessRole::Admin,
    }
}

pub(super) async fn enforce_auth(
    State(auth): State<HttpAuth>,
    mut request: Request,
    next: Next,
) -> Response {
    // Browser preflight carries no credential and never reaches a data handler.
    if request.method() == Method::OPTIONS
        && request
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .is_some_and(super::loopback_origin)
    {
        let mut response = StatusCode::NO_CONTENT.into_response();
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            "GET, HEAD, POST, PUT, DELETE, OPTIONS".parse().unwrap(),
        );
        response.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_HEADERS, "Authorization, Content-Type, If-Match, X-Request-Id, X-Rag-Client-Host, MCP-Protocol-Version, MCP-Session-Id".parse().unwrap());
        return response;
    }
    let Some(role) = auth.authenticate(request.headers()) else {
        return denied(
            &request,
            StatusCode::UNAUTHORIZED,
            "AUTH_REQUIRED",
            "valid bearer token required",
        );
    };
    if role < required_http_role(request.method(), request.uri().path()) {
        return denied(
            &request,
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "credential does not permit this operation",
        );
    }
    request.extensions_mut().insert(role);
    request.headers_mut().remove(header::AUTHORIZATION);
    next.run(request).await
}

fn denied(request: &Request, status: StatusCode, code: &str, message: &str) -> Response {
    let request_id = request
        .extensions()
        .get::<super::RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_default();
    let mut response = (status, axum::Json(serde_json::json!({"ok": false, "code": code, "error": message, "request_id": request_id}))).into_response();
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            "Bearer realm=\"rag-mcp\"".parse().unwrap(),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    const READ: &str = "read-credential-for-tests-000000000";
    const WRITE: &str = "write-credential-for-tests-00000000";
    const ADMIN: &str = "admin-credential-for-tests-00000000";

    #[test]
    fn remote_bind_requires_credentials_and_distinct_valid_secrets() {
        let local = HttpAuth::from_tokens(None, None, None).unwrap();
        assert!(local
            .validate_bind("127.0.0.1:7432".parse().unwrap())
            .is_ok());
        assert!(local
            .validate_bind("0.0.0.0:7432".parse().unwrap())
            .is_err());
        assert!(HttpAuth::from_tokens(Some(READ), None, None)
            .unwrap()
            .validate_bind("[::]:7432".parse().unwrap())
            .is_ok());
        assert!(HttpAuth::from_tokens(Some(READ), Some(READ), None).is_err());
        assert!(HttpAuth::from_tokens(None, None, Some("secret")).is_err());
    }

    #[test]
    fn proposal_routes_require_read_or_write_roles() {
        assert_eq!(required_http_role(&Method::GET, "/v1/wiki-proposals"), AccessRole::Read);
        assert_eq!(required_http_role(&Method::POST, "/v1/wiki-proposals"), AccessRole::Write);
        assert_eq!(required_http_role(&Method::GET, "/v1/wiki-review"), AccessRole::Read);
    }

    #[test]
    fn knowledge_and_feedback_roles_are_explicit() {
        for path in [
            "/v1/eval/feedback",
            "/v1/eval/runs",
            "/v1/eval/labels",
            "/v1/eval/traces",
            "/v1/eval/prompts",
            "/v1/eval/export",
            "/v1/knowledge",
            "/v1/knowledge/views",
        ] {
            assert_eq!(required_http_role(&Method::GET, path), AccessRole::Read);
        }
        for path in [
            "/v1/eval/feedback",
            "/v1/eval/feedback/run",
            "/v1/eval/runs/compare",
            "/v1/eval/labels",
            "/v1/eval/labels/claim",
            "/v1/eval/traces",
            "/v1/eval/judge",
            "/v1/eval/online",
            "/v1/eval/replay",
            "/v1/eval/prompts",
            "/v1/knowledge/views",
        ] {
            assert_eq!(required_http_role(&Method::POST, path), AccessRole::Write);
        }
        assert_eq!(
            required_http_role(&Method::PUT, "/v1/eval/labels/abc"),
            AccessRole::Write
        );
        assert_eq!(required_http_role(&Method::PUT, "/v1/knowledge"), AccessRole::Write);
    }

    #[tokio::test]
    async fn authentication_and_roles_precede_handlers_and_never_echo_credentials() {
        let auth = HttpAuth::from_tokens(Some(READ), Some(WRITE), Some(ADMIN)).unwrap();
        let app = Router::new()
            .route(
                "/v1/wiki",
                get(|| async { "read" })
                    .put(|| async { "write" })
                    .post(|| async { "create" }),
            )
            .route(
                "/v1/operations/checkpoint",
                axum::routing::post(|| async { "admin" }),
            )
            .layer(axum::middleware::from_fn_with_state(auth, enforce_auth));
        for (method, path, token, expected) in [
            ("GET", "/v1/wiki", None, 401),
            ("GET", "/v1/wiki", Some("bad-token"), 401),
            ("GET", "/v1/wiki", Some(READ), 200),
            ("PUT", "/v1/wiki", Some(READ), 403),
            ("PUT", "/v1/wiki", Some(WRITE), 200),
            ("POST", "/v1/wiki", Some(READ), 403),
            ("POST", "/v1/wiki", Some(WRITE), 200),
            ("POST", "/v1/operations/checkpoint", Some(WRITE), 403),
            ("POST", "/v1/operations/checkpoint", Some(ADMIN), 200),
        ] {
            let mut builder = Request::builder().method(method).uri(path);
            if let Some(token) = token {
                builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
            }
            let response = app
                .clone()
                .oneshot(builder.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), expected, "{method} {path}");
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            let body = String::from_utf8_lossy(&bytes);
            for secret in [READ, WRITE, ADMIN, "bad-token"] {
                assert!(!body.contains(secret));
            }
        }
    }

    #[tokio::test]
    async fn real_mcp_dispatch_checks_roles_before_schema_seed_or_wiki_write() {
        use crate::{Config, Store};
        use std::sync::Arc;
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            db_path: root.path().join("auth.duckdb"),
            embedding_dims: 32,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let embedder = crate::embeddings::build_provider(&config).unwrap();
        let mcp = super::super::mcp_http_service(
            store.clone(),
            embedder.clone(),
            config.clone(),
            tokio_util::sync::CancellationToken::new(),
        );
        let api = super::super::api_router(super::super::HttpState::new(
            Arc::new(store.clone()),
            true,
            config,
            embedder,
        ));
        let app = super::super::authenticated_gateway_layers(
            api.nest_service("/mcp", mcp),
            vec!["localhost".into()],
            HttpAuth::from_tokens(Some(READ), Some(WRITE), Some(ADMIN)).unwrap(),
        );
        for (method, params, token, denied) in [
            (
                "tools/call",
                serde_json::json!({"name":"get_schema","arguments":{}}),
                READ,
                true,
            ),
            (
                "tools/call",
                serde_json::json!({"name":"write_wiki_page","arguments":{"slug":"auth-page","title":"Auth page","content":"content"}}),
                READ,
                true,
            ),
            ("tools/list", serde_json::json!({}), READ, false),
            (
                "tools/call",
                serde_json::json!({"name":"write_wiki_page","arguments":{"slug":"auth-page","title":"Auth page","content":"content"}}),
                WRITE,
                false,
            ),
        ] {
            let request = Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("host", "localhost:7432")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
                        .to_string(),
                ))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
            let body = String::from_utf8_lossy(&bytes);
            assert_eq!(body.contains("FORBIDDEN"), denied, "{body}");
            if denied {
                assert_eq!(store.stats().unwrap().0, 0);
            }
            if method == "tools/list" {
                assert!(!body.contains("write_wiki_page"));
                assert!(body.contains("search"));
            }
        }
        assert!(store.find_by_uri("wiki://auth-page").unwrap().is_some());
    }
}
