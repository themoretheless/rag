//! `/v1/jobs*` routes over [`crate::jobs`].

use crate::config::Config;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use super::error::api_err;
use super::HttpState;
use crate::error::AppError;
pub use crate::jobs::*;

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/jobs/sync", post(start_sync))
        .route("/v1/jobs", get(list_jobs))
        .route("/v1/jobs/{id}", get(get_job).delete(cancel_job))
        .route("/v1/jobs/{id}/requeue", post(requeue_job))
}

async fn start_sync(
    State(state): State<HttpState>,
    Json(request): Json<SyncJobRequest>,
) -> Response {
    if request.path.trim().is_empty() {
        return api_err(AppError::invalid("path must not be empty"));
    }
    let snapshot = match state.jobs.start_source_sync(
        request,
        state.store.clone(),
        state.embedder.clone(),
        Config::clone(&state.config),
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => return api_err(error),
    };
    (
        StatusCode::ACCEPTED,
        Json(json!({"ok": true, "job": snapshot})),
    )
        .into_response()
}

async fn list_jobs(State(state): State<HttpState>) -> impl IntoResponse {
    Json(json!({"ok": true, "items": state.jobs.list()}))
}

async fn get_job(State(state): State<HttpState>, Path(id): Path<String>) -> Response {
    match state.jobs.get(&id) {
        Some(job) => Json(json!({"ok": true, "job": job})).into_response(),
        None => api_err(AppError::not_found(format!("job not found: {id}"))),
    }
}

async fn cancel_job(State(state): State<HttpState>, Path(id): Path<String>) -> Response {
    match state.jobs.cancel(&id) {
        Ok(job) => {
            let status = if job.status.is_terminal() {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            };
            (status, Json(json!({"ok": true, "job": job}))).into_response()
        }
        Err(error) => api_err(error),
    }
}

async fn requeue_job(State(state): State<HttpState>, Path(id): Path<String>) -> Response {
    match state.jobs.requeue_source_sync(
        &id,
        state.store.clone(),
        state.embedder.clone(),
        Config::clone(&state.config),
    ) {
        Ok(snapshot) => (
            StatusCode::ACCEPTED,
            Json(json!({
                "ok": true,
                "job": snapshot,
                "resume_limits": SOURCE_SYNC_REQUEUE_LIMITS,
            })),
        )
            .into_response(),
        Err(error) => api_err(error),
    }
}

#[cfg(test)]
mod tests;
