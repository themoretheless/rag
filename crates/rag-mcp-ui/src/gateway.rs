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
            .map_err(|error| format!("http client: {error}"))
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
        let response = builder.send().map_err(|error| {
            format!("HTTP {:?} {} failed: {error}", request.method, request.url)
        })?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .map_err(|error| format!("read HTTP response from {}: {error}", request.url))?;
        Ok(Response { status, body })
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
}
