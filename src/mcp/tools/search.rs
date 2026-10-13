//! Search, packing and retrieval helpers tool parameters.

use super::*;
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

// --- Search / packing ---

/// Parameters for `search`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SearchParams {
    /// Natural-language query to embed and rank against chunks.
    pub query: String,
    /// Max hits to return (defaults to config `RAG_DEFAULT_TOP_K`).
    #[serde(default)]
    pub top_k: Option<u32>,
    /// Optional filter: only search chunks of this document id.
    #[serde(default)]
    pub document_id: Option<String>,
    /// Search backend: `lex` | `vec` | `hybrid` (defaults to `RAG_DEFAULT_SEARCH_MODE`).
    #[serde(default)]
    pub mode: Option<String>,
    /// Drop hits whose primary score is strictly below this threshold.
    #[serde(default)]
    pub min_score: Option<f32>,
    /// Filter: only documents with this wing.
    #[serde(default)]
    pub wing: Option<String>,
    /// Filter: only documents with this room.
    #[serde(default)]
    pub room: Option<String>,
    /// Filter: only documents with this layer (`raw`, `wiki`, …).
    #[serde(default)]
    pub layer: Option<String>,
    /// Filter: exact `source_file` provenance match.
    #[serde(default)]
    pub source_file: Option<String>,
    /// When true, include archived/tombstone documents (default false).
    #[serde(default)]
    pub include_archived: Option<bool>,
    /// Result diversity: `mmr` | `collapse_by_document`.
    #[serde(default)]
    pub diversity: Option<String>,
    /// Alias for document-level grouping. `document` selects collapse_by_document.
    #[serde(default)]
    pub group_by: Option<String>,
    /// Optional exponential freshness boost in days. Disabled when omitted.
    #[serde(default)]
    pub recency_half_life_days: Option<f64>,
    /// Complete citation-block budget including headers and expansion (~chars/4; 0 returns no hits).
    #[serde(default)]
    pub max_context_tokens: Option<u32>,
    /// Cap retained chunks per document under diversity (default `RAG_MAX_CHUNKS_PER_DOC`).
    #[serde(default)]
    pub max_chunks_per_document: Option<u32>,
    /// Optional source context: `neighbors` or `parent_section`.
    #[serde(default)]
    pub context_expansion: Option<String>,
    /// Chunks on each side for neighbor expansion (default 1).
    #[serde(default)]
    pub neighbor_chunks: Option<u32>,
    /// End-to-end synchronous retrieval budget (default 5000 ms).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// RRF constant `k` in `1 / (k + rank)` for hybrid fusion (default 60).
    #[serde(default)]
    pub rrf_k: Option<f32>,
}

/// Explicit multi-query rewrite/RRF. Callers provide rewrites; no LLM is invoked by default.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MultiQuerySearchParams {
    /// Original query followed by optional deterministic/manual rewrites.
    pub queries: Vec<String>,
    #[serde(default)]
    pub top_k: Option<u32>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub wing: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub source_file: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Parameters for `pack_context`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PackContextParams {
    /// Ranked search hits to pack (highest score first).
    pub hits: Vec<PackHitParams>,
    /// Complete citation-block budget including headers and expansion (~chars/4; 0 yields empty).
    /// Defaults to `RAG_MAX_CONTEXT_TOKENS`; actual tokenizer counts may differ.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Optional source context: `neighbors` or `parent_section`.
    #[serde(default)]
    pub context_expansion: Option<String>,
    /// Chunks on each side for neighbor expansion (default 1).
    #[serde(default)]
    pub neighbor_chunks: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MultiGetParams {
    pub document_ids: Vec<String>,
    #[serde(default)]
    pub include_chunks: Option<bool>,
    /// Per-document chunk cap when `include_chunks=true` (default 100, hard cap 500).
    /// One call also has an 8 MiB aggregate chunk-text budget.
    #[serde(default)]
    pub chunk_limit: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FindSimilarParams {
    pub document_id: String,
    #[serde(default)]
    pub top_k: Option<u32>,
    #[serde(default)]
    pub wing: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExpandChunksParams {
    pub document_id: String,
    pub chunk_index: i32,
    #[serde(default)]
    pub radius: Option<u32>,
}

/// Parameters for `reembed_document`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ReembedDocumentParams {
    /// Document primary key whose chunks should be re-embedded with the live config.
    pub document_id: String,
}
