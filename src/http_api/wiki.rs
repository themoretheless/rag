//! Wiki catalog, write (CAS), and backlinks HTTP handlers.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::db::store::WikiPageMetaFilter;
use crate::db::{DEFAULT_CATALOG_PAGE_SIZE, MAX_CATALOG_PAGE_SIZE};
use crate::error::AppError;
use crate::wiki;

use super::HttpState;
use super::error::{api_err, api_ok};
use crate::app::{WikiWriteCommand, WikiWriteMode};

/// Wiki routes: `GET|POST|PUT /v1/wiki`, `GET /v1/backlinks`.
pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/wiki", get(wiki_list).post(wiki_post).put(wiki_put))
        .route("/v1/backlinks", get(wiki_backlinks))
        .route("/v1/wiki-review", get(wiki_review))
}

/// Read-only freshness queue. Never invoke an LLM or acknowledge stale evidence.
async fn wiki_review(State(st): State<HttpState>) -> Response {
    let store = st.store.clone();
    match super::run_blocking("wiki review", move || wiki::find_stale_wiki(&store)).await {
        Ok(items) => api_ok(json!({"items": items})),
        Err(error) => api_err(error),
    }
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
    let cursor_offset = match super::retrieval::decode_cursor(q.cursor.as_deref()) {
        Ok(v) => v,
        Err(e) => return api_err(e),
    };
    let page_limit = q
        .limit
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_CATALOG_PAGE_SIZE)
        .clamp(1, MAX_CATALOG_PAGE_SIZE);
    let page_offset = q
        .offset
        .map(|value| value as usize)
        .unwrap_or(cursor_offset);
    let filter = WikiPageMetaFilter {
        q: q.q
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        limit: Some(page_limit),
        offset: Some(page_offset),
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
    let store = st.store.clone();
    let page = super::run_blocking("wiki catalog", move || {
        store.documents().list_wiki_page_metas_filtered(&filter)
    })
    .await;
    match page {
        Ok((pages, total)) => {
            let etag = format!(
                "\"{}\"",
                blake3::hash(&serde_json::to_vec(&pages).unwrap_or_default()).to_hex()
            );
            if headers
                .get(axum::http::header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                == Some(etag.as_str())
            {
                return StatusCode::NOT_MODIFIED.into_response();
            }
            let next = (page_offset + pages.len() < total)
                .then(|| super::retrieval::encode_cursor(page_offset + pages.len()));
            let mut response = api_ok(json!({
                "ok": true,
                "count": pages.len(),
                "total": total,
                "limit": page_limit,
                "offset": page_offset,
                "items": pages.clone(),
                "pages": pages,
                "page": {"limit":page_limit,"next_cursor":next,"total":total},
            }));
            super::error::insert_etag(&mut response, &etag);
            response
        }
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
    /// Existing document id for update semantics. When supplied, it must resolve
    /// to the same canonical wiki URI as `slug` / `uri`.
    #[serde(default)]
    id: Option<String>,
    /// Optional `wiki://slug` URI when `slug` omitted.
    #[serde(default)]
    uri: Option<String>,
    title: String,
    content: String,
    #[serde(default)]
    wing: Option<String>,
    #[serde(default)]
    room: Option<String>,
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

/// Create or update a wiki page (`WikiWriteMode::Upsert`).
///
/// Optional `if_match_revision` / `if_match_etag` enforce CAS; mismatch → **409**.
async fn wiki_put(State(st): State<HttpState>, Json(body): Json<WikiPutBody>) -> impl IntoResponse {
    wiki_write(st, body, false).await
}

/// Create a new wiki page, failing atomically if its canonical URI is occupied.
async fn wiki_post(
    State(st): State<HttpState>,
    Json(body): Json<WikiPutBody>,
) -> impl IntoResponse {
    wiki_write(st, body, true).await
}

async fn wiki_write(st: HttpState, body: WikiPutBody, create_only: bool) -> Response {
    if body.content.len() > super::MAX_HTTP_BODY_BYTES {
        return api_err(AppError::invalid("wiki content exceeds HTTP body limit"));
    }
    let command = WikiWriteCommand {
        mode: if create_only {
            WikiWriteMode::Create
        } else {
            WikiWriteMode::Upsert
        },
        slug: body.slug,
        uri: body.uri,
        id: body.id,
        title: body.title,
        content: body.content,
        wing: body.wing,
        room: body.room,
        kind: body.kind,
        category: body.category,
        summary: body.summary,
        agent: body.agent,
        if_match_revision: body.if_match_revision,
        if_match_etag: body.if_match_etag,
        // PUT on an existing page keeps the current title when it is blank.
        blank_title_is_slug: false,
    };
    match st.wiki_use_case().write(command).await {
        Ok(res) => api_ok(super::retrieval::with_ok(res)),
        Err(e) => api_err(e),
    }
}

/// Backlinks for a wiki/raw document:
/// `GET /v1/backlinks?id=<document_id>&wing=<project>`.
async fn wiki_backlinks(
    State(st): State<HttpState>,
    Query(q): Query<BacklinksQuery>,
) -> impl IntoResponse {
    st.blocking_response("wiki_backlinks", move |store| {
        let id = q.id.trim();
        if id.is_empty() {
            return api_err(AppError::invalid("id query param required"));
        }
        let wing = q
            .wing
            .as_deref()
            .map(str::trim)
            .filter(|wing| !wing.is_empty());
        // One backlinks read serves both the rows and their contexts.
        let view = match store.documents().wiki_backlink_view(id) {
            Ok(view) => view,
            Err(error) => return api_err(error),
        };
        let rows = match wing {
            Some(wing) => store
                .documents()
                .wiki_backlinks_for_document_in_wing(id, wing),
            None => Ok(view
                .as_ref()
                .map(|(node_id, view)| crate::db::store::wiki_backlink_rows(node_id, view))
                .unwrap_or_default()),
        };
        let view = view.map(|(_, view)| view);
        match rows {
            Ok(rows) => {
                // Group wikilink contexts by source document key in one pass.
                let mut contexts_by_key: HashMap<&str, Vec<&str>> = HashMap::new();
                if let Some(view) = view.as_ref() {
                    let key_by_node: HashMap<&str, &str> = view
                        .nodes
                        .iter()
                        .map(|node| {
                            (
                                node.id.as_str(),
                                node.document_id.as_deref().unwrap_or(&node.id),
                            )
                        })
                        .collect();
                    for edge in view.edges.iter().filter(|edge| edge.rel_type == "wikilink") {
                        let (Some(key), Some(context)) = (
                            key_by_node.get(edge.source_id.as_str()),
                            edge.context.as_deref(),
                        ) else {
                            continue;
                        };
                        let contexts = contexts_by_key.entry(key).or_default();
                        if contexts.len() < 3 {
                            contexts.push(context);
                        }
                    }
                }
                let links: Vec<_> = rows
                    .into_iter()
                    .map(|(label, key)| {
                        let contexts = contexts_by_key
                            .get(key.as_str())
                            .cloned()
                            .unwrap_or_default();
                        json!({ "label": label, "id": key, "contexts": contexts })
                    })
                    .collect();
                api_ok(json!({ "ok": true, "count": links.len(), "backlinks": links }))
            }
            Err(e) => api_err(e),
        }
    })
    .await
}

#[derive(Debug, Deserialize)]
struct BacklinksQuery {
    id: String,
    #[serde(default)]
    wing: Option<String>,
}

#[cfg(test)]
mod tests;
