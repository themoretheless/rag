//! Optional reranking stage for search results.
//!
//! Rerank is **disabled by default** (`RAG_RERANK_PROVIDER=off`) and only runs
//! when the caller also passes `rerank=true` on `search`. Providers:
//!
//! - `mock`: deterministic token-overlap scorer for tests and offline eval.
//! - `openai_compat`: Jina / vLLM-style `POST {base_url}/rerank` API.
//! - `ollama`: local LLM relevance scoring via `POST {base_url}/api/chat`.
//!
//! Any provider error is handled by the caller as a fallback to the
//! un-reranked ranking; search never fails because of the reranker.

use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::models::SearchHit;

/// Maximum characters of each candidate sent to a rerank provider.
const MAX_DOC_CHARS: usize = 2_000;

/// Rerank backend selected via `RAG_RERANK_PROVIDER` (default `off`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RerankProviderKind {
    /// Disabled (default): `rerank=true` is a no-op.
    Off,
    /// Deterministic token-overlap scorer (tests / offline eval).
    Mock,
    /// Jina / vLLM-style `POST {base_url}/rerank`.
    OpenAiCompat,
    /// Ollama chat-based relevance scoring (`POST {base_url}/api/chat`).
    Ollama,
}

impl FromStr for RerankProviderKind {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "disabled" => Ok(Self::Off),
            "mock" => Ok(Self::Mock),
            "openai_compat" | "openai" | "jina" | "vllm" => Ok(Self::OpenAiCompat),
            "ollama" => Ok(Self::Ollama),
            other => Err(AppError::config(format!(
                "invalid RAG_RERANK_PROVIDER '{other}': expected 'off', 'mock', \
                 'openai_compat', or 'ollama'"
            ))),
        }
    }
}

impl RerankProviderKind {
    /// Wire / config name for this provider.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Mock => "mock",
            Self::OpenAiCompat => "openai_compat",
            Self::Ollama => "ollama",
        }
    }
}

/// A rerank backend: score each candidate document against the query.
///
/// Implementations return one relevance score per input document, aligned with
/// input order. Higher is more relevant.
#[async_trait]
pub trait RerankProvider: Send + Sync {
    async fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<f32>>;
}

/// Build the configured rerank provider, or `None` when disabled.
pub fn build_provider(config: &Config) -> Result<Option<Arc<dyn RerankProvider>>> {
    match config.rerank_provider {
        RerankProviderKind::Off => Ok(None),
        RerankProviderKind::Mock => Ok(Some(Arc::new(MockReranker))),
        RerankProviderKind::OpenAiCompat => Ok(Some(Arc::new(OpenAiCompatReranker::new(
            &config.rerank_base_url,
            &config.rerank_api_key,
            &config.rerank_model,
            config.rerank_timeout_secs,
        )?))),
        RerankProviderKind::Ollama => Ok(Some(Arc::new(OllamaReranker::new(
            &config.rerank_base_url,
            &config.rerank_model,
            config.rerank_timeout_secs,
        )?))),
    }
}

/// Apply provider scores to hits: set `score_rerank`, record the explanation
/// reason, and reorder by rerank score (ties keep the prior ranking).
///
/// `scores` must align with `hits`; a length mismatch is a config error the
/// caller turns into a fallback.
pub fn apply_rerank_scores(hits: &mut Vec<SearchHit>, scores: &[f32]) -> Result<()> {
    if scores.len() != hits.len() {
        return Err(AppError::llm(format!(
            "rerank provider returned {} scores for {} documents",
            scores.len(),
            hits.len()
        )));
    }
    let mut order: Vec<usize> = (0..hits.len()).collect();
    order.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]));
    for (hit, score) in hits.iter_mut().zip(scores) {
        hit.score_rerank = Some(*score);
        let explanation = hit
            .explanation
            .get_or_insert_with(crate::models::SearchExplanation::default);
        explanation.reasons.push("rerank".into());
    }
    let mut ranked: Vec<SearchHit> = Vec::with_capacity(hits.len());
    for index in order {
        ranked.push(hits[index].clone());
    }
    *hits = ranked;
    Ok(())
}

/// Trim a candidate body for rerank prompts / requests.
pub fn truncate_candidate(content: &str) -> String {
    if content.chars().count() <= MAX_DOC_CHARS {
        return content.to_string();
    }
    content.chars().take(MAX_DOC_CHARS).collect()
}

/// Deterministic mock reranker: Jaccard token overlap between query and document.
pub struct MockReranker;

#[async_trait]
impl RerankProvider for MockReranker {
    async fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<f32>> {
        let query_tokens = tokenize(query);
        Ok(documents
            .iter()
            .map(|doc| jaccard(&query_tokens, &tokenize(doc)))
            .collect())
    }
}

