//! Prompt versions, answer judge, online samples, export, and offline replay.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use duckdb::{params, OptionalExt};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    error::{api_err, api_ok},
    feedback::persist_eval_run,
    retrieval::{search_http, SearchBody},
    HttpState,
};
use crate::db::Store;
use crate::error::{AppError, Result};
use crate::eval::{
    builtin_answer_judge, judge_answer_heuristic, judge_answer_llm, merge_llm_scores,
    replay_bundle, replay_run_payload, validate_prompt, AnswerJudgeInput, AnswerJudgeResult,
    EvalExportBundle, PromptVersion,
};
use crate::llm::ChatClient;

const PAGE_SIZE: i64 = 50;
const EXPORT_CAP: usize = 200;

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/eval/prompts", get(list_prompts).post(create_prompt))
        .route("/v1/eval/prompts/{id}", get(get_prompt))
        .route("/v1/eval/judge", post(judge))
        .route("/v1/eval/online", post(online))
        .route("/v1/eval/export", get(export))
        .route("/v1/eval/replay", post(replay))
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    offset: u32,
}

#[derive(Deserialize)]
struct CreatePrompt {
    name: String,
    version: String,
    #[serde(default = "default_kind")]
    kind: String,
    body: String,
}

fn default_kind() -> String {
    "answer_judge".into()
}

#[derive(Deserialize)]
struct JudgeBody {
    #[serde(default)]
    question: String,
    #[serde(default)]
    answer_text: String,
    #[serde(default)]
    cited_document_ids: Vec<String>,
    #[serde(default)]
    expected_document_ids: Vec<String>,
    #[serde(default)]
    gold_answer: Option<String>,
    #[serde(default)]
    context_text: Option<String>,
    #[serde(default)]
    use_llm: bool,
    #[serde(default)]
    persist: bool,
    #[serde(default)]
    prompt_id: Option<String>,
}

#[derive(Deserialize)]
struct OnlineBody {
    search: SearchBody,
    #[serde(default)]
    answer_text: Option<String>,
    #[serde(default)]
    expected_document_ids: Vec<String>,
    #[serde(default)]
    gold_answer: Option<String>,
    #[serde(default)]
    use_llm: bool,
    #[serde(default)]
    prompt_id: Option<String>,
}

