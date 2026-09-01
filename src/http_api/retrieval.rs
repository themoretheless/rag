use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use super::{
    error::{api_err, api_ok},
    HttpState,
};
use crate::error::AppError;
use crate::models::{DocumentFilter, DrawerListItem};
use crate::retrieval::{self, SearchCommand, SimilarDocumentsQuery};

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/search", post(search_http))
        .route("/v1/multi-get", post(multi_get))
        .route("/v1/expand-chunks", get(expand_chunks))
        .route("/v1/find-similar", get(find_similar))
        .route("/v1/documents", get(documents))
}

#[derive(Deserialize)]
struct SearchBody {
    query: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    top_k: Option<usize>,
    #[serde(default)]
    wing: Option<String>,
    #[serde(default)]
    room: Option<String>,
    #[serde(default)]
    layer: Option<String>,
    #[serde(default)]
    source_file: Option<String>,
    #[serde(default)]
    include_archived: bool,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

async fn search_http(
    State(state): State<HttpState>,
    Json(body): Json<SearchBody>,
) -> impl IntoResponse {
    let command = SearchCommand {
        query: body.query,
        mode: body.mode,
        default_mode: state.config.default_search_mode,
        top_k: body.top_k,
        default_top_k: state.config.default_top_k,
        document_id: None,
        wing: clean(body.wing),
        room: clean(body.room),
        layer: clean(body.layer),
        source_file: clean(body.source_file),
        include_archived: body.include_archived,
        min_score: None,
        diversity: None,
        group_by: None,
        recency_half_life_days: None,
        max_context_tokens: None,
        max_chunks_per_document: None,
        context_expansion: None,
        neighbor_chunks: None,
        timeout_ms: body.timeout_ms.or(Some(5_000)),
        fts_stemmer: state.config.fts_stemmer.clone(),
    };
    match retrieval::execute_search(&state.store, state.embedder.as_ref(), command).await {
        Ok(hits) => api_ok(json!({"ok": true, "count": hits.len(), "items": hits})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct MultiGetBody {
    document_ids: Vec<String>,
    #[serde(default)]
    include_chunks: bool,
}

async fn multi_get(
    State(state): State<HttpState>,
    Json(body): Json<MultiGetBody>,
) -> impl IntoResponse {
    match retrieval::multi_get(&state.store, body.document_ids, body.include_chunks) {
        Ok(result) => api_ok(json!({
            "ok": true,
            "items": result.documents,
            "missing": result.missing,
        })),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct ExpandQuery {
    document_id: String,
    chunk_index: i32,
    #[serde(default)]
    radius: Option<u32>,
}

async fn expand_chunks(
    State(state): State<HttpState>,
    Query(query): Query<ExpandQuery>,
) -> impl IntoResponse {
    match retrieval::expand_chunks(
        &state.store,
        &query.document_id,
        query.chunk_index,
        query.radius.unwrap_or(1),
    ) {
        Ok(items) => api_ok(json!({"ok": true, "items": items})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct SimilarQuery {
    document_id: String,
    #[serde(default)]
    top_k: Option<usize>,
}

async fn find_similar(
    State(state): State<HttpState>,
    Query(query): Query<SimilarQuery>,
) -> impl IntoResponse {
    let query = SimilarDocumentsQuery {
        document_id: query.document_id,
        top_k: query.top_k.unwrap_or(state.config.default_top_k),
        wing: None,
        room: None,
        fts_stemmer: state.config.fts_stemmer.clone(),
    };
    match retrieval::find_similar(&state.store, query) {
        Ok(items) => api_ok(json!({"ok": true, "items": items})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct DocumentsQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    wing: Option<String>,
    #[serde(default)]
    room: Option<String>,
    #[serde(default)]
    layer: Option<String>,
}

async fn documents(
    State(state): State<HttpState>,
    Query(query): Query<DocumentsQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = match decode_cursor(query.cursor.as_deref()) {
        Ok(value) => value,
        Err(error) => return api_err(error),
    };
    let filter = DocumentFilter {
        wing: clean(query.wing),
        room: clean(query.room),
        layer: clean(query.layer),
        include_archived: Some(false),
        ..Default::default()
    };
    match state.store.list_documents_filtered(&filter) {
        Ok(documents) => {
            let total = documents.len();
            let items = documents
                .iter()
                .skip(offset)
                .take(limit)
                .map(DrawerListItem::from)
                .collect::<Vec<_>>();
            let next_cursor =
                (offset + items.len() < total).then(|| encode_cursor(offset + items.len()));
            api_ok(json!({
                "ok": true,
                "items": items,
                "page": {"limit": limit, "next_cursor": next_cursor, "total": total},
            }))
        }
        Err(error) => api_err(error),
    }
}

pub(super) fn encode_cursor(offset: usize) -> String {
    format!("v1:{offset}")
}

pub(super) fn decode_cursor(cursor: Option<&str>) -> Result<usize, AppError> {
    match cursor {
        None | Some("") => Ok(0),
        Some(value) => value
            .strip_prefix("v1:")
            .and_then(|offset| offset.parse().ok())
            .ok_or_else(|| AppError::config("invalid cursor")),
    }
}

fn clean(value: Option<String>) -> Option<String> {
    value.filter(|item| !item.trim().is_empty())
}
