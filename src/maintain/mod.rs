//! Local corpus maintenance: analyze → plan → dry_run/apply → compress → refresh.
//!
//! Design: [`docs/LOCAL_LLM_MAINTENANCE.md`](../../docs/LOCAL_LLM_MAINTENANCE.md).
//!
//! - Deterministic signals live in [`analyze`] (no LLM required).
//! - Action JSON is validated against a whitelist in [`plan`] before apply.
//! - [`plan`] builds LLM or heuristic [`MaintenancePlan`] via `plan_maintenance`.
//! - [`apply`] executes whitelist actions (`apply_maintenance_plan`, dry_run default).
//! - [`organize`] suggests/applies refiles; [`compress`] L0–L2 storage hygiene.
//! - [`refresh`] actualizes FTS / graph / wiki index / optional reembed.
//! - Mutations must append `ops_log` and never silently delete `layer=raw`.
//! - Compile/improve: `crate::wiki::consolidate` (LLM merge N docs → one wiki page).

pub mod analyze;
pub mod apply;
mod audit;
pub mod compress;
pub mod organize;
pub mod plan;
pub mod prompts;
pub mod refresh;

pub use crate::config::limits::DEFAULT_MAX_DOCS;

pub use analyze::{
    AnalysisIssue, AnalysisReport, AnalyzeOptions, ArchiveCandidate, CorpusCounts, CountBucket,
    DuplicateGroup, EmbedMismatchInfo, NearDuplicatePair, OrphanNode, SizeStats, StaleWikiPage,
    StubInfo, analyze_corpus,
};
pub use apply::{
    ActionOutcome, ActionOutcomeKind, ApplyMaintenancePlanReport, ApplyPlanOptions,
    apply_maintenance_plan,
};
pub use compress::{
    COMPRESS_ACTION_WHITELIST, COMPRESS_LEVEL_MAX, CompressL0Report, CompressMergeAction,
    CompressOptions, MaintainCompressReport, maintain_compress,
};
pub use organize::{
    OrganizeMode, OrganizeOptions, OrganizeReport, RefileApplyResult, RefileSuggestion,
    maintain_organize,
};
pub use plan::{
    ALLOWED_ACTIONS, MaintenanceAction, MaintenanceActionKind, MaintenancePlan,
    MaintenancePlanItem, PlanOptions, PlanSource, RejectedAction, action_from_parts, filter_plan,
    heuristic_plan, is_allowed_action, log_plan, maintenance_actions_json_schema,
    maintenance_plan_json_schema, parse_and_filter_plan, parse_plan_json, plan_maintenance,
    plan_with_llm, validate_action, validate_plan,
};
pub use prompts::{PLAN_MAINTENANCE_SYSTEM, PLAN_MAINTENANCE_USER_PREFIX};
pub use refresh::{
    GraphRebuildReport, MaintainRefreshFlags, MaintainRefreshReport, REFRESH_ACTION_WHITELIST,
    ReembedAllReport, ReembedMigrationStatus, ReembedStatus, ReindexFtsReport,
    WikiIndexRebuildReport, maintain_refresh, rebuild_graph_for_all_or_dirty, rebuild_wiki_index,
    reembed_all, reembed_status, reindex_fts,
};

// Stale wiki recompile (Karpathy layer hygiene); implementation in `crate::wiki`.
pub use crate::wiki::{
    RawRefreshTarget, RefreshError, RefreshStaleWikiResult, StaleWikiItem, find_stale_wiki,
    refresh_stale_wiki,
};
