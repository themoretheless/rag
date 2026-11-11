//! `WikiUseCase`: the one wiki write entrypoint for both transports.
//!
//! | entrypoint | mode |
//! |---|---|
//! | MCP `write_wiki_page` | `Upsert` (`Create` with `create_only=true`) |
//! | MCP `update_wiki_page` | `Cas` |
//! | HTTP `POST /v1/wiki` | `Create` |
//! | HTTP `PUT /v1/wiki` | `Upsert` |

use std::sync::Arc;

use crate::config::Config;
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::models::resolve_if_match;
use crate::wiki::{self, WikiPageRequest, WikiWriteResult};

pub use crate::wiki::WikiWriteMode;

/// Typed wiki write command; `if_match_revision` wins over `if_match_etag`.
#[derive(Debug, Clone, Default)]
pub struct WikiWriteCommand {
    pub mode: WikiWriteMode,
    pub slug: Option<String>,
    pub uri: Option<String>,
    pub id: Option<String>,
    pub title: String,
    pub content: String,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub kind: Option<String>,
    pub category: Option<String>,
    pub summary: Option<String>,
    pub agent: Option<String>,
    pub if_match_revision: Option<i64>,
    pub if_match_etag: Option<String>,
    /// See [`WikiPageRequest::blank_title_is_slug`].
    pub blank_title_is_slug: bool,
}

/// Wiki use cases over one store, embedder and configuration.
#[derive(Clone)]
pub struct WikiUseCase {
    store: Arc<Store>,
    embedder: Arc<dyn EmbeddingProvider>,
    config: Arc<Config>,
}

impl WikiUseCase {
    pub fn new(
        store: Arc<Store>,
        embedder: Arc<dyn EmbeddingProvider>,
        config: Arc<Config>,
    ) -> Self {
        Self {
            store,
            embedder,
            config,
        }
    }

    /// Create, upsert or CAS-update one page (see [`crate::wiki::write_page`]).
    pub async fn write(&self, command: WikiWriteCommand) -> Result<WikiWriteResult, AppError> {
        let if_match_revision =
            resolve_if_match(command.if_match_revision, command.if_match_etag.as_deref())?;
        wiki::write_page(
            &self.store,
            &self.embedder,
            &self.config,
            WikiPageRequest {
                mode: command.mode,
                slug: command.slug,
                uri: command.uri,
                id: command.id,
                title: command.title,
                content: command.content,
                wing: command.wing,
                room: command.room,
                kind: command.kind,
                category: command.category,
                summary: command.summary,
                agent: command.agent,
                if_match_revision,
                blank_title_is_slug: command.blank_title_is_slug,
            },
        )
        .await
    }
}
