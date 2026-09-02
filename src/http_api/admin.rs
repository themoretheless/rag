//! Read-only console routes that mirror MCP query tools the web UI needs.
//!
//! Reads: `ops-log`, `taxonomy` / `wings` / `rooms`, `diary`, `kg*`, `tunnels`,
//! `llm-status`, `embedding-manifest`, `lint-wiki`, `eval/history`.
//! Mutation handlers imported with the console work are deliberately not
//! mounted: writes must use the existing guarded job/operation routes instead
//! of bypassing the single writer lane.

use std::path::PathBuf;

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
use crate::diagnostics;
use crate::error::AppError;
use crate::llm::ChatClient;
use crate::mcp::facade::parse_optional_ts;
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

fn clean(value: Option<String>) -> Option<String> {
    value.filter(|item| !item.trim().is_empty())
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
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let has_key = clean(q.id.clone()).is_some() || q.seq.is_some();
    match st.store.read_ops_log(q.id.as_deref(), q.seq, Some(limit)) {
        Ok(rows) if has_key && rows.is_empty() => {
            api_err(AppError::not_found("ops_log entry not found"))
        }
        Ok(rows) => {
            let agent = clean(q.agent);
            let prefix = clean(q.prefix).map(|p| p.to_ascii_uppercase());
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
    match st.store.list_rooms(clean(q.wing).as_deref()) {
        Ok(items) => api_ok(json!({"ok": true, "items": items})),
        Err(error) => api_err(error),
    }
}

async fn llm_status(State(st): State<HttpState>) -> impl IntoResponse {
    let client = ChatClient::from_config(&st.config).ok();
    let report = diagnostics::llm_status(&st.config, client.as_ref()).await;
    api_ok(json!({"ok": true, "llm": report}))
}

async fn embedding_manifest(State(st): State<HttpState>) -> impl IntoResponse {
    match st.store.get_embedding_manifest() {
        Ok(manifest) => {
            let matches = manifest.as_ref().is_some_and(|m| {
                m.dims as usize == st.config.embedding_dims
                    && m.provider == st.config.embedding_provider.as_str()
                    && m.model == st.config.embedding_model
            });
            api_ok(json!({
                "ok": true,
                "manifest": manifest,
                "live": {
                    "provider": st.config.embedding_provider.as_str(),
                    "model": st.config.embedding_model,
                    "dims": st.config.embedding_dims,
                    "base_url": st.config.embedding_base_url,
                },
                "match": matches,
            }))
        }
        Err(error) => api_err(error),
    }
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
        .list_diary_entries(clean(q.agent).as_deref(), limit)
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
    let at_time = match parse_optional_ts(q.at_time.as_deref()) {
        Ok(value) => value,
        Err(error) => return api_err(error),
    };
    match st.store.kg_query(
        clean(q.subject).as_deref(),
        clean(q.predicate).as_deref(),
        clean(q.object).as_deref(),
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
    match st.store.list_tunnels(clean(q.node_id).as_deref()) {
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
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return api_err(AppError::from(error)),
    };
    let mut items: Vec<Value> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let total = items.len();
    if items.len() > limit {
        items.drain(..items.len() - limit);
    }
    api_ok(json!({
        "ok": true, "configured": true, "path": path.display().to_string(),
        "total": total, "count": items.len(), "items": items,
    }))
}
