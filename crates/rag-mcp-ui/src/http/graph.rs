//! Graph topology over HTTP: full export, project catalog and lazy expansion.

use rag_mcp_core::{GraphView, UI_GRAPH_EXPORT_MAX_NODES};

use crate::dto::{GatewayHealth, GraphSourceKind, LoadedGraph};
use crate::gateway::{
    client as gateway_client, decode_json, percent_encode as urlencoding_minimal, GatewayApi,
    GatewayClient, Response,
};
use crate::topology::merge_graph_views;

#[derive(Debug, Clone, serde::Deserialize)]
struct ProjectCatalogResponse {
    #[serde(default)]
    items: Vec<ProjectCatalogItem>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ProjectCatalogItem {
    project_id: String,
}

/// Load GraphView from rag-mcp HTTP API (same process holds DuckDB; no second open).
///
/// Server: `RAG_HTTP_BIND=127.0.0.1:7432`  
/// UI: `--http http://127.0.0.1:7432`
pub fn load_http(
    base: &str,
    seed: Option<&str>,
    depth: u32,
    project: Option<&str>,
    include_tags: bool,
) -> Result<LoadedGraph, String> {
    let client = gateway_client(30)?;
    load_http_with_client(&client, base, seed, depth, project, include_tags)
}

/// Fetch the authoritative project catalog independently from graph topology.
/// This is also used by the UI's targeted retry when graph loading succeeds but
/// `/v1/projects` is temporarily unavailable.
pub fn fetch_project_catalog_http(base: &str) -> Result<Vec<String>, String> {
    let client = gateway_client(10)?;
    fetch_project_catalog_with_client(&client, base)
}

fn fetch_project_catalog_with_client(
    client: &dyn GatewayClient,
    base: &str,
) -> Result<Vec<String>, String> {
    let api = GatewayApi::new(client, base);
    if api.base().is_empty() {
        return Err("HTTP base URL is empty".into());
    }
    let mut projects = api
        .get::<ProjectCatalogResponse>("v1/projects", "Project catalog")?
        .items
        .into_iter()
        .map(|item| item.project_id)
        .filter(|project| !project.trim().is_empty())
        .collect::<Vec<_>>();
    projects.sort();
    projects.dedup();
    Ok(projects)
}

fn load_http_with_client(
    client: &dyn GatewayClient,
    base: &str,
    seed: Option<&str>,
    depth: u32,
    project: Option<&str>,
    include_tags: bool,
) -> Result<LoadedGraph, String> {
    let api = GatewayApi::new(client, base);
    if api.base().is_empty() {
        return Err("--http base URL is empty".into());
    }
    let health = api
        .get_response("health")
        .ok()
        .filter(Response::is_success)
        .and_then(|response| serde_json::from_str::<GatewayHealth>(&response.body).ok());
    let response = api
        .get_response(&http_graph_path(seed, depth, project, include_tags))
        .map_err(|e| format!("{e}. Is rag-mcp running with RAG_HTTP_BIND set?"))?;
    let view: GraphView = decode_json(response, "Project graph")?;
    let raw_node_count = view.nodes.len();
    // The gateway clamps at the requested limit, so equality is the only
    // client-visible signal that more server-side nodes may exist.
    let truncated = http_export_may_be_truncated(raw_node_count);
    Ok(LoadedGraph {
        view,
        projects: None,
        source: GraphSourceKind::HttpService {
            base: api.base().to_string(),
        },
        truncated,
        raw_node_count,
        health,
    })
}

fn http_export_may_be_truncated(raw_node_count: usize) -> bool {
    raw_node_count >= UI_GRAPH_EXPORT_MAX_NODES as usize
}

fn http_graph_path(
    seed: Option<&str>,
    depth: u32,
    project: Option<&str>,
    include_tags: bool,
) -> String {
    if let Some(seed) = seed.map(str::trim).filter(|seed| !seed.is_empty()) {
        return http_neighbors_path(
            seed,
            depth,
            UI_GRAPH_EXPORT_MAX_NODES,
            project,
            include_tags,
        );
    }
    let mut path =
        format!("v1/graph?max_nodes={UI_GRAPH_EXPORT_MAX_NODES}&include_tags={include_tags}");
    if let Some(project) = project.map(str::trim).filter(|project| !project.is_empty()) {
        path.push_str("&project=");
        path.push_str(&urlencoding_minimal(project));
    }
    path
}

fn http_neighbors_path(
    seed: &str,
    depth: u32,
    max_nodes: u32,
    project: Option<&str>,
    include_tags: bool,
) -> String {
    let mut path = format!(
        "v1/neighbors?seed={}&depth={}&max_nodes={}&include_tags={include_tags}",
        urlencoding_minimal(seed.trim()),
        depth.clamp(1, 3),
        max_nodes.max(1)
    );
    if let Some(project) = project.map(str::trim).filter(|project| !project.is_empty()) {
        path.push_str("&project=");
        path.push_str(&urlencoding_minimal(project));
    }
    path
}

/// Expand a selected node through the gateway so nodes outside the initial
/// bounded project export remain discoverable. The project scope is repeated
/// on every lazy request; omitting it could leak or merge another project.
pub fn expand_neighbors_http(
    base: &str,
    current: &GraphView,
    selected_id: &str,
    max_nodes: u32,
    project: Option<&str>,
    include_tags: bool,
) -> Result<GraphView, String> {
    let client = gateway_client(20)?;
    expand_neighbors_http_with_client(
        &client,
        base,
        current,
        selected_id,
        max_nodes,
        project,
        include_tags,
    )
}

fn expand_neighbors_http_with_client(
    client: &dyn GatewayClient,
    base: &str,
    current: &GraphView,
    selected_id: &str,
    max_nodes: u32,
    project: Option<&str>,
    include_tags: bool,
) -> Result<GraphView, String> {
    let api = GatewayApi::new(client, base);
    if api.base().is_empty() {
        return Err("HTTP base URL is empty".into());
    }
    let extra: GraphView = api.get(
        &http_neighbors_path(selected_id, 1, max_nodes, project, include_tags),
        "Graph expansion",
    )?;
    Ok(merge_graph_views(
        current,
        &extra,
        None,
        max_nodes.max(1) as usize,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::Method;
    use crate::http::testing::FakeGateway;
    use crate::topology::test_support::node;
    use std::sync::Mutex;

    #[test]
    fn graph_and_lazy_neighbor_urls_repeat_encoded_project_scope() {
        assert_eq!(
            http_graph_path(None, 1, Some("Project A/B"), false),
            format!(
                "v1/graph?max_nodes={UI_GRAPH_EXPORT_MAX_NODES}&include_tags=false&project=Project%20A%2FB"
            )
        );
        assert_eq!(
            http_graph_path(
                Some("doc://seed one"),
                2,
                Some("Project A/B"),
                false,
            ),
            format!(
                "v1/neighbors?seed=doc%3A%2F%2Fseed%20one&depth=2&max_nodes={UI_GRAPH_EXPORT_MAX_NODES}&include_tags=false&project=Project%20A%2FB"
            )
        );
        assert_eq!(
            http_neighbors_path("node/42", 1, 120, Some("Project A/B"), false),
            "v1/neighbors?seed=node%2F42&depth=1&max_nodes=120&include_tags=false&project=Project%20A%2FB"
        );
    }

    #[test]
    fn graph_and_neighbor_urls_forward_tag_fetch_toggle() {
        assert_eq!(
            http_graph_path(None, 1, Some("alpha"), true),
            format!(
                "v1/graph?max_nodes={UI_GRAPH_EXPORT_MAX_NODES}&include_tags=true&project=alpha"
            )
        );
        assert_eq!(
            http_neighbors_path("doc://one", 3, 50, Some("alpha"), true),
            "v1/neighbors?seed=doc%3A%2F%2Fone&depth=3&max_nodes=50&include_tags=true&project=alpha"
        );
    }

    #[test]
    fn reaching_server_export_limit_marks_result_as_potentially_truncated() {
        let limit = UI_GRAPH_EXPORT_MAX_NODES as usize;
        assert!(!http_export_may_be_truncated(limit - 1));
        assert!(http_export_may_be_truncated(limit));
        assert!(http_export_may_be_truncated(limit + 1));
    }

    #[test]
    fn lazy_http_expand_sends_project_scoped_request() {
        let graph = r#"{"nodes":[{"id":"a","kind":"document","label":"A","document_id":"d1","uri":"doc://a","resolved":true,"metadata_json":"{}"}],"edges":[]}"#;
        let gateway = FakeGateway::ok(graph);
        let current = GraphView {
            nodes: vec![node("a", "A")],
            edges: Vec::new(),
        };
        let expanded = expand_neighbors_http_with_client(
            &gateway,
            "http://gateway/",
            &current,
            "node/42",
            120,
            Some("Project A/B"),
            false,
        )
        .unwrap();
        assert_eq!(expanded.nodes.len(), 1);
        let requests = gateway.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::Get);
        assert_eq!(
            requests[0].url,
            "http://gateway/v1/neighbors?seed=node%2F42&depth=1&max_nodes=120&include_tags=false&project=Project%20A%2FB"
        );
    }

    #[test]
    fn project_catalog_is_authoritative_and_reports_independent_failures() {
        let gateway = FakeGateway::ok(
            r#"{"items":[{"project_id":"beta"},{"project_id":"alpha"},{"project_id":"alpha"}]}"#,
        );
        let projects =
            fetch_project_catalog_with_client(&gateway, "http://gateway/").expect("catalog");
        assert_eq!(projects, ["alpha", "beta"]);

        let unavailable = FakeGateway {
            response: Response {
                status: 503,
                body: "store busy".to_string(),
            },
            requests: Mutex::new(Vec::new()),
        };
        let error = fetch_project_catalog_with_client(&unavailable, "http://gateway")
            .expect_err("catalog failure must be visible");
        assert_eq!(error, "HTTP 503 · Project catalog: store busy");
    }

    #[test]
    fn malformed_graph_success_never_exposes_seed_or_gateway_url() {
        let gateway = FakeGateway::ok(r#"{"nodes":[{"id":"private-value""#);
        let error = load_http_with_client(
            &gateway,
            "http://user:secret@gateway",
            Some("private-seed?token=private-token"),
            1,
            Some("private-project"),
            false,
        )
        .unwrap_err();

        assert!(error.starts_with("Could not parse Project graph response"));
        for secret in [
            "user",
            "secret",
            "private-seed",
            "private-token",
            "private-project",
            "private-value",
        ] {
            assert!(!error.contains(secret));
        }
    }
}
