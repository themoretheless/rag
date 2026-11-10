//! Graph + document HTTP handlers (`/v1/graph`, neighbors, find, document).

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use crate::app::NeighborsCommand;
use crate::db::Store;
use crate::error::AppError;

use super::error::{api_err, api_ok};
use super::HttpState;

/// Node cap for a whole-graph export when `max_nodes` is absent.
const DEFAULT_GRAPH_MAX_NODES: u32 = 500;

/// Graph + document UI routes (no `/health`, no wiki).
pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/graph", get(graph))
        .route("/v1/neighbors", get(neighbors))
        .route("/v1/find", get(find_node))
        .route("/v1/document", get(document))
}

#[derive(Debug, Deserialize)]
struct GraphQuery {
    #[serde(default)]
    max_nodes: Option<u32>,
    #[serde(default)]
    include_tags: Option<bool>,
    #[serde(default, alias = "wing")]
    project: Option<String>,
}

async fn graph(State(st): State<HttpState>, Query(q): Query<GraphQuery>) -> impl IntoResponse {
    let max = q.max_nodes.unwrap_or(DEFAULT_GRAPH_MAX_NODES);
    let tags = q.include_tags.unwrap_or(false);
    let project = q.project.and_then(|project| {
        let project = project.trim();
        (!project.is_empty()).then(|| project.to_owned())
    });
    let store = st.store.clone();
    let result = super::run_blocking("graph", move || match project.as_deref() {
        Some(project) => store
            .graph()
            .export_project_graph_for_ui(project, Some(max), tags),
        None => store.graph().export_graph_for_ui(Some(max), tags),
    })
    .await;
    match result {
        Ok(view) => api_ok(view),
        Err(e) => api_err(e),
    }
}

/// `GET /v1/neighbors`: `seed` or its alias `node_id`, same rules as MCP
/// `get_neighbors` (see [`crate::app::GraphUseCase::neighbors`]).
#[derive(Debug, Deserialize)]
struct NeighborsQuery {
    #[serde(default, alias = "node_id")]
    seed: String,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    max_nodes: Option<u32>,
    #[serde(default, alias = "wing")]
    project: Option<String>,
    #[serde(default)]
    include_tags: Option<bool>,
    /// Comma-separated relation set (MCP passes a list).
    #[serde(default)]
    rel_types: Option<String>,
}

async fn neighbors(
    State(st): State<HttpState>,
    Query(q): Query<NeighborsQuery>,
) -> impl IntoResponse {
    let command = NeighborsCommand {
        seed: q.seed,
        depth: q.depth,
        max_nodes: q.max_nodes,
        rel_types: q.rel_types.map(|rels| {
            rels.split(',')
                .map(str::trim)
                .filter(|rel| !rel.is_empty())
                .map(str::to_owned)
                .collect()
        }),
        include_tags: q.include_tags,
        project: q.project,
    };
    match st.graph_use_case().neighbors(command).await {
        Ok(view) => api_ok(view),
        Err(e) => api_err(e),
    }
}

#[derive(Debug, Deserialize)]
struct FindQuery {
    q: String,
}

async fn find_node(State(st): State<HttpState>, Query(q): Query<FindQuery>) -> impl IntoResponse {
    let key = q.q.trim();
    if key.is_empty() {
        return api_err(AppError::invalid("q required"));
    }
    let key = key.to_owned();
    let store = st.store.clone();
    let result = super::run_blocking("find graph node", move || {
        let id = resolve_seed(&store, &key)?;
        store
            .graph()
            .find_node_by_id(&id)?
            .ok_or_else(|| AppError::not_found(format!("node {id}")))
    })
    .await;
    match result {
        Ok(node) => api_ok(node),
        Err(e) => api_err(e),
    }
}

/// Query for `GET /v1/document` — resolve by document id, uri, or node seed (label/id).
#[derive(Debug, Deserialize)]
struct DocumentQuery {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    uri: Option<String>,
    /// Same resolve rules as graph seed: node id, document_id, or label.
    #[serde(default)]
    q: Option<String>,
}

