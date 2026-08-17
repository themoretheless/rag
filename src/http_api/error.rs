//! HTTP JSON envelope helpers: success + map [`AppError`] to status/body.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::error::AppError;

/// Map domain error to HTTP status (single owner for handler status maps).
pub(super) fn status_for(err: &AppError) -> StatusCode {
    match err {
        AppError::NotFound(_) => StatusCode::NOT_FOUND,
        AppError::Config(_) => StatusCode::BAD_REQUEST,
        AppError::Conflict(_) => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Success JSON body (HTTP 200).
pub(super) fn api_ok(value: impl serde::Serialize) -> Response {
    Json(value).into_response()
}

/// Error JSON envelope: `{"ok": false, "error": "..."}` with status from [`status_for`].
pub(super) fn api_err(err: AppError) -> Response {
    let status = status_for(&err);
    let body = Json(json!({
        "ok": false,
        "error": err.to_string(),
    }));
    (status, body).into_response()
}
