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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use async_trait::async_trait;
use serde::Serialize;

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

/// [`EmbeddingProvider::embed`] that fails unless exactly one vector comes back
/// per input, so callers can zip results with their inputs safely.
pub async fn embed_exact(
    embedder: &dyn EmbeddingProvider,
    texts: &[String],
) -> Result<Vec<Vec<f32>>> {
    let vectors = embedder.embed(texts).await?;
    if vectors.len() != texts.len() {
        return Err(crate::error::AppError::embeddings(format!(
            "embedder returned {} vectors for {} inputs",
            vectors.len(),
            texts.len()
        )));
    }
    Ok(vectors)
}

/// Serves vectors computed ahead of time by one batched call, and forwards
/// anything else to the wrapped provider.
///
/// Callers that write many documents one at a time (replication applies one
/// page per event) prepare all chunk texts first, embed them in a single
/// request, then run the unchanged per-document write with this wrapper. A
/// text the prediction missed still embeds correctly through `inner`.
pub(crate) struct PrecomputedEmbedder {
    inner: Arc<dyn EmbeddingProvider>,
    vectors: std::collections::HashMap<String, Vec<f32>>,
}

impl PrecomputedEmbedder {
    /// Wrap `inner` with vectors already known for some texts.
    pub(crate) fn with_vectors(
        inner: Arc<dyn EmbeddingProvider>,
        vectors: std::collections::HashMap<String, Vec<f32>>,
    ) -> Self {
        Self { inner, vectors }
    }

    /// Embed `texts` once (deduplicated) and wrap `inner` with the results.
    pub(crate) async fn prepare(
        inner: Arc<dyn EmbeddingProvider>,
        texts: Vec<String>,
    ) -> Result<Self> {
        let mut unique = texts;
        unique.sort();
        unique.dedup();
        let vectors = if unique.is_empty() {
            Vec::new()
        } else {
            embed_exact(inner.as_ref(), &unique).await?
        };
        Ok(Self {
            inner,
            vectors: unique.into_iter().zip(vectors).collect(),
        })
    }
}

#[async_trait]
impl EmbeddingProvider for PrecomputedEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let missing: Vec<String> = texts
            .iter()
            .filter(|text| !self.vectors.contains_key(*text))
            .cloned()
            .collect();
        let mut fetched = if missing.is_empty() {
            Vec::new()
        } else {
            embed_exact(self.inner.as_ref(), &missing).await?
        }
        .into_iter();
        Ok(texts
            .iter()
            .map(|text| match self.vectors.get(text) {
                Some(vector) => vector.clone(),
                None => fetched.next().expect("one vector per missing text"),
            })
            .collect())
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}

/// Sole kind→ctor switch for production embedders.
///
/// Matches [`Config::embedding_provider`] once and returns a trait object.
/// Ollama's `/v1` vs native dialect choice is confined to this arm (via
/// [`ollama::prefers_openai_compat_path`]); callers never branch on kind for construction.
pub fn build_provider(config: &Config) -> Result<Arc<dyn EmbeddingProvider>> {
    let inner = match config.embedding_provider {
        EmbeddingProviderKind::Mock => {
            Arc::new(MockEmbedder::new(config.embedding_dims)) as Arc<dyn EmbeddingProvider>
        }
        EmbeddingProviderKind::OpenAi => {
            let provider = OpenAiEmbedder::new(
                config.embedding_base_url.clone(),
                config.embedding_api_key.expose().to_string(),
                config.embedding_model.clone(),
                config.embedding_dims,
            )?;
            Arc::new(provider) as Arc<dyn EmbeddingProvider>
        }
        EmbeddingProviderKind::Ollama => {
            // Dialect policy for this kind only: OpenAI `/v1/embeddings` when BASE_URL
            // ends with `/v1`; otherwise native Ollama routes under the host root.
            if ollama::prefers_openai_compat_path(&config.embedding_base_url) {
                let provider = OpenAiEmbedder::new(
                    config.embedding_base_url.clone(),
                    config.embedding_api_key.expose().to_string(),
                    config.embedding_model.clone(),
                    config.embedding_dims,
                )?;
                Arc::new(provider) as Arc<dyn EmbeddingProvider>
            } else {
                let provider = OllamaEmbedder::new(
                    config.embedding_base_url.clone(),
                    config.embedding_model.clone(),
                    config.embedding_dims,
                )?;
                Arc::new(provider) as Arc<dyn EmbeddingProvider>
            }
        }
    };
    Ok(Arc::new(MeteredEmbedder::new(inner)))
}

// --- Concurrency gate + usage metrics ---------------------------------------

