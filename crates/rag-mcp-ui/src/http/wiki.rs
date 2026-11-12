//! Wiki catalog and CAS-protected page writes (`GET` / `PUT /v1/wiki`).

use super::document::fetch_document_http_with_client;
use crate::dto::{
    document_body_from_json, sort_wiki_pages, DocumentBody, DocumentBodyJsonDefaults, WikiPageMeta,
    WikiPutRequest,
};
use crate::gateway::{
    client as gateway_client, format_http_error, percent_encode as urlencoding_minimal, GatewayApi,
    GatewayClient, Method,
};

#[derive(Debug, Clone, serde::Deserialize)]
struct WikiListResponse {
    #[serde(default)]
    pages: Vec<WikiPageMeta>,
    total: Option<usize>,
    limit: Option<usize>,
    offset: Option<usize>,
}

/// Fetch wiki page catalog via `GET /v1/wiki`.
pub fn fetch_wiki_list_http(
    base: &str,
    project: Option<&str>,
) -> Result<Vec<WikiPageMeta>, String> {
    let client = gateway_client(30)?;
    fetch_wiki_list_http_with_client(&client, base, project)
}

fn fetch_wiki_list_http_with_client(
    client: &dyn GatewayClient,
    base: &str,
    project: Option<&str>,
) -> Result<Vec<WikiPageMeta>, String> {
    const PAGE_SIZE: usize = 200;
    const MAX_PAGES: usize = 10_000;
    let api = GatewayApi::new(client, base);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let path = wiki_catalog_path(project);
    let separator = if path.contains('?') { '&' } else { '?' };
    let mut pages = Vec::new();
    let mut ids = std::collections::HashSet::new();
    let mut offset = 0;
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(
                "Wiki catalog exceeded the one-minute loading limit; select a narrower project"
                    .into(),
            );
        }
        let body: WikiListResponse = api.get(
            &format!("{path}{separator}limit={PAGE_SIZE}&offset={offset}"),
            "Wiki catalog",
        )?;
        if body.offset.is_some_and(|value| value != offset) {
            return Err(
                "Wiki catalog pagination returned an unexpected offset; reload the catalog".into(),
            );
        }
        if body.total.is_some_and(|total| total > MAX_PAGES)
            || pages.len() + body.pages.len() > MAX_PAGES
        {
            return Err(format!(
                "Wiki catalog exceeds {MAX_PAGES} pages; select a narrower project"
            ));
        }
        let count = body.pages.len();
        for page in body.pages {
            if !ids.insert(page.id.clone()) {
                return Err(
                    "Wiki catalog changed or repeated a page while loading; reload the catalog"
                        .into(),
                );
            }
            pages.push(page);
        }
        offset += count;
        let more = body
            .total
            .map_or_else(|| count >= body.limit.unwrap_or(50), |total| offset < total);
        if !more {
            break;
        }
        if count == 0 || offset >= MAX_PAGES {
            return Err("Wiki catalog pagination stopped before all pages were loaded".into());
        }
    }
    sort_wiki_pages(&mut pages);
    Ok(pages)
}

fn wiki_catalog_path(project: Option<&str>) -> String {
    project
        .map(|value| format!("v1/wiki?wing={}", urlencoding_minimal(value)))
        .unwrap_or_else(|| "v1/wiki".to_string())
}

/// Save wiki page content via HTTP `PUT /v1/wiki` (requires gateway write support).
///
/// Sends the document id and CAS fields when known. On 404/405, returns a clear
/// message that writes require a newer gateway or MCP `update_wiki_page`.
pub fn put_wiki_http(base: &str, req: &WikiPutRequest) -> Result<DocumentBody, String> {
    let client = gateway_client(60)?;
    put_wiki_http_with_client(&client, base, req)
}

