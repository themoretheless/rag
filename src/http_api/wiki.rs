//! Wiki catalog, write (CAS), and backlinks HTTP handlers.

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::db::store::WikiPageMetaFilter;
use crate::error::AppError;
use crate::models::resolve_if_match;
use crate::util::wiki_slug_from_uri;
use crate::wiki::{self, WriteWikiOpts};

use super::error::{api_err, api_ok};
use super::HttpState;

/// Wiki routes: `GET|PUT /v1/wiki`, `GET /v1/backlinks`.
pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/wiki", get(wiki_list).put(wiki_put))
        .route("/v1/backlinks", get(wiki_backlinks))
}

/// Query for `GET /v1/wiki` — optional text filter, pagination, placement/kind.
#[derive(Debug, Deserialize)]
struct WikiListQuery {
    /// Case-insensitive substring on title / slug / uri / summary / category / kind.
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    wing: Option<String>,
    #[serde(default)]
    room: Option<String>,
}

/// Catalog of wiki pages for Obsidian/Notion-style UI sidebar (no body load).
///
/// `GET /v1/wiki?q=&limit=&offset=&kind=&category=&wing=&room=`
async fn wiki_list(
    State(st): State<HttpState>,
    Query(q): Query<WikiListQuery>,
    headers: HeaderMap,
) -> Response {
    wiki_list_with_headers(st, q, headers).await
}

async fn wiki_list_with_headers(st: HttpState, q: WikiListQuery, headers: HeaderMap) -> Response {
    let cursor_offset = match super::retrieval::decode_cursor(q.cursor.as_deref()) { Ok(v) => v, Err(e) => return api_err(e) };
    let filter = WikiPageMetaFilter {
        q: q.q
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        limit: q.limit.map(|n| (n as usize).clamp(1, 10_000)),
        offset: Some(q.offset.map(|n| n as usize).unwrap_or(cursor_offset)),
        kind: q
            .kind
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        category: q
            .category
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        wing: q
            .wing
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        room: q
            .room
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
    };
    match st.store.list_wiki_page_metas_filtered(&filter) {
        Ok((pages, total)) => {
            let etag = format!("\"{}\"", blake3::hash(&serde_json::to_vec(&pages).unwrap_or_default()).to_hex());
            if headers.get(axum::http::header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
                return StatusCode::NOT_MODIFIED.into_response();
            }
            let offset = filter.offset.unwrap_or(0); let next = (offset + pages.len() < total).then(|| super::retrieval::encode_cursor(offset + pages.len()));
            let mut response = api_ok(json!({
            "ok": true,
            "count": pages.len(),
            "total": total,
            "limit": filter.limit,
            "offset": offset,
            "items": pages.clone(),
            "pages": pages,
            "page": {"limit":filter.limit,"next_cursor":next,"total":total},
        }));
            response.headers_mut().insert(axum::http::header::ETAG, etag.parse().unwrap()); response
        },
        Err(e) => api_err(e),
    }
}

/// JSON body for `PUT /v1/wiki` (mirrors MCP `write_wiki_page` params).
///
/// `slug` is preferred; if empty, derived from `uri` (`wiki://…`).
#[derive(Debug, Deserialize)]
struct WikiPutBody {
    #[serde(default)]
    slug: Option<String>,
    /// Optional document id (ignored for write key; accepted for UI compatibility).
    #[serde(default)]
    id: Option<String>,
    /// Optional `wiki://slug` URI when `slug` omitted.
    #[serde(default)]
    uri: Option<String>,
    title: String,
    content: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    /// Optimistic concurrency: revision from last GET document / MCP get_wiki_page.
    #[serde(default)]
    if_match_revision: Option<i64>,
    /// Same as `if_match_revision` as etag (`W/"3"`, `"3"`, or bare digits).
    #[serde(default)]
    if_match_etag: Option<String>,
}

/// Create/overwrite a wiki page via [`wiki::write_wiki_page_with_opts`].
///
/// Optional `if_match_revision` / `if_match_etag` enforce CAS; mismatch → **409**.
async fn wiki_put(State(st): State<HttpState>, Json(body): Json<WikiPutBody>) -> impl IntoResponse {
    if body.content.len() > super::MAX_HTTP_BODY_BYTES as usize { return api_err(AppError::config("wiki content exceeds HTTP body limit")); }
    let if_match = match resolve_if_match(
        body.if_match_revision,
        body.if_match_etag.as_deref(),
    ) {
        Ok(v) => v,
        Err(e) => return api_err(e),
    };
    let slug_from_uri = body.uri.as_deref().and_then(wiki_slug_from_uri);
    let slug = body
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or(slug_from_uri)
        .unwrap_or_default();
    let slug = slug.trim();
    if slug.is_empty() {
        return api_err(AppError::config("slug required (or uri=wiki://slug)"));
    }
    let _ = body.id; // accepted for UI payloads; write is slug-keyed
    match wiki::write_wiki_page_with_opts(
        &st.store,
        &st.embedder,
        &st.config,
        slug,
        &body.title,
        &body.content,
        body.kind.as_deref().unwrap_or("wiki"),
        body.category.as_deref(),
        body.summary.as_deref(),
        body.agent.as_deref(),
        WriteWikiOpts {
            if_match_revision: if_match,
            ..Default::default()
        },
    )
    .await
    {
        Ok(res) => api_ok(json!({
            "ok": true,
            "document_id": res.document_id,
            "uri": res.uri,
            "slug": res.slug,
            "chunk_count": res.chunk_count,
            "node_id": res.node_id,
            "edge_count": res.edge_count,
            "index_id": res.index_id,
            "revision": res.revision,
            "etag": res.etag,
        })),
        Err(e) => api_err(e),
    }
}

/// Backlinks for a wiki/raw document: `GET /v1/backlinks?id=<document_id>`.
async fn wiki_backlinks(
    State(st): State<HttpState>,
    Query(q): Query<BacklinksQuery>,
) -> impl IntoResponse {
    let id = q.id.trim();
    if id.is_empty() {
        return api_err(AppError::config("id query param required"));
    }
    match st.store.wiki_backlinks_for_document(id) {
        Ok(rows) => {
            let links: Vec<_> = rows
                .into_iter()
                .map(|(label, key)| json!({ "label": label, "id": key }))
                .collect();
            api_ok(json!({ "ok": true, "count": links.len(), "backlinks": links }))
        }
        Err(e) => api_err(e),
    }
}

#[derive(Debug, Deserialize)]
struct BacklinksQuery {
    id: String,
}
