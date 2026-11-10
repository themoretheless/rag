//! The configuration view persistence needs for corpus embedding identity.
//!
//! `db` decides whether stored vectors match the running provider, model,
//! dimensions and endpoint, but it does not depend on the whole `Config`:
//! callers pass anything that exposes these four values.

pub trait EmbeddingIdentity {
    /// Provider name as recorded in the manifest (`mock`, `openai`, `ollama`, …).
    fn embedding_provider_name(&self) -> &str;
    fn embedding_model_name(&self) -> &str;
    fn embedding_dimensions(&self) -> usize;
    fn embedding_endpoint(&self) -> &str;
}

/// `Config` carries the corpus embedding identity.
impl EmbeddingIdentity for crate::config::Config {
    fn embedding_provider_name(&self) -> &str {
        self.embedding_provider.as_str()
    }
    fn embedding_model_name(&self) -> &str {
        &self.embedding_model
    }
    fn embedding_dimensions(&self) -> usize {
        self.embedding_dims
    }
    fn embedding_endpoint(&self) -> &str {
        &self.embedding_base_url
    }
}
