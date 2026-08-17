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
pub mod compress;
pub mod organize;
pub mod plan;
pub mod prompts;
pub mod refresh;

pub use analyze::{
    analyze_corpus, AnalyzeOptions, AnalysisIssue, AnalysisReport, ArchiveCandidate,
    CountBucket, CorpusCounts, DuplicateGroup, EmbedMismatchInfo, NearDuplicatePair,
    OrphanNode, SizeStats, StaleWikiPage, StubInfo,
};
pub use apply::{
    apply_maintenance_plan, ActionOutcome, ActionOutcomeKind, ApplyMaintenancePlanReport,
    ApplyPlanOptions,
};
pub use compress::{
    maintain_compress, CompressMergeAction, CompressOptions, CompressL0Report,
    MaintainCompressReport, COMPRESS_ACTION_WHITELIST, COMPRESS_LEVEL_MAX,
};
pub use organize::{
    maintain_organize, OrganizeMode, OrganizeOptions, OrganizeReport, RefileApplyResult,
    RefileSuggestion,
};
pub use plan::{
    action_from_parts, filter_plan, heuristic_plan, is_allowed_action, log_plan,
    maintenance_actions_json_schema, maintenance_plan_json_schema, parse_and_filter_plan,
    parse_plan_json, plan_maintenance, plan_with_llm, validate_action, validate_plan,
    MaintenanceAction, MaintenanceActionKind, MaintenancePlan, MaintenancePlanItem, PlanOptions,
    PlanSource, RejectedAction, ALLOWED_ACTIONS,
};
pub use prompts::{PLAN_MAINTENANCE_SYSTEM, PLAN_MAINTENANCE_USER_PREFIX};
pub use refresh::{
    maintain_refresh, rebuild_graph_for_all_or_dirty, rebuild_wiki_index, reembed_all,
    reindex_fts, GraphRebuildReport, MaintainRefreshFlags, MaintainRefreshReport,
    ReembedAllReport, ReindexFtsReport, WikiIndexRebuildReport, REFRESH_ACTION_WHITELIST,
};

// Stale wiki recompile (Karpathy layer hygiene); implementation in `crate::wiki`.
pub use crate::wiki::{
    find_stale_wiki, refresh_stale_wiki, RawRefreshTarget, RefreshError, RefreshStaleWikiResult,
    StaleWikiItem,
};
