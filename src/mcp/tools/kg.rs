//! Temporal knowledge graph tool parameters.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

// --- Temporal knowledge graph (`kg_facts`) ---

/// Parameters for `kg_add` (insert temporal SPO fact; open-active SPO is idempotent).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct KgAddParams {
    /// Fact subject (entity / name).
    pub subject: String,
    /// Relation label (normalized: lowercased, spaces → underscores).
    pub predicate: String,
    /// Fact object / value.
    pub object: String,
    /// Optional validity start (RFC3339 or `YYYY-MM-DD[ HH:MM:SS]`); null = open past.
    #[serde(default)]
    pub valid_from: Option<String>,
    /// Optional validity end (half-open upper bound); null = open future.
    #[serde(default)]
    pub valid_to: Option<String>,
    /// Optional provenance document id (also stored as free-text `source`).
    #[serde(default)]
    pub source_document_id: Option<String>,
    /// Confidence score, typically 0.0–1.0 (default 1.0).
    #[serde(default)]
    pub confidence: Option<f64>,
    /// Optional metadata JSON object string (default `{}`).
    #[serde(default)]
    pub metadata_json: Option<String>,
}

/// Parameters for `kg_query` (filter active facts, or all statuses valid at `at_time`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct KgQueryParams {
    /// Filter by subject.
    #[serde(default)]
    pub subject: Option<String>,
    /// Filter by predicate (case-insensitive; spaces → underscores).
    #[serde(default)]
    pub predicate: Option<String>,
    /// Filter by object.
    #[serde(default)]
    pub object: Option<String>,
    /// Point-in-time query (RFC3339 / date). When set, uses half-open
    /// `[valid_from, valid_to)` instead of `status = active`.
    #[serde(default)]
    pub at_time: Option<String>,
}

/// Parameters for `kg_invalidate` (close open active fact(s) matching SPO).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct KgInvalidateParams {
    /// Fact subject.
    pub subject: String,
    /// Relation label.
    pub predicate: String,
    /// Fact object.
    pub object: String,
    /// End of validity (default now). Must not be before `valid_from`.
    #[serde(default)]
    pub ended: Option<String>,
}

/// Parameters for `kg_supersede` (close old object, open successor at shared boundary).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct KgSupersedeParams {
    /// Fact subject.
    pub subject: String,
    /// Relation label.
    pub predicate: String,
    /// Object value being replaced.
    pub old_object: String,
    /// Successor object value.
    pub new_object: String,
    /// Shared boundary instant (default now); old gets `valid_to=at`, new `valid_from=at`.
    #[serde(default)]
    pub at: Option<String>,
    /// Optional provenance document id for the successor fact.
    #[serde(default)]
    pub source_document_id: Option<String>,
    /// Confidence for the successor (default 1.0).
    #[serde(default)]
    pub confidence: Option<f64>,
}

/// Parameters for `kg_timeline` (all facts for a subject, any status, chronological).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct KgTimelineParams {
    /// Subject whose fact history to return.
    pub subject: String,
}