fn put_wiki_http_with_client(
    client: &dyn GatewayClient,
    base: &str,
    req: &WikiPutRequest,
) -> Result<DocumentBody, String> {
    let api = GatewayApi::new(client, base);
    if api.base().is_empty() {
        return Err("http base URL is empty".into());
    }
    let mut headers = Vec::new();
    if let Some(etag) = req
        .if_match_etag
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        headers.push(("If-Match".to_string(), etag.to_string()));
    } else if let Some(rev) = req.if_match_revision {
        headers.push(("If-Match".to_string(), format!("W/\"{rev}\"")));
    }
    let body = serde_json::to_string(req).map_err(|e| format!("serialize wiki: {e}"))?;
    let response = api
        .request(Method::Put, "v1/wiki", Some(body), headers)
        .map_err(|e| format!("{e}. Is rag-mcp running with RAG_HTTP_BIND?"))?;
    if response.status == 404 || response.status == 405 {
        return Err(format!(
            "{}. Wiki writing is not available on this gateway; use a newer gateway or MCP update_wiki_page.",
            format_http_error(&response, "Wiki save")
        ));
    }
    if response.status == 409 {
        return Err(format!(
            "{}. Reload the latest revision before saving again.",
            format_http_error(&response, "Wiki save conflict")
        ));
    }
    if !response.is_success() {
        return Err(format_http_error(&response, "Wiki save"));
    }
    // Accept full DocumentBody or a write-result envelope with document fields.
    let text = response.body;
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
        let defaults = DocumentBodyJsonDefaults {
            content: Some(req.content.as_str()),
            title: Some(req.title.as_str()),
            uri: req.uri.as_deref(),
            revision: req.if_match_revision.map(|r| r + 1),
        };
        if let Some(body) = document_body_from_json(&v, defaults) {
            return Ok(body);
        }
    }
    // Server accepted write but returned no body: re-fetch document.
    fetch_document_http_with_client(client, api.base(), Some(&req.id), req.uri.as_deref(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{Request, Response};
    use crate::http::testing::FakeGateway;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[test]
    fn wiki_catalog_path_scopes_and_encodes_project() {
        assert_eq!(wiki_catalog_path(None), "v1/wiki");
        assert_eq!(
            wiki_catalog_path(Some("Project A/B")),
            "v1/wiki?wing=Project%20A%2FB"
        );
    }

    #[test]
    fn wiki_catalog_uses_injected_transport() {
        let gateway = FakeGateway::ok(
            r#"{"pages":[{"id":"1","uri":"wiki://a","slug":"a","title":"Alpha","kind":"page","summary":"","category":null,"revision":1,"etag":null,"updated_at":null}],"count":1}"#,
        );
        let pages =
            fetch_wiki_list_http_with_client(&gateway, "http://gateway/", Some("Project A/B"))
                .unwrap();

        assert_eq!(pages[0].title, "Alpha");
        let requests = gateway.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::Get);
        assert_eq!(
            requests[0].url,
            "http://gateway/v1/wiki?wing=Project%20A%2FB&limit=200&offset=0"
        );
    }

    struct PagedWikiGateway {
        responses: Mutex<VecDeque<Response>>,
        requests: Mutex<Vec<Request>>,
    }

    impl PagedWikiGateway {
        fn new(bodies: Vec<serde_json::Value>) -> Self {
            Self {
                responses: Mutex::new(
                    bodies
                        .into_iter()
                        .map(|body| Response {
                            status: 200,
                            body: body.to_string(),
                        })
                        .collect(),
                ),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl GatewayClient for PagedWikiGateway {
        fn execute(&self, request: Request) -> Result<Response, String> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| "Unexpected extra page request".into())
        }
    }

    fn wiki_batch(offset: usize, count: usize, total: usize) -> serde_json::Value {
        serde_json::json!({
            "pages": (offset..offset + count).map(|i| serde_json::json!({
                "id": i.to_string(), "slug": format!("page-{i}"), "uri": format!("wiki://page-{i}"), "title": format!("Page {i:04}"),
            })).collect::<Vec<_>>(),
            "count": count, "total": total, "limit": 200, "offset": offset,
        })
    }

    #[test]
    fn wiki_catalog_fetches_every_page_beyond_the_default_fifty() {
        let gateway =
            PagedWikiGateway::new(vec![wiki_batch(0, 200, 225), wiki_batch(200, 25, 225)]);
        let pages =
            fetch_wiki_list_http_with_client(&gateway, "http://gateway", Some("Alpha")).unwrap();
        assert_eq!(pages.len(), 225);
        assert_eq!(pages.last().unwrap().id, "224");
        let requests = gateway.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].url,
            "http://gateway/v1/wiki?wing=Alpha&limit=200&offset=200"
        );
    }

    #[test]
    fn wiki_catalog_legacy_metadata_still_fetches_after_fifty() {
        let first = wiki_batch(0, 50, 52);
        let second = wiki_batch(50, 2, 52);
        let gateway = PagedWikiGateway::new(vec![
            serde_json::json!({"pages": first["pages"]}),
            serde_json::json!({"pages": second["pages"]}),
        ]);
        let pages = fetch_wiki_list_http_with_client(&gateway, "http://gateway", None).unwrap();
        assert_eq!(pages.len(), 52);
        assert!(gateway.requests.lock().unwrap()[1]
            .url
            .ends_with("offset=50"));
    }

    #[test]
    fn wiki_catalog_rejects_empty_or_repeated_pages_instead_of_silently_truncating() {
        for second in [wiki_batch(50, 0, 60), wiki_batch(0, 50, 60)] {
            let gateway = PagedWikiGateway::new(vec![wiki_batch(0, 50, 60), second]);
            assert!(fetch_wiki_list_http_with_client(&gateway, "http://gateway", None).is_err());
        }
    }

    #[test]
    fn wiki_catalog_fails_explicitly_above_the_memory_cap() {
        let gateway = PagedWikiGateway::new(vec![wiki_batch(0, 200, 10_001)]);
        let error = fetch_wiki_list_http_with_client(&gateway, "http://gateway", None).unwrap_err();
        assert!(error.contains("10000"));
        assert_eq!(gateway.requests.lock().unwrap().len(), 1);
    }
}
