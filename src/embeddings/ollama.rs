//! Native Ollama embedding provider (`POST /api/embeddings`, batch `/api/embed`).
//!
//! When `base_url` ends with `/v1`, callers should prefer the OpenAI-compatible
//! path (`OpenAiEmbedder` against `{base}/embeddings`). This module targets the
//! Ollama-native host root (e.g. `http://127.0.0.1:11434`).

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

use super::EmbeddingProvider;

/// Maximum number of input strings per `/api/embed` batch request.
const MAX_BATCH_SIZE: usize = 64;

/// Native Ollama embedder.
///
/// Prefer `POST {root}/api/embed` (batch). On HTTP 404, fall back to
/// per-text `POST {root}/api/embeddings` (`prompt` field).
#[derive(Debug, Clone)]
pub struct OllamaEmbedder {
    client: Client,
    /// Host root without trailing slash and without `/v1` suffix.
    root: String,
    model: String,
    dims: usize,
}

impl OllamaEmbedder {
    /// Build an embedder. `base_url` may be `http://127.0.0.1:11434` or `.../v1`
    /// (the `/v1` suffix is stripped so native routes stay under the host root).
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        dims: usize,
    ) -> Result<Self> {
        if dims == 0 {
            return Err(AppError::embeddings(
                "embedding dimensions must be greater than zero",
            ));
        }
        let root = normalize_ollama_root(&base_url.into());
        if root.is_empty() {
            return Err(AppError::embeddings(
                "embedding base_url must not be empty",
            ));
        }
        let model = model.into();
        if model.trim().is_empty() {
            return Err(AppError::embeddings(
                "embedding model must not be empty",
            ));
        }
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| AppError::embeddings(format!("failed to build HTTP client: {e}")))?;

        Ok(Self {
            client,
            root,
            model,
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

    /// Normalized Ollama host root (no trailing `/v1`).
    pub fn root(&self) -> &str {
        &self.root
    }

    async fn embed_batch_api_embed(&self, texts: &[String]) -> Result<Option<Vec<Vec<f32>>>> {
        debug_assert!(!texts.is_empty());
        debug_assert!(texts.len() <= MAX_BATCH_SIZE);

        let url = format!("{}/api/embed", self.root);
        let body = ApiEmbedRequest {
            model: &self.model,
            input: texts,
        };

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::embeddings(format!("ollama /api/embed request failed: {e}")))?;

        let status = response.status();
        if status.as_u16() == 404 {
            return Ok(None);
        }
        if !status.is_success() {
            let status_code = status.as_u16();
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("<unreadable body>"));
            return Err(AppError::embeddings(format!(
                "ollama /api/embed returned HTTP {status_code}: {body_text}"
            )));
        }

        let parsed: ApiEmbedResponse = response.json().await.map_err(|e| {
            AppError::embeddings(format!("failed to parse ollama /api/embed response: {e}"))
        })?;

        if parsed.embeddings.len() != texts.len() {
            return Err(AppError::embeddings(format!(
                "ollama /api/embed returned {} vectors for {} inputs",
                parsed.embeddings.len(),
                texts.len()
            )));
        }

        let mut vectors = Vec::with_capacity(parsed.embeddings.len());
        for emb in parsed.embeddings {
            if emb.len() != self.dims {
                return Err(AppError::embeddings(format!(
                    "embedding dimension mismatch: expected {}, got {}",
                    self.dims,
                    emb.len()
                )));
            }
            vectors.push(emb);
        }
        Ok(Some(vectors))
    }

    async fn embed_one_api_embeddings(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!("{}/api/embeddings", self.root);
        let body = ApiEmbeddingsRequest {
            model: &self.model,
            prompt: text,
        };

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                AppError::embeddings(format!("ollama /api/embeddings request failed: {e}"))
            })?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("<unreadable body>"));
            return Err(AppError::embeddings(format!(
                "ollama /api/embeddings returned HTTP {status_code}: {body_text}"
            )));
        }

        let parsed: ApiEmbeddingsResponse = response.json().await.map_err(|e| {
            AppError::embeddings(format!(
                "failed to parse ollama /api/embeddings response: {e}"
            ))
        })?;

        if parsed.embedding.len() != self.dims {
            return Err(AppError::embeddings(format!(
                "embedding dimension mismatch: expected {}, got {}",
                self.dims,
                parsed.embedding.len()
            )));
        }
        Ok(parsed.embedding)
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if let Some(vectors) = self.embed_batch_api_embed(texts).await? {
            return Ok(vectors);
        }
        // Legacy single-prompt endpoint.
        let mut vectors = Vec::with_capacity(texts.len());
        for text in texts {
            vectors.push(self.embed_one_api_embeddings(text).await?);
        }
        Ok(vectors)
    }
}

