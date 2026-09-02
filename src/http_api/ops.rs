use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use super::{
    error::{api_err, api_ok},
    HttpState,
};
use crate::diagnostics::DiagnosticsService;
use crate::mcp::surface::spine_tool_names;
use crate::mcp::RagServer;
use crate::util::resolve_allowlisted_output_file;

const CAPABILITY_FEATURES: &[&str] = &[
    "status",
    "doctor",
    "activity",
    "project_home",
    "unified_library",
    "search",
    "multi_get",
    "expand_chunks",
    "find_similar",
    "source_file_download",
    "graph",
    "wiki",
    "background_jobs",
    "cancellable_sync",
    "revision_history",
    "revision_snapshot",
    "revision_diff",
    "revision_restore",
    "checkpoint",
    "backup",
    "cursor_pagination",
    "conditional_get",
];

const PRODUCT_ROUTES: &[(&str, &str)] = &[
    ("GET", "/health"),
    ("GET", "/live"),
    ("GET", "/ready"),
    ("GET", "/v1/status"),
    ("GET", "/v1/doctor"),
    ("GET", "/v1/activity"),
    ("POST", "/v1/search"),
    ("POST", "/v1/multi-get"),
    ("GET", "/v1/expand-chunks"),
    ("GET", "/v1/find-similar"),
    ("GET", "/v1/documents"),
    ("GET", "/v1/source-file"),
    ("GET", "/v1/graph"),
    ("GET", "/v1/neighbors"),
    ("GET", "/v1/find"),
    ("GET", "/v1/document"),
    ("GET", "/v1/wiki"),
    ("PUT", "/v1/wiki"),
    ("GET", "/v1/backlinks"),
    ("GET", "/v1/revisions"),
    ("GET", "/v1/revisions/snapshot"),
    ("GET", "/v1/revisions/diff"),
    ("POST", "/v1/revisions/restore"),
    ("POST", "/v1/jobs/sync"),
    ("GET", "/v1/jobs"),
    ("GET", "/v1/jobs/{id}"),
    ("DELETE", "/v1/jobs/{id}"),
    ("POST", "/v1/operations/checkpoint"),
    ("POST", "/v1/operations/backup"),
    ("GET", "/v1/projects"),
    ("GET", "/v1/project-home"),
    ("GET", "/v1/capabilities"),
    ("GET", "/v1/version"),
    ("GET", "/v1/routes"),
];

const MCP_ROUTES: &[(&str, &str)] = &[("POST", "/mcp"), ("GET", "/mcp"), ("DELETE", "/mcp")];

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/status", get(status))
        .route("/v1/doctor", get(doctor))
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/version", get(version))
        .route("/v1/routes", get(route_inventory))
        .route("/v1/projects", get(projects))
        .route("/v1/project-home", get(project_home))
        .route("/v1/operations/checkpoint", post(checkpoint))
        .route("/v1/operations/backup", post(backup))
}

