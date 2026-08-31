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
        Ok((docs, chunks, nodes, edges)) => {
            let schema_version = st.store.schema_version().ok().flatten().unwrap_or(0);
            let fts_ready = st.store.fts_ready().unwrap_or(false);
            let (missing_chunks, orphan_chunks, orphan_nodes, orphan_edges, unscoped) =
                st.store.integrity_counts().unwrap_or((0, 0, 0, 0, 0));
            let relational_integrity_ok = orphan_chunks == 0 && orphan_nodes == 0 && orphan_edges == 0;
            let wal_bytes = st.store.wal_file_size_bytes();
            let wal_warn_bytes = crate::ops::wal_warn_bytes();
            api_ok(json!({
            "ok": true,
            "backend": "duckdb",
            "storage_capabilities": crate::storage::duckdb_capability_names(),
            "db_path": st.store.path().display().to_string(),
            "schema_version": schema_version,
            "fts_ready": fts_ready,
            "documents": docs,
            "chunks": chunks,
            "nodes": nodes,
            "edges": edges,
            "documents_without_chunks": missing_chunks,
            "orphan_chunks": orphan_chunks,
            "orphan_document_nodes": orphan_nodes,
            "orphan_edges": orphan_edges,
            "unscoped_documents": unscoped,
            "relational_integrity_ok": relational_integrity_ok,
            "wal_bytes": wal_bytes,
            "wal_warn_bytes": wal_warn_bytes,
            "wal_too_large": wal_bytes >= wal_warn_bytes,
            "mcp_http": st.mcp_http,
            "mcp_path": if st.mcp_http { Some("/mcp") } else { None::<&str> },
        }))
        }
        Err(e) => api_err(e),
    }
}