#[async_trait]
impl EmbeddingProvider for OllamaEmbedder {
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

/// Strip trailing slashes and optional `/v1` so native routes hit the Ollama root.
pub fn normalize_ollama_root(base_url: &str) -> String {
    let mut u = base_url.trim().trim_end_matches('/').to_string();
    if u.to_ascii_lowercase().ends_with("/v1") {
        u.truncate(u.len().saturating_sub(3));
        u = u.trim_end_matches('/').to_string();
    }
    u
}

/// True when `base_url` is intended as OpenAI-compatible (`.../v1`).
pub fn prefers_openai_compat_path(base_url: &str) -> bool {
    let u = base_url.trim().trim_end_matches('/').to_ascii_lowercase();
    u.ends_with("/v1")
}

#[derive(Debug, Serialize)]
struct ApiEmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Debug, Deserialize)]
struct ApiEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[derive(Debug, Serialize)]
struct ApiEmbeddingsRequest<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Debug, Deserialize)]
struct ApiEmbeddingsResponse {
    embedding: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn normalize_strips_v1_and_slash() {
        assert_eq!(
            normalize_ollama_root("http://127.0.0.1:11434/v1/"),
            "http://127.0.0.1:11434"
        );
        assert_eq!(
            normalize_ollama_root("http://127.0.0.1:11434"),
            "http://127.0.0.1:11434"
        );
        assert!(prefers_openai_compat_path("http://127.0.0.1:11434/v1"));
        assert!(!prefers_openai_compat_path("http://127.0.0.1:11434"));
    }

    #[test]
    fn new_rejects_zero_dims() {
        assert!(OllamaEmbedder::new("http://127.0.0.1:11434", "nomic-embed-text", 0).is_err());
    }

    #[tokio::test]
    async fn embed_via_legacy_api_embeddings_mock() {
        let dims = 4;
        let (base, handle) = spawn_routed_mock(dims);

        let emb = OllamaEmbedder::new(&base, "nomic-embed-text", dims).unwrap();
        let out = emb
            .embed(&[String::from("hello ollama")])
            .await
            .expect("embed ok");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), dims);
        assert!((out[0][0] - 0.0).abs() < 1e-5);
        assert!((out[0][1] - 0.01).abs() < 1e-5);
        handle.join().expect("mock thread");
    }

    /// Mock that 404s `/api/embed` and answers `/api/embeddings` (and a second request).
    fn spawn_routed_mock(dims: usize) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let addr = listener.local_addr().expect("local_addr");
        let handle = thread::spawn(move || {
            // First request: /api/embed -> 404
            {
                let (mut stream, _) = listener.accept().expect("accept embed");
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .ok();
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                assert!(
                    req.contains("/api/embed"),
                    "expected /api/embed first, got: {req}"
                );
                let body = "not found";
                let resp = format!(
                    "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
            // Second request: /api/embeddings -> 200
            {
                let (mut stream, _) = listener.accept().expect("accept embeddings");
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .ok();
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                assert!(
                    req.contains("/api/embeddings"),
                    "expected /api/embeddings, got: {req}"
                );
                let mut emb = Vec::with_capacity(dims);
                for i in 0..dims {
                    emb.push((i as f32) * 0.01);
                }
                let body = serde_json::json!({ "embedding": emb }).to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn empty_batch() {
        let emb = OllamaEmbedder::new("http://127.0.0.1:9", "m", 8).unwrap();
        let out = emb.embed(&[]).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn embed_via_api_embed_batch_mock() {
        let dims = 3;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .ok();
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(req.contains("/api/embed"));
            let body = serde_json::json!({
                "embeddings": [
                    [0.1, 0.2, 0.3],
                    [0.4, 0.5, 0.6],
                ]
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        });

        let emb = OllamaEmbedder::new(format!("http://{addr}"), "nomic-embed-text", dims).unwrap();
        let out = emb
            .embed(&[String::from("a"), String::from("b")])
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], vec![0.1, 0.2, 0.3]);
        assert_eq!(out[1], vec![0.4, 0.5, 0.6]);
        handle.join().unwrap();
    }
}
