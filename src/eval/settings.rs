//! Named search-parameter profiles for A/B evaluation (subset of SearchBody / SearchQuery).

use serde::{Deserialize, Serialize};

/// Stable search knobs that can differ between experiment sides without changing the corpus.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SearchSettingsProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_score: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diversity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_chunks_per_document: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rrf_k: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recency_half_life_days: Option<f64>,
    /// Optional rerank stage for A/B eval (`mock` = deterministic token-overlap
    /// scorer; requires no live rerank endpoint). Expands the candidate pool to
    /// 50, reranks, then truncates back to `top_k` — same shape as production.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank: Option<String>,
    /// Optional server-side query rewrite for A/B eval (`mock` = deterministic
    /// stub variants, no live LLM). Mirrors the production `rewrite=true`
    /// plumbing: one search per variant plus the original, fused with RRF.
    /// Mock results validate plumbing only — they say nothing about real LLM
    /// rewrite quality.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewrite: Option<String>,
}

impl SearchSettingsProfile {
    pub fn load(path: &std::path::Path) -> crate::error::Result<Self> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            crate::error::AppError::config(format!("read settings {}: {e}", path.display()))
        })?;
        serde_json::from_str(&raw).map_err(|e| {
            crate::error::AppError::config(format!("parse settings {}: {e}", path.display()))
        })
    }

    pub fn display_name(&self, fallback: &str) -> String {
        self.name.clone().unwrap_or_else(|| fallback.to_string())
    }
}