/// Cap on simultaneous `embed` batches across the whole process.
///
/// Without a gate, parallel ingest/sync/wiki jobs can fan out unbounded
/// embedding batches and pin every core. `RAG_EMBED_CONCURRENCY` overrides
/// the default of half the logical CPUs (clamped to 1..=8).
pub fn embed_concurrency_limit() -> usize {
    let logical = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    std::env::var("RAG_EMBED_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(|| (logical / 2).clamp(1, 8))
}

static EMBED_GATE: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(embed_concurrency_limit())));
static EMBED_CALLS: AtomicU64 = AtomicU64::new(0);
static EMBED_TEXTS: AtomicU64 = AtomicU64::new(0);
static EMBED_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
static EMBED_WAIT_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static EMBED_MS_TOTAL: AtomicU64 = AtomicU64::new(0);
static EMBED_LAST_MS: AtomicU64 = AtomicU64::new(0);
static EMBED_MAX_MS: AtomicU64 = AtomicU64::new(0);

/// Live usage counters for the `/v1/resources` endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct EmbedderStats {
    pub concurrency_limit: usize,
    pub in_flight: u64,
    pub calls_total: u64,
    pub texts_total: u64,
    pub wait_ms_total: u64,
    pub embed_ms_total: u64,
    pub last_batch_ms: u64,
    pub max_batch_ms: u64,
}

pub fn embedder_stats() -> EmbedderStats {
    EmbedderStats {
        concurrency_limit: embed_concurrency_limit(),
        in_flight: EMBED_IN_FLIGHT.load(Ordering::Relaxed),
        calls_total: EMBED_CALLS.load(Ordering::Relaxed),
        texts_total: EMBED_TEXTS.load(Ordering::Relaxed),
        wait_ms_total: EMBED_WAIT_MS_TOTAL.load(Ordering::Relaxed),
        embed_ms_total: EMBED_MS_TOTAL.load(Ordering::Relaxed),
        last_batch_ms: EMBED_LAST_MS.load(Ordering::Relaxed),
        max_batch_ms: EMBED_MAX_MS.load(Ordering::Relaxed),
    }
}

/// Decorator around any [`EmbeddingProvider`] that serializes bursts through
/// [`EMBED_GATE`] and records timing/volume metrics. Installed once in
/// [`build_provider`]; tests constructing providers directly bypass it.
pub struct MeteredEmbedder {
    inner: Arc<dyn EmbeddingProvider>,
}

impl MeteredEmbedder {
    pub fn new(inner: Arc<dyn EmbeddingProvider>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl EmbeddingProvider for MeteredEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let wait_started = Instant::now();
        let _permit = EMBED_GATE
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| crate::error::AppError::Embeddings("embed gate closed".into()))?;
        EMBED_WAIT_MS_TOTAL.fetch_add(wait_started.elapsed().as_millis() as u64, Ordering::Relaxed);
        EMBED_IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        let result = self.inner.embed(texts).await;
        EMBED_IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
        let ms = started.elapsed().as_millis() as u64;
        EMBED_CALLS.fetch_add(1, Ordering::Relaxed);
        EMBED_TEXTS.fetch_add(texts.len() as u64, Ordering::Relaxed);
        EMBED_MS_TOTAL.fetch_add(ms, Ordering::Relaxed);
        EMBED_LAST_MS.store(ms, Ordering::Relaxed);
        EMBED_MAX_MS.fetch_max(ms, Ordering::Relaxed);
        result
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
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

    #[tokio::test]
    async fn precomputed_embedder_serves_prepared_texts_and_forwards_the_rest() {
        struct Counting {
            inner: mock::MockEmbedder,
            calls: std::sync::atomic::AtomicUsize,
        }
        #[async_trait::async_trait]
        impl EmbeddingProvider for Counting {
            async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
                self.calls
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.inner.embed(texts).await
            }
            fn dimensions(&self) -> usize {
                self.inner.dimensions()
            }
        }
        let counting = Arc::new(Counting {
            inner: mock::MockEmbedder::new(8),
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let inner: Arc<dyn EmbeddingProvider> = counting.clone();
        let texts = vec!["alpha".to_string(), "beta".to_string(), "alpha".to_string()];
        let precomputed = PrecomputedEmbedder::prepare(inner.clone(), texts)
            .await
            .unwrap();
        assert_eq!(counting.calls.load(std::sync::atomic::Ordering::Relaxed), 1);

        let served = precomputed
            .embed(&["beta".to_string(), "alpha".to_string()])
            .await
            .unwrap();
        assert_eq!(counting.calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        let direct = inner
            .embed(&["beta".to_string(), "alpha".to_string()])
            .await
            .unwrap();
        assert_eq!(served, direct);

        let mixed = precomputed
            .embed(&["gamma".to_string(), "alpha".to_string()])
            .await
            .unwrap();
        assert_eq!(mixed.len(), 2);
        assert_eq!(mixed[1], direct[1]);
    }

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