fn tokenize(text: &str) -> std::collections::HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

fn jaccard(a: &std::collections::HashSet<String>, b: &std::collections::HashSet<String>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Jina / vLLM-style rerank API (`POST {base_url}/rerank`).
#[derive(Debug)]
pub struct OpenAiCompatReranker {
    client: Client,
    url: String,
    api_key: String,
    model: String,
}

impl OpenAiCompatReranker {
    pub fn new(base_url: &str, api_key: &str, model: &str, timeout_secs: u64) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/');
        if base_url.is_empty() {
            return Err(AppError::config(
                "RAG_RERANK_BASE_URL must be non-empty when RAG_RERANK_PROVIDER=openai_compat",
            ));
        }
        if model.trim().is_empty() {
            return Err(AppError::config(
                "RAG_RERANK_MODEL must be non-empty when RAG_RERANK_PROVIDER=openai_compat",
            ));
        }
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(timeout_secs.max(1)))
            .build()
            .map_err(|e| AppError::llm(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            url: format!("{base_url}/rerank"),
            api_key: api_key.to_string(),
            model: model.to_string(),
        })
    }
}

#[derive(Debug, Serialize)]
struct RerankRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: &'a [String],
    top_n: usize,
}

#[derive(Debug, Deserialize)]
struct RerankResponse {
    results: Vec<RerankResultItem>,
}

#[derive(Debug, Deserialize)]
struct RerankResultItem {
    index: usize,
    relevance_score: f32,
}

#[async_trait]
impl RerankProvider for OpenAiCompatReranker {
    async fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let body = RerankRequest {
            model: &self.model,
            query,
            documents,
            top_n: documents.len(),
        };
        let mut request = self.client.post(&self.url).json(&body);
        if !self.api_key.trim().is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let response = request
            .send()
            .await
            .map_err(|e| AppError::llm(format!("rerank request failed: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("<unreadable body>"));
            return Err(AppError::llm(format!(
                "rerank API returned HTTP {status_code}: {body_text}"
            )));
        }
        let parsed: RerankResponse = response
            .json()
            .await
            .map_err(|e| AppError::llm(format!("failed to parse rerank response: {e}")))?;
        let mut scores = vec![0.0f32; documents.len()];
        let mut seen = vec![false; documents.len()];
        for item in parsed.results {
            if item.index >= documents.len() || seen[item.index] {
                return Err(AppError::llm(format!(
                    "rerank API returned unexpected index {}",
                    item.index
                )));
            }
            seen[item.index] = true;
            scores[item.index] = item.relevance_score;
        }
        Ok(scores)
    }
}

/// Ollama chat-based relevance scoring fallback (`POST {base_url}/api/chat`).
///
/// Sends one prompt containing the query and numbered candidates and asks for
/// a JSON array of 0..1 relevance scores. Lenient parsing: the first `[..]`
/// JSON array in the reply wins.
#[derive(Debug)]
pub struct OllamaReranker {
    client: Client,
    url: String,
    model: String,
}

impl OllamaReranker {
    pub fn new(base_url: &str, model: &str, timeout_secs: u64) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/');
        if base_url.is_empty() {
            return Err(AppError::config(
                "RAG_RERANK_BASE_URL must be non-empty when RAG_RERANK_PROVIDER=ollama",
            ));
        }
        if model.trim().is_empty() {
            return Err(AppError::config(
                "RAG_RERANK_MODEL must be non-empty when RAG_RERANK_PROVIDER=ollama",
            ));
        }
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(timeout_secs.max(1)))
            .build()
            .map_err(|e| AppError::llm(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            url: format!("{base_url}/api/chat"),
            model: model.to_string(),
        })
    }
}

#[derive(Debug, Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: [OllamaChatMessage<'a>; 1],
    stream: bool,
}

#[derive(Debug, Serialize)]
struct OllamaChatMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    message: OllamaChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponseMessage {
    content: String,
}

/// Parse the first JSON number array found in `text`.
fn parse_score_array(text: &str) -> Result<Vec<f32>> {
    let start = text
        .find('[')
        .ok_or_else(|| AppError::llm("rerank reply contained no JSON array"))?;
    let end = text[start..]
        .rfind(']')
        .map(|i| start + i)
        .ok_or_else(|| AppError::llm("rerank reply contained no closing ']'"))?;
    serde_json::from_str::<Vec<f32>>(&text[start..=end])
        .map_err(|e| AppError::llm(format!("failed to parse rerank scores: {e}")))
}

