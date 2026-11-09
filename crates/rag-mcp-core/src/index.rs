//! Code-index run reports and stage timings.

use serde::{Deserialize, Serialize};

/// Measured native index-code phases; excludes metrics persistence and commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStageTimings {
    pub contract_version: u32,
    pub run_id: String,
    pub lock_wait_ms: f64,
    pub source_load_decode_ms: f64,
    pub cache_parse_serialize_ms: f64,
    pub cache_replace_ms: f64,
    pub revision_publish_ms: f64,
    pub transaction_body_ms: f64,
    pub before_metrics_write_ms: f64,
    pub documents_loaded: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub unsupported_documents: usize,
    pub parse_error_documents: usize,
    pub commit_included: bool,
    pub metrics_write_included: bool,
    pub embedding_fts_included: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexResolutionStats {
    pub contract_version: u32,
    pub documents_total: usize,
    pub documents_resolved: usize,
    pub documents_reused: usize,
    pub reference_sites_resolved: usize,
    pub reference_sites_reused: usize,
    pub rust_package_scopes: usize,
    pub project_fallback_scopes: usize,
    pub empty_reference_documents: usize,
    pub dependency_inputs_hashed: usize,
    pub invalid_cache_entries: usize,
    pub forced_full: bool,
    pub cache_available: bool,
    pub no_resolution_required: bool,
    pub resolution_ms: f64,
    pub equivalent_to_existing: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexCompletionTimings {
    pub contract_version: u32,
    pub native_total_ms: f64,
    pub commit_ms: f64,
    pub metrics_write_ms: f64,
    pub commit_included: bool,
    pub process_lifetime_peak_rss_bytes: Option<u64>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexReport {
    pub indexed: usize,
    pub unchanged: usize,
    pub unsupported: usize,
    pub parse_errors: Vec<String>,
    #[serde(default)]
    pub index_revision: Option<i64>,
    #[serde(default)]
    pub revision_created: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_timings: Option<IndexStageTimings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<IndexResolutionStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<IndexCompletionTimings>,
}
