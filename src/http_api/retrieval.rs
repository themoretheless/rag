use axum::{extract::{Query, State}, response::IntoResponse, routing::{get, post}, Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::db::search::{search, DiversityMode, SearchQuery, MAX_QUERY_CHARS, MAX_TOP_K};
use crate::error::AppError;
use crate::models::{DocumentFilter, DrawerListItem, SearchMode};
use super::{error::{api_err, api_ok}, HttpState};

pub(super) fn routes() -> Router<HttpState> {
    Router::new().route("/v1/search", post(search_http)).route("/v1/multi-get", post(multi_get))
        .route("/v1/expand-chunks", get(expand_chunks)).route("/v1/find-similar", get(find_similar))
        .route("/v1/documents", get(documents))
}

#[derive(Deserialize)] struct SearchBody { query: String, #[serde(default)] mode: Option<String>, #[serde(default)] top_k: Option<usize>,
    #[serde(default)] wing: Option<String>, #[serde(default)] room: Option<String>, #[serde(default)] layer: Option<String>,
    #[serde(default)] source_file: Option<String>, #[serde(default)] include_archived: bool, #[serde(default)] timeout_ms: Option<u64> }
async fn search_http(State(st): State<HttpState>, Json(body): Json<SearchBody>) -> impl IntoResponse {
    if body.query.chars().count() > MAX_QUERY_CHARS { return api_err(AppError::config(format!("query exceeds {MAX_QUERY_CHARS} characters"))); }
    let mode = match SearchMode::parse(body.mode.as_deref().unwrap_or("hybrid")) { Ok(v) => v, Err(e) => return api_err(AppError::config(e)) };
    let embedding = if matches!(mode, SearchMode::Vec | SearchMode::Hybrid) {
        match st.embedder.embed(std::slice::from_ref(&body.query)).await { Ok(v) => match v.into_iter().next() { Some(v) => Some(v), None => return api_err(AppError::embeddings("embedder returned no vector")) }, Err(e) => return api_err(e) }
    } else { None };
    let query = SearchQuery { mode, top_k: body.top_k.unwrap_or(st.config.default_top_k), query_text: Some(body.query), query_embedding: embedding,
        wing: clean(body.wing), room: clean(body.room), layer: clean(body.layer), source_file: clean(body.source_file), include_archived: body.include_archived,
        timeout_ms: body.timeout_ms.or(Some(5_000)), fts_stemmer: st.config.fts_stemmer.clone(), ..SearchQuery::default() };
    match search(&st.store, &query) { Ok(hits) => api_ok(json!({"ok":true,"count":hits.len(),"items":hits})), Err(e) => api_err(e) }
}

#[derive(Deserialize)] struct MultiGetBody { document_ids: Vec<String>, #[serde(default)] include_chunks: bool }
async fn multi_get(State(st): State<HttpState>, Json(body): Json<MultiGetBody>) -> impl IntoResponse {
    if body.document_ids.is_empty() || body.document_ids.len() > 100 { return api_err(AppError::config("document_ids must contain 1..=100 ids")); }
    let mut items = Vec::new(); let mut missing = Vec::new();
    for id in body.document_ids { match st.store.get_document(id.trim()) { Ok(Some(doc)) => {
        let chunks = if body.include_chunks { match st.store.list_chunks_for_document(&doc.id) { Ok(v) => Some(v), Err(e) => return api_err(e) } } else { None };
        items.push(json!({"document":doc,"chunks":chunks}));
    }, Ok(None) => missing.push(id), Err(e) => return api_err(e) } }
    api_ok(json!({"ok":true,"items":items,"missing":missing}))
}

#[derive(Deserialize)] struct ExpandQuery { document_id: String, chunk_index: i32, #[serde(default)] radius: Option<u32> }
async fn expand_chunks(State(st): State<HttpState>, Query(q): Query<ExpandQuery>) -> impl IntoResponse {
    let radius = q.radius.unwrap_or(1).min(20) as i32; let start = q.chunk_index.saturating_sub(radius); let end = q.chunk_index.saturating_add(radius);
    match st.store.list_chunks_for_document(q.document_id.trim()) { Ok(chunks) => {
        let items: Vec<_> = chunks.into_iter().filter(|c| c.chunk_index >= start && c.chunk_index <= end).collect();
        if items.is_empty() { api_err(AppError::not_found("no chunks in requested range")) } else { api_ok(json!({"ok":true,"items":items})) }
    }, Err(e) => api_err(e) }
}

#[derive(Deserialize)] struct SimilarQuery { document_id: String, #[serde(default)] top_k: Option<usize> }
async fn find_similar(State(st): State<HttpState>, Query(q): Query<SimilarQuery>) -> impl IntoResponse {
    let chunks = match st.store.list_chunks_for_document(q.document_id.trim()) { Ok(v) => v, Err(e) => return api_err(e) };
    let dims = chunks.first().map(|c| c.embedding.len()).unwrap_or(0); if dims == 0 { return api_err(AppError::config("seed document has no embeddings")); }
    let mut centroid = vec![0.0f32; dims]; for chunk in chunks { if chunk.embedding.len() == dims { for (dst, value) in centroid.iter_mut().zip(chunk.embedding) { *dst += value; } } }
    crate::embeddings::l2_normalize(&mut centroid); let top_k = q.top_k.unwrap_or(st.config.default_top_k).min(MAX_TOP_K - 1);
    match search(&st.store, &SearchQuery { mode: SearchMode::Vec, top_k: top_k + 1, query_embedding: Some(centroid),
        diversity: Some(DiversityMode::CollapseByDocument), max_chunks_per_document: Some(1), ..SearchQuery::default() }) {
        Ok(mut hits) => { hits.retain(|h| h.document_id != q.document_id); hits.truncate(top_k); api_ok(json!({"ok":true,"items":hits})) }, Err(e) => api_err(e)
    }
}

#[derive(Deserialize)] struct DocumentsQuery { #[serde(default)] limit: Option<usize>, #[serde(default)] cursor: Option<String>,
    #[serde(default)] wing: Option<String>, #[serde(default)] room: Option<String>, #[serde(default)] layer: Option<String> }
async fn documents(State(st): State<HttpState>, Query(q): Query<DocumentsQuery>) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50).clamp(1, 200); let offset = match decode_cursor(q.cursor.as_deref()) { Ok(v) => v, Err(e) => return api_err(e) };
    let filter = DocumentFilter { wing: clean(q.wing), room: clean(q.room), layer: clean(q.layer), include_archived: Some(false), ..Default::default() };
    match st.store.list_documents_filtered(&filter) { Ok(docs) => { let total = docs.len(); let items: Vec<DrawerListItem> = docs.iter().skip(offset).take(limit).map(DrawerListItem::from).collect();
        let next = (offset + items.len() < total).then(|| encode_cursor(offset + items.len())); api_ok(json!({"ok":true,"items":items,"page":{"limit":limit,"next_cursor":next,"total":total}})) }, Err(e) => api_err(e) }
}

pub(super) fn encode_cursor(offset: usize) -> String { format!("v1:{offset}") }
pub(super) fn decode_cursor(cursor: Option<&str>) -> Result<usize, AppError> { match cursor { None | Some("") => Ok(0), Some(v) => v.strip_prefix("v1:").and_then(|n| n.parse().ok()).ok_or_else(|| AppError::config("invalid cursor")) } }
fn clean(value: Option<String>) -> Option<String> { value.filter(|v| !v.trim().is_empty()) }