#[async_trait]
impl RerankProvider for OllamaReranker {
    async fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let mut prompt = format!(
            "Score each document's relevance to the query on a scale from 0.0 to 1.0.\n\
             Reply with a JSON array of {} numbers only, in document order.\n\n\
             Query: {query}\n\n",
            documents.len()
        );
        for (index, doc) in documents.iter().enumerate() {
            prompt.push_str(&format!("[{}] {}\n\n", index + 1, truncate_candidate(doc)));
        }
        let body = OllamaChatRequest {
            model: &self.model,
            messages: [OllamaChatMessage {
                role: "user",
                content: prompt,
            }],
            stream: false,
        };
        let response = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::llm(format!("ollama rerank request failed: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("<unreadable body>"));
            return Err(AppError::llm(format!(
                "ollama rerank returned HTTP {status_code}: {body_text}"
            )));
        }
        let parsed: OllamaChatResponse = response
            .json()
            .await
            .map_err(|e| AppError::llm(format!("failed to parse ollama rerank response: {e}")))?;
        parse_score_array(&parsed.message.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(chunk_id: &str, score: f32) -> SearchHit {
        SearchHit {
            chunk_id: chunk_id.into(),
            score,
            ..SearchHit::default()
        }
    }

    #[test]
    fn provider_kind_from_str() {
        assert_eq!(
            "off".parse::<RerankProviderKind>().unwrap(),
            RerankProviderKind::Off
        );
        assert_eq!(
            "MOCK".parse::<RerankProviderKind>().unwrap(),
            RerankProviderKind::Mock
        );
        assert_eq!(
            "openai_compat".parse::<RerankProviderKind>().unwrap(),
            RerankProviderKind::OpenAiCompat
        );
        assert_eq!(
            "jina".parse::<RerankProviderKind>().unwrap(),
            RerankProviderKind::OpenAiCompat
        );
        assert_eq!(
            "ollama".parse::<RerankProviderKind>().unwrap(),
            RerankProviderKind::Ollama
        );
        assert!("foo".parse::<RerankProviderKind>().is_err());
        assert_eq!(RerankProviderKind::Off.as_str(), "off");
    }

    #[tokio::test]
    async fn mock_reranker_scores_by_token_overlap() {
        let scores = MockReranker
            .rerank(
                "quantum resonance",
                &[
                    "quantum resonance protocol details".to_string(),
                    "banana farming in tropical climates".to_string(),
                ],
            )
            .await
            .unwrap();
        assert_eq!(scores.len(), 2);
        assert!(scores[0] > scores[1]);
        assert!(scores[1] == 0.0);
    }

    #[test]
    fn apply_rerank_scores_reorders_and_stamps_metadata() {
        let mut hits = vec![hit("a", 0.9), hit("b", 0.8), hit("c", 0.7)];
        apply_rerank_scores(&mut hits, &[0.1, 0.9, 0.5]).unwrap();
        let order: Vec<&str> = hits.iter().map(|h| h.chunk_id.as_str()).collect();
        assert_eq!(order, ["b", "c", "a"]);
        assert_eq!(hits[0].score_rerank, Some(0.9));
        // Primary score stays the fusion/retrieval score.
        assert_eq!(hits[0].score, 0.8);
        assert!(hits[0]
            .explanation
            .as_ref()
            .unwrap()
            .reasons
            .contains(&"rerank".to_string()));
    }

    #[test]
    fn apply_rerank_scores_rejects_length_mismatch() {
        let mut hits = vec![hit("a", 0.9)];
        assert!(apply_rerank_scores(&mut hits, &[0.1, 0.2]).is_err());
    }

    #[test]
    fn truncate_candidate_bounds_long_documents() {
        let long = "x".repeat(MAX_DOC_CHARS * 2);
        assert_eq!(truncate_candidate(&long).chars().count(), MAX_DOC_CHARS);
        assert_eq!(truncate_candidate("short"), "short");
    }

    #[test]
    fn parse_score_array_accepts_noisy_reply() {
        assert_eq!(
            parse_score_array("Scores: [0.9, 0.1, 0.5] done.").unwrap(),
            vec![0.9, 0.1, 0.5]
        );
        assert!(parse_score_array("no array here").is_err());
    }

    #[test]
    fn openai_compat_requires_base_url_and_model() {
        assert!(OpenAiCompatReranker::new("", "k", "m", 30).is_err());
        assert!(OpenAiCompatReranker::new("http://x", "k", "", 30).is_err());
        assert!(OpenAiCompatReranker::new("http://x", "", "m", 30).is_ok());
    }

    #[test]
    fn ollama_requires_base_url_and_model() {
        assert!(OllamaReranker::new("", "m", 30).is_err());
        assert!(OllamaReranker::new("http://x", "", 30).is_err());
        assert!(OllamaReranker::new("http://x", "m", 30).is_ok());
    }
}
