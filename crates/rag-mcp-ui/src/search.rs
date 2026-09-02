//! Native search workspace models and background HTTP loader.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::gateway::{
    execute_request, format_http_error, format_json_parse_error, GatewayClient, Method, Request,
    ReqwestGatewayClient,
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchRequest {
    pub query: String,
    pub mode: String,
    pub top_k: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    pub include_archived: bool,
    pub timeout_ms: u64,
}

impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: "hybrid".to_string(),
            top_k: 20,
            wing: None,
            room: None,
            layer: None,
            include_archived: false,
            timeout_ms: 5_000,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SearchHit {
    pub chunk_id: String,
    pub document_id: String,
    pub document_title: String,
    pub document_uri: String,
    pub chunk_index: i32,
    pub content: String,
    pub score: f32,
    #[serde(default)]
    pub score_vec: Option<f32>,
    #[serde(default)]
    pub score_lex: Option<f32>,
    #[serde(default)]
    pub score_rrf: Option<f32>,
    #[serde(default)]
    pub snippet: Option<String>,
    #[serde(default)]
    pub heading_path: Option<Vec<String>>,
    #[serde(default)]
    pub section: Option<String>,
}

impl SearchHit {
    pub fn visible_excerpt(&self) -> &str {
        self.snippet.as_deref().unwrap_or(&self.content)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResults {
    pub items: Vec<SearchHit>,
    /// Exact controls used for these hits. Search controls remain editable
    /// while results are visible, so result provenance cannot come from them.
    pub request: SearchRequest,
}

#[derive(Deserialize)]
struct SearchEnvelope {
    #[serde(default)]
    items: Vec<SearchHit>,
}

pub fn fetch_search_http(base: &str, request: &SearchRequest) -> Result<SearchResults, String> {
    let client = ReqwestGatewayClient::new(Duration::from_secs(15))?;
    fetch_search_with_client(&client, base, request)
}

fn fetch_search_with_client(
    client: &dyn GatewayClient,
    base: &str,
    request: &SearchRequest,
) -> Result<SearchResults, String> {
    if request.query.trim().is_empty() {
        return Err("enter a search query".to_string());
    }
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("http base URL is empty".to_string());
    }
    let url = format!("{base}/v1/search");
    let response = execute_request(
        client,
        Request {
            method: Method::Post,
            url: url.clone(),
            body: Some(
                serde_json::to_string(request)
                    .map_err(|error| format!("serialize search request: {error}"))?,
            ),
            headers: Vec::new(),
        },
    )?;
    if !response.is_success() {
        return Err(format_http_error(&response, "Search"));
    }
    serde_json::from_str::<SearchEnvelope>(&response.body)
        .map(|envelope| SearchResults {
            items: envelope.items,
            request: request.clone(),
        })
        .map_err(|error| format_json_parse_error("Search", &error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::Response;
    use std::sync::Mutex;

    struct FakeGateway {
        response: Response,
        requests: Mutex<Vec<Request>>,
    }

    impl GatewayClient for FakeGateway {
        fn execute(&self, request: Request) -> Result<Response, String> {
            self.requests.lock().unwrap().push(request);
            Ok(self.response.clone())
        }
    }

    #[test]
    fn search_posts_project_scoped_query_and_parses_results() {
        let gateway = FakeGateway {
            response: Response {
                status: 200,
                body: r#"{"ok":true,"count":1,"items":[{"chunk_id":"c1","document_id":"d1","document_title":"Architecture","document_uri":"file:///architecture.md","chunk_index":2,"content":"responsibility boundary","score":0.82,"score_vec":0.7,"score_lex":4.2,"snippet":"responsibility boundary","heading_path":["Design"],"section":"Design"}]}"#.to_string(),
            },
            requests: Mutex::new(Vec::new()),
        };
        let results = fetch_search_with_client(
            &gateway,
            "http://gateway/",
            &SearchRequest {
                query: "responsibility".to_string(),
                wing: Some("alpha".to_string()),
                ..SearchRequest::default()
            },
        )
        .unwrap();
        assert_eq!(results.items[0].document_id, "d1");
        assert_eq!(results.request.mode, "hybrid");
        assert_eq!(results.request.wing.as_deref(), Some("alpha"));
        assert_eq!(
            results.items[0].visible_excerpt(),
            "responsibility boundary"
        );
        let requests = gateway.requests.lock().unwrap();
        assert_eq!(requests[0].method, Method::Post);
        let body = requests[0].body.as_deref().unwrap();
        assert!(body.contains(r#""wing":"alpha""#));
        assert!(body.contains(r#""mode":"hybrid""#));
    }

    #[test]
    fn empty_query_is_rejected_before_transport() {
        let gateway = FakeGateway {
            response: Response {
                status: 200,
                body: "{}".to_string(),
            },
            requests: Mutex::new(Vec::new()),
        };
        let error = fetch_search_with_client(&gateway, "http://gateway", &SearchRequest::default())
            .unwrap_err();
        assert_eq!(error, "enter a search query");
        assert!(gateway.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn search_json_error_omits_url_and_envelope_metadata() {
        let gateway = FakeGateway {
            response: Response {
                status: 400,
                body: r#"{"ok":false,"code":"INVALID_QUERY","error":"Query is required","request_id":"private-id"}"#
                    .to_string(),
            },
            requests: Mutex::new(Vec::new()),
        };

        let error = fetch_search_with_client(
            &gateway,
            "http://gateway/private-base",
            &SearchRequest {
                query: "needle".to_string(),
                ..SearchRequest::default()
            },
        )
        .unwrap_err();

        assert_eq!(
            error,
            "HTTP 400 · Search: Query is required (INVALID_QUERY)"
        );
        assert!(!error.contains("gateway/private-base"));
        assert!(!error.contains("private-id"));
        assert!(!error.contains('{'));
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
    fn search_transport_error_is_safe_for_display() {
        let error = fetch_search_with_client(
            &FailingGateway,
            "http://127.0.0.1:7432",
            &SearchRequest {
                query: "needle".to_string(),
                ..SearchRequest::default()
            },
        )
        .unwrap_err();

        assert_eq!(error, "RAG gateway request failed");
        assert!(!error.contains("127.0.0.1"));
        assert!(!error.contains("/v1/search"));
    }
}
