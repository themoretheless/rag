use std::collections::BTreeMap;

use axum::{extract::{Query, State}, response::IntoResponse, routing::get, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::diagnostics::DiagnosticsService;
use crate::mcp::RagServer;
use crate::mcp::surface::spine_tool_names;
use crate::telemetry;
use super::{error::{api_err, api_ok}, HttpState};

pub(super) fn routes() -> Router<HttpState> {
    Router::new().route("/v1/status", get(status)).route("/v1/doctor", get(doctor))
        .route("/v1/capabilities", get(capabilities)).route("/v1/version", get(version))
        .route("/v1/routes", get(route_inventory)).route("/v1/projects", get(projects))
        .route("/v1/runtime", get(runtime)).route("/v1/calls", get(calls))
        .route("/v1/agents", get(agents))
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
/// Process / startup / autosync / auto-backup snapshot (`pid`, uptime, phases).
async fn runtime() -> impl IntoResponse { api_ok(json!({"ok": true, "runtime": crate::ops::runtime_snapshot()})) }

#[derive(Deserialize)]
struct CallsQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    tool: Option<String>,
}

/// Recent MCP / HTTP calls from the in-memory ring buffer plus latency and share summary.
async fn calls(Query(q): Query<CallsQuery>) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let agent = q.agent.filter(|v| !v.trim().is_empty());
    let tool = q.tool.filter(|v| !v.trim().is_empty());
    let items = telemetry::recent(limit, agent.as_deref(), tool.as_deref());
    api_ok(json!({"ok": true, "summary": telemetry::summary(), "count": items.len(), "items": items}))
}

/// One console row per agent: live call activity merged with durable ops_log / diary footprint.
#[derive(Debug, Default, Serialize)]
struct AgentRow {
    agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<String>,
    online: bool,
    calls_today: usize,
    calls_total: usize,
    call_errors: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_call_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_tool: Option<String>,
    p95_ms: f64,
    ops_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_op_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_op: Option<String>,
    diary_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_diary_at: Option<DateTime<Utc>>,
}

const AGENT_SCAN_ROWS: usize = 500;

async fn agents(State(st): State<HttpState>) -> impl IntoResponse {
    let mut rows: BTreeMap<String, AgentRow> = BTreeMap::new();
    fn row<'a>(rows: &'a mut BTreeMap<String, AgentRow>, name: &str) -> &'a mut AgentRow {
        let key = name.trim().to_ascii_lowercase();
        rows.entry(key).or_insert_with(|| AgentRow { agent: name.trim().to_string(), ..Default::default() })
    }
    for activity in telemetry::agent_activity() {
        let entry = row(&mut rows, &activity.agent);
        entry.transport = Some(activity.transport);
        entry.online = activity.online;
        entry.calls_today = activity.calls_today;
        entry.calls_total = activity.calls_total;
        entry.call_errors = activity.errors;
        entry.last_call_at = Some(activity.last_call_at);
        entry.last_tool = Some(activity.last_tool);
        entry.p95_ms = activity.p95_ms;
    }
    let ops = match st.store.list_recent_ops(AGENT_SCAN_ROWS) { Ok(v) => v, Err(error) => return api_err(error) };
    for op in ops {
        let Some(name) = op.agent_name.as_deref().filter(|n| !n.trim().is_empty()) else { continue };
        let entry = row(&mut rows, name);
        entry.ops_count += 1;
        if entry.last_op_at.is_none_or(|ts| op.ts > ts) {
            entry.last_op_at = Some(op.ts);
            entry.last_op = Some(op.prefix.clone().unwrap_or_else(|| op.op.clone()));
        }
    }
    let diary = match st.store.list_diary_entries(None, AGENT_SCAN_ROWS) { Ok(v) => v, Err(error) => return api_err(error) };
    for entry in diary {
        let agent = row(&mut rows, &entry.agent_name);
        agent.diary_count += 1;
        if agent.last_diary_at.is_none_or(|ts| entry.created_at > ts) { agent.last_diary_at = Some(entry.created_at); }
    }
    let mut items: Vec<AgentRow> = rows.into_values().collect();
    items.sort_by(|a, b| b.online.cmp(&a.online).then(b.last_call_at.cmp(&a.last_call_at)).then(b.last_op_at.cmp(&a.last_op_at)).then(a.agent.cmp(&b.agent)));
    api_ok(json!({"ok": true, "online_window_secs": telemetry::ONLINE_WINDOW_SECS, "count": items.len(), "items": items}))
}

