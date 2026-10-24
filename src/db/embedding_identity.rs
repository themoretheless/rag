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
