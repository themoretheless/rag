//! Console routes that mirror MCP tools the web UI needs.
//!
//! Reads: `ops-log`, `taxonomy` / `wings` / `rooms`, `diary`, `kg*`, `tunnels`,
//! `llm-status`, `embedding-manifest`, `lint-wiki`, `eval/history`.
//! Mutations: `ingest/text`, `ingest/file`, `sync-sources`, `PATCH document`,
//! `DELETE document`, `reembed`, `backup`, `vacuum`, `doctor/repair`.
//!
//! Every handler delegates to the same service layer as the MCP tool, so
//! behaviour (allowlists, CAS, manifest fail-closed) is identical on both
//! transports. Mutations are recorded in the in-memory call log as agent `http`.

use std::future::Future;
use std::path::PathBuf;

use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    error::{api_err, api_ok},
    HttpState,
};
use crate::diagnostics::{self, DiagnosticsService};
use crate::error::AppError;
use crate::ingest::{IngestCommand, IngestFileCommand, IngestService, UpdateDocumentCommand};
use crate::llm::ChatClient;
use crate::mcp::facade::{parse_optional_ts, recovery_path, refuse_live_db_target};
use crate::models::DocumentMetaUpdate;
use crate::source_sync::{SourceSyncCommand, SourceSyncService};
use crate::telemetry;
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
        .route("/v1/ingest/text", post(ingest_text))
        .route("/v1/ingest/file", post(ingest_file))
        .route("/v1/sync-sources", post(sync_sources))
        .route("/v1/document", patch(update_document).delete(delete_document))
        .route("/v1/reembed", post(reembed))
        .route("/v1/backup", post(backup))
        .route("/v1/vacuum", post(vacuum))
        .route("/v1/doctor/repair", post(doctor_repair))
}

