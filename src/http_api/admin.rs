//! Read-only console routes that mirror MCP query tools the web UI needs.
//!
//! Reads: `ops-log`, `taxonomy` / `wings` / `rooms`, `diary`, `kg*`, `tunnels`,
//! `llm-status`, `embedding-manifest`, `lint-wiki`, `eval/history`.
//! Mutation handlers imported with the console work are deliberately not
//! mounted: writes must use the existing guarded job/operation routes instead
//! of bypassing the single writer lane.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::get,
    Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    error::{api_err, api_ok},
    HttpState,
};
use crate::config::Config;
use crate::db::store::embedding_manifest_matches_config;
use crate::db::Store;
use crate::diagnostics;
use crate::error::AppError;
use crate::util::{non_blank, parse_optional_timestamp, run_blocking};
use crate::wiki;

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/ops-log", get(ops_log))
        .route("/v1/taxonomy", get(taxonomy))
        .route("/v1/wings", get(wings))
        .route("/v1/rooms", get(rooms))
        .route("/v1/llm-status", get(llm_status))
        .route("/v1/embedding-manifest", get(embedding_manifest))
        .route("/v1/diary", get(diary))
        .route("/v1/kg", get(kg_query))
        .route("/v1/kg/timeline", get(kg_timeline))
        .route("/v1/kg/stats", get(kg_stats))
        .route("/v1/tunnels", get(tunnels))
        .route("/v1/lint-wiki", get(lint_wiki))
        .route("/v1/eval/history", get(eval_history))
}

// --- Reads -----------------------------------------------------------------

#[derive(Deserialize)]
struct OpsLogQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    seq: Option<i64>,
    /// Client-side agent filter (applied after the limit).
    #[serde(default)]
    agent: Option<String>,
    /// Client-side prefix filter, e.g. `INGEST` / `WIKI`.
    #[serde(default)]
    prefix: Option<String>,
}

async fn ops_log(State(st): State<HttpState>, Query(q): Query<OpsLogQuery>) -> impl IntoResponse {
    // Page size is bounded by `Store` (MAX_OPS_LOG_PAGE) for every transport.
    let limit = q.limit.unwrap_or(50);
    let id = non_blank(q.id);
    let has_key = id.is_some() || q.seq.is_some();
    match st.store.read_ops_log(id.as_deref(), q.seq, Some(limit)) {
        Ok(rows) if has_key && rows.is_empty() => {
            api_err(AppError::not_found("ops_log entry not found"))
        }
        Ok(rows) => {
            let agent = non_blank(q.agent);
            let prefix = non_blank(q.prefix).map(|p| p.to_ascii_uppercase());
            let items: Vec<_> = rows
                .into_iter()
                .filter(|row| {
                    agent.as_deref().is_none_or(|a| {
                        row.agent_name
                            .as_deref()
                            .is_some_and(|name| name.eq_ignore_ascii_case(a))
                    })
                })
                .filter(|row| {
                    prefix.as_deref().is_none_or(|p| {
                        row.prefix
                            .as_deref()
                            .is_some_and(|value| value.eq_ignore_ascii_case(p))
                    })
                })
                .collect();
            let next_seq = st.store.next_ops_seq().ok();
            api_ok(json!({"ok": true, "count": items.len(), "next_seq": next_seq, "items": items}))
        }
        Err(error) => api_err(error),
    }
}

async fn taxonomy(State(st): State<HttpState>) -> impl IntoResponse {
    match st.store.get_taxonomy() {
        Ok(value) => api_ok(json!({"ok": true, "taxonomy": value})),
        Err(error) => api_err(error),
    }
}