#[derive(Deserialize)]
struct ExportQuery {
    #[serde(default)]
    include: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct ReplayBody {
    bundle: EvalExportBundle,
    #[serde(default)]
    persist: bool,
}

async fn list_prompts(State(st): State<HttpState>, Query(q): Query<Page>) -> Response {
    let result = super::run_blocking("list eval prompts", move || {
        seed_builtin_prompt(&st.store)?;
        let conn = st.store.lock()?;
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM eval_prompts", [], |r| r.get(0))?;
        let mut stmt = conn.prepare("SELECT id, payload FROM eval_prompts")?;
        let mut items = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .map(|r| {
                let (id, payload) = r?;
                let mut value = serde_json::from_str::<Value>(&payload)?;
                if let Some(obj) = value.as_object_mut() {
                    obj.entry("id").or_insert(json!(id));
                }
                Ok::<_, AppError>(value)
            })
            .collect::<Result<Vec<_>>>()?;
        items.sort_by(|a, b| {
            let na = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let nb = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
            na.cmp(nb).then_with(|| {
                let va = a.get("version").and_then(|v| v.as_str()).unwrap_or("");
                let vb = b.get("version").and_then(|v| v.as_str()).unwrap_or("");
                va.cmp(vb)
            })
        });
        let page: Vec<Value> = items
            .into_iter()
            .skip(q.offset as usize)
            .take(PAGE_SIZE as usize)
            .collect();
        Ok(json!({"ok": true, "items": page, "total": total, "offset": q.offset}))
    })
    .await;
    match result {
        Ok(v) => api_ok(v),
        Err(e) => api_err(e),
    }
}

async fn create_prompt(State(st): State<HttpState>, Json(body): Json<CreatePrompt>) -> Response {
    let result = super::run_blocking("create eval prompt", move || {
        validate_prompt(&body.name, &body.version, &body.kind, &body.body)?;
        let prompt = PromptVersion::new(body.name, body.version, body.kind, body.body, false);
        upsert_prompt(&st.store, &prompt)?;
        Ok(prompt)
    })
    .await;
    match result {
        Ok(prompt) => api_ok(json!({"ok": true, "prompt": prompt})),
        Err(e) => api_err(e),
    }
}

async fn get_prompt(State(st): State<HttpState>, Path(id): Path<String>) -> Response {
    let result = super::run_blocking("get eval prompt", move || {
        seed_builtin_prompt(&st.store)?;
        load_prompt(&st.store, &id)?
            .ok_or_else(|| AppError::not_found(format!("prompt not found: {id}")))
    })
    .await;
    match result {
        Ok(prompt) => api_ok(json!({"ok": true, "prompt": prompt})),
        Err(e) => api_err(e),
    }
}

async fn judge(State(st): State<HttpState>, Json(body): Json<JudgeBody>) -> Response {
    match judge_one(st, body).await {
        Ok(v) => api_ok(v),
        Err(e) => api_err(e),
    }
}

async fn judge_one(st: HttpState, body: JudgeBody) -> Result<Value> {
    if body.question.trim().is_empty() && body.answer_text.trim().is_empty() {
        return Err(AppError::config(
            "judge requires a question or answer_text",
        ));
    }
    let input = AnswerJudgeInput {
        question: body.question.clone(),
        answer_text: body.answer_text.clone(),
        cited_document_ids: body.cited_document_ids.clone(),
        expected_document_ids: body.expected_document_ids.clone(),
        gold_answer: body.gold_answer.clone(),
        context_text: body.context_text.clone(),
    };
    let mut judged = judge_answer_heuristic(&input);
    if body.use_llm {
        if let Some(enriched) = maybe_llm_judge(&st, input, body.prompt_id.as_deref()).await? {
            judged = enriched;
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let stored = json!({
        "kind": "answer_judge",
        "id": id,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "question": body.question,
        "answer_text": body.answer_text,
        "cited_document_ids": body.cited_document_ids,
        "expected_document_ids": body.expected_document_ids,
        "gold_answer": body.gold_answer,
        "context_text": body.context_text,
        "answer_judge": judged,
    });
    if body.persist {
        persist_eval_run(&st.store, &stored)?;
    }
    Ok(json!({"ok": true, "id": id, "persisted": body.persist, "answer_judge": judged}))
}

async fn online(State(st): State<HttpState>, Json(body): Json<OnlineBody>) -> Response {
    match online_one(st, body).await {
        Ok(v) => api_ok(v),
        Err(e) => api_err(e),
    }
}

async fn online_one(st: HttpState, body: OnlineBody) -> Result<Value> {
    let question = body.search.query.clone();
    if question.trim().is_empty() {
        return Err(AppError::config("online eval requires a search query"));
    }
    let response = search_http(State(st.clone()), Json(body.search.clone()))
        .await
        .into_response();
    if !response.status().is_success() {
        return Err(search_failed(response).await);
    }
    let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .map_err(|e| AppError::config(e.to_string()))?;
    let search_result: Value = serde_json::from_slice(&bytes)?;
    let hits = search_result["items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let cited_ids: Vec<String> = hits
        .iter()
        .filter_map(|h| h.get("document_id").and_then(|v| v.as_str()))
        .map(str::to_string)
        .collect();
    let input = AnswerJudgeInput {
        question: question.clone(),
        answer_text: body.answer_text.clone().unwrap_or_default(),
        cited_document_ids: cited_ids.clone(),
        expected_document_ids: body.expected_document_ids.clone(),
        gold_answer: body.gold_answer.clone(),
        context_text: None,
    };
    let mut judged = judge_answer_heuristic(&input);
    if body.use_llm {
        if let Some(enriched) = maybe_llm_judge(&st, input, body.prompt_id.as_deref()).await? {
            judged = enriched;
        }
    }
    let citation_judge = judged.citation.clone();
    let id = uuid::Uuid::new_v4().to_string();
    let payload = json!({
        "kind": "online_eval",
        "id": id,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "question": question,
        "answer_text": body.answer_text,
        "cited_document_ids": cited_ids,
        "expected_document_ids": body.expected_document_ids,
        "gold_answer": body.gold_answer,
        "mode": search_result["mode"],
        "result_count": hits.len(),
        "settings": body.search,
        "answer_judge": judged,
        "citation_judge": citation_judge,
        "scope": "explicit online sample; not automatic query logging; mid-batch embed resume is not claimed"
    });
    persist_eval_run(&st.store, &payload)?;
    Ok(json!({"ok": true, "id": id, "answer_judge": judged, "result_count": hits.len()}))
}

async fn export(State(st): State<HttpState>, Query(q): Query<ExportQuery>) -> Response {
    let result = super::run_blocking("export eval bundle", move || {
        seed_builtin_prompt(&st.store)?;
        let include = q.include.unwrap_or_else(|| "runs,traces,prompts,feedback".into());
        let limit = q.limit.unwrap_or(EXPORT_CAP).min(EXPORT_CAP);
        let want = |name: &str| include.split(',').any(|p| p.trim() == name || p.trim() == "all");
        let prompts = if want("prompts") {
            list_table_payloads(&st.store, "eval_prompts", limit)?
        } else {
            Vec::new()
        };
        let runs = if want("runs") {
            list_table_payloads(&st.store, "eval_runs", limit)?
        } else {
            Vec::new()
        };
        let traces = if want("traces") {
            list_table_payloads(&st.store, "eval_traces", limit)?
        } else {
            Vec::new()
        };
        let feedback = if want("feedback") {
            list_table_payloads(&st.store, "search_feedback", limit)?
        } else {
            Vec::new()
        };
        Ok(EvalExportBundle::new(prompts, runs, traces, feedback))
    })
    .await;
    match result {
        Ok(bundle) => api_ok(json!({"ok": true, "bundle": bundle})),
        Err(e) => api_err(e),
    }
}

async fn replay(State(st): State<HttpState>, Json(body): Json<ReplayBody>) -> Response {
    match replay_one(st, body).await {
        Ok(v) => api_ok(v),
        Err(e) => api_err(e),
    }
}

async fn replay_one(st: HttpState, body: ReplayBody) -> Result<Value> {
    let report = replay_bundle(&body.bundle)?;
    if body.persist {
        persist_eval_run(&st.store, &replay_run_payload(&report))?;
    }
    Ok(json!({"ok": true, "persisted": body.persist, "report": report}))
}

/// Optional LLM enrichment. Returns `None` when disabled or the client cannot be built.
pub(super) async fn maybe_llm_judge(
    st: &HttpState,
    input: AnswerJudgeInput,
    prompt_id: Option<&str>,
) -> Result<Option<AnswerJudgeResult>> {
    if !st.config.llm_enabled {
        return Ok(None);
    }
    let prompt = if let Some(id) = prompt_id {
        load_prompt(&st.store, id)?
            .ok_or_else(|| AppError::not_found(format!("prompt not found: {id}")))?
    } else {
        seed_builtin_prompt(&st.store)?;
        builtin_answer_judge()
    };
    let client = ChatClient::from_config(&st.config)?;
    let llm = judge_answer_llm(&client, &prompt, &input).await?;
    let heuristic = judge_answer_heuristic(&input);
    Ok(Some(merge_llm_scores(heuristic, llm, &prompt)))
}

fn seed_builtin_prompt(store: &Store) -> Result<()> {
    let builtin = builtin_answer_judge();
    upsert_prompt(store, &builtin)
}

fn upsert_prompt(store: &Store, prompt: &PromptVersion) -> Result<()> {
    let conn = store.lock()?;
    conn.execute(
        "INSERT OR REPLACE INTO eval_prompts VALUES (?,?)",
        params![prompt.id, serde_json::to_string(prompt)?],
    )?;
    Ok(())
}

fn load_prompt(store: &Store, id: &str) -> Result<Option<PromptVersion>> {
    let conn = store.lock()?;
    let payload: Option<String> = conn
        .query_row("SELECT payload FROM eval_prompts WHERE id=?", [id], |r| {
            r.get(0)
        })
        .optional()?;
    match payload {
        Some(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        None => {
            let builtin = builtin_answer_judge();
            if builtin.id == id {
                Ok(Some(builtin))
            } else {
                Ok(None)
            }
        }
    }
}

fn list_table_payloads(store: &Store, table: &str, limit: usize) -> Result<Vec<Value>> {
    let conn = store.lock()?;
    let sql = format!("SELECT id, payload FROM {table}");
    let mut stmt = conn.prepare(&sql)?;
    let mut items = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .map(|r| {
            let (id, payload) = r?;
            let mut value = serde_json::from_str::<Value>(&payload)?;
            if let Some(obj) = value.as_object_mut() {
                obj.entry("id").or_insert(json!(id));
            }
            Ok::<_, AppError>(value)
        })
        .collect::<Result<Vec<_>>>()?;
    items.sort_by(|a, b| {
        let ca = a.get("created_at").and_then(|v| v.as_str());
        let cb = b.get("created_at").and_then(|v| v.as_str());
        match (ca, cb) {
            (Some(a), Some(b)) => b.cmp(a),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });
    items.truncate(limit);
    Ok(items)
}

async fn search_failed(response: axum::response::Response) -> AppError {
    let status = response.status();
    let bytes = match axum::body::to_bytes(response.into_body(), 65536).await {
        Ok(bytes) => bytes,
        Err(e) => return AppError::config(e.to_string()),
    };
    AppError::config(format!(
        "search failed ({status}): {}",
        String::from_utf8_lossy(&bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::embeddings::MockEmbedder;
    use crate::http_api::jobs::JobRegistry;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn state(root: &std::path::Path) -> HttpState {
        let config = Config {
            db_path: root.join("eval-ops.duckdb"),
            embedding_dims: 16,
            llm_enabled: false,
            ..Config::for_tests()
        };
        HttpState {
            store: Arc::new(Store::open(&config.db_path).unwrap()),
            mcp_http: false,
            embedder: Arc::new(MockEmbedder::new(16)),
            config,
            jobs: JobRegistry::default(),
        }
    }

    #[tokio::test]
    async fn builtin_prompt_lists_and_judge_persists() {
        let root = tempfile::tempdir().unwrap();
        let state = state(root.path());
        crate::wiki::write_wiki_page(
            &state.store,
            &state.embedder,
            &state.config,
            "alpha",
            "Alpha",
            "alpha unique source",
            "wiki",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let app = routes().with_state(state.clone());

        let listed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/eval/prompts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let body = to_bytes(listed.into_body(), 16 * 1024).await.unwrap();
        let got: Value = serde_json::from_slice(&body).unwrap();
        assert!(got["total"].as_i64().unwrap() >= 1);
        assert_eq!(got["items"][0]["name"], "answer_judge");

        let judged = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/eval/judge")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "question": "what is alpha?",
                            "answer_text": "alpha unique source ## Citations",
                            "cited_document_ids": ["x"],
                            "expected_document_ids": ["x"],
                            "gold_answer": "alpha unique source",
                            "persist": true
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(judged.status(), StatusCode::OK);

        let exported = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/eval/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(exported.status(), StatusCode::OK);
        let export_body = to_bytes(exported.into_body(), 64 * 1024).await.unwrap();
        let export_json: Value = serde_json::from_slice(&export_body).unwrap();
        assert_eq!(export_json["bundle"]["version"], 1);
        assert!(!export_json["bundle"]["runs"].as_array().unwrap().is_empty());

        let replayed = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/eval/replay")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"bundle": export_json["bundle"], "persist": false}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replayed.status(), StatusCode::OK);
        let replay_body = to_bytes(replayed.into_body(), 64 * 1024).await.unwrap();
        let replay_json: Value = serde_json::from_slice(&replay_body).unwrap();
        assert_eq!(replay_json["report"]["kind"], "replay");
    }
}
