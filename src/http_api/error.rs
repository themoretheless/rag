//! HTTP JSON envelope helpers: success + map [`AppError`] to status/body.

use axum::Json;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::error::AppError;

/// Map domain error to HTTP status (single owner for handler status maps).
pub(super) fn status_for(err: &AppError) -> StatusCode {
    match err {
        AppError::NotFound(_) => StatusCode::NOT_FOUND,
        AppError::Config(_) | AppError::Invalid(_) | AppError::Timeout(_) => {
            StatusCode::BAD_REQUEST
        }
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
            .insert("retry-after", HeaderValue::from_static("1"));
    }
    response
}

/// Insert an `ETag` header when `etag` is a valid header value; otherwise log
/// and leave the response without one (never panics on runtime values).
pub(super) fn insert_etag(response: &mut Response, etag: &str) -> bool {
    match HeaderValue::from_str(etag) {
        Ok(value) => {
            response
                .headers_mut()
                .insert(axum::http::header::ETAG, value);
            true
        }
        Err(err) => {
            tracing::warn!(etag, error = %err, "skipping invalid ETag header value");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_errors_share_the_bad_request_status() {
        for err in [
            AppError::config("x"),
            AppError::invalid("x"),
            AppError::timeout("x"),
        ] {
            assert_eq!(status_for(&err), StatusCode::BAD_REQUEST);
        }
    }

    #[test]
    fn insert_etag_sets_valid_value() {
        let mut response = StatusCode::OK.into_response();
        assert!(insert_etag(&mut response, "W/\"3\""));
        assert_eq!(response.headers()[axum::http::header::ETAG], "W/\"3\"");
    }

    #[test]
    fn insert_etag_skips_invalid_value_without_panicking() {
        let mut response = StatusCode::OK.into_response();
        assert!(!insert_etag(&mut response, "bad\netag"));
        assert!(response.headers().get(axum::http::header::ETAG).is_none());
    }
}
