//! Upsert by slug, `wiki://` uri or document id (HTTP `PUT`/`POST /v1/wiki`).
//!
//! MCP exposes create and update as separate tools; this is the one place that
//! resolves a combined request to the existing page (if any) and picks the path.

use super::*;
use crate::util::wiki_slug_from_uri;

/// Transport-neutral upsert request.
#[derive(Debug, Clone, Default)]
pub struct WikiPutCommand {
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
    /// Refuse to touch an existing page (`POST`).
    pub create_only: bool,
}

/// Resolve the target page and create or CAS-update it.
pub async fn put_wiki_page(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    command: WikiPutCommand,
) -> Result<WikiWriteResult> {
    let slug = command
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| command.uri.as_deref().and_then(wiki_slug_from_uri))
        .unwrap_or_default();
    let slug = slug.trim();
    if slug.is_empty() {
        return Err(AppError::invalid("slug required (or uri=wiki://slug)"));
    }
    let canonical_uri = format!("wiki://{}", shared_slugify(slug, SlugPolicy::WikiPage));
    let requested_id = command
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    if command.create_only && (requested_id.is_some() || command.if_match_revision.is_some()) {
        return Err(AppError::invalid(
            "creating a wiki page does not accept id or if_match",
        ));
    }
    let existing = match requested_id {
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
        None => crate::util::blocking_section(|| store.find_by_uri(&canonical_uri))?,
    };
    if command.create_only && existing.is_some() {
        return Err(AppError::conflict("wiki page URI is already in use"));
    }
    if let Some(existing) = existing {
        return update_wiki_page_cas(
            store,
            embedder,
            config,
            &existing.id,
            Some(&command.title),
            &command.content,
            command.kind.as_deref(),
            command.category.as_deref(),
            command.summary.as_deref(),
            command.agent.as_deref(),
            command.if_match_revision,
        )
        .await;
    }
    let write = WikiWriteCommand {
        slug: slug.to_string(),
        title: command.title,
        content: command.content,
        wing: command.wing,
        room: command.room,
        kind: command
            .kind
            .unwrap_or_else(|| crate::models::DocumentKind::Wiki.as_str().into()),
        category: command.category,
        summary: command.summary,
        agent: command.agent,
        options: WriteWikiOpts {
            if_match_revision: command.if_match_revision,
            ..Default::default()
        },
    };
    if command.create_only {
        create_wiki_page_command(store, embedder, config, write).await
    } else {
        write_wiki_page_command(store, embedder, config, write).await
    }
}