/// Full document body for graph UI "Read content" (wiki / raw / any layer).
async fn document(
    State(st): State<HttpState>,
    Query(q): Query<DocumentQuery>,
    headers: HeaderMap,
) -> Response {
    let store = st.store.clone();
    let result = super::run_blocking("get document", move || resolve_document(&store, &q)).await;
    match result {
        Ok(doc) => {
            let etag = doc.etag();
            if headers
                .get(axum::http::header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                == Some(etag.as_str())
            {
                return StatusCode::NOT_MODIFIED.into_response();
            }
            let mut response = api_ok(json!({
            "id": doc.id,
            "uri": doc.uri,
            "title": doc.title,
            "layer": doc.layer,
            "kind": doc.kind,
            "content": doc.content,
            "content_hash": doc.content_hash,
            "wing": doc.wing,
            "room": doc.room,
            "source_file": doc.source_file,
            "source_versions": doc
                .metadata_field("source_versions")
                .filter(serde_json::Value::is_array)
                .unwrap_or_else(|| json!([])),
            "updated_at": doc.updated_at.to_rfc3339(),
            "revision": doc.revision,
            "etag": doc.etag(),
            }));
            super::error::insert_etag(&mut response, &etag);
            response
        }
        Err(e) => api_err(e),
    }
}

fn resolve_document(store: &Store, q: &DocumentQuery) -> Result<crate::models::Document, AppError> {
    if let Some(id) = q.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return store
            .documents()
            .get_document(id)?
            .ok_or_else(|| AppError::not_found(format!("document id '{id}'")));
    }
    if let Some(uri) = q.uri.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return store
            .documents()
            .find_by_uri(uri)?
            .ok_or_else(|| AppError::not_found(format!("document uri '{uri}'")));
    }
    let key =
        q.q.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::invalid("provide id, uri, or q"))?;

    // Direct document id or uri before graph seed.
    if let Some(doc) = store.documents().get_document(key)? {
        return Ok(doc);
    }
    if let Some(doc) = store.documents().find_by_uri(key)? {
        return Ok(doc);
    }
    if key.starts_with("wiki://") || key.starts_with("file://") || key.starts_with("text://") {
        return Err(AppError::not_found(format!("document uri '{key}'")));
    }

    let node = store
        .graph()
        .resolve_seed(key)?
        .ok_or_else(|| seed_not_found(key))?;
    if let Some(doc_id) = node.document_id.as_deref() {
        return store
            .documents()
            .get_document(doc_id)?
            .ok_or_else(|| AppError::not_found(format!("document id '{doc_id}'")));
    }
    if let Some(uri) = node.uri.as_deref() {
        if let Some(doc) = store.documents().find_by_uri(uri)? {
            return Ok(doc);
        }
    }
    Err(AppError::not_found(format!(
        "node '{}' ({}) has no document body (stub/tag?)",
        node.label, node.kind
    )))
}

fn resolve_seed(store: &Store, seed: &str) -> Result<String, AppError> {
    store
        .graph()
        .resolve_seed(seed)?
        .map(|node| node.id)
        .ok_or_else(|| seed_not_found(seed))
}

fn seed_not_found(seed: &str) -> AppError {
    AppError::not_found(format!(
        "no graph node for seed '{seed}' (try id, document_id, or exact label)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn state() -> HttpState {
        let root = crate::testkit::scratch_dir();
        let config = crate::config::Config {
            db_path: root.join("graph-http.duckdb"),
            embedding_dims: 8,
            ..crate::config::Config::for_tests()
        };
        let store = Arc::new(Store::open(&config.db_path).unwrap());
        HttpState::from_context(
            crate::context::AppContext::with_store(
                store,
                Arc::new(crate::embeddings::MockEmbedder::new(8)),
                config,
            ),
            false,
        )
    }

    fn query(seed: &str, project: Option<&str>) -> NeighborsQuery {
        serde_json::from_value(json!({"seed": seed, "project": project})).unwrap()
    }

    #[tokio::test]
    async fn unknown_seed_is_not_found_instead_of_an_empty_graph() {
        let st = state();
        for project in [None, Some("rag")] {
            let response = neighbors(State(st.clone()), Query(query("missing", project)))
                .await
                .into_response();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{project:?}");
        }
        let blank = neighbors(State(st), Query(query("   ", None)))
            .await
            .into_response();
        assert_eq!(blank.status(), StatusCode::BAD_REQUEST);
    }
}