async fn capabilities(State(st): State<HttpState>) -> impl IntoResponse { api_ok(json!({
    "api_version":"v1", "mcp_http":st.mcp_http, "tool_surface":st.config.tool_surface.as_str(),
    "tool_count": if st.config.tool_surface.as_str() == "spine" { spine_tool_names().len() } else { server(&st).tool_count() },
    "features":["status","doctor","search","search_full_params","search_stage_timings","pack_context","multi_get","expand_chunks","find_similar",
        "cursor_pagination","conditional_get","ops_log","taxonomy","diary","kg","tunnels","llm_status","embedding_manifest","lint_wiki",
        "eval_history","ingest","document_meta","reembed","backup","vacuum","doctor_repair","runtime","call_log","agents"],
    "deprecated_tools":[{"name":"ingest_text","replacement":"ingest_raw"},{"name":"stats","replacement":"status"}]
})) }
async fn route_inventory() -> impl IntoResponse { api_ok(json!({"openapi":"3.1-style", "routes":[
    {"method":"GET","path":"/health"},{"method":"GET","path":"/live"},{"method":"GET","path":"/ready"},
    {"method":"GET","path":"/v1/status"},{"method":"GET","path":"/v1/doctor"},{"method":"POST","path":"/v1/doctor/repair"},
    {"method":"GET","path":"/v1/runtime"},{"method":"GET","path":"/v1/calls"},{"method":"GET","path":"/v1/agents"},
    {"method":"POST","path":"/v1/search"},{"method":"POST","path":"/v1/pack-context"},
    {"method":"POST","path":"/v1/multi-get"},{"method":"GET","path":"/v1/expand-chunks"},{"method":"GET","path":"/v1/find-similar"},
    {"method":"GET","path":"/v1/documents"},{"method":"GET","path":"/v1/document"},{"method":"PATCH","path":"/v1/document"},{"method":"DELETE","path":"/v1/document"},
    {"method":"GET","path":"/v1/revisions"},{"method":"GET","path":"/v1/wiki"},{"method":"PUT","path":"/v1/wiki"},{"method":"GET","path":"/v1/backlinks"},
    {"method":"GET","path":"/v1/graph"},{"method":"GET","path":"/v1/neighbors"},{"method":"GET","path":"/v1/find"},{"method":"GET","path":"/v1/tunnels"},
    {"method":"GET","path":"/v1/ops-log"},{"method":"GET","path":"/v1/taxonomy"},{"method":"GET","path":"/v1/wings"},{"method":"GET","path":"/v1/rooms"},
    {"method":"GET","path":"/v1/diary"},{"method":"GET","path":"/v1/kg"},{"method":"GET","path":"/v1/kg/timeline"},{"method":"GET","path":"/v1/kg/stats"},
    {"method":"GET","path":"/v1/llm-status"},{"method":"GET","path":"/v1/embedding-manifest"},{"method":"GET","path":"/v1/lint-wiki"},{"method":"GET","path":"/v1/eval/history"},
    {"method":"POST","path":"/v1/ingest/text"},{"method":"POST","path":"/v1/ingest/file"},{"method":"POST","path":"/v1/sync-sources"},
    {"method":"POST","path":"/v1/reembed"},{"method":"POST","path":"/v1/backup"},{"method":"POST","path":"/v1/vacuum"},
    {"method":"GET","path":"/v1/projects"},{"method":"GET","path":"/v1/capabilities"},{"method":"GET","path":"/v1/version"},{"method":"GET","path":"/v1/routes"}
]})) }
