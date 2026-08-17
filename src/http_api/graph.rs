//! Graph + document HTTP handlers (`/v1/graph`, neighbors, find, document).

use axum::extract::{Query, State};
use axum::response::IntoResponse;
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
}

async fn graph(
    State(st): State<HttpState>,
    Query(q): Query<GraphQuery>,
) -> impl IntoResponse {
    let max = q.max_nodes.unwrap_or(500);
    let tags = q.include_tags.unwrap_or(false);
    match st.store.export_graph_for_ui(Some(max), tags) {
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
}

async fn neighbors(
    State(st): State<HttpState>,
    Query(q): Query<NeighborsQuery>,
) -> impl IntoResponse {
    let seed = q.seed.trim();
    if seed.is_empty() {
        return api_err(AppError::config("seed query param required"));
    }
    let depth = q.depth.unwrap_or(1).clamp(1, 3);
    let max_nodes = q.max_nodes.unwrap_or(100).clamp(1, 300);

    // Resolve seed like UI: id, document_id, or label.
    let node_id = match resolve_seed(&st.store, seed) {
        Ok(id) => id,
        Err(e) => return api_err(e),
    };

    match st.store.neighbors(&node_id, depth, max_nodes) {
        Ok(view) => api_ok(view),
        Err(e) => api_err(e),
    }
}

#[derive(Debug, Deserialize)]
struct FindQuery {
    q: String,
}

async fn find_node(
    State(st): State<HttpState>,
    Query(q): Query<FindQuery>,
) -> impl IntoResponse {
    let key = q.q.trim();
    if key.is_empty() {
        return api_err(AppError::config("q required"));
    }
    match resolve_seed(&st.store, key) {
        Ok(id) => match st.store.find_node_by_id(&id) {
            Ok(Some(n)) => api_ok(n),
            Ok(None) => api_err(AppError::not_found(format!("node {id}"))),
            Err(e) => api_err(e),
        },
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
) -> impl IntoResponse {
    match resolve_document(&st.store, &q) {
        Ok(doc) => api_ok(json!({
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
            "updated_at": doc.updated_at.to_rfc3339(),
            "revision": doc.revision,
            "etag": doc.etag(),
        })),
        Err(e) => api_err(e),
    }
}

fn resolve_document(
    store: &Store,
    q: &DocumentQuery,
) -> Result<crate::models::Document, AppError> {
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
    let key = q
        .q
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::config("provide id, uri, or q"))?;

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

    let node_id = resolve_seed(store, key)?;
    let node = store
        .find_node_by_id(&node_id)?
        .ok_or_else(|| AppError::not_found(format!("node {node_id}")))?;
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
    if let Some(n) = store.find_node_by_id(seed)? {
        return Ok(n.id);
    }
    if let Some(n) = store.find_node_by_document_id(seed)? {
        return Ok(n.id);
    }
    let by_label = store.find_nodes_by_label(seed)?;
    if let Some(n) = by_label.into_iter().next() {
        return Ok(n.id);
    }
    Err(AppError::not_found(format!(
        "no graph node for seed '{seed}' (try id, document_id, or exact label)"
    )))
}
