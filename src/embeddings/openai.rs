//! OpenAI-compatible HTTP embedding provider.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

use super::EmbeddingProvider;

/// Maximum number of input strings per `/embeddings` request.
const MAX_BATCH_SIZE: usize = 64;

/// OpenAI-compatible embedder (`POST {base_url}/embeddings`).
#[derive(Debug, Clone)]
pub struct OpenAiEmbedder {
    client: Client,
    base_url: String,
    api_key: String,
    model: String,
    dims: usize,
}

impl OpenAiEmbedder {
    /// Build an embedder. `base_url` is the API root (e.g. `https://api.openai.com/v1`).
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        dims: usize,
    ) -> Result<Self> {
        if dims == 0 {
            return Err(AppError::embeddings(
                "embedding dimensions must be greater than zero",
            ));
        }
        let base_url = base_url.into().trim_end_matches('/').to_string();
        if base_url.is_empty() {
            return Err(AppError::embeddings("embedding base_url must not be empty"));
        }
        let client = Client::builder()
            .build()
            .map_err(|e| AppError::embeddings(format!("failed to build HTTP client: {e}")))?;

        Ok(Self {
            client,
            base_url,
            api_key: api_key.into(),
            model: model.into(),
            dims,
        })
    }

    /// Configured vector dimensionality.
    pub fn dims(&self) -> usize {
        self.dims
    }

    /// Model name sent in the request body.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Base URL (without trailing slash).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn embeddings_url(&self) -> String {
        format!("{}/embeddings", self.base_url)
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        debug_assert!(!texts.is_empty());
        debug_assert!(texts.len() <= MAX_BATCH_SIZE);

        let body = EmbeddingsRequest {
            model: &self.model,
            input: texts,
        };

        let response = self
            .client
            .post(self.embeddings_url())
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::embeddings(format!("embeddings request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("<unreadable body>"));
            return Err(AppError::embeddings(format!(
                "embeddings API returned HTTP {status_code}: {body_text}"
            )));
        }

        let parsed: EmbeddingsResponse = response
            .json()
            .await
            .map_err(|e| AppError::embeddings(format!("failed to parse embeddings response: {e}")))?;

        if parsed.data.len() != texts.len() {
            return Err(AppError::embeddings(format!(
                "embeddings API returned {} vectors for {} inputs",
                parsed.data.len(),
                texts.len()
            )));
        }

        // Prefer restoring order via `index` when the API provides distinct indices.
        // If `index` is omitted (serde default 0 for every item), keep response order.
        let has_distinct_indices = parsed
            .data
            .iter()
            .enumerate()
            .any(|(i, item)| item.index != i);

        let mut items = parsed.data;
        if has_distinct_indices {
            items.sort_by_key(|item| item.index);
            for (i, item) in items.iter().enumerate() {
                if item.index != i {
                    return Err(AppError::embeddings(format!(
                        "embeddings API returned unexpected or duplicate index {} at position {i}",
                        item.index
                    )));
                }
            }
        }

        let mut vectors = Vec::with_capacity(items.len());
        for item in items {
            if item.embedding.len() != self.dims {
                return Err(AppError::embeddings(format!(
                    "embedding dimension mismatch: expected {}, got {}",
                    self.dims,
                    item.embedding.len()
                )));
            }
            vectors.push(item.embedding);
        }

        Ok(vectors)
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut all = Vec::with_capacity(texts.len());
        for batch in texts.chunks(MAX_BATCH_SIZE) {
            let vectors = self.embed_batch(batch).await?;
            all.extend(vectors);
        }
        Ok(all)
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

#[derive(Debug, Serialize)]
struct EmbeddingsRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Debug, Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    #[serde(default)]
    index: usize,
}
