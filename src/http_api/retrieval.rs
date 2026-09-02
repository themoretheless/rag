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
use crate::db::search::{attach_context, ContextExpansion};
use crate::error::AppError;
use crate::mcp::tools::PackHitParams;
use crate::models::{DocumentFilter, DrawerListItem, SearchHit};
use crate::retrieval::{self, SearchCommand, SimilarDocumentsQuery};
use crate::search_pack::pack_hits;
use crate::telemetry;

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/search", post(search_http))
        .route("/v1/pack-context", post(pack_context_http))
        .route("/v1/multi-get", post(multi_get))
        .route("/v1/expand-chunks", get(expand_chunks))
        .route("/v1/find-similar", get(find_similar))
        .route("/v1/documents", get(documents))
        .route("/v1/revisions", get(revisions))
}

#[derive(Deserialize)]
struct RevisionsQuery { document_id: String }

async fn revisions(State(state): State<HttpState>, Query(query): Query<RevisionsQuery>) -> impl IntoResponse {
    match state.store.list_document_revisions(&query.document_id) {
        Ok(items) => api_ok(json!({"ok":true, "items":items})),
        Err(error) => api_err(error),
    }
}

/// Full `SearchParams` mirror (same names as the MCP `search` tool).
#[derive(Deserialize)]
struct SearchBody {
    query: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    top_k: Option<usize>,
    #[serde(default)]
    document_id: Option<String>,
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
    min_score: Option<f32>,
    /// `mmr` | `collapse_by_document`.
    #[serde(default)]
    diversity: Option<String>,
    /// `document` | `none`.
    #[serde(default)]
    group_by: Option<String>,
    #[serde(default)]
    recency_half_life_days: Option<f64>,
    #[serde(default)]
    max_context_tokens: Option<usize>,
    #[serde(default)]
    max_chunks_per_document: Option<usize>,
    /// `neighbors` | `parent_section`.
    #[serde(default)]
    context_expansion: Option<String>,
    #[serde(default)]
    neighbor_chunks: Option<usize>,
    #[serde(default)]
    rrf_k: Option<f32>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

async fn search_http(
    State(state): State<HttpState>,
    Json(body): Json<SearchBody>,
) -> impl IntoResponse {
    let call = telemetry::begin(
        "http",
        "http",
        "search",
        Some(&json!({"query": body.query, "mode": body.mode, "top_k": body.top_k})),
    );
    let command = SearchCommand {
        query: body.query,
        mode: body.mode,
        default_mode: state.config.default_search_mode,
        top_k: body.top_k,
        default_top_k: state.config.default_top_k,
        document_id: clean(body.document_id),
        wing: clean(body.wing),
        room: clean(body.room),
        layer: clean(body.layer),
        source_file: clean(body.source_file),
        include_archived: body.include_archived,
        min_score: body.min_score,
        diversity: clean(body.diversity),
        group_by: clean(body.group_by),
        recency_half_life_days: body.recency_half_life_days,
        max_context_tokens: body.max_context_tokens.or(Some(state.config.max_context_tokens)),
        max_chunks_per_document: body
            .max_chunks_per_document
            .or(Some(state.config.max_chunks_per_doc)),
        context_expansion: clean(body.context_expansion),
        neighbor_chunks: body.neighbor_chunks,
        timeout_ms: body.timeout_ms.or(Some(5_000)),
        fts_stemmer: state.config.fts_stemmer.clone(),
        rrf_k: body.rrf_k,
    };
    let mode = command.mode.clone().unwrap_or_else(|| command.default_mode.as_str().to_string());
    match retrieval::execute_search(&state.store, state.embedder.as_ref(), command).await {
        Ok(hits) => {
            call.finish(true, None, Some(format!("{} hits", hits.len())));
            let timings = hits.first().and_then(|hit| hit.explanation.clone());
            api_ok(json!({
                "ok": true,
                "mode": mode,
                "count": hits.len(),
                "timings": timings,
                "items": hits,
            }))
        }
        Err(error) => {
            call.finish(false, Some(error.to_string()), None);
            api_err(error)
        }
    }
}

/// `pack_context` mirror: pack ranked hits under a token budget with optional expansion.
#[derive(Deserialize)]
struct PackContextBody {
    hits: Vec<PackHitParams>,
    #[serde(default)]
    max_tokens: Option<usize>,
    #[serde(default)]
    context_expansion: Option<String>,
    #[serde(default)]
    neighbor_chunks: Option<usize>,
}

async fn pack_context_http(
    State(state): State<HttpState>,
    Json(body): Json<PackContextBody>,
) -> impl IntoResponse {
    let expansion = match clean(body.context_expansion).as_deref().map(ContextExpansion::parse) {
        Some(Ok(value)) => Some(value),
        Some(Err(error)) => return api_err(error),
        None => None,
    };
    let mut hits: Vec<SearchHit> = body.hits.into_iter().map(SearchHit::from).collect();
    if let Err(error) =
        attach_context(&state.store, &mut hits, expansion, body.neighbor_chunks.unwrap_or(1))
    {
        return api_err(error);
    }
    let packed = pack_hits(&hits, body.max_tokens.unwrap_or(state.config.max_context_tokens));
    api_ok(json!({
        "ok": true,
        "total_tokens": packed.total_tokens,
        "max_tokens": packed.max_tokens,
        "omitted_count": packed.omitted_count,
        "context_text": packed.context_text,
        "hits": packed.hits,
    }))
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
