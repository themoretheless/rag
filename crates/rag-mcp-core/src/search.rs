//! Search hits and their ranking explanations.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SearchExplanation {
    #[serde(default)]
    pub reasons: Vec<String>,
    pub retrieval_ms: f64,
    pub postprocess_ms: f64,
    pub total_ms: f64,
    /// Query embedding time (vec / hybrid only; measured by the adapter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed_ms: Option<f64>,
    /// Dense candidate retrieval time (vec / hybrid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vec_ms: Option<f64>,
    /// Lexical candidate retrieval time (lex / hybrid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lex_ms: Option<f64>,
    /// RRF constant used for fusion (hybrid only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rrf_k: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deduplication: Option<String>,
    /// LLM HyDE passage generation time (only when `hyde=true` and it ran).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hyde_ms: Option<f64>,
    /// LLM query-rewrite time (only when `rewrite=true` and variants ran).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewrite_ms: Option<f64>,
    /// Rerank stage time (only when `rerank=true` and a provider ran).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchContextChunk {
    pub chunk_id: String,
    pub chunk_index: i32,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_path: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

/// One hit from semantic / hybrid search.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SearchHit {
    pub chunk_id: String,
    pub document_id: String,
    pub document_title: String,
    pub document_uri: String,
    pub chunk_index: i32,
    pub content: String,
    /// Primary ranking score (cosine for vec; fused for hybrid; lex for lex-only).
    pub score: f32,
    /// Dense vector cosine when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_vec: Option<f32>,
    /// Lexical / BM25 (or TF fallback) score when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_lex: Option<f32>,
    /// Reciprocal rank fusion score when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_rrf: Option<f32>,
    /// 1-based rank in the dense candidate list before fusion (vec / hybrid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_vec: Option<u32>,
    /// 1-based rank in the lexical candidate list before fusion (lex / hybrid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_lex: Option<u32>,
    /// Optional short excerpt for citations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Character offset of the hit span in the source document (start).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub char_start: Option<i32>,
    /// Character offset of the hit span in the source document (end).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub char_end: Option<i32>,
    /// Markdown heading ancestry for this chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_path: Option<Vec<String>>,
    /// Leaf Markdown heading for this chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// Opt-in neighboring or same-section chunks, kept in source order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Vec<SearchContextChunk>>,
    /// Ranking factors and stage timings for audit/debugging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<SearchExplanation>,
    /// Rerank provider relevance score when `rerank=true` and a provider ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_rerank: Option<f32>,
}

impl SearchHit {
    /// Snippet when the server sent one, else the chunk content.
    pub fn visible_excerpt(&self) -> &str {
        self.snippet.as_deref().unwrap_or(&self.content)
    }
}
