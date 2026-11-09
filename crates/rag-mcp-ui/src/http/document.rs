//! Document bodies and backlinks (`GET /v1/document`, `GET /v1/backlinks`).

use crate::dto::{document_body_from_json, BacklinkItem, DocumentBody, DocumentBodyJsonDefaults};
use crate::gateway::{
    client as gateway_client, decode_json, percent_encode as urlencoding_minimal, GatewayApi,
    GatewayClient,
};

#[derive(Debug, Clone, serde::Deserialize)]
struct BacklinksResponse {
    #[serde(default)]
    backlinks: Vec<BacklinkItem>,
}

/// Fetch incoming links for a document id, optionally constrained to a project.
pub fn fetch_backlinks_http(
    base: &str,
    document_id: &str,
    project: Option<&str>,
) -> Result<Vec<BacklinkItem>, String> {
    let client = gateway_client(15)?;
    fetch_backlinks_http_with_client(&client, base, document_id, project)
}

fn fetch_backlinks_http_with_client(
    client: &dyn GatewayClient,
    base: &str,
    document_id: &str,
    project: Option<&str>,
) -> Result<Vec<BacklinkItem>, String> {
    GatewayApi::new(client, base)
        .get::<BacklinksResponse>(&backlinks_path(document_id, project), "Backlinks")
        .map(|body| body.backlinks)
}

fn backlinks_path(document_id: &str, project: Option<&str>) -> String {
    let mut path = format!("v1/backlinks?id={}", urlencoding_minimal(document_id));
    if let Some(project) = project.map(str::trim).filter(|project| !project.is_empty()) {
        path.push_str("&wing=");
        path.push_str(&urlencoding_minimal(project));
    }
    path
}

/// Fetch full document text via `GET /v1/document` on the rag-mcp HTTP gateway.
pub fn fetch_document_http(
    base: &str,
    document_id: Option<&str>,
    uri: Option<&str>,
    q: Option<&str>,
) -> Result<DocumentBody, String> {
    let client = gateway_client(30)?;
    fetch_document_http_with_client(&client, base, document_id, uri, q)
}

pub(super) fn fetch_document_http_with_client(
    client: &dyn GatewayClient,
    base: &str,
    document_id: Option<&str>,
    uri: Option<&str>,
    q: Option<&str>,
) -> Result<DocumentBody, String> {
    let api = GatewayApi::new(client, base);
    if api.base().is_empty() {
        return Err("http base URL is empty".into());
    }
    let mut parts = Vec::new();
    if let Some(id) = document_id.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("id={}", urlencoding_minimal(id)));
    } else if let Some(u) = uri.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("uri={}", urlencoding_minimal(u)));
    } else if let Some(key) = q.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("q={}", urlencoding_minimal(key)));
    } else {
        return Err("need document_id, uri, or label to load content".into());
    }
    let response = api
        .get_response(&format!("v1/document?{}", parts.join("&")))
        .map_err(|e| format!("{e}. Is rag-mcp running with RAG_HTTP_BIND?"))?;
    let v: serde_json::Value = decode_json(response, "Document")?;
    document_body_from_json(&v, DocumentBodyJsonDefaults::default())
        .ok_or_else(|| "Document response is missing document fields".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::Response;
    use crate::http::testing::FakeGateway;
    use std::sync::Mutex;

    #[test]
    fn backlinks_path_scopes_and_encodes_project() {
        assert_eq!(backlinks_path("doc/1", None), "v1/backlinks?id=doc%2F1");
        assert_eq!(
            backlinks_path("doc/1", Some("Project A/B")),
            "v1/backlinks?id=doc%2F1&wing=Project%20A%2FB"
        );
    }

    #[test]
    fn transport_errors_keep_status_and_safe_context() {
        let gateway = FakeGateway {
            response: Response {
                status: 503,
                body: "temporarily unavailable".to_string(),
            },
            requests: Mutex::new(Vec::new()),
        };

        let error =
            fetch_backlinks_http_with_client(&gateway, "http://gateway", "doc", Some("alpha"))
                .unwrap_err();
        assert_eq!(error, "HTTP 503 · Backlinks: temporarily unavailable");
        assert_eq!(
            gateway.requests.lock().unwrap()[0].url,
            "http://gateway/v1/backlinks?id=doc&wing=alpha"
        );
    }
}