async fn wings(State(st): State<HttpState>) -> impl IntoResponse {
    match st.store.list_wings() {
        Ok(items) => api_ok(json!({"ok": true, "items": items})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct RoomsQuery {
    #[serde(default)]
    wing: Option<String>,
}

async fn rooms(State(st): State<HttpState>, Query(q): Query<RoomsQuery>) -> impl IntoResponse {
    match st.store.list_rooms(non_blank(q.wing).as_deref()) {
        Ok(items) => api_ok(json!({"ok": true, "items": items})),
        Err(error) => api_err(error),
    }
}

async fn llm_status(State(st): State<HttpState>) -> impl IntoResponse {
    let report = diagnostics::llm_status(&st.config, st.llm.as_ref()).await;
    api_ok(json!({"ok": true, "llm": report}))
}

async fn embedding_manifest(State(st): State<HttpState>) -> impl IntoResponse {
    let store = st.store.clone();
    let config = st.config.clone();
    match run_blocking("embedding manifest", move || {
        embedding_manifest_report(&store, &config)
    })
    .await
    {
        Ok(report) => api_ok(report),
        Err(error) => api_err(error),
    }
}

/// `match` uses the canonical corpus-identity predicate (provider, model, dims,
/// base endpoint and fingerprint), the same one that gates vector retrieval.
fn embedding_manifest_report(store: &Store, config: &Config) -> Result<Value, AppError> {
    let manifest = store.get_embedding_manifest()?;
    let matches = manifest
        .as_ref()
        .is_some_and(|m| embedding_manifest_matches_config(m, config));
    Ok(json!({
        "ok": true,
        "manifest": manifest,
        "live": {
            "provider": config.embedding_provider.as_str(),
            "model": config.embedding_model,
            "dims": config.embedding_dims,
            "base_url": config.embedding_base_url,
        },
        "match": matches,
    }))
}

#[derive(Deserialize)]
struct DiaryQuery {
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn diary(State(st): State<HttpState>, Query(q): Query<DiaryQuery>) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    match st
        .store
        .list_diary_entries(non_blank(q.agent).as_deref(), limit)
    {
        Ok(items) => api_ok(json!({"ok": true, "count": items.len(), "items": items})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct KgQueryParams {
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    predicate: Option<String>,
    #[serde(default)]
    object: Option<String>,
    #[serde(default)]
    at_time: Option<String>,
}

async fn kg_query(
    State(st): State<HttpState>,
    Query(q): Query<KgQueryParams>,
) -> impl IntoResponse {
    let at_time = match parse_optional_timestamp(q.at_time.as_deref()) {
        Ok(value) => value,
        Err(error) => return api_err(error),
    };
    match st.store.kg_query(
        non_blank(q.subject).as_deref(),
        non_blank(q.predicate).as_deref(),
        non_blank(q.object).as_deref(),
        at_time,
    ) {
        Ok(items) => api_ok(json!({"ok": true, "count": items.len(), "items": items})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct KgTimelineQuery {
    subject: String,
}

async fn kg_timeline(
    State(st): State<HttpState>,
    Query(q): Query<KgTimelineQuery>,
) -> impl IntoResponse {
    match st.store.kg_timeline(q.subject.trim()) {
        Ok(items) => api_ok(json!({"ok": true, "count": items.len(), "items": items})),
        Err(error) => api_err(error),
    }
}

async fn kg_stats(State(st): State<HttpState>) -> impl IntoResponse {
    match st.store.kg_stats() {
        Ok(value) => api_ok(json!({"ok": true, "stats": value})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct TunnelsQuery {
    #[serde(default)]
    node_id: Option<String>,
}

async fn tunnels(State(st): State<HttpState>, Query(q): Query<TunnelsQuery>) -> impl IntoResponse {
    match st.store.list_tunnels(non_blank(q.node_id).as_deref()) {
        Ok(items) => api_ok(json!({"ok": true, "count": items.len(), "items": items})),
        Err(error) => api_err(error),
    }
}

async fn lint_wiki(State(st): State<HttpState>) -> impl IntoResponse {
    match wiki::lint_wiki(&st.store) {
        Ok(report) => api_ok(json!({"ok": true, "report": report})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct EvalHistoryQuery {
    #[serde(default)]
    limit: Option<usize>,
}

const MAX_EVAL_HISTORY_BYTES: u64 = 64 * 1024 * 1024;

/// Path of the eval history JSONL written by `cargo run --bin eval -- --history-jsonl`.
///
/// Read from `RAG_EVAL_HISTORY`; never from the request, so the gateway cannot be
/// used to read arbitrary files.
fn eval_history_path() -> Option<PathBuf> {
    std::env::var("RAG_EVAL_HISTORY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
}

async fn eval_history(Query(q): Query<EvalHistoryQuery>) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(30).clamp(1, 500);
    let Some(path) = eval_history_path() else {
        return api_ok(json!({
            "ok": true, "configured": false, "path": null, "count": 0, "items": [],
            "hint": "set RAG_EVAL_HISTORY to the --history-jsonl file written by the eval binary",
        }));
    };
    let read_path = path.clone();
    let result =
        super::run_blocking("eval history", move || read_eval_history(&read_path, limit)).await;
    let (total, items) = match result {
        Ok(result) => result,
        Err(error) => return api_err(error),
    };
    api_ok(json!({
        "ok": true, "configured": true, "path": path.display().to_string(),
        "total": total, "count": items.len(), "items": items,
    }))
}

fn read_eval_history(path: &Path, limit: usize) -> Result<(usize, Vec<Value>), AppError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, Vec::new())),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(AppError::config(format!(
            "RAG_EVAL_HISTORY '{}' is not a regular file",
            path.display()
        )));
    }
    if metadata.len() > MAX_EVAL_HISTORY_BYTES {
        return Err(eval_history_too_large(path, metadata.len()));
    }

    let mut reader = BufReader::new(file.take(MAX_EVAL_HISTORY_BYTES.saturating_add(1)));
    let mut line = String::new();
    let mut bytes_read = 0u64;
    let mut total = 0usize;
    let mut items = VecDeque::with_capacity(limit);
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(read as u64);
        if bytes_read > MAX_EVAL_HISTORY_BYTES {
            return Err(eval_history_too_large(path, bytes_read));
        }
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str(&line) else {
            continue;
        };
        total = total.saturating_add(1);
        if items.len() == limit {
            items.pop_front();
        }
        items.push_back(value);
    }
    Ok((total, items.into_iter().collect()))
}

fn eval_history_too_large(path: &Path, bytes: u64) -> AppError {
    AppError::config(format!(
        "RAG_EVAL_HISTORY '{}' is {bytes} bytes; maximum is {MAX_EVAL_HISTORY_BYTES}. Rotate or compact the JSONL history file before reading it through HTTP",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn body(response: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn ops_log_filters_by_agent_and_prefix_and_404s_unknown_ids() {
        let root = crate::testkit::scratch_dir();
        let config = Config {
            db_path: root.join("admin.duckdb"),
            embedding_dims: 8,
            ..Config::for_tests()
        };
        let store = Arc::new(Store::open(&config.db_path).unwrap());
        for (op, prefix, agent) in [
            ("ingest", "INGEST", Some("alice")),
            ("wiki_write", "WIKI", Some("alice")),
            ("wiki_write", "WIKI", Some("bob")),
        ] {
            store
                .append_ops_log(
                    &crate::models::OpsLogEntry::new(op, prefix, op)
                        .agent(agent.map(str::to_string)),
                )
                .unwrap();
        }
        let st = HttpState::new(
            store,
            false,
            config,
            Arc::new(crate::embeddings::MockEmbedder::new(8)),
        );
        let query = |agent: Option<&str>, prefix: Option<&str>, id: Option<&str>| OpsLogQuery {
            limit: None,
            id: id.map(str::to_string),
            seq: None,
            agent: agent.map(str::to_string),
            prefix: prefix.map(str::to_string),
        };

        let response = ops_log(State(st.clone()), Query(query(Some("ALICE"), Some("wiki"), None)))
            .await
            .into_response();
        let json = body(response).await;
        assert_eq!(json["count"], 1, "{json}");
        assert_eq!(json["items"][0]["op"], "wiki_write");

        let missing = ops_log(State(st), Query(query(None, None, Some("no-such-id"))))
            .await
            .into_response();
        assert_eq!(missing.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[test]
    fn embedding_manifest_match_requires_the_same_base_endpoint() {
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            db_path: root.path().join("manifest.duckdb"),
            embedding_dims: 16,
            ..Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        store.ensure_embedding_manifest(&config).unwrap();
        let same = embedding_manifest_report(&store, &config).unwrap();
        assert_eq!(same["match"], true);

        let moved = Config {
            embedding_base_url: "http://other-endpoint.invalid/v1".into(),
            ..config.clone()
        };
        let report = embedding_manifest_report(&store, &moved).unwrap();
        assert_eq!(report["match"], false, "{report}");
    }
}
