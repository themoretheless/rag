//! Graph + document HTTP handlers (`/v1/graph`, neighbors, find, document).

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use crate::db::Store;
use crate::error::AppError;

use super::error::{api_err, api_ok};
use super::HttpState;

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
    let max = q.max_nodes.unwrap_or(500);
    let tags = q.include_tags.unwrap_or(false);
    let project = q.project.and_then(|project| {
        let project = project.trim();
        (!project.is_empty()).then(|| project.to_owned())
    });
    let store = st.store.clone();
    let result = super::run_blocking("graph", move || match project.as_deref() {
        Some(project) => store.export_project_graph_for_ui(project, Some(max), tags),
        None => store.export_graph_for_ui(Some(max), tags),
    })
    .await;
    match result {
        Ok(view) => api_ok(view),
        Err(e) => api_err(e),
    }
}

#[derive(Debug, Deserialize)]
struct NeighborsQuery {
    seed: String,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    max_nodes: Option<u32>,
    #[serde(default, alias = "wing")]
    project: Option<String>,
    #[serde(default)]
    include_tags: Option<bool>,
}

async fn neighbors(
    State(st): State<HttpState>,
    Query(q): Query<NeighborsQuery>,
) -> impl IntoResponse {
    let seed = q.seed.trim();
    if seed.is_empty() {
        return api_err(AppError::invalid("seed query param required"));
    }
    let depth = q.depth.unwrap_or(1).clamp(1, 3);
    let max_nodes = q
        .max_nodes
        .unwrap_or(100)
        .clamp(1, crate::models::UI_GRAPH_EXPORT_MAX_NODES);

    let seed = seed.to_owned();
    let include_tags = q.include_tags.unwrap_or(false);
    let project = q.project.and_then(|project| {
        let project = project.trim();
        (!project.is_empty()).then(|| project.to_owned())
    });
    let store = st.store.clone();
    if let Some(project) = project {
        let query_project = project.clone();
        let query_seed = seed.clone();
        let result = super::run_blocking("project neighbors", move || {
            store.export_project_neighbors_for_ui(
                &query_project,
                &query_seed,
                depth,
                max_nodes,
                include_tags,
            )
        })
        .await;
        return match result {
            Ok(view) if view.nodes.is_empty() => api_err(AppError::not_found(format!(
                "no graph node for seed '{seed}' in project '{project}'"
            ))),
            Ok(view) => api_ok(view),
            Err(e) => api_err(e),
        };
    }

    let query_seed = seed.clone();
    let result = super::run_blocking("neighbors", move || {
        store.export_pkb_neighbors_for_ui(&query_seed, depth, max_nodes, include_tags)
    })
    .await;
    match result {
        Ok(view) if view.nodes.is_empty() => api_err(AppError::not_found(format!(
            "no graph node for seed '{seed}'"
        ))),
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
            response
                .headers_mut()
                .insert(axum::http::header::ETAG, etag.parse().unwrap());
            response
        }
        Err(e) => api_err(e),
    }
}

fn resolve_document(store: &Store, q: &DocumentQuery) -> Result<crate::models::Document, AppError> {
    if let Some(id) = q.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return store
            .get_document(id)?
            .ok_or_else(|| AppError::not_found(format!("document id '{id}'")));
    }
    if let Some(uri) = q.uri.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return store
            .find_by_uri(uri)?
            .ok_or_else(|| AppError::not_found(format!("document uri '{uri}'")));
    }
    let key =
        q.q.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::invalid("provide id, uri, or q"))?;

    // Direct document id or uri before graph seed.
    if let Some(doc) = store.get_document(key)? {
        return Ok(doc);
    }
    if let Some(doc) = store.find_by_uri(key)? {
        return Ok(doc);
    }
    if key.starts_with("wiki://") || key.starts_with("file://") || key.starts_with("text://") {
        return Err(AppError::not_found(format!("document uri '{key}'")));
    }

    let node = store
        .resolve_seed(key)?
        .ok_or_else(|| seed_not_found(key))?;
    if let Some(doc_id) = node.document_id.as_deref() {
        return store
            .get_document(doc_id)?
            .ok_or_else(|| AppError::not_found(format!("document id '{doc_id}'")));
    }
    if let Some(uri) = node.uri.as_deref() {
        if let Some(doc) = store.find_by_uri(uri)? {
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
        HttpState::new(
            store,
            false,
            config,
            Arc::new(crate::embeddings::MockEmbedder::new(8)),
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
