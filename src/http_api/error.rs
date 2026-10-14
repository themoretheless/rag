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
        AppError::Config(_) | AppError::Invalid(_) => StatusCode::BAD_REQUEST,
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

/// Error JSON envelope: `{"ok": false, "code": "...", "error": "..."}` with
/// status from [`status_for`] and the code from [`AppError::code`].
pub(super) fn api_err(err: AppError) -> Response {
    let status = status_for(&err);
    let body = Json(json!({
        "ok": false,
        "code": err.code(),
        "error": err.to_string(),
    }));
    let mut response = (status, body).into_response();
    if err.is_retryable() {
        response
            .headers_mut()
            .insert("retry-after", "1".parse().unwrap());
    }
    response
}

/// Error for an internal `/v1/search` call that returned a non-success status,
/// carrying the status and (bounded) response body.
pub(super) async fn search_response_error(response: axum::response::Response) -> AppError {
    let status = response.status();
    let bytes = match axum::body::to_bytes(response.into_body(), 65536).await {
        Ok(bytes) => bytes,
        Err(e) => return AppError::invalid(e.to_string()),
    };
    AppError::invalid(format!(
        "search failed ({status}): {}",
        String::from_utf8_lossy(&bytes)
    ))
}