/// Run a mutation, record it in the call log, and wrap the outcome as JSON.
async fn tracked<T, F>(tool: &str, args: Value, work: F) -> Response
where
    T: Serialize,
    F: Future<Output = Result<T, AppError>>,
{
    let call = telemetry::begin("http", "http", tool, Some(&args));
    match work.await {
        Ok(value) => {
            call.finish(true, None, None);
            api_ok(json!({"ok": true, "result": value}))
        }
        Err(error) => {
            call.finish(false, Some(error.to_string()), None);
            api_err(error)
        }
    }
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
                        row.agent_name.as_deref().is_some_and(|name| name.eq_ignore_ascii_case(a))
                    })
                })
                .filter(|row| {
                    prefix.as_deref().is_none_or(|p| {
                        row.prefix.as_deref().is_some_and(|value| value.eq_ignore_ascii_case(p))
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
    match st.store.list_diary_entries(clean(q.agent).as_deref(), limit) {
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

async fn kg_query(State(st): State<HttpState>, Query(q): Query<KgQueryParams>) -> impl IntoResponse {
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
    std::env::var("RAG_EVAL_HISTORY").ok().filter(|v| !v.trim().is_empty()).map(PathBuf::from)
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

// --- Mutations -------------------------------------------------------------

#[derive(Deserialize)]
struct IngestTextBody {
    text: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    metadata_json: Option<String>,
    #[serde(default)]
    wing: Option<String>,
    #[serde(default)]
    room: Option<String>,
    #[serde(default)]
    source_file: Option<String>,
    /// When true, behaves like `ingest_raw` (same uri + content is a no-op; changed content is refused).
    #[serde(default)]
    immutable: bool,
}

async fn ingest_text(State(st): State<HttpState>, Json(body): Json<IngestTextBody>) -> Response {
    let args = json!({"uri": body.uri, "title": body.title, "wing": body.wing, "room": body.room, "chars": body.text.len()});
    tracked("ingest_text", args, async {
        IngestService::new(&st.store, &st.embedder, &st.config)
            .ingest(IngestCommand {
                text: body.text,
                title: clean(body.title),
                uri: clean(body.uri),
                metadata_json: clean(body.metadata_json),
                wing: clean(body.wing),
                room: clean(body.room),
                source_file: clean(body.source_file),
                layer: "raw".into(),
                kind: "document".into(),
                immutable: body.immutable,
            })
            .await
    })
    .await
}

#[derive(Deserialize)]
struct IngestFileBody {
    path: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    metadata_json: Option<String>,
    #[serde(default)]
    wing: Option<String>,
    #[serde(default)]
    room: Option<String>,
}

async fn ingest_file(State(st): State<HttpState>, Json(body): Json<IngestFileBody>) -> Response {
    let args = json!({"path": body.path, "wing": body.wing, "room": body.room});
    tracked("ingest_file", args, async {
        IngestService::new(&st.store, &st.embedder, &st.config)
            .ingest_file(IngestFileCommand {
                path: body.path,
                title: clean(body.title),
                uri: clean(body.uri),
                metadata_json: clean(body.metadata_json),
                wing: clean(body.wing),
                room: clean(body.room),
            })
            .await
    })
    .await
}

#[derive(Deserialize)]
struct SyncSourcesBody {
    path: String,
    #[serde(default)]
    remove_deleted: bool,
    #[serde(default)]
    wing: Option<String>,
    #[serde(default)]
    room: Option<String>,
}

async fn sync_sources(State(st): State<HttpState>, Json(body): Json<SyncSourcesBody>) -> Response {
    let args = json!({"path": body.path, "remove_deleted": body.remove_deleted, "wing": body.wing, "room": body.room});
    tracked("sync_sources", args, async {
        SourceSyncService::new(&st.store, &st.embedder, &st.config)
            .sync(SourceSyncCommand {
                path: PathBuf::from(body.path),
                remove_deleted: body.remove_deleted,
                wing: clean(body.wing),
                room: clean(body.room),
            })
            .await
    })
    .await
}

/// `update_document_meta` mirror. Empty string clears nullable columns (`wing`, `room`, `source_file`).
#[derive(Deserialize)]
struct UpdateDocumentBody {
    document_id: String,
    #[serde(default)]
    wing: Option<String>,
    #[serde(default)]
    room: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    metadata_json: Option<String>,
    #[serde(default)]
    pinned: Option<bool>,
    #[serde(default)]
    boost: Option<f64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    layer: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    source_file: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

async fn update_document(
    State(st): State<HttpState>,
    Json(body): Json<UpdateDocumentBody>,
) -> Response {
    let args = json!({
        "document_id": body.document_id, "wing": body.wing, "room": body.room,
        "status": body.status, "pinned": body.pinned, "boost": body.boost,
        "content_changed": body.content.is_some(),
    });
    tracked("update_document_meta", args, async {
        IngestService::new(&st.store, &st.embedder, &st.config)
            .update_document(UpdateDocumentCommand {
                document_id: body.document_id,
                update: DocumentMetaUpdate {
                    wing: body.wing,
                    room: body.room,
                    status: body.status,
                    layer: body.layer,
                    kind: body.kind,
                    source_file: body.source_file,
                    title: body.title,
                    metadata_json: body.metadata_json,
                    pinned: body.pinned,
                    boost: body.boost,
                    content: body.content,
                },
            })
            .await
    })
    .await
}

#[derive(Deserialize)]
struct DeleteDocumentQuery {
    id: String,
}

async fn delete_document(
    State(st): State<HttpState>,
    Query(q): Query<DeleteDocumentQuery>,
) -> Response {
    let id = q.id.trim().to_string();
    tracked("delete_document", json!({"document_id": id}), async {
        match st.store.delete_document(&id)? {
            true => Ok(json!({"deleted": true, "document_id": id})),
            false => Err(AppError::not_found(format!("document not found: {id}"))),
        }
    })
    .await
}

#[derive(Deserialize)]
struct ReembedBody {
    document_id: String,
}

async fn reembed(State(st): State<HttpState>, Json(body): Json<ReembedBody>) -> Response {
    tracked("reembed_document", json!({"document_id": body.document_id}), async {
        IngestService::new(&st.store, &st.embedder, &st.config)
            .reembed_document(body.document_id.trim())
            .await
    })
    .await
}

#[derive(Deserialize)]
struct BackupBody {
    path: String,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    overwrite: bool,
}

async fn backup(State(st): State<HttpState>, Json(body): Json<BackupBody>) -> Response {
    let args = json!({"path": body.path, "dry_run": body.dry_run, "overwrite": body.overwrite});
    tracked("backup_db", args, async {
        let path = recovery_path(&body.path, &st.config.ingest_roots)?;
        refuse_live_db_target(&path, st.store.path())?;
        st.store.backup_database(&path, body.dry_run, body.overwrite)
    })
    .await
}

async fn vacuum(State(st): State<HttpState>) -> Response {
    tracked("vacuum_store", json!({}), async { st.store.vacuum_store() }).await
}

#[derive(Deserialize, Default)]
struct DoctorRepairBody {
    #[serde(default)]
    dry_run: Option<bool>,
    #[serde(default)]
    max_docs: Option<usize>,
}

async fn doctor_repair(
    State(st): State<HttpState>,
    body: Option<Json<DoctorRepairBody>>,
) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let dry_run = body.dry_run.unwrap_or(true);
    tracked("doctor_repair", json!({"dry_run": dry_run, "max_docs": body.max_docs}), async {
        DiagnosticsService::new(&st.store, &st.config)
            .repair(&st.embedder, dry_run, body.max_docs)
            .await
    })
    .await
}
