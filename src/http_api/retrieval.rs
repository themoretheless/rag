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
use crate::db::search::{search, SearchQuery, MAX_QUERY_CHARS};
use crate::error::AppError;
use crate::models::{DocumentFilter, DrawerListItem, SearchMode};
use crate::retrieval::{self, SimilarDocumentsQuery};

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
    if body.query.chars().count() > MAX_QUERY_CHARS {
        return api_err(AppError::config(format!(
            "query exceeds {MAX_QUERY_CHARS} characters"
        )));
    }
    let mode = match SearchMode::parse(body.mode.as_deref().unwrap_or("hybrid")) {
        Ok(value) => value,
        Err(error) => return api_err(AppError::config(error)),
    };
    let embedding = if matches!(mode, SearchMode::Vec | SearchMode::Hybrid) {
        match state
            .embedder
            .embed(std::slice::from_ref(&body.query))
            .await
        {
            Ok(values) => match values.into_iter().next() {
                Some(value) => Some(value),
                None => return api_err(AppError::embeddings("embedder returned no vector")),
            },
            Err(error) => return api_err(error),
        }
    } else {
        None
    };
    let query = SearchQuery {
        mode,
        top_k: body.top_k.unwrap_or(state.config.default_top_k),
        query_text: Some(body.query),
        query_embedding: embedding,
        wing: clean(body.wing),
        room: clean(body.room),
        layer: clean(body.layer),
        source_file: clean(body.source_file),
        include_archived: body.include_archived,
        timeout_ms: body.timeout_ms.or(Some(5_000)),
        fts_stemmer: state.config.fts_stemmer.clone(),
        ..SearchQuery::default()
    };
    match search(&state.store, &query) {
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
