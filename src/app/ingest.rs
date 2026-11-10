//! `IngestUseCase`: document ingest shared by MCP tools and HTTP handlers.
//!
//! The pipelines themselves live in [`crate::ingest::IngestService`]; this is
//! the one entry both transports call, so a fetch timeout, a default layer or a
//! placement rule is decided here once.

use std::sync::Arc;

use crate::config::Config;
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::ingest::{
    IngestCommand, IngestFileCommand, IngestService, IngestUrlCommand, IngestUrlResult,
};
use crate::models::IngestResult;
use crate::web_ingest::ReqwestFetcher;

/// Ingest use cases over one store, embedder and config.
#[derive(Clone)]
pub struct IngestUseCase {
    store: Arc<Store>,
    embedder: Arc<dyn EmbeddingProvider>,
    config: Arc<Config>,
}

impl IngestUseCase {
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

    fn service(&self) -> IngestService<'_> {
        IngestService::new(&self.store, &self.embedder, &self.config)
    }

    /// Chunk, embed and store text (MCP `ingest_text`, `ingest_raw`, `add_drawer`).
    pub async fn ingest(&self, command: IngestCommand) -> Result<IngestResult, AppError> {
        self.service().ingest(command).await
    }

    /// Read one allowlisted file from disk and ingest it (upsert by `file://` URI).
    pub async fn ingest_file(&self, command: IngestFileCommand) -> Result<IngestResult, AppError> {
        self.service().ingest_file(command).await
    }

    /// Fetch one allowlisted web page and ingest it (MCP `ingest_url`,
    /// HTTP `POST /v1/ingest/url`). The fetch timeout is
    /// `RAG_INGEST_URL_TIMEOUT_SECS` on both transports.
    pub async fn ingest_url(&self, command: IngestUrlCommand) -> Result<IngestUrlResult, AppError> {
        let fetcher = ReqwestFetcher::new(self.config.ingest_url_timeout_secs);
        self.service().ingest_url(&fetcher, command).await
    }
}
