//! Gateway-persisted eval error-label queue with lease + CAS.
//!
//! Replaces JSONL-only labels for interactive regression triage. Items can
//! attach to feedback question ids and eval run ids.

use axum::{
    extract::{Path, Query, State},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    error::{api_err, api_ok},
    HttpState,
};
use crate::error::{AppError, Result};
use crate::eval::ErrorLabel;

const DEFAULT_LEASE_SECS: i64 = 300;
const MAX_LEASE_SECS: i64 = 3_600;
const PAGE_SIZE: i64 = 50;

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/eval/labels", get(list).post(create))
        .route("/v1/eval/labels/claim", post(claim))
        .route("/v1/eval/labels/{id}", get(get_one).put(update))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelStatus {
    Open,
    Claimed,
    Resolved,
    Discarded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelQueueItem {
    pub id: String,
    pub revision: u64,
    pub status: LabelStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<ErrorLabel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct CreateBody {
    #[serde(default)]
    query_id: Option<String>,
    #[serde(default)]
    label: Option<ErrorLabel>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    feedback_id: Option<String>,
    #[serde(default)]
    eval_run_id: Option<String>,
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    offset: u32,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize)]
struct ClaimBody {
    owner: String,
    #[serde(default)]
    lease_secs: Option<i64>,
}

#[derive(Deserialize)]
struct UpdateBody {
    if_match_revision: u64,
    #[serde(default)]
    status: Option<LabelStatus>,
    #[serde(default)]
    label: Option<ErrorLabel>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    feedback_id: Option<String>,
    #[serde(default)]
    eval_run_id: Option<String>,
    /// When set with status=claimed, renews the lease for this owner.
    #[serde(default)]
    lease_owner: Option<String>,
    #[serde(default)]
    lease_secs: Option<i64>,
}

async fn list(State(st): State<HttpState>, Query(q): Query<ListQuery>) -> Response {
    let result = super::run_blocking("list eval labels", move || {
        let status_filter = q
            .status
            .as_deref()
            .map(parse_status)
            .transpose()?;
        let mut items = super::payload_kv::list_payload_pairs(
            &st.store,
            super::payload_kv::PayloadTable::EvalLabelQueue,
        )?
        .into_iter()
        .map(|(_id, payload)| Ok::<_, AppError>(serde_json::from_str::<LabelQueueItem>(&payload)?))
        .collect::<Result<Vec<_>>>()?;
        if let Some(want) = status_filter {
            items.retain(|item| item.status == want);
        }
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| b.id.cmp(&a.id)));
        let total = items.len() as i64;
        let page: Vec<Value> = items
            .into_iter()
            .skip(q.offset as usize)
            .take(PAGE_SIZE as usize)
            .map(|item| serde_json::to_value(item).unwrap_or(Value::Null))
            .collect();
        Ok(json!({"ok": true, "items": page, "total": total, "offset": q.offset}))
    })
    .await;
    match result {
        Ok(v) => api_ok(v),
        Err(e) => api_err(e),
    }
}

async fn create(State(st): State<HttpState>, Json(body): Json<CreateBody>) -> Response {
    let result = super::run_blocking("create eval label", move || {
        let now = Utc::now();
        let item = LabelQueueItem {
            id: Uuid::new_v4().to_string(),
            revision: 1,
            status: LabelStatus::Open,
            query_id: clean(body.query_id),
            label: body.label,
            note: clean(body.note),
            feedback_id: clean(body.feedback_id),
            eval_run_id: clean(body.eval_run_id),
            lease_owner: None,
            lease_until: None,
            created_at: now,
            updated_at: now,
        };
        super::payload_kv::upsert_payload(
            &st.store,
            super::payload_kv::PayloadTable::EvalLabelQueue,
            &item.id,
            &item,
        )?;
        Ok(item)
    })
    .await;
    match result {
        Ok(item) => api_ok(json!({"ok": true, "item": item})),
        Err(e) => api_err(e),
    }
}

async fn get_one(State(st): State<HttpState>, Path(id): Path<String>) -> Response {
    let result = super::run_blocking("get eval label", move || load_item(&st.store, &id)).await;
    match result {
        Ok(item) => api_ok(json!({"ok": true, "item": item})),
        Err(e) => api_err(e),
    }
}

