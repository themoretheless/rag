//! Embedding providers, factory, and vector similarity helpers.
//!
//! | `RAG_EMBEDDING_PROVIDER` | Client |
//! |--------------------------|--------|
//! | `mock` | deterministic hash embedder |
//! | `openai` / `openai_compat` | `POST {base}/embeddings` (cloud or local OpenAI-compatible) |
//! | `ollama` | native `POST {root}/api/embed` (fallback `/api/embeddings`); if base ends with `/v1`, uses OpenAI-compatible path |
//!
//! # Extension point (Open-Closed)
//!
//! Domain code (ingest, search, wiki, MCP, bins) depends only on
//! [`EmbeddingProvider`] and [`build_provider`]. Do **not** match on
//! [`EmbeddingProviderKind`] at call sites to construct clients; that kind→ctor
//! switch lives solely in [`build_provider`].
//!
//! To add a backend without touching those callers:
//! 1. Add a variant on [`crate::config::EmbeddingProviderKind`] (`FromStr` / `as_str`).
//! 2. Implement [`EmbeddingProvider`] in a new `embeddings/` submodule; `pub use` it here.
//! 3. Add **one** arm to the match in [`build_provider`] (ctor only; keep dialect
//!    policy inside the provider module when possible).
//! 4. Wire env defaults and credential checks in `config` if the kind needs them.
//!
//! Tests may construct [`MockEmbedder`] (or another impl) directly as
//! `Arc<dyn EmbeddingProvider>` without going through the factory.

use std::sync::Arc;

use async_trait::async_trait;

use crate::config::{Config, EmbeddingProviderKind};
use crate::error::Result;

pub mod mock;
pub mod ollama;
pub mod openai;

pub use mock::MockEmbedder;
pub use ollama::OllamaEmbedder;
pub use openai::OpenAiEmbedder;

/// Produces dense vector embeddings for batches of text.
///
/// Extension seam: implement this trait for a new backend; register the type in
/// [`build_provider`]. Callers only hold `Arc<dyn EmbeddingProvider>`.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed each input string, preserving order. Empty input yields an empty vec.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Dimensionality of vectors produced by this provider.
    fn dimensions(&self) -> usize;
}

/// Sole kind→ctor switch for production embedders.
///
/// Matches [`Config::embedding_provider`] once and returns a trait object.
/// Ollama's `/v1` vs native dialect choice is confined to this arm (via
/// [`ollama::prefers_openai_compat_path`]); callers never branch on kind for construction.
pub fn build_provider(config: &Config) -> Result<Arc<dyn EmbeddingProvider>> {
    match config.embedding_provider {
        EmbeddingProviderKind::Mock => {
            Ok(Arc::new(MockEmbedder::new(config.embedding_dims)))
        }
        EmbeddingProviderKind::OpenAi => {
            let provider = OpenAiEmbedder::new(
                config.embedding_base_url.clone(),
                config.embedding_api_key.clone(),
                config.embedding_model.clone(),
                config.embedding_dims,
            )?;
            Ok(Arc::new(provider))
        }
        EmbeddingProviderKind::Ollama => {
            // Dialect policy for this kind only: OpenAI `/v1/embeddings` when BASE_URL
            // ends with `/v1`; otherwise native Ollama routes under the host root.
            if ollama::prefers_openai_compat_path(&config.embedding_base_url) {
                let provider = OpenAiEmbedder::new(
                    config.embedding_base_url.clone(),
                    config.embedding_api_key.clone(),
                    config.embedding_model.clone(),
                    config.embedding_dims,
                )?;
                Ok(Arc::new(provider))
            } else {
                let provider = OllamaEmbedder::new(
                    config.embedding_base_url.clone(),
                    config.embedding_model.clone(),
                    config.embedding_dims,
                )?;
                Ok(Arc::new(provider))
            }
        }
    }
}

/// Cosine similarity between two vectors.
///
/// Returns a value in approximately `[-1, 1]`. If lengths differ or either vector
/// has zero L2 norm, returns `0.0`.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        (dot / denom).clamp(-1.0, 1.0)
    }
}

/// In-place L2 normalization. A zero vector is left unchanged.
pub fn l2_normalize(v: &mut [f32]) {
    let mut sum_sq = 0.0f32;
    for x in v.iter() {
        sum_sq += *x * *x;
    }
    let norm = sum_sq.sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_unit_vectors() {
        let a = [1.0f32, 0.0, 0.0];
        let b = [1.0f32, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal() {
        let a = [1.0f32, 0.0];
        let b = [0.0f32, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cosine_opposite() {
        let a = [1.0f32, 0.0];
        let b = [-1.0f32, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_length_mismatch_and_empty() {
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn l2_normalize_unit_length() {
        let mut v = [3.0f32, 4.0];
        l2_normalize(&mut v);
        let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
        assert!((len - 1.0).abs() < 1e-6);
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert!((v[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn l2_normalize_zero_unchanged() {
        let mut v = [0.0f32, 0.0];
        l2_normalize(&mut v);
        assert_eq!(v, [0.0, 0.0]);
    }
}
