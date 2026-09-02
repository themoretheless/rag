use std::collections::BTreeMap;

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    error::{api_err, api_ok},
    HttpState,
};
use crate::diagnostics::DiagnosticsService;
use crate::mcp::surface::spine_tool_names;
use crate::mcp::RagServer;
use crate::telemetry;
use crate::util::resolve_allowlisted_output_file;

const CAPABILITY_FEATURES: &[&str] = &[
    "status",
    "doctor",
    "activity",
    "project_home",
    "unified_library",
    "search",
    "search_full_params",
    "search_stage_timings",
    "pack_context",
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
    "ops_log",
    "taxonomy",
    "diary",
    "kg",
    "tunnels",
    "llm_status",
    "embedding_manifest",
    "lint_wiki",
    "eval_history",
    "runtime",
    "call_log",
    "agents",
];

const PRODUCT_ROUTES: &[(&str, &str)] = &[
    ("GET", "/health"),
    ("GET", "/live"),
    ("GET", "/ready"),
    ("GET", "/v1/status"),
    ("GET", "/v1/doctor"),
    ("GET", "/v1/activity"),
    ("GET", "/v1/runtime"),
    ("GET", "/v1/calls"),
    ("GET", "/v1/agents"),
    ("POST", "/v1/search"),
    ("POST", "/v1/pack-context"),
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
    ("GET", "/v1/ops-log"),
    ("GET", "/v1/taxonomy"),
    ("GET", "/v1/wings"),
    ("GET", "/v1/rooms"),
    ("GET", "/v1/diary"),
    ("GET", "/v1/kg"),
    ("GET", "/v1/kg/timeline"),
    ("GET", "/v1/kg/stats"),
    ("GET", "/v1/tunnels"),
    ("GET", "/v1/llm-status"),
    ("GET", "/v1/embedding-manifest"),
    ("GET", "/v1/lint-wiki"),
    ("GET", "/v1/eval/history"),
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
        .route("/v1/runtime", get(runtime))
        .route("/v1/calls", get(calls))
        .route("/v1/agents", get(agents))
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
    let store = st.store.clone();
    let backup = super::run_blocking("backup", move || {
        store.backup_database(&path, body.dry_run, body.overwrite)
    })
    .await;
    match backup {
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
                "search_full_params",
                "search_stage_timings",
                "pack_context",
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
                "ops_log",
                "taxonomy",
                "diary",
                "kg",
                "tunnels",
                "llm_status",
                "embedding_manifest",
                "lint_wiki",
                "eval_history",
                "runtime",
                "call_log",
                "agents",
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
                ("GET", "/v1/runtime"),
                ("GET", "/v1/calls"),
                ("GET", "/v1/agents"),
                ("POST", "/v1/search"),
                ("POST", "/v1/pack-context"),
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
                ("GET", "/v1/ops-log"),
                ("GET", "/v1/taxonomy"),
                ("GET", "/v1/wings"),
                ("GET", "/v1/rooms"),
                ("GET", "/v1/diary"),
                ("GET", "/v1/kg"),
                ("GET", "/v1/kg/timeline"),
                ("GET", "/v1/kg/stats"),
                ("GET", "/v1/tunnels"),
                ("GET", "/v1/llm-status"),
                ("GET", "/v1/embedding-manifest"),
                ("GET", "/v1/lint-wiki"),
                ("GET", "/v1/eval/history"),
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

/// Process / startup / autosync / auto-backup snapshot (`pid`, uptime, phases).
async fn runtime() -> impl IntoResponse {
    api_ok(json!({"ok": true, "runtime": crate::ops::runtime_snapshot()}))
}

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
    api_ok(json!({
        "ok": true,
        "summary": telemetry::summary(),
        "count": items.len(),
        "items": items,
    }))
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
        rows.entry(key).or_insert_with(|| AgentRow {
            agent: name.trim().to_string(),
            ..Default::default()
        })
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
    let ops = match st.store.list_recent_ops(AGENT_SCAN_ROWS) {
        Ok(value) => value,
        Err(error) => return api_err(error),
    };
    for op in ops {
        let Some(name) = op.agent_name.as_deref().filter(|n| !n.trim().is_empty()) else {
            continue;
        };
        let entry = row(&mut rows, name);
        entry.ops_count += 1;
        if entry.last_op_at.is_none_or(|ts| op.ts > ts) {
            entry.last_op_at = Some(op.ts);
            entry.last_op = Some(op.prefix.clone().unwrap_or_else(|| op.op.clone()));
        }
    }
    let diary = match st.store.list_diary_entries(None, AGENT_SCAN_ROWS) {
        Ok(value) => value,
        Err(error) => return api_err(error),
    };
    for entry in diary {
        let agent = row(&mut rows, &entry.agent_name);
        agent.diary_count += 1;
        if agent.last_diary_at.is_none_or(|ts| entry.created_at > ts) {
            agent.last_diary_at = Some(entry.created_at);
        }
    }
    let mut items: Vec<AgentRow> = rows.into_values().collect();
    items.sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then(b.last_call_at.cmp(&a.last_call_at))
            .then(b.last_op_at.cmp(&a.last_op_at))
            .then(a.agent.cmp(&b.agent))
    });
    api_ok(json!({
        "ok": true,
        "online_window_secs": telemetry::ONLINE_WINDOW_SECS,
        "count": items.len(),
        "items": items,
    }))
}
