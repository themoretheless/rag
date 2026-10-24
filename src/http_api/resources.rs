//! `/v1/resources`: live process/system resource telemetry plus concurrency
//! configuration, backing the "Ресурсы" tab in the web console.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use serde_json::json;

use super::error::api_ok;
use super::HttpState;

pub(super) fn routes() -> Router<HttpState> {
    Router::new().route("/v1/resources", get(resources))
}

async fn resources(State(st): State<HttpState>) -> impl IntoResponse {
    let process = crate::ops::resource_snapshot();
    let embedder = crate::embeddings::embedder_stats();
    let db_bytes = std::fs::metadata(st.store.path())
        .map(|m| m.len())
        .unwrap_or(0);
    let wal_bytes = st.store.wal_file_size_bytes();
    api_ok(json!({
        "ok": true,
        "process": process,
        "embedder": embedder,
        "database": {
            "path": st.store.path().display().to_string(),
            "bytes": db_bytes,
            "wal_bytes": wal_bytes,
            "wal_warn_bytes": crate::ops::wal_warn_bytes(),
            "threads": db_threads_setting(),
            "memory_limit": std::env::var("RAG_DB_MEMORY_LIMIT").ok(),
        },
        "runtime": {
            "version": env!("CARGO_PKG_VERSION"),
            "build_commit": option_env!("RAG_BUILD_COMMIT").unwrap_or("unknown"),
            "worker_threads_env": std::env::var("RAG_WORKER_THREADS").ok(),
        },
    }))
}

/// DuckDB `threads` PRAGMA value actually applied at open (0 = DuckDB default).
fn db_threads_setting() -> usize {
    std::env::var("RAG_DB_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(0)
}
