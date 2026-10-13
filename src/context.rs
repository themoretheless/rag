//! Process-wide application context shared by every transport.
//!
//! One `AppContext` is built at start-up and cloned into the stdio MCP server,
//! the HTTP-MCP prototype and the HTTP gateway, so the configuration is parsed
//! once, one embedding provider and one chat client exist, and the embedding
//! manifest is checked once.

use std::sync::Arc;

use crate::config::Config;
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::llm::ChatClient;

#[derive(Clone)]
pub struct AppContext {
    pub store: Arc<Store>,
    pub embedder: Arc<dyn EmbeddingProvider>,
    pub config: Config,
    /// Built from `RAG_LLM_*`; `None` when the client cannot be constructed.
    pub llm: Option<ChatClient>,
}

impl AppContext {
    /// Record the embedding manifest when missing and build the chat client.
    /// Both failures are logged and tolerated, as at every previous call site.
    pub fn new(store: Store, embedder: Arc<dyn EmbeddingProvider>, config: Config) -> Self {
        if let Err(error) = store.ensure_embedding_manifest(&config) {
            tracing::warn!(%error, "failed to ensure embedding_manifest on start");
        }
        let llm = match ChatClient::from_config(&config) {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::warn!(%error, "failed to build ChatClient; LLM tools unavailable");
                None
            }
        };
        Self {
            store: Arc::new(store),
            embedder,
            config,
            llm,
        }
    }
}
