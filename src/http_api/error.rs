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
        AppError::Forbidden(_) => StatusCode::FORBIDDEN,
        AppError::Busy(_) => StatusCode::SERVICE_UNAVAILABLE,
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
    let code = match &err {
        AppError::NotFound(_) => "NOT_FOUND", AppError::Config(_) => "INVALID_REQUEST",
        AppError::Conflict(_) => "CONFLICT", AppError::Forbidden(_) => "FORBIDDEN",
        AppError::Busy(_) => "STORE_BUSY", AppError::Db(_) => "DATABASE_ERROR",
        AppError::Embeddings(_) => "EMBEDDINGS_ERROR", AppError::Fts(_) => "FTS_ERROR",
        AppError::Io(_) => "IO_ERROR", _ => "INTERNAL_ERROR",
    };
    let body = Json(json!({
        "ok": false,
        "code": code,
        "error": err.to_string(),
    }));
    let mut response = (status, body).into_response();
    if matches!(err, AppError::Busy(_)) { response.headers_mut().insert("retry-after", "1".parse().unwrap()); }
    response
}
