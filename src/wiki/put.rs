//! The one wiki page write entrypoint for transports (MCP `write_wiki_page` /
//! `update_wiki_page`, HTTP `POST` / `PUT /v1/wiki`).
//!
//! [`write_page`] resolves the target page (slug, `wiki://` uri or document id)
//! and applies one [`WikiWriteMode`]:
//!
//! | mode | page absent | page present |
//! |---|---|---|
//! | `Create` | create | `CONFLICT` (if_match / id refused) |
//! | `Upsert` | create | update (omitted kind/category/summary/placement kept) |
//! | `Cas` | `NOT_FOUND` | update |
//!
//! In every mode a stale `if_match_revision` is `CONFLICT` and leaves the page
//! unchanged: that is the one meaning of a CAS conflict on both transports.

use super::*;
use crate::util::wiki_slug_from_uri;

/// How a write treats an existing page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WikiWriteMode {
    /// Create only; an occupied canonical URI is a conflict.
    Create,
    /// Create or update.
    #[default]
    Upsert,
    /// Update an existing page only.
    Cas,
}

/// Transport-neutral wiki write request.
#[derive(Debug, Clone, Default)]
pub struct WikiPageRequest {
    pub mode: WikiWriteMode,
    pub slug: Option<String>,
    pub uri: Option<String>,
    pub id: Option<String>,
    pub title: String,
    pub content: String,
    pub wing: Option<String>,
    pub room: Option<String>,
    /// Omitted on update keeps the current kind; on create defaults to `wiki`.
    pub kind: Option<String>,
    pub category: Option<String>,
    pub summary: Option<String>,
    pub agent: Option<String>,
    pub if_match_revision: Option<i64>,
}

fn non_blank_owned(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Resolve the target page and create or update it per `request.mode`.
pub async fn write_page(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    request: WikiPageRequest,
) -> Result<WikiWriteResult> {
    let requested_id = non_blank_owned(request.id.as_deref());
    let slug = non_blank_owned(request.slug.as_deref())
        .or_else(|| request.uri.as_deref().and_then(wiki_slug_from_uri))
        .unwrap_or_default();
    let slug = slug.trim().to_string();
    if request.mode == WikiWriteMode::Cas {
        // Update by id, `wiki://slug` or bare slug of an existing page.
        let key = requested_id
            .clone()
            .or_else(|| (!slug.is_empty()).then(|| slug.clone()))
            .ok_or_else(|| AppError::invalid("slug, uri or id required"))?;
        let existing = get_wiki_page(store, &key)?;
        return update_existing(store, embedder, config, &existing, request).await;
    }
    if slug.is_empty() {
        return Err(AppError::invalid("slug required (or uri=wiki://slug)"));
    }
    let create_only = request.mode == WikiWriteMode::Create;
    let canonical_uri = format!("wiki://{}", shared_slugify(&slug, SlugPolicy::WikiPage));
    if create_only && (requested_id.is_some() || request.if_match_revision.is_some()) {
        return Err(AppError::invalid(
            "creating a wiki page does not accept id or if_match",
        ));
    }
    let existing = match requested_id.as_deref() {
        Some(id) => {
            let document = get_wiki_page(store, id)?;
            if document.uri != canonical_uri {
                return Err(AppError::conflict(format!(
                    "wiki document id '{id}' belongs to '{}', not requested URI '{canonical_uri}'",
                    document.uri
                )));
            }
            Some(document)
        }
        None => crate::util::blocking_section(|| store.documents().find_by_uri(&canonical_uri))?,
    };
    if create_only && existing.is_some() {
        return Err(AppError::conflict("wiki page URI is already in use"));
    }
    if let Some(existing) = existing {
        return update_existing(store, embedder, config, &existing, request).await;
    }
    let write = WikiWriteCommand {
        slug,
        title: request.title,
        content: request.content,
        wing: request.wing,
        room: request.room,
        kind: request
            .kind
            .unwrap_or_else(|| crate::models::DocumentKind::Wiki.as_str().into()),
        category: request.category,
        summary: request.summary,
        agent: request.agent,
        options: WriteWikiOpts {
            if_match_revision: request.if_match_revision,
            ..Default::default()
        },
    };
    if create_only {
        create_wiki_page_command(store, embedder, config, write).await
    } else {
        write_wiki_page_command(store, embedder, config, write).await
    }
}

/// CAS-update an existing page; blank title/kind keep the current values.
async fn update_existing(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    existing: &Document,
    request: WikiPageRequest,
) -> Result<WikiWriteResult> {
    update_wiki_page_cas(
        store,
        embedder,
        config,
        WikiPageUpdate {
            id_or_slug: &existing.id,
            title: Some(&request.title),
            content: &request.content,
            kind: request.kind.as_deref(),
            category: request.category.as_deref(),
            summary: request.summary.as_deref(),
            agent: request.agent.as_deref(),
            wing: request.wing.as_deref(),
            room: request.room.as_deref(),
            if_match_revision: request.if_match_revision,
        },
    )
    .await
}
