//! Maintenance tool parameters.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct DoctorRepairParams {
    /// Preview only by default. Set false to reingest missing chunks and prune orphan rows.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Maximum documents without chunks to repair in one call.
    #[serde(default)]
    pub max_docs: Option<usize>,
}

/// Parameters for precise legacy source duplicate cleanup.
///
/// Preview is the default. Apply requires both `dry_run=false` and
/// `confirm=true`; the configured maintenance cap remains the hard upper bound.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct CleanupSourceDuplicatesParams {
    /// When true, inspect and report without changing the store (default true).
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Required together with `dry_run=false` before raw duplicates are removed.
    #[serde(default)]
    pub confirm: Option<bool>,
    /// Maximum documents that may be removed atomically in one call.
    #[serde(default)]
    pub max_candidates: Option<u32>,
}

// --- Maintenance ---

/// Parameters for `analyze_corpus` (deterministic health report; no LLM).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct AnalyzeCorpusParams {
    /// Unresolved stubs older than this many days are flagged as aging (default 14).
    #[serde(default)]
    pub stub_age_days: Option<u32>,
    /// When true, scan first-chunk embeddings for near-duplicates (default false).
    #[serde(default)]
    pub include_near_dups: Option<bool>,
    /// Cosine threshold θ for near-dups (default `RAG_MAINT_NEAR_DUP_THRESHOLD`).
    #[serde(default)]
    pub near_dup_threshold: Option<f64>,
    /// Min age (days) for low-boost archive candidates (default 90).
    #[serde(default)]
    pub archive_min_age_days: Option<u32>,
    /// When false, skip appending `ops_log` for this run (default true).
    #[serde(default)]
    pub log_ops: Option<bool>,
}

/// Parameters for `plan_maintenance` (LLM or heuristic action plan from analysis).
///
/// When `RAG_LLM_ENABLED=false` (or `force_heuristic=true`), returns a deterministic
/// plan from the analysis signals with no network call. When LLM is enabled, calls
/// the local chat model with the analysis JSON + system prompt, parses JSON actions,
/// and validates against the server whitelist.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanMaintenanceParams {
    /// Optional AnalysisReport JSON object (from `analyze_corpus`). When omitted,
    /// runs `analyze_corpus` first with default options (no near-dups unless set).
    #[serde(default)]
    pub analysis: Option<serde_json::Value>,
    /// Cap validated actions in the plan (default min(30, `RAG_MAINT_MAX_DOCS`)).
    #[serde(default)]
    pub max_actions: Option<u32>,
    /// When true, skip LLM even if `RAG_LLM_ENABLED=true` (tests / offline).
    #[serde(default)]
    pub force_heuristic: Option<bool>,
    /// When false, skip `ops_log` for this plan (default true).
    #[serde(default)]
    pub log_ops: Option<bool>,
    /// When analysis is omitted: pass through to analyze (near-dups).
    #[serde(default)]
    pub include_near_dups: Option<bool>,
}

/// Parameters for `maintain_organize` (suggest/apply refiles for docs missing wing).
///
/// Heuristic path/title/embedding suggestions by default; optional local LLM.
/// `dry_run` defaults to **true** (preview only). Set `dry_run=false` to apply
/// whitelist `refile` actions. Cap: `RAG_MAINT_MAX_DOCS` / `max_docs`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct MaintainOrganizeParams {
    /// When true (default), report suggestions without mutating placement.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// `heuristic` (default) | `llm` | `auto` (heuristic then LLM fill-in).
    #[serde(default)]
    pub mode: Option<String>,
    /// Cap unscoped documents considered / applied (default `RAG_MAINT_MAX_DOCS`).
    #[serde(default)]
    pub max_docs: Option<u32>,
    /// Only act on suggestions with confidence ≥ this (default 0.5).
    #[serde(default)]
    pub min_confidence: Option<f64>,
    /// When true (and not dry_run), rebuild wiki_index after refiles.
    #[serde(default)]
    pub rebuild_index: Option<bool>,
    /// Optional agent name for ops_log.
    #[serde(default)]
    pub agent: Option<String>,
}

