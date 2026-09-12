//! Explicitly saved, human-labelled search questions; never automatic query logging.
use super::{
    error::{api_err, api_ok},
    retrieval::{search_http, SearchBody},
    HttpState,
};
use crate::{
    db::Store,
    error::{AppError, Result},
    util::content_hash,
};
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use duckdb::{params, OptionalExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/eval/feedback", get(list).post(create))
        .route("/v1/eval/feedback/run", post(run))
        .route("/v1/eval/runs", get(list_runs))
        .route("/v1/eval/runs/compare", post(compare))
}
#[derive(Serialize, Deserialize, Clone)]
struct Expected {
    document_id: String,
    uri: String,
    hash: String,
}
#[derive(Serialize, Deserialize, Clone)]
struct Feedback {
    id: String,
    search: Value,
    expected: Vec<Expected>,
    no_answer: bool,
    saved_at: String,
}
#[derive(Deserialize)]
struct Capture {
    id: String,
    search: Value,
    expected: Vec<String>,
    #[serde(default)]
    no_answer: bool,
}
#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    offset: u32,
}
#[derive(Deserialize)]
struct CompareBody {
    settings_a: Value,
    settings_b: Value,
    #[serde(default)]
    question_ids: Option<Vec<String>>,
}
async fn list(State(st): State<HttpState>, Query(q): Query<Page>) -> Response {
    let result = super::run_blocking("list feedback", move || {
        let conn = st.store.lock()?;
        let total: i64 =
            conn.query_row("SELECT COUNT(*) FROM search_feedback", [], |r| r.get(0))?;
        let mut stmt =
            conn.prepare("SELECT payload FROM search_feedback ORDER BY id LIMIT 50 OFFSET ?")?;
        let items = stmt
            .query_map([q.offset as i64], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str::<Value>(&r?)?))
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"items":items,"total":total,"offset":q.offset}))
    })
    .await;
    match result {
        Ok(v) => api_ok(v),
        Err(e) => api_err(e),
    }
}
fn capture(store: &Store, body: Capture) -> Result<Feedback> {
    if uuid::Uuid::parse_str(&body.id).is_err()
        || body.expected.len() > 100
        || body.no_answer == !body.expected.is_empty()
    {
        return Err(AppError::config(
            "provide expected sources or mark no_answer; id must be UUID",
        ));
    }
    let search: SearchBody =
        serde_json::from_value(body.search).map_err(|e| AppError::config(e.to_string()))?;
    let search = serde_json::to_value(search)?;
    let query = search["query"].as_str().unwrap_or("");
    if query.trim().is_empty() || query.len() > 8000 {
        return Err(AppError::config("query must contain 1..8000 bytes"));
    }
    let mut expected = Vec::new();
    for key in body.expected {
        let doc = store
            .get_document(&key)?
            .or(store.find_by_uri(&key)?)
            .ok_or_else(|| AppError::not_found(format!("expected source not found: {key}")))?;
        if !expected.iter().any(|e: &Expected| e.document_id == doc.id) {
            expected.push(Expected {
                document_id: doc.id,
                uri: doc.uri,
                hash: content_hash(&doc.content),
            });
        }
    }
    expected.sort_by(|a, b| a.document_id.cmp(&b.document_id));
    let item = Feedback {
        id: body.id,
        search,
        expected,
        no_answer: body.no_answer,
        saved_at: chrono::Utc::now().to_rfc3339(),
    };
    let conn = store.lock()?;
    let prior: Option<String> = conn
        .query_row(
            "SELECT payload FROM search_feedback WHERE id=?",
            [&item.id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(prior) = prior {
        let prior: Feedback = serde_json::from_str(&prior)?;
        if prior.search != item.search
            || prior.no_answer != item.no_answer
            || serde_json::to_value(&prior.expected)? != serde_json::to_value(&item.expected)?
        {
            return Err(AppError::conflict(
                "capture id already belongs to a different evaluation",
            ));
        }
        return Ok(prior);
    }
    conn.execute(
        "INSERT INTO search_feedback VALUES (?,?)",
        params![item.id, serde_json::to_string(&item)?],
    )?;
    Ok(item)
}
async fn create(State(st): State<HttpState>, Json(body): Json<Capture>) -> Response {
    match super::run_blocking("save labelled question", move || capture(&st.store, body)).await {
        Ok(item) => api_ok(item),
        Err(e) => api_err(e),
    }
}
#[derive(Deserialize)]
struct Run {
    id: String,
    #[serde(default)]
    answer_text: Option<String>,
    #[serde(default)]
    use_llm: bool,
}
fn stale(store: &Store, item: &Feedback) -> Result<bool> {
    for e in &item.expected {
        let doc = store
            .get_document(&e.document_id)?
            .or(store.find_by_uri(&e.uri)?);
        if doc.map(|d| content_hash(&d.content)) != Some(e.hash.clone()) {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(super) fn persist_eval_run(store: &Store, payload: &Value) -> Result<()> {
    let id = payload
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let conn = store.lock()?;
    conn.execute(
        "INSERT OR REPLACE INTO eval_runs VALUES (?,?)",
        params![id, serde_json::to_string(payload)?],
    )?;
    Ok(())
}
fn score_hits(item: &Feedback, hits: &[Value]) -> (Option<f64>, Option<f64>) {
    if item.no_answer {
        return (None, None);
    }
    let found = item
        .expected
        .iter()
        .filter(|e| {
            hits.iter()
                .any(|h| h["document_id"] == e.document_id || h["document_uri"] == e.uri)
        })
        .count();
    let rank = hits.iter().position(|h| {
        item.expected
            .iter()
            .any(|e| h["document_id"] == e.document_id || h["document_uri"] == e.uri)
    });
    let recall = found as f64 / item.expected.len().max(1) as f64;
    let mrr = rank.map(|r| 1.0 / (r + 1) as f64).unwrap_or(0.0);
    (Some(recall), Some(mrr))
}
fn merge_search(base: &Value, overlay: &Value) -> Result<SearchBody> {
    let mut merged = match base.as_object() {
        Some(obj) => obj.clone(),
        None => Map::new(),
    };
    if let Some(over) = overlay.as_object() {
        for (k, v) in over {
            merged.insert(k.clone(), v.clone());
        }
    }
    serde_json::from_value(Value::Object(merged)).map_err(|e| AppError::config(e.to_string()))
}
async fn search_with_body(st: HttpState, body: SearchBody) -> Result<(Value, Vec<Value>)> {
    let response = search_http(State(st), Json(body)).await.into_response();
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .map_err(|e| AppError::config(e.to_string()))?;
    let result: Value = serde_json::from_slice(&bytes)?;
    let hits = result["items"]
        .as_array()
        .cloned()
        .ok_or_else(|| AppError::config("missing search items"))?;
    Ok((result, hits))
}
async fn list_runs(State(st): State<HttpState>, Query(q): Query<Page>) -> Response {
    let result = super::run_blocking("list eval runs", move || {
        let conn = st.store.lock()?;
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM eval_runs", [], |r| r.get(0))?;
        let mut stmt = conn.prepare("SELECT id, payload FROM eval_runs")?;
        let mut items = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .map(|r| {
                let (id, payload) = r?;
                Ok::<_, AppError>((id, serde_json::from_str::<Value>(&payload)?))
            })
            .collect::<Result<Vec<_>>>()?;
        items.sort_by(|(id_a, a), (id_b, b)| {
            let ca = a.get("created_at").and_then(|v| v.as_str());
            let cb = b.get("created_at").and_then(|v| v.as_str());
            match (ca, cb) {
                (Some(a), Some(b)) => b.cmp(a).then_with(|| id_b.cmp(id_a)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => id_b.cmp(id_a),
            }
        });
        let page: Vec<Value> = items
            .into_iter()
            .skip(q.offset as usize)
            .take(50)
            .map(|(_, v)| v)
            .collect();
        Ok(json!({"items":page,"total":total,"offset":q.offset}))
    })
    .await;
    match result {
        Ok(v) => api_ok(v),
        Err(e) => api_err(e),
    }
}
fn load_feedback_questions(store: &Store, ids: Option<&[String]>) -> Result<Vec<Feedback>> {
    let conn = store.lock()?;
    let mut stmt = conn.prepare("SELECT id, payload FROM search_feedback")?;
    let items = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .map(|r| {
            let (_id, payload) = r?;
            Ok(serde_json::from_str::<Feedback>(&payload)?)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(match ids {
        Some(filter) if !filter.is_empty() => items
            .into_iter()
            .filter(|item| filter.iter().any(|id| id == &item.id))
            .collect(),
        _ => items,
    })
}
async fn compare(State(st): State<HttpState>, Json(body): Json<CompareBody>) -> Response {
    match compare_runs(st, body).await {
        Ok(report) => api_ok(report),
        Err(e) => api_err(e),
    }
}
async fn compare_runs(st: HttpState, body: CompareBody) -> Result<Value> {
    let questions = load_feedback_questions(&st.store, body.question_ids.as_deref())?;
    let mut results = Vec::new();
    let mut regressions = Vec::new();
    const EPSILON: f64 = 1e-9;
    for item in &questions {
        let body_a = merge_search(&item.search, &body.settings_a)?;
        let body_b = merge_search(&item.search, &body.settings_b)?;
        let (result_a, hits_a) = search_with_body(st.clone(), body_a).await?;
        let (result_b, hits_b) = search_with_body(st.clone(), body_b).await?;
        let (recall_a, mrr_a) = score_hits(item, &hits_a);
        let (recall_b, mrr_b) = score_hits(item, &hits_b);
        let side_a = json!({
            "recall": recall_a,
            "mrr": mrr_a,
            "result_count": hits_a.len(),
            "mode": result_a["mode"],
        });
        let side_b = json!({
            "recall": recall_b,
            "mrr": mrr_b,
            "result_count": hits_b.len(),
            "mode": result_b["mode"],
        });
        for (metric, a, b) in [("recall", recall_a, recall_b), ("mrr", mrr_a, mrr_b)] {
            if let (Some(av), Some(bv)) = (a, b) {
                let delta = bv - av;
                if delta < -EPSILON {
                    regressions.push(json!({
                        "question_id": item.id,
                        "metric": metric,
                        "side_a": av,
                        "side_b": bv,
                        "delta": delta,
                    }));
                }
            }
        }
        results.push(json!({
            "question_id": item.id,
            "side_a": side_a,
            "side_b": side_b,
        }));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = chrono::Utc::now().to_rfc3339();
    let report = json!({
        "kind": "feedback_compare",
        "id": id,
        "created_at": created_at,
        "settings_a": body.settings_a,
        "settings_b": body.settings_b,
        "question_ids": questions.iter().map(|q| &q.id).collect::<Vec<_>>(),
        "results": results,
        "regressions": regressions,
    });
    persist_eval_run(&st.store, &report)?;
    Ok(report)
}
async fn run(State(st): State<HttpState>, Json(body): Json<Run>) -> Response {
    let result = run_one(st, body.id, body.answer_text, body.use_llm).await;
    match result {
        Ok(result) => api_ok(result),
        Err(e) => api_err(e),
    }
}
async fn run_one(
    st: HttpState,
    id: String,
    answer_text: Option<String>,
    use_llm: bool,
) -> Result<Value> {
    let item: Feedback = {
        let conn = st.store.lock()?;
        let payload: Option<String> = conn
            .query_row(
                "SELECT payload FROM search_feedback WHERE id=?",
                [&id],
                |r| r.get(0),
            )
            .optional()?;
        serde_json::from_str(&payload.ok_or_else(|| AppError::not_found("question not found"))?)?
    };
    if stale(&st.store, &item)? {
        let result = json!({
            "id": id,
            "status": "source_changed",
            "hint": "Recheck labels against current sources"
        });
        persist_feedback_run(&st.store, &id, &item.search, &result)?;
        return Ok(result);
    }
    let (search_result, hits) = search_with_body(
        st.clone(),
        serde_json::from_value(item.search.clone())?,
    )
    .await?;
    if stale(&st.store, &item)? {
        let result = json!({"id":id,"status":"source_changed"});
        persist_feedback_run(&st.store, &id, &item.search, &result)?;
        return Ok(result);
    }
    let (recall, mrr) = score_hits(&item, &hits);
    let cited_ids: Vec<String> = hits
        .iter()
        .map(|h| {
            h.get("document_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();
    let expected_ids: Vec<String> = item
        .expected
        .iter()
        .map(|e| e.document_id.clone())
        .collect();
    let question = item.search["query"].as_str().unwrap_or("").to_string();
    let judge_input = crate::eval::AnswerJudgeInput {
        question: question.clone(),
        answer_text: answer_text.clone().unwrap_or_default(),
        cited_document_ids: cited_ids.clone(),
        expected_document_ids: expected_ids.clone(),
        gold_answer: None,
        context_text: None,
    };
    let mut answer_judge = crate::eval::judge_answer_heuristic(&judge_input);
    if use_llm {
        if let Some(enriched) = super::eval_ops::maybe_llm_judge(&st, judge_input, None).await? {
            answer_judge = enriched;
        }
    }
    let citation_judge = answer_judge.citation.clone();
    let result = json!({
        "id": id,
        "status": "evaluated",
        "run_at": chrono::Utc::now().to_rfc3339(),
        "question": question,
        "answer_text": answer_text,
        "cited_document_ids": cited_ids,
        "expected_document_ids": expected_ids,
        "mode": search_result["mode"],
        "recall": recall,
        "mrr": mrr,
        "citation_judge": citation_judge,
        "answer_judge": answer_judge,
        "empty_result_for_no_answer": item.no_answer.then_some(hits.is_empty()),
        "result_count": hits.len(),
        "timings": search_result["timings"],
        "embedding_manifest": st.store.get_embedding_manifest()?,
        "scope": "current corpus; only supplied positive labels; empty retrieval is not answer correctness; citation_judge is deterministic expected-doc coverage; answer_judge is heuristic unless use_llm and RAG_LLM_ENABLED"
    });
    persist_feedback_run(&st.store, &id, &item.search, &result)?;
    Ok(result)
}
fn persist_feedback_run(store: &Store, question_id: &str, settings: &Value, result: &Value) -> Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let payload = json!({
        "kind": "feedback_run",
        "id": id,
        "question_id": question_id,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "result": result,
        "settings": settings,
    });
    persist_eval_run(store, &payload)
}
async fn response_error(response: Response) -> AppError {
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
    use crate::{config::Config, embeddings::MockEmbedder};
    use std::sync::Arc;
    async fn fixture() -> (HttpState, String) {
        let root = tempfile::tempdir().unwrap().keep();
        let config = Config {
            db_path: root.join("feedback.duckdb"),
            embedding_dims: 16,
            ..Config::for_tests()
        };
        let store = Arc::new(Store::open(&config.db_path).unwrap());
        let st = HttpState::new(store, false, config, Arc::new(MockEmbedder::new(16)));
        let p = crate::wiki::write_wiki_page(
            &st.store,
            &st.embedder,
            &st.config,
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
        (st, p.document_id)
    }
    fn input(id: String, expected: Vec<String>, no_answer: bool) -> Capture {
        Capture {
            id,
            search: json!({"query":"alpha","mode":"lex","top_k":5}),
            expected,
            no_answer,
        }
    }
    #[tokio::test]
    async fn explicit_feedback_is_idempotent_and_replays_with_labels() {
        let (st, doc) = fixture().await;
        let id = uuid::Uuid::new_v4().to_string();
        let a = capture(&st.store, input(id.clone(), vec![doc.clone()], false)).unwrap();
        let b = capture(&st.store, input(id.clone(), vec![doc.clone()], false)).unwrap();
        assert_eq!(a.saved_at, b.saved_at);
        let report = run_one(st.clone(), id.clone(), None, false).await.unwrap();
        assert_eq!(report["recall"], 1.0);
        assert_eq!(report["mrr"], 1.0);
        let mut source = st.store.get_document(&doc).unwrap().unwrap();
        source.content = "changed".into();
        st.store.upsert_document(&source).unwrap();
        assert_eq!(run_one(st, id, None, false).await.unwrap()["status"], "source_changed");
    }
    #[tokio::test]
    async fn negative_questions_are_not_counted_as_positive_recall() {
        let (st, doc) = fixture().await;
        assert!(capture(
            &st.store,
            input(uuid::Uuid::new_v4().to_string(), vec![], false)
        )
        .is_err());
        assert!(capture(
            &st.store,
            input(uuid::Uuid::new_v4().to_string(), vec![doc], true)
        )
        .is_err());
        let item = capture(
            &st.store,
            input(uuid::Uuid::new_v4().to_string(), vec![], true),
        )
        .unwrap();
        let report = run_one(st, item.id, None, false).await.unwrap();
        assert!(report["recall"].is_null());
        assert_eq!(report["empty_result_for_no_answer"], false);
    }
    #[tokio::test]
    async fn feedback_fixture_survives_reopen() {
        let (st, doc) = fixture().await;
        let item = capture(
            &st.store,
            input(uuid::Uuid::new_v4().to_string(), vec![doc], false),
        )
        .unwrap();
        let path = st.config.db_path.clone();
        drop(st);
        let store = Store::open(&path).unwrap();
        let count: i64 = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM search_feedback WHERE id=?",
                [item.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        eprintln!("feedback fixture: {}", path.display());
    }
    #[tokio::test]
    async fn run_persists_eval_run_row() {
        let (st, doc) = fixture().await;
        let id = uuid::Uuid::new_v4().to_string();
        capture(&st.store, input(id.clone(), vec![doc], false)).unwrap();
        let report = run_one(st.clone(), id.clone(), None, false).await.unwrap();
        assert_eq!(report["status"], "evaluated");
        let conn = st.store.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM eval_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let payload: String = conn
            .query_row("SELECT payload FROM eval_runs LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let stored: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(stored["kind"], "feedback_run");
        assert_eq!(stored["question_id"], id);
        assert_eq!(stored["result"]["recall"], 1.0);
    }
    #[tokio::test]
    async fn compare_produces_regressions_field() {
        let (st, doc) = fixture().await;
        let id = uuid::Uuid::new_v4().to_string();
        capture(&st.store, input(id.clone(), vec![doc], false)).unwrap();
        let report = compare_runs(
            st.clone(),
            CompareBody {
                settings_a: json!({"mode":"lex","top_k":5}),
                settings_b: json!({"query":"zzz-no-match-xyz","mode":"lex","top_k":5}),
                question_ids: Some(vec![id.clone()]),
            },
        )
        .await
        .unwrap();
        assert_eq!(report["kind"], "feedback_compare");
        assert!(report["regressions"].as_array().unwrap().len() >= 1);
        assert!(report["regressions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["question_id"] == id && r["metric"] == "recall"));
        let count: i64 = st
            .store
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM eval_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
