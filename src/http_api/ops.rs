use axum::{extract::State, response::IntoResponse, routing::get, Router};
use serde_json::json;

use crate::diagnostics::DiagnosticsService;
use crate::mcp::{RagServer};
use crate::mcp::surface::spine_tool_names;
use super::{error::{api_err, api_ok}, HttpState};

pub(super) fn routes() -> Router<HttpState> {
    Router::new().route("/v1/status", get(status)).route("/v1/doctor", get(doctor))
        .route("/v1/capabilities", get(capabilities)).route("/v1/version", get(version))
        .route("/v1/routes", get(route_inventory)).route("/v1/projects", get(projects))
}

fn server(st: &HttpState) -> RagServer { RagServer::new((*st.store).clone(), st.embedder.clone(), st.config.clone()) }
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
async fn version() -> impl IntoResponse { api_ok(json!({"api_version":"v1", "server_version":env!("CARGO_PKG_VERSION"), "build_commit":option_env!("RAG_BUILD_COMMIT")})) }
async fn projects(State(st): State<HttpState>) -> impl IntoResponse {
    match st.store.list_projects() {
        Ok(items) => api_ok(json!({"ok":true, "items":items})),
        Err(error) => api_err(error),
    }
}
async fn capabilities(State(st): State<HttpState>) -> impl IntoResponse { api_ok(json!({
    "api_version":"v1", "mcp_http":st.mcp_http, "tool_surface":st.config.tool_surface.as_str(),
    "tool_count": if st.config.tool_surface.as_str() == "spine" { spine_tool_names().len() } else { server(&st).tool_count() },
    "features":["status","doctor","search","multi_get","expand_chunks","find_similar","cursor_pagination","conditional_get"],
    "deprecated_tools":[{"name":"ingest_text","replacement":"ingest_raw"},{"name":"stats","replacement":"status"}]
})) }
async fn route_inventory() -> impl IntoResponse { api_ok(json!({"openapi":"3.1-style", "routes":[
    {"method":"GET","path":"/health"},{"method":"GET","path":"/live"},{"method":"GET","path":"/ready"},
    {"method":"GET","path":"/v1/status"},{"method":"GET","path":"/v1/doctor"},{"method":"POST","path":"/v1/search"},
    {"method":"POST","path":"/v1/multi-get"},{"method":"GET","path":"/v1/expand-chunks"},{"method":"GET","path":"/v1/find-similar"},
    {"method":"GET","path":"/v1/documents"},{"method":"GET","path":"/v1/wiki"},{"method":"PUT","path":"/v1/wiki"},
    {"method":"GET","path":"/v1/projects"},{"method":"GET","path":"/v1/capabilities"},{"method":"GET","path":"/v1/version"},{"method":"GET","path":"/v1/routes"}
]})) }