fn server(st: &HttpState) -> RagServer {
    RagServer::new((*st.store).clone(), st.embedder.clone(), st.config.clone())
}
async fn status(State(st): State<HttpState>) -> impl IntoResponse {
    match DiagnosticsService::new(&st.store, &st.config).status() {
        Ok(value) => api_ok(value),
        Err(error) => api_err(error),
    }
}
async fn doctor(State(st): State<HttpState>) -> impl IntoResponse {
    match DiagnosticsService::new(&st.store, &st.config).doctor() {
        Ok(value) => api_ok(value),
        Err(error) => api_err(error),
    }
}
async fn version() -> impl IntoResponse {
    api_ok(
        json!({"api_version":"v1", "server_version":env!("CARGO_PKG_VERSION"), "build_commit":option_env!("RAG_BUILD_COMMIT")}),
    )
}
async fn projects(State(st): State<HttpState>) -> impl IntoResponse {
    match st.store.list_projects() {
        Ok(items) => api_ok(json!({"ok":true, "items":items})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct ProjectHomeQuery {
    project: String,
}

async fn project_home(
    State(st): State<HttpState>,
    Query(query): Query<ProjectHomeQuery>,
) -> impl IntoResponse {
    match st.store.project_home(&query.project) {
        Ok(value) => api_ok(json!({"ok": true, "project": value})),
        Err(error) => api_err(error),
    }
}

async fn checkpoint(State(st): State<HttpState>) -> impl IntoResponse {
    match st.store.vacuum_store() {
        Ok(report) => api_ok(json!({"ok": true, "report": report})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct BackupBody {
    path: String,
    #[serde(default = "default_true")]
    dry_run: bool,
    #[serde(default)]
    overwrite: bool,
}

async fn backup(State(st): State<HttpState>, Json(body): Json<BackupBody>) -> impl IntoResponse {
    let path = match resolve_allowlisted_output_file(&body.path, &st.config.ingest_roots) {
        Ok(path) => path,
        Err(error) => return api_err(error),
    };
    if let Err(error) =
        crate::util::validate_backup_output_paths(&path, &st.config.ingest_roots, st.store.path())
    {
        return api_err(error);
    }
    match st
        .store
        .backup_database(&path, body.dry_run, body.overwrite)
    {
        Ok(report) => api_ok(json!({"ok": true, "report": report})),
        Err(error) => api_err(error),
    }
}

const fn default_true() -> bool {
    true
}
async fn capabilities(State(st): State<HttpState>) -> impl IntoResponse {
    api_ok(json!({
        "api_version":"v1", "mcp_http":st.mcp_http, "tool_surface":st.config.tool_surface.as_str(),
        "tool_count": if st.config.tool_surface.as_str() == "spine" { spine_tool_names().len() } else { server(&st).tool_count() },
        "features":CAPABILITY_FEATURES,
        "deprecated_tools":[]
    }))
}
async fn route_inventory(State(st): State<HttpState>) -> impl IntoResponse {
    let routes = PRODUCT_ROUTES
        .iter()
        .chain(st.mcp_http.then_some(MCP_ROUTES).into_iter().flatten())
        .map(|(method, path)| json!({"method":method, "path":path}))
        .collect::<Vec<_>>();
    api_ok(json!({"openapi":"3.1-style", "routes":routes}))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::to_bytes;
    use axum::extract::State;
    use axum::response::IntoResponse;

    use super::*;
    use crate::config::Config;
    use crate::embeddings::MockEmbedder;

    fn state(mcp_http: bool) -> HttpState {
        let root = tempfile::tempdir().unwrap().keep();
        let config = Config {
            db_path: root.join("ops-http.duckdb"),
            embedding_dims: 16,
            tool_surface: "spine".parse().unwrap(),
            ..Config::for_tests()
        };
        HttpState::new(
            Arc::new(crate::db::Store::open(&config.db_path).unwrap()),
            mcp_http,
            config,
            Arc::new(MockEmbedder::new(16)),
        )
    }

    async fn json_body(response: axum::response::Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[test]
    fn discovery_contract_has_exact_routes_and_features() {
        assert_eq!(
            CAPABILITY_FEATURES,
            &[
                "status",
                "doctor",
                "activity",
                "project_home",
                "unified_library",
                "search",
                "multi_get",
                "expand_chunks",
                "find_similar",
                "source_file_download",
                "graph",
                "wiki",
                "background_jobs",
                "cancellable_sync",
                "revision_history",
                "revision_snapshot",
                "revision_diff",
                "revision_restore",
                "checkpoint",
                "backup",
                "cursor_pagination",
                "conditional_get",
            ]
        );
        assert_eq!(
            PRODUCT_ROUTES,
            &[
                ("GET", "/health"),
                ("GET", "/live"),
                ("GET", "/ready"),
                ("GET", "/v1/status"),
                ("GET", "/v1/doctor"),
                ("GET", "/v1/activity"),
                ("POST", "/v1/search"),
                ("POST", "/v1/multi-get"),
                ("GET", "/v1/expand-chunks"),
                ("GET", "/v1/find-similar"),
                ("GET", "/v1/documents"),
                ("GET", "/v1/source-file"),
                ("GET", "/v1/graph"),
                ("GET", "/v1/neighbors"),
                ("GET", "/v1/find"),
                ("GET", "/v1/document"),
                ("GET", "/v1/wiki"),
                ("PUT", "/v1/wiki"),
                ("GET", "/v1/backlinks"),
                ("GET", "/v1/revisions"),
                ("GET", "/v1/revisions/snapshot"),
                ("GET", "/v1/revisions/diff"),
                ("POST", "/v1/revisions/restore"),
                ("POST", "/v1/jobs/sync"),
                ("GET", "/v1/jobs"),
                ("GET", "/v1/jobs/{id}"),
                ("DELETE", "/v1/jobs/{id}"),
                ("POST", "/v1/operations/checkpoint"),
                ("POST", "/v1/operations/backup"),
                ("GET", "/v1/projects"),
                ("GET", "/v1/project-home"),
                ("GET", "/v1/capabilities"),
                ("GET", "/v1/version"),
                ("GET", "/v1/routes"),
            ]
        );
        assert_eq!(
            MCP_ROUTES,
            &[("POST", "/mcp"), ("GET", "/mcp"), ("DELETE", "/mcp")]
        );
    }

    #[tokio::test]
    async fn serialized_discovery_is_exact_and_mcp_is_conditional() {
        let without_mcp =
            json_body(route_inventory(State(state(false))).await.into_response()).await;
        let expected_product = PRODUCT_ROUTES
            .iter()
            .map(|(method, path)| json!({"method":method, "path":path}))
            .collect::<Vec<_>>();
        assert_eq!(without_mcp["routes"], json!(expected_product));

        let with_mcp = json_body(route_inventory(State(state(true))).await.into_response()).await;
        let expected_mcp = PRODUCT_ROUTES
            .iter()
            .chain(MCP_ROUTES)
            .map(|(method, path)| json!({"method":method, "path":path}))
            .collect::<Vec<_>>();
        assert_eq!(with_mcp["routes"], json!(expected_mcp));

        let capabilities = json_body(capabilities(State(state(true))).await.into_response()).await;
        assert_eq!(capabilities["features"], json!(CAPABILITY_FEATURES));
        assert_eq!(capabilities["tool_count"], spine_tool_names().len());
        assert_eq!(capabilities["deprecated_tools"], json!([]));
    }
}
