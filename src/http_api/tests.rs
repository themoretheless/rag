use super::*;
use crate::embeddings::build_provider;
use axum::{
    body::{Body, to_bytes},
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
    assert!(
        configured_http_allowed_hosts(&concrete, None)
            .iter()
            .any(|host| host == "192.168.50.205")
    );
}

fn app() -> Router {
    let root = crate::testkit::scratch_dir();
    let config = Config {
        db_path: root.join("http.duckdb"),
        ..Config::default()
    };
    let store = Arc::new(crate::testkit::open_test_store(&config.db_path).unwrap());
    let embedder = build_provider(&config).unwrap();
    let allowed_hosts = configured_http_allowed_hosts(&config, None);
    gateway_layers(
        api_router(HttpState {
            store,
            mcp_http: false,
            config: config.into(),
            embedder,
            jobs: jobs::JobRegistry::default(),
            llm: None,
            auth: crate::http_api::auth::AuthPosture::default(),
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
    assert!(
        activity_json["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["action"] == "GET /v1/status" && event["request_id"].is_null())
    );
}

#[test]
fn activity_path_redacts_job_resource_ids_without_over_normalizing() {
    for path in [
        "/v1/jobs/3bb23ff0-f9ac-4d72-b47c-fb6d8508b2e1",
        "/v1/jobs/private-job-resource",
    ] {
        assert_eq!(activity_path(path), "/v1/jobs/{id}");
    }

    for path in [
        "/v1/jobs",
        "/v1/jobs/",
        "/v1/jobs/sync",
        "/v1/jobs/private-job-resource/details",
        "/v1/jobs-archive/private-job-resource",
        "/v1/status",
    ] {
        assert_eq!(activity_path(path), path);
    }
}

#[tokio::test]
async fn job_resource_ids_are_never_persisted_in_activity() {
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
    let resource_id = "3bb23ff0-f9ac-4d72-b47c-fb6d8508b2e1";

    for method in ["GET", "DELETE"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/v1/jobs/{resource_id}"))
                    .header("host", "localhost:7432")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

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
    assert!(!String::from_utf8_lossy(&activity).contains(resource_id));
    let actions = activity_json["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|event| event["action"].as_str())
        .collect::<Vec<_>>();
    assert!(actions.contains(&"GET /v1/jobs/{id}"));
    assert!(actions.contains(&"DELETE /v1/jobs/{id}"));
}

#[tokio::test]
async fn mounted_mcp_rejects_streamed_oversized_bodies() {
    let root = crate::testkit::scratch_dir();
    let config = Config {
        db_path: root.join("mcp-http.duckdb"),
        ..Config::default()
    };
    let store = crate::testkit::open_test_store(&config.db_path).unwrap();
    let embedder = build_provider(&config).unwrap();
    let mcp = crate::mcp::mcp_http_service_for(
        crate::context::AppContext::new(store.clone(), embedder.clone(), config.clone()),
        tokio_util::sync::CancellationToken::new(),
    );
    let allowed_hosts = configured_http_allowed_hosts(&config, None);
    let api = api_router(HttpState {
        store: Arc::new(store),
        mcp_http: true,
        config: config.into(),
        embedder,
        jobs: jobs::JobRegistry::default(),
        llm: None,
        auth: crate::http_api::auth::AuthPosture::default(),
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
    let root = crate::testkit::scratch_dir();
    let config = Config {
        db_path: root.join("mcp-host-guard.duckdb"),
        http_bind: Some("192.168.50.205:7432".into()),
        ..Config::default()
    };
    let store = crate::testkit::open_test_store(&config.db_path).unwrap();
    let embedder = build_provider(&config).unwrap();
    let service = |store: Store| {
        Router::new().nest_service(
            "/mcp",
            crate::mcp::mcp_http_service_for(
                crate::context::AppContext::new(store, embedder.clone(), config.clone()),
                tokio_util::sync::CancellationToken::new(),
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
    let root = crate::testkit::scratch_dir();
    let config = Config {
        db_path: root.join("revisions-http.duckdb"),
        ..Config::default()
    };
    let store = Arc::new(crate::testkit::open_test_store(&config.db_path).unwrap());
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
    store
        .documents()
        .upsert_document_cas(&document, None)
        .unwrap();
    document.content = "second body".into();
    store
        .documents()
        .upsert_document_cas(&document, Some(1))
        .unwrap();
    document.content = "third body".into();
    store
        .documents()
        .upsert_document_cas(&document, Some(2))
        .unwrap();
    let embedder = build_provider(&config).unwrap();
    let allowed_hosts = configured_http_allowed_hosts(&config, None);
    let app = gateway_layers(
        api_router(HttpState {
            store,
            mcp_http: false,
            config: config.into(),
            embedder,
            jobs: jobs::JobRegistry::default(),
            llm: None,
            auth: crate::http_api::auth::AuthPosture::default(),
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
    assert_eq!(
        client_label(
            Some(" TMTL-MacBook-Pro-M4.local "),
            Some("192.0.2.10"),
            Some("Secret Browser/1")
        )
        .as_deref(),
        Some("host:tmtl-macbook-pro-m4.local")
    );
    assert_eq!(
        client_label(
            Some("invalid host name"),
            Some("192.0.2.10"),
            Some("Secret Browser/1")
        ),
        Some(client)
    );
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
    let (status, summary) = get_json(&app, "/v1/status?summary=true").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(summary["document_count"], body["document_count"]);
    assert_eq!(summary["chunk_count"], body["chunk_count"]);
    assert_eq!(summary["db_path"], body["db_path"]);
    assert!(summary.get("uncompiled_raw_count").is_none());
    let (status, health) = get_json(&app, "/health").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(health["orphan_chunks"], 0);
    let (status, light) = get_json(&app, "/health?summary=true").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(light["documents"], health["documents"]);
    assert!(light.get("orphan_chunks").is_none());
    for flag in ["/health?summary", "/health?summary=1"] {
        let (status, light) = get_json(&app, flag).await;
        assert_eq!(status, axum::http::StatusCode::OK, "{flag}");
        assert!(light.get("orphan_chunks").is_none(), "{flag}");
    }
    let (_, full) = get_json(&app, "/health?summary=false").await;
    assert_eq!(full["orphan_chunks"], 0);
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
    let token = tokio_util::sync::CancellationToken::new();
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
