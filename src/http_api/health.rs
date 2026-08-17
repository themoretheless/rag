//! Health / readiness route for the HTTP gateway.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use serde_json::json;

use super::error::{api_err, api_ok};
use super::HttpState;

/// `/health` only.
pub(super) fn routes() -> Router<HttpState> {
    Router::new().route("/health", get(health))
}

async fn health(State(st): State<HttpState>) -> impl IntoResponse {
    match st.store.stats() {
        Ok((docs, chunks, nodes, edges)) => api_ok(json!({
            "ok": true,
            "db_path": st.store.path().display().to_string(),
            "documents": docs,
            "chunks": chunks,
            "nodes": nodes,
            "edges": edges,
            "mcp_http": st.mcp_http,
            "mcp_path": if st.mcp_http { Some("/mcp") } else { None::<&str> },
        })),
        Err(e) => api_err(e),
    }
}