async fn claim(State(st): State<HttpState>, Json(body): Json<ClaimBody>) -> Response {
    let result = super::run_blocking("claim eval label", move || {
        let owner = body.owner.trim();
        if owner.is_empty() || owner.len() > 128 {
            return Err(AppError::config("owner must be 1..=128 chars"));
        }
        let lease_secs = body
            .lease_secs
            .unwrap_or(DEFAULT_LEASE_SECS)
            .clamp(1, MAX_LEASE_SECS);
        let now = Utc::now();
        let conn = st.store.lock()?;
        let mut candidates = super::payload_kv::list_payload_pairs_conn(
            &conn,
            super::payload_kv::PayloadTable::EvalLabelQueue,
        )?
        .into_iter()
        .map(|(id, payload)| Ok::<_, AppError>((id, serde_json::from_str::<LabelQueueItem>(&payload)?)))
        .collect::<Result<Vec<_>>>()?;
        candidates.sort_by(|(_, a), (_, b)| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
        let Some((id, mut item)) = candidates.into_iter().find(|(_, item)| {
            item.status == LabelStatus::Open
                || (item.status == LabelStatus::Claimed
                    && item.lease_until.is_none_or(|until| until <= now))
        }) else {
            return Ok(json!({"ok": true, "item": Value::Null, "claimed": false}));
        };
        item.status = LabelStatus::Claimed;
        item.lease_owner = Some(owner.to_string());
        item.lease_until = Some(now + Duration::seconds(lease_secs));
        item.revision = item.revision.saturating_add(1);
        item.updated_at = now;
        super::payload_kv::upsert_payload_conn(
            &conn,
            super::payload_kv::PayloadTable::EvalLabelQueue,
            &id,
            &item,
        )?;
        Ok(json!({"ok": true, "item": item, "claimed": true}))
    })
    .await;
    match result {
        Ok(v) => api_ok(v),
        Err(e) => api_err(e),
    }
}

async fn update(
    State(st): State<HttpState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Response {
    let result = super::run_blocking("update eval label", move || {
        let conn = st.store.lock()?;
        let payload = super::payload_kv::get_payload_raw_conn(
            &conn,
            super::payload_kv::PayloadTable::EvalLabelQueue,
            &id,
        )?;
        let Some(payload) = payload else {
            return Err(AppError::not_found(format!("label not found: {id}")));
        };
        let mut item: LabelQueueItem = serde_json::from_str(&payload)?;
        if item.revision != body.if_match_revision {
            return Err(AppError::conflict(format!(
                "label revision mismatch: have {}, if_match {}",
                item.revision, body.if_match_revision
            )));
        }
        if let Some(status) = body.status {
            item.status = status;
        }
        if body.label.is_some() {
            item.label = body.label;
        }
        if body.note.is_some() {
            item.note = clean(body.note);
        }
        if body.feedback_id.is_some() {
            item.feedback_id = clean(body.feedback_id);
        }
        if body.eval_run_id.is_some() {
            item.eval_run_id = clean(body.eval_run_id);
        }
        if item.status == LabelStatus::Claimed {
            if let Some(owner) = clean(body.lease_owner) {
                let lease_secs = body
                    .lease_secs
                    .unwrap_or(DEFAULT_LEASE_SECS)
                    .clamp(1, MAX_LEASE_SECS);
                item.lease_owner = Some(owner);
                item.lease_until = Some(Utc::now() + Duration::seconds(lease_secs));
            }
        } else if matches!(item.status, LabelStatus::Resolved | LabelStatus::Discarded | LabelStatus::Open)
        {
            item.lease_owner = None;
            item.lease_until = None;
        }
        item.revision = item.revision.saturating_add(1);
        item.updated_at = Utc::now();
        super::payload_kv::upsert_payload_conn(
            &conn,
            super::payload_kv::PayloadTable::EvalLabelQueue,
            &id,
            &item,
        )?;
        Ok(item)
    })
    .await;
    match result {
        Ok(item) => api_ok(json!({"ok": true, "item": item})),
        Err(e) => api_err(e),
    }
}

fn load_item(store: &crate::db::Store, id: &str) -> Result<LabelQueueItem> {
    let payload = super::payload_kv::get_payload_raw(
        store,
        super::payload_kv::PayloadTable::EvalLabelQueue,
        id,
    )?;
    let Some(payload) = payload else {
        return Err(AppError::not_found(format!("label not found: {id}")));
    };
    Ok(serde_json::from_str(&payload)?)
}

fn parse_status(raw: &str) -> Result<LabelStatus> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "open" => Ok(LabelStatus::Open),
        "claimed" => Ok(LabelStatus::Claimed),
        "resolved" => Ok(LabelStatus::Resolved),
        "discarded" => Ok(LabelStatus::Discarded),
        other => Err(AppError::config(format!("unknown label status: {other}"))),
    }
}

fn clean(value: Option<String>) -> Option<String> {
    value.and_then(|s| {
        let t = s.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::Store;
    use crate::embeddings::MockEmbedder;
    use crate::http_api::jobs::JobRegistry;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn state(root: &std::path::Path) -> HttpState {
        let config = Config {
            db_path: root.join("labels.duckdb"),
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
    async fn create_claim_cas_update_flow() {
        let root = tempfile::tempdir().unwrap();
        let state = state(root.path());
        let app = routes().with_state(state);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/eval/labels")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"query_id":"q1","feedback_id":"fb-1","note":"regress"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let created: Value = serde_json::from_slice(&body).unwrap();
        let id = created["item"]["id"].as_str().unwrap().to_string();
        assert_eq!(created["item"]["revision"], 1);
        assert_eq!(created["item"]["status"], "open");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/eval/labels/claim")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"owner":"agent-a","lease_secs":60}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let claimed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(claimed["claimed"], true);
        assert_eq!(claimed["item"]["id"], id);
        assert_eq!(claimed["item"]["status"], "claimed");
        assert_eq!(claimed["item"]["lease_owner"], "agent-a");
        let rev = claimed["item"]["revision"].as_u64().unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/eval/labels/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "if_match_revision": rev,
                            "status": "resolved",
                            "label": "wrong_top_hit",
                            "eval_run_id": "run-1"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let updated: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(updated["item"]["status"], "resolved");
        assert_eq!(updated["item"]["label"], "wrong_top_hit");
        assert_eq!(updated["item"]["eval_run_id"], "run-1");
        assert!(updated["item"]["lease_owner"].is_null());

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/eval/labels/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"if_match_revision": rev, "status": "discarded"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }
}
