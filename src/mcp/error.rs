//! Map domain errors onto JSON-RPC error data for MCP clients.
//!
//! Mirrors `http_api::error` for the other transport: the wire code comes from
//! [`AppError::code`], only the JSON-RPC classification is decided here.

use rmcp::ErrorData as McpError;

use crate::error::AppError;

impl From<AppError> for McpError {
    /// Every error carries `data.code` from [`AppError::code`], so agents can
    /// tell a CAS `CONFLICT` from `INVALID_REQUEST` even though both use the
    /// JSON-RPC `invalid_params` class. `Busy` additionally carries retry hints.
    fn from(err: AppError) -> Self {
        let code = err.code();
        let data = Some(serde_json::json!({ "code": code }));
        match err {
            AppError::NotFound(msg) => McpError::resource_not_found(msg, data),
            AppError::Config(msg)
            | AppError::Invalid(msg)
            | AppError::Chunking(msg)
            | AppError::Embeddings(msg)
            | AppError::Conflict(msg)
            | AppError::Forbidden(msg) => McpError::invalid_params(msg, data),
            AppError::Busy(msg) => McpError::internal_error(
                msg,
                Some(serde_json::json!({
                    "code": code,
                    "retryable": true,
                    "retry_after_ms": 1_000,
                })),
            ),
            AppError::Llm(msg) => McpError::internal_error(msg, data),
            other => McpError::internal_error(other.to_string(), data),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ErrorCode;

    #[test]
    fn config_maps_to_invalid_params_and_busy_carries_retry_data() {
        let invalid = McpError::from(AppError::config("bad"));
        assert_eq!(invalid.code, ErrorCode::INVALID_PARAMS);
        let missing = McpError::from(AppError::not_found("gone"));
        assert_eq!(missing.code, ErrorCode::RESOURCE_NOT_FOUND);
        let busy = McpError::from(AppError::busy("lane"));
        assert_eq!(busy.code, ErrorCode::INTERNAL_ERROR);
        let data = busy.data.expect("busy carries retry data");
        assert_eq!(data["code"], "STORE_BUSY");
        assert_eq!(data["retryable"], true);
    }

    #[test]
    fn conflict_and_invalid_input_are_distinguishable_by_data_code() {
        let conflict = McpError::from(AppError::conflict("stale revision"));
        let invalid = McpError::from(AppError::config("slug required"));
        assert_eq!(conflict.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(invalid.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(conflict.data.unwrap()["code"], "CONFLICT");
        assert_eq!(invalid.data.unwrap()["code"], "INVALID_REQUEST");
    }
}
