//! Persist and list local retrieval traces for eval debugging.

use axum::{
    extract::{Path, Query, State},
    response::Response,
    routing::get,
    Json, Router,
};
use duckdb::{params, OptionalExt};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    error::{api_err, api_ok},
    HttpState,
};
use crate::error::{AppError, Result};
use crate::eval::RetrievalTrace;

const PAGE_SIZE: i64 = 50;

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/eval/traces", get(list).post(create))
        .route("/v1/eval/traces/{id}", get(get_one))
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    offset: u32,
}

async fn list(State(st): State<HttpState>, Query(q): Query<Page>) -> Response {
    let result = super::run_blocking("list eval traces", move || {
        let conn = st.store.lock()?;
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM eval_traces", [], |r| r.get(0))?;
        let mut stmt = conn.prepare("SELECT id, payload FROM eval_traces")?;
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

async fn create(State(st): State<HttpState>, Json(body): Json<RetrievalTrace>) -> Response {
    let result = super::run_blocking("persist eval trace", move || {
        if body.spans.is_empty() {
            return Err(AppError::config("trace must include at least one span"));
        }
        if body.id.trim().is_empty() {
            return Err(AppError::config("trace id must not be empty"));
        }
        let conn = st.store.lock()?;
        conn.execute(
            "INSERT OR REPLACE INTO eval_traces VALUES (?,?)",
            params![body.id, serde_json::to_string(&body)?],
        )?;
        Ok(body)
    })
    .await;
    match result {
        Ok(trace) => api_ok(json!({"ok": true, "trace": trace})),
        Err(e) => api_err(e),
    }
}

async fn get_one(State(st): State<HttpState>, Path(id): Path<String>) -> Response {
    let result = super::run_blocking("get eval trace", move || {
        let conn = st.store.lock()?;
        let payload: Option<String> = conn
            .query_row("SELECT payload FROM eval_traces WHERE id=?", [&id], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(payload) = payload else {
            return Err(AppError::not_found(format!("trace not found: {id}")));
        };
        let trace: RetrievalTrace = serde_json::from_str(&payload)?;
        Ok(trace)
    })
    .await;
    match result {
        Ok(trace) => api_ok(json!({"ok": true, "trace": trace})),
        Err(e) => api_err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::Store;
    use crate::embeddings::MockEmbedder;
    use crate::eval::{RetrievalTrace, SpanBuilder};
    use crate::http_api::jobs::JobRegistry;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn state(root: &std::path::Path) -> HttpState {
        let config = Config {
            db_path: root.join("traces.duckdb"),
            embedding_dims: 16,
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
    async fn persist_and_get_trace() {
        let root = tempfile::tempdir().unwrap();
        let state = state(root.path());
        let mut trace = RetrievalTrace::new("search_pack");
        trace.push(SpanBuilder::start("search").attr("hit_count", 1).finish());
        let app = routes().with_state(state);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/eval/traces")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&trace).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/eval/traces/{}", trace.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let got: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(got["trace"]["id"], trace.id);
        assert_eq!(got["trace"]["spans"].as_array().unwrap().len(), 1);
    }
}
