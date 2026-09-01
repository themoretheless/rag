//! Deterministic mock embedding provider for tests and offline use.
//!
//! Hash text into `dims` floats in `[-1, 1]`, then L2-normalize.
//! Same text always yields the same vector.

use async_trait::async_trait;

use crate::error::Result;

use super::{l2_normalize, EmbeddingProvider};

/// Deterministic hash-based embedder producing fixed-dimension L2-normalized vectors.
#[derive(Debug, Clone)]
pub struct MockEmbedder {
    dims: usize,
}

impl MockEmbedder {
    /// Create a mock embedder that emits vectors of length `dims`.
    ///
    /// # Panics
    ///
    /// Panics if `dims` is zero.
    pub fn new(dims: usize) -> Self {
        assert!(dims > 0, "embedding dimensions must be > 0");
        Self { dims }
    }

    /// Deterministic single-text embedding (synchronous helper for tests).
    pub fn embed_one(&self, text: &str) -> Vec<f32> {
        let normalized = normalize_text(text);
        let mut vec = Vec::with_capacity(self.dims);

        for i in 0..self.dims {
            let mut hasher = Fnv64::new();
            hasher.write(normalized.as_bytes());
            // Mix dimension index so each component is independent.
            hasher.write(&i.to_le_bytes());
            let h = hasher.finish();
            vec.push(hash_to_unit(h));
        }

        l2_normalize(&mut vec);
        vec
    }
}

#[async_trait]
impl EmbeddingProvider for MockEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.embed_one(t)).collect())
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

/// Light text normalization: trim outer whitespace so padding is stable.
fn normalize_text(text: &str) -> &str {
    text.trim()
}

/// Map a 64-bit hash into `[-1.0, 1.0]`.
fn hash_to_unit(h: u64) -> f32 {
    // Use full range of u64 as f64 fraction in [0, 1], then scale to [-1, 1].
    let t = (h as f64) / (u64::MAX as f64);
    ((t * 2.0) - 1.0) as f32
}

/// FNV-1a 64-bit hasher (stable, no external dependency).
struct Fnv64 {
    state: u64,
}

impl Fnv64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    fn new() -> Self {
        Self {
            state: Self::OFFSET,
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut h = self.state;
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(Self::PRIME);
        }
        self.state = h;
    }

    fn finish(self) -> u64 {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn same_text_same_vector() {
        let emb = MockEmbedder::new(32);
        let a = emb
            .embed(&[String::from("hello world")])
            .await
            .unwrap();
        let b = emb
            .embed(&[String::from("hello world")])
            .await
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(a[0].len(), 32);
    }

    #[tokio::test]
    async fn different_text_different_vector() {
        let emb = MockEmbedder::new(32);
        let a = emb.embed_one("alpha");
        let b = emb.embed_one("beta");
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn vectors_are_l2_normalized() {
        let emb = MockEmbedder::new(64);
        let v = emb.embed_one("normalize me");
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm={norm}");
    }

    #[tokio::test]
    async fn dimensions_match() {
        let emb = MockEmbedder::new(128);
        assert_eq!(emb.dimensions(), 128);
        let v = emb.embed_one("dims");
        assert_eq!(v.len(), 128);
    }

    #[tokio::test]
    async fn batch_length_matches_input() {
        let emb = MockEmbedder::new(16);
        let texts = vec![
            String::from("one"),
            String::from("two"),
            String::from("three"),
        ];
        let out = emb.embed(&texts).await.unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], emb.embed_one("one"));
        assert_eq!(out[1], emb.embed_one("two"));
        assert_eq!(out[2], emb.embed_one("three"));
    }

    #[tokio::test]
    async fn empty_batch() {
        let emb = MockEmbedder::new(8);
        let out = emb.embed(&[]).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn independent_instances_agree() {
        let a = MockEmbedder::new(24);
        let b = MockEmbedder::new(24);
        assert_eq!(a.embed_one("shared"), b.embed_one("shared"));
    }

    #[tokio::test]
    async fn trim_normalization_is_stable() {
        let emb = MockEmbedder::new(16);
        // Leading/trailing whitespace is stripped by normalize_text.
        assert_eq!(emb.embed_one("  pad  "), emb.embed_one("pad"));
    }

    #[tokio::test]
    async fn components_are_finite() {
        let emb = MockEmbedder::new(48);
        for x in emb.embed_one("finite check") {
            assert!(x.is_finite());
            assert!((-1.0 - 1e-5..=1.0 + 1e-5).contains(&x));
        }
    }

    #[test]
    #[should_panic(expected = "embedding dimensions must be > 0")]
    fn zero_dims_panics() {
        let _ = MockEmbedder::new(0);
    }
}
