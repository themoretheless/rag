//! HTTP transport boundary for the native UI.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Delete,
    Get,
    Post,
    Put,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub body: Option<String>,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

const TRANSPORT_TIMEOUT_MESSAGE: &str = "RAG gateway did not respond in time";
const TRANSPORT_CONNECT_MESSAGE: &str = "Could not connect to the RAG gateway";
const TRANSPORT_FAILURE_MESSAGE: &str = "RAG gateway request failed";

/// Execute one gateway request and normalize every transport failure before it
/// can reach product UI. This also keeps injected/test transports on the same
/// safe contract as the reqwest implementation.
pub fn execute_request(client: &dyn GatewayClient, request: Request) -> Result<Response, String> {
    client
        .execute(request)
        .map_err(|error| format_transport_error(&error))
}

/// Convert an opaque transport error into a stable, user-facing message.
///
/// Never echo the original value: reqwest errors may contain the HTTP method,
/// full URL, query parameters, or lower-level request diagnostics. Known
/// timeout and connection phrases are classified for non-reqwest transports;
/// unknown values deliberately collapse to a generic message.
pub fn format_transport_error(error: &str) -> String {
    if matches!(
        error,
        TRANSPORT_TIMEOUT_MESSAGE | TRANSPORT_CONNECT_MESSAGE | TRANSPORT_FAILURE_MESSAGE
    ) {
        return error.to_string();
    }

    let normalized = error.to_ascii_lowercase();
    if normalized.contains("timed out")
        || normalized.contains("timeout")
        || normalized.contains("deadline elapsed")
    {
        TRANSPORT_TIMEOUT_MESSAGE.to_string()
    } else if normalized.contains("connection refused")
        || normalized.contains("failed to connect")
        || normalized.contains("connect error")
        || normalized.contains("tcp connect")
        || normalized.contains("dns error")
    {
        TRANSPORT_CONNECT_MESSAGE.to_string()
    } else {
        TRANSPORT_FAILURE_MESSAGE.to_string()
    }
}

/// Format a non-success gateway response for display in the native UI.
///
/// Gateway JSON envelopes can contain internal request metadata and are not
/// useful as a raw blob. Keep the HTTP status and the caller-provided action,
/// extract the stable `code` and human-readable `error`, and fall back to a
/// compact plain-text response when the server did not return JSON.
pub fn format_http_error(response: &Response, context: &str) -> String {
    let context = compact_text(context, 80);
    let prefix = if context.is_empty() {
        format!("HTTP {} request failed", response.status)
    } else {
        format!("HTTP {} · {context}", response.status)
    };

    match error_detail(&response.body) {
        Some(detail) => format!("{prefix}: {detail}"),
        None => prefix,
    }
}

fn error_detail(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }

    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(value) => json_error_detail(&value)
            .or_else(|| value.as_str().map(|value| compact_text(value, 300)))
            .filter(|value| !value.is_empty())
            .or_else(|| Some("gateway returned an error response".to_string())),
        Err(_) if trimmed.starts_with('{') || trimmed.starts_with('[') => {
            Some("gateway returned a malformed error response".to_string())
        }
        Err(_) => {
            let detail = compact_text(trimmed, 300);
            (!detail.is_empty()).then_some(detail)
        }
    }
}

fn json_error_detail(value: &serde_json::Value) -> Option<String> {
    let code = value
        .get("code")
        .and_then(serde_json::Value::as_str)
        .map(|value| compact_text(value, 80))
        .filter(|value| !value.is_empty());
    let message = value
        .get("error")
        .and_then(serde_json::Value::as_str)
        .or_else(|| value.get("message").and_then(serde_json::Value::as_str))
        .or_else(|| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(serde_json::Value::as_str)
        })
        .map(|value| compact_text(value, 300))
        .filter(|value| !value.is_empty());

    match (message, code) {
        (Some(message), Some(code)) if message.eq_ignore_ascii_case(&code) => Some(message),
        (Some(message), Some(code)) => Some(format!("{message} ({code})")),
        (Some(message), None) => Some(message),
        (None, Some(code)) => Some(code),
        (None, None) => None,
    }
}

