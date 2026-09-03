//! Wiki catalog, write (CAS), and backlinks HTTP handlers.

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
use crate::models::resolve_if_match;
use crate::util::{slugify, wiki_slug_from_uri, SlugPolicy};
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
        store.list_wiki_page_metas_filtered(&filter)
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
            response
                .headers_mut()
                .insert(axum::http::header::ETAG, etag.parse().unwrap());
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

/// Create/overwrite a wiki page via [`wiki::write_wiki_page_with_opts`].
///
/// Optional `if_match_revision` / `if_match_etag` enforce CAS; mismatch → **409**.
async fn wiki_put(State(st): State<HttpState>, Json(body): Json<WikiPutBody>) -> impl IntoResponse {
    if body.content.len() > super::MAX_HTTP_BODY_BYTES {
        return api_err(AppError::config("wiki content exceeds HTTP body limit"));
    }
    let if_match = match resolve_if_match(body.if_match_revision, body.if_match_etag.as_deref()) {
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
    let canonical_uri = format!("wiki://{}", slugify(slug, SlugPolicy::WikiPage));
    let requested_id = body
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let existing = match requested_id {
        Some(id) => match wiki::get_wiki_page(&st.store, id) {
            Ok(document) if document.uri == canonical_uri => Some(document),
            Ok(document) => {
                return api_err(AppError::conflict(format!(
                    "wiki document id '{id}' belongs to '{}', not requested URI '{canonical_uri}'",
                    document.uri
                )))
            }
            Err(error) => return api_err(error),
        },
        None => match st.store.find_by_uri(&canonical_uri) {
            Ok(document) => document,
            Err(error) => return api_err(error),
        },
    };

    let result = if let Some(existing) = existing {
        wiki::update_wiki_page_cas(
            &st.store,
            &st.embedder,
            &st.config,
            &existing.id,
            Some(&body.title),
            &body.content,
            body.kind.as_deref(),
            body.category.as_deref(),
            body.summary.as_deref(),
            body.agent.as_deref(),
            if_match,
        )
        .await
    } else {
        wiki::write_wiki_page_command(
            &st.store,
            &st.embedder,
            &st.config,
            wiki::WikiWriteCommand {
                slug: slug.to_string(),
                title: body.title,
                content: body.content,
                wing: body.wing,
                room: body.room,
                kind: body.kind.unwrap_or_else(|| "wiki".into()),
                category: body.category,
                summary: body.summary,
                agent: body.agent,
                options: WriteWikiOpts {
                    if_match_revision: if_match,
                    ..Default::default()
                },
            },
        )
        .await
    };
    match result {
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

/// Backlinks for a wiki/raw document:
/// `GET /v1/backlinks?id=<document_id>&wing=<project>`.
async fn wiki_backlinks(
    State(st): State<HttpState>,
    Query(q): Query<BacklinksQuery>,
) -> impl IntoResponse {
    let id = q.id.trim();
    if id.is_empty() {
        return api_err(AppError::config("id query param required"));
    }
    let wing = q
        .wing
        .as_deref()
        .map(str::trim)
        .filter(|wing| !wing.is_empty());
    let rows = match wing {
        Some(wing) => st.store.wiki_backlinks_for_document_in_wing(id, wing),
        None => st.store.wiki_backlinks_for_document(id),
    };
    match rows {
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
    #[serde(default)]
    wing: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::to_bytes;

    use super::*;
    use crate::config::Config;
    use crate::embeddings::MockEmbedder;
    use crate::models::{Document, GraphNode};

    fn test_state(name: &str) -> HttpState {
        let root = tempfile::tempdir().unwrap().keep();
        let config = Config {
            db_path: root.join(format!("{name}.duckdb")),
            embedding_dims: 16,
            ..Config::for_tests()
        };
        let store = Arc::new(crate::db::Store::open(&config.db_path).unwrap());
        HttpState::new(store, false, config, Arc::new(MockEmbedder::new(16)))
    }

    #[tokio::test]
    async fn wiki_put_with_document_id_uses_preserving_update_semantics() {
        let state = test_state("wiki-put-preserve");
        let created = wiki::write_wiki_page(
            &state.store,
            &state.embedder,
            &state.config,
            "project-page",
            "Project page",
            "original body",
            "concept",
            Some("architecture"),
            Some("original summary"),
            None,
        )
        .await
        .expect("create page");
        let mut document = state
            .store
            .get_document(&created.document_id)
            .unwrap()
            .expect("created document");
        document.wing = Some("alpha".into());
        document.room = Some("design".into());
        document.source_file = Some("/sources/alpha/page.md".into());
        document.status = "archived".into();
        document.pinned = true;
        document.boost = 3.0;
        document.metadata_json =
            r#"{"category":"architecture","summary":"original summary","custom":"keep"}"#.into();
        let revision = document.revision;
        document.revision = state
            .store
            .upsert_document_cas(&document, Some(revision))
            .expect("seed project fields");

        let response = wiki_put(
            State(state.clone()),
            Json(WikiPutBody {
                slug: Some("project-page".into()),
                id: Some(created.document_id.clone()),
                uri: Some("wiki://project-page".into()),
                title: "Edited title".into(),
                content: "edited body".into(),
                wing: None,
                room: None,
                kind: None,
                category: None,
                summary: None,
                agent: None,
                if_match_revision: Some(document.revision),
                if_match_etag: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let updated = state
            .store
            .get_document(&created.document_id)
            .unwrap()
            .expect("updated document");
        assert_eq!(updated.wing.as_deref(), Some("alpha"));
        assert_eq!(updated.room.as_deref(), Some("design"));
        assert_eq!(
            updated.source_file.as_deref(),
            Some("/sources/alpha/page.md")
        );
        assert_eq!(updated.status, "archived");
        assert!(updated.pinned);
        assert_eq!(updated.boost, 3.0);
        assert_eq!(updated.kind, "concept");
        assert_eq!(updated.metadata_json, document.metadata_json);
        let index = state
            .store
            .get_wiki_index_by_slug("project-page")
            .unwrap()
            .expect("updated index");
        assert_eq!(index.category.as_deref(), Some("architecture"));
        assert_eq!(index.summary.as_deref(), Some("original summary"));
    }

    #[tokio::test]
    async fn wiki_put_places_new_page_in_requested_project() {
        let state = test_state("wiki-put-placement");
        let response = wiki_put(
            State(state.clone()),
            Json(WikiPutBody {
                slug: Some("alpha-overview".into()),
                id: None,
                uri: None,
                title: "Alpha overview".into(),
                content: "Project navigation".into(),
                wing: Some("alpha".into()),
                room: Some("overview".into()),
                kind: Some("source_summary".into()),
                category: Some("projects".into()),
                summary: None,
                agent: None,
                if_match_revision: None,
                if_match_etag: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let document = state
            .store
            .find_by_uri("wiki://alpha-overview")
            .unwrap()
            .expect("created page");
        assert_eq!(document.wing.as_deref(), Some("alpha"));
        assert_eq!(document.room.as_deref(), Some("overview"));
    }

    #[tokio::test]
    async fn backlinks_http_query_enforces_wing_scope() {
        let state = test_state("backlinks");
        let store = state.store.clone();
        for (id, wing) in [
            ("target", "alpha"),
            ("source-alpha", "alpha"),
            ("source-beta", "beta"),
        ] {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("wiki://{id}"),
                    title: id.into(),
                    content: id.into(),
                    wing: Some(wing.into()),
                    layer: "wiki".into(),
                    kind: "wiki".into(),
                    ..Document::default()
                })
                .unwrap();
            store
                .upsert_graph_node(&GraphNode {
                    id: format!("node-{id}"),
                    kind: "document".into(),
                    label: id.into(),
                    document_id: Some(id.into()),
                    uri: Some(format!("wiki://{id}")),
                    resolved: true,
                    metadata_json: "{}".into(),
                })
                .unwrap();
        }
        for source in ["source-alpha", "source-beta"] {
            store
                .link_nodes(&format!("node-{source}"), "node-target", "wikilink", 1.0)
                .unwrap();
        }
        let response = wiki_backlinks(
            State(state),
            Query(BacklinksQuery {
                id: "target".into(),
                wing: Some("alpha".into()),
            }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["count"], 1);
        assert_eq!(body["backlinks"][0]["id"], "source-alpha");
    }
}