/// Parameters for `maintain_refresh` (actualize FTS / graph / wiki index / reembed).
///
/// Whitelist actions only (`reindex_fts`, `rebuild_graph`, `rebuild_wiki_index`,
/// `reembed_all`). When no action flag is set, safe defaults run: FTS reindex +
/// dirty graph rebuild + wiki index rebuild (no reembed). When any action is
/// set explicitly, unspecified actions default to false.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct MaintainRefreshParams {
    /// Rebuild DuckDB FTS / BM25 index on `chunks` (after bulk deletes).
    #[serde(default)]
    pub reindex_fts: Option<bool>,
    /// Rebuild object-graph slices from document content (wikilinks/tags).
    #[serde(default)]
    pub rebuild_graph: Option<bool>,
    /// When rebuilding graph: only docs missing a document node (default true).
    /// Set false to rebuild graph for all documents (capped by max_docs).
    #[serde(default)]
    pub graph_dirty_only: Option<bool>,
    /// Clear and rebuild `wiki_index` from all `layer=wiki` documents.
    #[serde(default)]
    pub rebuild_wiki_index: Option<bool>,
    /// Opt-in: re-embed all document chunks with the live embedder and refresh
    /// `embedding_manifest` (use complete corpus pass after embedding identity
    /// change). Cap: `RAG_MAINT_MAX_DOCS`.
    #[serde(default)]
    pub reembed_all: Option<bool>,
    /// When true, report planned work without mutating store (still logs dry_run).
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Requested graph/reembed cap; values above `RAG_MAINT_MAX_DOCS` are clamped.
    #[serde(default)]
    pub max_docs: Option<u32>,
}

/// One action in an `apply_maintenance_plan` request (mirrors maintain plan JSON).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MaintenanceActionParams {
    /// Whitelisted op name (refile, pin, archive, rebuild_index, …).
    pub action: String,
    /// Human rationale from planner (optional).
    #[serde(default)]
    pub reason: Option<String>,
    /// Target document / node / slug id when applicable.
    #[serde(default)]
    pub target_id: Option<String>,
    /// Free-form parameters (wing, room, boost, tags, …) as a JSON object.
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// Parameters for `apply_maintenance_plan`.
///
/// Executes a whitelist-only action list. **`dry_run` defaults to true** (preview
/// without mutation; still logs). Document-scoped ops are capped by
/// `max_docs` / `RAG_MAINT_MAX_DOCS`. Every action is ops_logged; returns
/// `applied` / `skipped` / `errors`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ApplyMaintenancePlanParams {
    /// Ordered whitelist actions to apply (or preview).
    pub actions: Vec<MaintenanceActionParams>,
    /// When true (default), preview without mutating store.
    /// Set `false` to execute.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Requested document cap; values above `RAG_MAINT_MAX_DOCS` are clamped.
    #[serde(default)]
    pub max_docs: Option<u32>,
    /// Optional agent name for ops_log.
    #[serde(default)]
    pub agent: Option<String>,
}

/// Parameters for `maintain_compress` (L0 checkpoint+FTS, L1 exact hash merge,
/// L2 near-dup list; merge only with `confirm`).
///
/// Never hard-deletes `layer=raw` without `allow_raw_delete=true` (tombstones
/// instead). Default `dry_run=true` for safety.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct MaintainCompressParams {
    /// Compression level: 0=checkpoint+FTS, 1=+exact hash merge, 2=+near-dup list
    /// (near-dup merge only when `confirm=true`). Default 0.
    #[serde(default)]
    pub level: Option<u32>,
    /// When true (default), plan only — no mutations. Set false to apply.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Required to apply near-duplicate merges at level ≥ 2. List always runs.
    #[serde(default)]
    pub confirm: Option<bool>,
    /// When true, hard-delete raw-layer duplicates. Default false (tombstone).
    #[serde(default)]
    pub allow_raw_delete: Option<bool>,
    /// Cosine threshold θ for near-dups (default `RAG_MAINT_NEAR_DUP_THRESHOLD`).
    #[serde(default)]
    pub near_dup_threshold: Option<f64>,
    /// Cap docs removed/tombstoned per run (default `RAG_MAINT_MAX_DOCS`).
    #[serde(default)]
    pub max_docs: Option<u32>,
}