fn compact_text(value: &str, max_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = compact.chars();
    let mut result = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

pub trait GatewayClient {
    fn execute(&self, request: Request) -> Result<Response, String>;
}

pub struct ReqwestGatewayClient {
    client: reqwest::blocking::Client,
}

impl ReqwestGatewayClient {
    pub fn new(timeout: Duration) -> Result<Self, String> {
        reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .map(|client| Self { client })
            .map_err(|_| "Could not initialize the RAG gateway connection".to_string())
    }
}

impl GatewayClient for ReqwestGatewayClient {
    fn execute(&self, request: Request) -> Result<Response, String> {
        let mut builder = match request.method {
            Method::Delete => self.client.delete(&request.url),
            Method::Get => self.client.get(&request.url),
            Method::Post => self.client.post(&request.url),
            Method::Put => self.client.put(&request.url),
        };
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = request.body {
            builder = builder
                .header("Content-Type", "application/json")
                .body(body);
        }
        let response = builder.send().map_err(format_reqwest_transport_error)?;
        let status = response.status().as_u16();
        let body = response.text().map_err(format_reqwest_transport_error)?;
        Ok(Response { status, body })
    }
}

fn format_reqwest_transport_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        TRANSPORT_TIMEOUT_MESSAGE.to_string()
    } else if error.is_connect() {
        TRANSPORT_CONNECT_MESSAGE.to_string()
    } else {
        TRANSPORT_FAILURE_MESSAGE.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_error_is_human_readable_without_raw_envelope() {
        let response = Response {
            status: 400,
            body: r#"{"ok":false,"code":"INVALID_QUERY","error":"Query is required","request_id":"private-id"}"#
                .to_string(),
        };

        let formatted = format_http_error(&response, "Search");

        assert_eq!(
            formatted,
            "HTTP 400 · Search: Query is required (INVALID_QUERY)"
        );
        assert!(!formatted.contains("private-id"));
        assert!(!formatted.contains('{'));
    }

    #[test]
    fn json_error_supports_nested_message_and_code_only() {
        let nested = Response {
            status: 409,
            body: r#"{"error":{"message":"Revision changed"},"code":"REVISION_CONFLICT"}"#
                .to_string(),
        };
        let code_only = Response {
            status: 403,
            body: r#"{"code":"RAW_RESTORE_FORBIDDEN","request_id":"private-id"}"#.to_string(),
        };

        assert_eq!(
            format_http_error(&nested, "Wiki save"),
            "HTTP 409 · Wiki save: Revision changed (REVISION_CONFLICT)"
        );
        assert_eq!(
            format_http_error(&code_only, "Revision restore"),
            "HTTP 403 · Revision restore: RAW_RESTORE_FORBIDDEN"
        );
    }

    #[test]
    fn non_json_errors_remain_compact_and_useful() {
        let response = Response {
            status: 503,
            body: "  temporarily\n  unavailable  ".to_string(),
        };

        assert_eq!(
            format_http_error(&response, "Project Home"),
            "HTTP 503 · Project Home: temporarily unavailable"
        );
    }

    #[test]
    fn unknown_json_is_not_exposed_as_a_raw_blob() {
        let response = Response {
            status: 500,
            body: r#"{"debug":"sensitive detail","request_id":"private-id"}"#.to_string(),
        };

        assert_eq!(
            format_http_error(&response, "Library"),
            "HTTP 500 · Library: gateway returned an error response"
        );
    }

    #[test]
    fn raw_reqwest_transport_error_never_exposes_method_or_url() {
        let raw =
            "HTTP Post http://127.0.0.1:7432/v1/search failed: error sending request for url (...)";

        let formatted = format_transport_error(raw);

        assert_eq!(formatted, TRANSPORT_FAILURE_MESSAGE);
        assert!(!formatted.contains("Post"));
        assert!(!formatted.contains("127.0.0.1"));
        assert!(!formatted.contains("/v1/search"));
    }

    #[test]
    fn transport_error_classifies_timeout_and_connection_without_raw_details() {
        assert_eq!(
            format_transport_error(
                "error sending request for url (http://secret.internal/v1/search): operation timed out"
            ),
            TRANSPORT_TIMEOUT_MESSAGE
        );
        assert_eq!(
            format_transport_error("tcp connect error: Connection refused at secret.internal:7432"),
            TRANSPORT_CONNECT_MESSAGE
        );
    }

    struct FailingGateway;

    impl GatewayClient for FailingGateway {
        fn execute(&self, _request: Request) -> Result<Response, String> {
            Err(
                "HTTP Post http://127.0.0.1:7432/v1/search failed: error sending request for url (...)"
                    .to_string(),
            )
        }
    }

    #[test]
    fn request_boundary_sanitizes_injected_transport_errors() {
        let error = execute_request(
            &FailingGateway,
            Request {
                method: Method::Post,
                url: "http://127.0.0.1:7432/v1/search".to_string(),
                body: None,
                headers: Vec::new(),
            },
        )
        .unwrap_err();

        assert_eq!(error, TRANSPORT_FAILURE_MESSAGE);
    }
}
