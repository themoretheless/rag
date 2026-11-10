//! Whitelist of maintenance actions and `plan_maintenance` (LLM or heuristic).
//!
//! Server-side validation only: inventing SQL or shell is never allowed.
//! Design: `docs/LOCAL_LLM_MAINTENANCE.md` M2 + §5 (action whitelist).

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::analyze::AnalysisReport;
use super::prompts::{PLAN_MAINTENANCE_SYSTEM, PLAN_MAINTENANCE_USER_PREFIX};
use crate::config::Config;
use crate::db::Store;
use crate::error::{AppError, Result};
use crate::llm::json::{parse_embedded, strip_json_fences, ERROR_EXCERPT_CHARS};
use crate::llm::ChatMessage;
use crate::models::OpsLogEntry;
use crate::util::excerpt_chars as truncate_chars;

/// Default cap on actions returned by a plan (LLM or heuristic).
const DEFAULT_MAX_ACTIONS: usize = 30;
/// Truncate analysis JSON fed to the LLM.
const ANALYSIS_JSON_MAX_CHARS: usize = 24_000;

// ---------------------------------------------------------------------------
// Action whitelist (enum)
// ---------------------------------------------------------------------------

/// Canonical maintenance operations accepted by `plan_maintenance` /
/// `apply_maintenance_plan`.
///
/// This enum **is** the safety whitelist: unknown wire names are rejected.
/// JSON uses `snake_case` (e.g. `"refile"`, `"merge_exact_dup"`).
///
/// Task aliases: Refile, Archive, Pin, MergeExactDup, RebuildIndex, Vacuum,
/// ReindexFts, ConsolidatePropose, ResolveStub, …
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceAction {
    // Organize / place
    Refile,
    Pin,
    Unpin,
    SetBoost,
    SetTags,
    Archive,
    Unarchive,
    // Catalog / index
    RebuildIndex,
    RebuildGraph,
    ReindexFts,
    // Embeddings
    Reembed,
    ReembedAll,
    // Compile / improve
    CompileSource,
    /// Propose consolidate body (no write).
    ConsolidatePropose,
    /// Apply consolidate / write concept page.
    Consolidate,
    RefreshStaleWiki,
    FileAnswer,
    /// Promote unresolved stub → real document.
    ResolveStub,
    /// Propose missing wikilinks / backlinks.
    LintAndFixLinks,
    /// Create/update outline page.
    BuildOutline,
    // Compress / hygiene
    MergeExactDup,
    MergeNearDup,
    DropTombstones,
    Vacuum,
    Checkpoint,
    // Meta
    Noop,
}

impl MaintenanceAction {
    /// All whitelist variants in stable wire order.
    pub const ALL: &'static [MaintenanceAction] = &[
        Self::Refile,
        Self::Pin,
        Self::Unpin,
        Self::SetBoost,
        Self::SetTags,
        Self::Archive,
        Self::Unarchive,
        Self::RebuildIndex,
        Self::RebuildGraph,
        Self::ReindexFts,
        Self::Reembed,
        Self::ReembedAll,
        Self::CompileSource,
        Self::ConsolidatePropose,
        Self::Consolidate,
        Self::RefreshStaleWiki,
        Self::FileAnswer,
        Self::ResolveStub,
        Self::LintAndFixLinks,
        Self::BuildOutline,
        Self::MergeExactDup,
        Self::MergeNearDup,
        Self::DropTombstones,
        Self::Vacuum,
        Self::Checkpoint,
        Self::Noop,
    ];

    /// Snake_case wire name (matches serde).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Refile => "refile",
            Self::Pin => "pin",
            Self::Unpin => "unpin",
            Self::SetBoost => "set_boost",
            Self::SetTags => "set_tags",
            Self::Archive => "archive",
            Self::Unarchive => "unarchive",
            Self::RebuildIndex => "rebuild_index",
            Self::RebuildGraph => "rebuild_graph",
            Self::ReindexFts => "reindex_fts",
            Self::Reembed => "reembed",
            Self::ReembedAll => "reembed_all",
            Self::CompileSource => "compile_source",
            Self::ConsolidatePropose => "consolidate_propose",
            Self::Consolidate => "consolidate",
            Self::RefreshStaleWiki => "refresh_stale_wiki",
            Self::FileAnswer => "file_answer",
            Self::ResolveStub => "resolve_stub",
            Self::LintAndFixLinks => "lint_and_fix_links",
            Self::BuildOutline => "build_outline",
            Self::MergeExactDup => "merge_exact_dup",
            Self::MergeNearDup => "merge_near_dup",
            Self::DropTombstones => "drop_tombstones",
            Self::Vacuum => "vacuum",
            Self::Checkpoint => "checkpoint",
            Self::Noop => "noop",
        }
    }

    /// Parse trimmed snake_case name; unknown → `None`.
    pub fn parse(name: &str) -> Option<Self> {
        let a = name.trim();
        Self::ALL.iter().copied().find(|v| v.as_str() == a)
    }

    /// Ops that normally require a non-empty `target_id`.
    pub fn requires_target_id(self) -> bool {
        matches!(
            self,
            Self::Refile
                | Self::Pin
                | Self::Unpin
                | Self::SetBoost
                | Self::SetTags
                | Self::Archive
                | Self::Unarchive
                | Self::Reembed
                | Self::CompileSource
                | Self::ResolveStub
        )
    }

    /// Merge family (keep + drop / multi-id params).
    pub fn is_merge(self) -> bool {
        matches!(self, Self::MergeExactDup | Self::MergeNearDup)
    }

    /// Higher-risk / confirm-friendly compress-adjacent ops.
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            Self::MergeExactDup
                | Self::MergeNearDup
                | Self::DropTombstones
                | Self::Archive
                | Self::ReembedAll
        )
    }
}

impl std::fmt::Display for MaintenanceAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Alias for the whitelist enum (older name used by apply/organize).
pub type MaintenanceActionKind = MaintenanceAction;

/// Snake_case names of every allowed action (prompts / errors).
pub const ALLOWED_ACTIONS: &[&str] = &[
    "refile",
    "pin",
    "unpin",
    "set_boost",
    "set_tags",
    "archive",
    "unarchive",
    "rebuild_index",
    "rebuild_graph",
    "reindex_fts",
    "reembed",
    "reembed_all",
    "compile_source",
    "consolidate_propose",
    "consolidate",
    "refresh_stale_wiki",
    "file_answer",
    "resolve_stub",
    "lint_and_fix_links",
    "build_outline",
    "merge_exact_dup",
    "merge_near_dup",
    "drop_tombstones",
    "vacuum",
    "checkpoint",
    "noop",
];

// ---------------------------------------------------------------------------
// Plan item (LLM JSON step)
// ---------------------------------------------------------------------------

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}

/// One validated maintenance step (LLM output unit after whitelist filter).
///
/// Example:
/// ```json
/// {
///   "action": "refile",
///   "reason": "wrong wing",
///   "target_id": "doc-id",
///   "params": { "wing": "projects", "room": "rag" }
/// }
/// ```
///
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MaintenancePlanItem {
    /// Whitelisted op ([`MaintenanceAction`]).
    pub action: MaintenanceAction,
    /// Human rationale from planner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Target document / node / stub id when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    /// Free-form parameters (wing, room, document_ids, …).
    #[serde(default = "empty_object")]
    pub params: serde_json::Value,
}

/// Same as [`MaintenancePlanItem`] (plan step with reason/target/params).
pub type MaintenanceActionStep = MaintenancePlanItem;

/// Loose wire shape used while parsing LLM output (action is a string so
/// unknown names can be soft-rejected via [`filter_plan`]).
#[derive(Debug, Clone, Deserialize)]
struct RawPlanItem {
    #[serde(default)]
    action: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default = "empty_object")]
    params: serde_json::Value,
}

impl RawPlanItem {
    fn into_validated(self) -> Result<MaintenancePlanItem> {
        let kind = MaintenanceAction::parse(&self.action).ok_or_else(|| {
            AppError::forbidden(format!(
                "maintenance action '{}' is not on the whitelist; allowed: {}",
                self.action.trim(),
                ALLOWED_ACTIONS.join(", ")
            ))
        })?;
        validate_action(MaintenancePlanItem {
            action: kind,
            reason: self.reason,
            target_id: self.target_id,
            params: self.params,
        })
    }
}

/// Source of a [`MaintenancePlan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanSource {
    /// Deterministic mapping from AnalysisReport issues (no LLM).
    Heuristic,
    /// Local chat model proposed actions.
    Llm,
}

/// Action dropped during plan validation (non-fatal for LLM plans).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct RejectedAction {
    /// Original action name (if any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Why it was rejected.
    pub reason: String,
    /// Raw JSON fragment when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

/// Result of `plan_maintenance`: ordered, whitelist-validated actions.
/// No JsonSchema: result type, never handed to the LLM as a schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenancePlan {
    pub generated_at: DateTime<Utc>,
    /// `heuristic` or `llm`.
    pub source: PlanSource,
    /// Whether chat LLM was enabled for this call.
    pub llm_enabled: bool,
    /// Model tag when source is LLM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Validated actions ready for `apply_maintenance_plan`.
    pub actions: Vec<MaintenancePlanItem>,
    /// Actions dropped by whitelist / parse filter (LLM path).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<RejectedAction>,
    /// Planner notes (LLM notes field or heuristic summary).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Issue count from the input analysis report.
    pub issue_count: usize,
    pub action_count: usize,
}

/// Options for [`plan_maintenance`].
#[derive(Debug, Clone)]
pub struct PlanOptions {
    /// Cap accepted actions after validation (default 30).
    pub max_actions: usize,
    /// When true, always use heuristic even if LLM is enabled.
    pub force_heuristic: bool,
    /// When true, append a `plan_maintenance` row to `ops_log`.
    pub log_ops: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            max_actions: DEFAULT_MAX_ACTIONS,
            force_heuristic: false,
            log_ops: true,
        }
    }
}

impl PlanOptions {
    /// Build from config (`maint_max_docs` as soft upper bound on action count).
    pub fn from_config(config: &Config) -> Self {
        Self {
            max_actions: config.maint_max_docs.clamp(1, DEFAULT_MAX_ACTIONS * 2),
            force_heuristic: false,
            log_ops: true,
        }
    }
}

// ---------------------------------------------------------------------------
// JSON Schema (LLM structured output / prompts)
// ---------------------------------------------------------------------------

/// JSON Schema for [`MaintenancePlanItem`] arrays (LLM tool / response format).
pub fn maintenance_actions_json_schema() -> serde_json::Value {
    let schema = schemars::schema_for!(Vec<MaintenancePlanItem>);
    serde_json::to_value(schema).unwrap_or_else(|_| {
        serde_json::json!({
            "type": "array",
            "items": { "type": "object" }
        })
    })
}

/// JSON Schema for a plan envelope `{ actions, notes? }` (simplified for prompts).
pub fn maintenance_plan_json_schema() -> serde_json::Value {
    let item = schemars::schema_for!(MaintenancePlanItem);
    let item_val = serde_json::to_value(item).unwrap_or(serde_json::json!({"type": "object"}));
    serde_json::json!({
        "type": "object",
        "required": ["actions"],
        "properties": {
            "actions": {
                "type": "array",
                "items": item_val
            },
            "notes": { "type": ["string", "null"] }
        },
        "additionalProperties": true
    })
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// True when `action` is on the server whitelist (case-sensitive snake_case).
pub fn is_allowed_action(action: &str) -> bool {
    MaintenanceAction::parse(action).is_some()
}

fn non_empty_id(id: &str) -> bool {
    !id.trim().is_empty()
}

/// Validate one plan item; normalizes empty params; enforces per-op constraints.
pub fn validate_action(mut item: MaintenancePlanItem) -> Result<MaintenancePlanItem> {
    if !item.params.is_object() && !item.params.is_null() {
        return Err(AppError::invalid(
            "maintenance action params must be a JSON object",
        ));
    }
    if item.params.is_null() {
        item.params = empty_object();
    }

    if let Some(ref tid) = item.target_id {
        if !non_empty_id(tid) {
            return Err(AppError::invalid(
                "maintenance action target_id must not be empty when set",
            ));
        }
    }

    if item.action.requires_target_id() {
        match item.target_id.as_deref() {
            Some(id) if non_empty_id(id) => {}
            _ => {
                return Err(AppError::invalid(format!(
                    "maintenance action '{}' requires non-empty target_id",
                    item.action
                )));
            }
        }
    }

    // Light param checks for high-traffic ops (apply layer does deeper checks).
    match item.action {
        MaintenanceAction::SetBoost => {
            let boost = item.params.get("boost");
            match boost {
                Some(v) if v.is_number() => {}
                Some(_) => {
                    return Err(AppError::invalid(
                        "maintenance action 'set_boost' params.boost must be a number",
                    ));
                }
                None => {
                    return Err(AppError::invalid(
                        "maintenance action 'set_boost' requires params.boost",
                    ));
                }
            }
        }
        MaintenanceAction::MergeExactDup | MaintenanceAction::MergeNearDup => {
            // Apply may resolve peers by content_hash / embedding from target_id alone.
            let Some(obj) = item.params.as_object() else {
                return Err(AppError::invalid(format!(
                    "maintenance action '{}' params must be a JSON object",
                    item.action
                )));
            };
            let has_group = obj
                .get("document_ids")
                .and_then(|v| v.as_array())
                .map(|a| a.len() >= 2)
                .unwrap_or(false);
            let has_pair = obj.get("document_id_b").and_then(|v| v.as_str()).is_some()
                || obj.get("drop_id").and_then(|v| v.as_str()).is_some()
                || obj.get("source_id").and_then(|v| v.as_str()).is_some()
                || obj
                    .get("source_ids")
                    .and_then(|v| v.as_array())
                    .map(|a| !a.is_empty())
                    .unwrap_or(false);
            let has_target = item.target_id.as_deref().map(non_empty_id).unwrap_or(false);
            if !(has_group || has_target || has_pair) {
                return Err(AppError::invalid(format!(
                    "maintenance action '{}' needs target_id and/or params.document_ids (or drop/source ids)",
                    item.action
                )));
            }
        }
        MaintenanceAction::ConsolidatePropose | MaintenanceAction::Consolidate => {
            let Some(obj) = item.params.as_object() else {
                return Err(AppError::invalid(format!(
                    "maintenance action '{}' params must be a JSON object",
                    item.action
                )));
            };
            let has_sources = obj
                .get("source_ids")
                .or_else(|| obj.get("document_ids"))
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .any(|x| x.as_str().map(non_empty_id).unwrap_or(false))
                })
                .unwrap_or(false);
            let has_target = item.target_id.as_deref().map(non_empty_id).unwrap_or(false);
            if !has_sources && !has_target {
                return Err(AppError::invalid(format!(
                    "maintenance action '{}' requires params.source_ids/document_ids or target_id",
                    item.action
                )));
            }
        }
        _ => {}
    }

    Ok(item)
}

/// Validate a plan list; rejects any invalid entry (all-or-nothing).
pub fn validate_plan(actions: Vec<MaintenancePlanItem>) -> Result<Vec<MaintenancePlanItem>> {
    let mut out = Vec::with_capacity(actions.len());
    for a in actions {
        out.push(validate_action(a)?);
    }
    Ok(out)
}

/// Validate each action independently; collect accepted + rejected (LLM-friendly).
pub fn filter_plan(
    actions: Vec<MaintenancePlanItem>,
) -> (Vec<MaintenancePlanItem>, Vec<RejectedAction>) {
    let mut accepted = Vec::with_capacity(actions.len());
    let mut rejected = Vec::new();
    for a in actions {
        let name = a.action.as_str().to_string();
        match validate_action(a) {
            Ok(v) => accepted.push(v),
            Err(e) => rejected.push(RejectedAction {
                action: Some(name),
                reason: e.to_string(),
                raw: None,
            }),
        }
    }
    (accepted, rejected)
}

/// Soft-parse raw items (string action names) into validated plan items.
fn filter_raw_plan(raw: Vec<RawPlanItem>) -> (Vec<MaintenancePlanItem>, Vec<RejectedAction>) {
    let mut accepted = Vec::with_capacity(raw.len());
    let mut rejected = Vec::new();
    for r in raw {
        let name = r.action.clone();
        let raw_json = serde_json::to_value(serde_json::json!({
            "action": r.action,
            "reason": r.reason,
            "target_id": r.target_id,
            "params": r.params,
        }))
        .ok();
        match r.into_validated() {
            Ok(v) => accepted.push(v),
            Err(e) => rejected.push(RejectedAction {
                action: Some(name),
                reason: e.to_string(),
                raw: raw_json,
            }),
        }
    }
    (accepted, rejected)
}

// ---------------------------------------------------------------------------
// Parse LLM JSON
// ---------------------------------------------------------------------------

/// Raw LLM envelope: `{ "actions": [...], "notes": "..." }` or bare action array.
#[derive(Debug, Deserialize)]
struct PlanEnvelope {
    #[serde(default)]
    actions: Vec<RawPlanItem>,
    #[serde(default)]
    notes: Option<String>,
}

/// Parse model output into raw actions + optional notes (not yet whitelist-filtered).
///
/// Accepts:
/// - `{"actions":[...],"notes":...}`
/// - bare `[...]` array of actions
/// - fenced ```json blocks
///
/// Returns string-keyed raw items so unknown action names can be soft-rejected.
pub fn parse_plan_json(raw: &str) -> Result<(Vec<serde_json::Value>, Option<String>)> {
    let unfenced = strip_json_fences(raw);
    let (items, notes) = parse_plan_json_raw(unfenced)?;
    let values = items
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "action": r.action,
                "reason": r.reason,
                "target_id": r.target_id,
                "params": r.params,
            })
        })
        .collect();
    Ok((values, notes))
}

fn parse_plan_json_raw(unfenced: &str) -> Result<(Vec<RawPlanItem>, Option<String>)> {
    if let Ok(env) = serde_json::from_str::<PlanEnvelope>(unfenced) {
        if !env.actions.is_empty() || env.notes.is_some() {
            return Ok((env.actions, env.notes));
        }
    }
    if let Ok(list) = serde_json::from_str::<Vec<RawPlanItem>>(unfenced) {
        return Ok((list, None));
    }
    if let Some(env) = parse_embedded::<PlanEnvelope>(unfenced, '{', '}') {
        return Ok((env.actions, env.notes));
    }
    if let Some(list) = parse_embedded::<Vec<RawPlanItem>>(unfenced, '[', ']') {
        return Ok((list, None));
    }
    Err(AppError::llm(format!(
        "failed to parse maintenance plan JSON from model output: {}",
        truncate_chars(unfenced, ERROR_EXCERPT_CHARS)
    )))
}

/// Parse + whitelist-filter model output into validated plan items.
pub fn parse_and_filter_plan(
    raw: &str,
) -> Result<(
    Vec<MaintenancePlanItem>,
    Vec<RejectedAction>,
    Option<String>,
)> {
    let unfenced = strip_json_fences(raw);
    let (items, notes) = parse_plan_json_raw(unfenced)?;
    let (accepted, rejected) = filter_raw_plan(items);
    Ok((accepted, rejected, notes))
}

/// Build a [`MaintenancePlanItem`] from MCP/string parts (unknown name → error).
pub fn action_from_parts(
    action: &str,
    reason: Option<String>,
    target_id: Option<String>,
    params: serde_json::Value,
) -> Result<MaintenancePlanItem> {
    RawPlanItem {
        action: action.to_string(),
        reason,
        target_id,
        params,
    }
    .into_validated()
}

// ---------------------------------------------------------------------------
// Heuristic + LLM planners
// ---------------------------------------------------------------------------

/// Build a deterministic plan from an [`AnalysisReport`] (no LLM).
///
/// Priority: FTS / embed mismatch → exact dups → stale wiki → near dups → archives → vacuum.
pub fn heuristic_plan(report: &AnalysisReport, max_actions: usize) -> MaintenancePlan {
    let max_actions = max_actions.max(1);
    let mut actions: Vec<MaintenancePlanItem> = Vec::new();

    if report.embed_mismatch.is_some() {
        push_capped(
            &mut actions,
            max_actions,
            MaintenancePlanItem {
                action: MaintenanceAction::ReembedAll,
                reason: Some("embedding manifest disagrees with config".into()),
                target_id: None,
                params: empty_object(),
            },
        );
    }

    if !report.fts_ready && report.counts.chunks > 0 {
        push_capped(
            &mut actions,
            max_actions,
            MaintenancePlanItem {
                action: MaintenanceAction::ReindexFts,
                reason: Some("FTS index not ready while chunks exist".into()),
                target_id: None,
                params: empty_object(),
            },
        );
    }

    for g in &report.exact_duplicates {
        if actions.len() >= max_actions {
            break;
        }
        let keep = g.document_ids.first().cloned();
        actions.push(MaintenancePlanItem {
            action: MaintenanceAction::MergeExactDup,
            reason: Some(format!("{} documents share content_hash", g.count)),
            target_id: keep,
            params: serde_json::json!({
                "document_ids": g.document_ids,
                "content_hash": g.content_hash,
                "titles": g.titles,
            }),
        });
    }

    for w in &report.stale_wiki {
        if actions.len() >= max_actions {
            break;
        }
        actions.push(MaintenancePlanItem {
            action: if w.link_kind == "source_missing" {
                MaintenanceAction::Noop
            } else {
                MaintenanceAction::RefreshStaleWiki
            },
            reason: Some(match w.link_kind.as_str() {
                "source_missing" => format!(
                    "source '{}' for wiki '{}' is unavailable; restore or relink before refresh",
                    w.raw_title, w.wiki_title
                ),
                "source_version" => format!(
                    "source content '{}' changed since wiki '{}' was generated",
                    w.raw_title, w.wiki_title
                ),
                _ => format!("wiki '{}' older than raw '{}'", w.wiki_title, w.raw_title),
            }),
            target_id: Some(w.wiki_document_id.clone()),
            params: serde_json::json!({
                "raw_document_id": w.raw_document_id,
            }),
        });
    }

    for p in &report.near_duplicates {
        if actions.len() >= max_actions {
            break;
        }
        actions.push(MaintenancePlanItem {
            action: MaintenanceAction::MergeNearDup,
            reason: Some(format!("near-duplicate cosine={:.3}", p.cosine)),
            target_id: Some(p.document_id_a.clone()),
            params: serde_json::json!({
                "document_id_a": p.document_id_a,
                "document_id_b": p.document_id_b,
                "cosine": p.cosine,
            }),
        });
    }

    for a in &report.archive_candidates {
        if actions.len() >= max_actions {
            break;
        }
        actions.push(MaintenancePlanItem {
            action: MaintenanceAction::Archive,
            reason: Some(a.reason.clone()),
            target_id: Some(a.document_id.clone()),
            params: serde_json::json!({
                "title": a.title,
                "boost": a.boost,
            }),
        });
    }

    for s in &report.unresolved_stubs {
        if actions.len() >= max_actions {
            break;
        }
        actions.push(MaintenancePlanItem {
            action: MaintenanceAction::ResolveStub,
            reason: Some(format!("unresolved stub '{}'", s.label)),
            target_id: Some(s.node_id.clone()),
            params: serde_json::json!({
                "label": s.label,
            }),
        });
    }

    if !report.stale_wiki.is_empty() || !report.orphan_wiki_pages.is_empty() {
        push_capped(
            &mut actions,
            max_actions,
            MaintenancePlanItem {
                action: MaintenanceAction::RebuildIndex,
                reason: Some("wiki catalog may be stale after refresh/orphans".into()),
                target_id: None,
                params: empty_object(),
            },
        );
    }

    if !report.orphan_nodes.is_empty() {
        push_capped(
            &mut actions,
            max_actions,
            MaintenancePlanItem {
                action: MaintenanceAction::RebuildGraph,
                reason: Some(format!(
                    "{} orphan graph nodes (degree 0)",
                    report.orphan_nodes.len()
                )),
                target_id: None,
                params: serde_json::json!({ "dirty_only": false }),
            },
        );
    }

    let mut notes = format!(
        "heuristic plan from {} issues ({} actions)",
        report.issue_count,
        actions.len()
    );

    if actions.is_empty() {
        actions.push(MaintenancePlanItem {
            action: MaintenanceAction::Noop,
            reason: Some("corpus healthy; no maintenance actions proposed".into()),
            target_id: None,
            params: empty_object(),
        });
        notes = "heuristic: no issues requiring actions".into();
    } else {
        push_capped(
            &mut actions,
            max_actions,
            MaintenancePlanItem {
                action: MaintenanceAction::Vacuum,
                reason: Some("checkpoint after maintenance".into()),
                target_id: None,
                params: empty_object(),
            },
        );
        notes = format!(
            "heuristic plan from {} issues ({} actions)",
            report.issue_count,
            actions.len()
        );
    }

    let (actions, rejected) = filter_plan(actions);
    let action_count = actions.len();

    MaintenancePlan {
        generated_at: Utc::now(),
        source: PlanSource::Heuristic,
        llm_enabled: false,
        model: None,
        actions,
        rejected,
        notes: Some(notes),
        issue_count: report.issue_count,
        action_count,
    }
}

fn push_capped(actions: &mut Vec<MaintenancePlanItem>, max: usize, action: MaintenancePlanItem) {
    if actions.len() < max {
        actions.push(action);
    }
}

/// Call local LLM with analysis JSON + system prompt; parse and validate actions.
pub async fn plan_with_llm(
    report: &AnalysisReport,
    llm: &dyn crate::llm::ChatCompleter,
    max_actions: usize,
) -> Result<MaintenancePlan> {
    let analysis_json = serde_json::to_string(report).map_err(|e| {
        AppError::llm(format!(
            "failed to serialize AnalysisReport for planner: {e}"
        ))
    })?;
    let analysis_json = truncate_chars(&analysis_json, ANALYSIS_JSON_MAX_CHARS);

    let user = format!("{PLAN_MAINTENANCE_USER_PREFIX}{analysis_json}");
    let messages = [
        ChatMessage {
            role: "system".into(),
            content: PLAN_MAINTENANCE_SYSTEM.into(),
        },
        ChatMessage {
            role: "user".into(),
            content: user,
        },
    ];
    let raw = llm.complete(&messages).await?;
    let unfenced = strip_json_fences(&raw);
    let (parsed, notes) = parse_plan_json_raw(unfenced).map_err(|e| {
        // Preserve llm-flavoured error for bad model output.
        AppError::llm(e.to_string())
    })?;
    let (mut actions, mut rejected) = filter_raw_plan(parsed);

    if actions.len() > max_actions.max(1) {
        let dropped = actions.split_off(max_actions.max(1));
        for d in dropped {
            rejected.push(RejectedAction {
                action: Some(d.action.as_str().to_string()),
                reason: format!("exceeded max_actions={max_actions}"),
                raw: None,
            });
        }
    }

    if actions.is_empty() {
        actions.push(MaintenancePlanItem {
            action: MaintenanceAction::Noop,
            reason: Some("LLM returned no valid whitelist actions".into()),
            target_id: None,
            params: empty_object(),
        });
    }

    let action_count = actions.len();
    Ok(MaintenancePlan {
        generated_at: Utc::now(),
        source: PlanSource::Llm,
        llm_enabled: true,
        model: Some(llm.model().to_string()),
        actions,
        rejected,
        notes,
        issue_count: report.issue_count,
        action_count,
    })
}

/// Plan maintenance from an analysis report.
///
/// - When `llm_enabled` is false, or `force_heuristic`, or no client: heuristic plan.
/// - When LLM is enabled and a client is provided: call local chat, parse JSON, validate.
pub async fn plan_maintenance(
    report: &AnalysisReport,
    config: &Config,
    llm: Option<&dyn crate::llm::ChatCompleter>,
    opts: &PlanOptions,
) -> Result<MaintenancePlan> {
    let max_actions = opts.max_actions.max(1);
    let use_llm = config.llm_enabled && !opts.force_heuristic && llm.is_some();

    let llm_client = if use_llm { llm } else { None };
    let mut plan = if let Some(client) = llm_client {
        plan_with_llm(report, client, max_actions).await?
    } else {
        let mut p = heuristic_plan(report, max_actions);
        p.llm_enabled = config.llm_enabled;
        if config.llm_enabled && llm.is_none() && !opts.force_heuristic {
            p.notes = Some(format!(
                "{}; ChatClient unavailable, used heuristic",
                p.notes.as_deref().unwrap_or("heuristic plan")
            ));
        }
        p
    };

    plan.llm_enabled = config.llm_enabled;
    Ok(plan)
}

/// Append `plan_maintenance` to ops_log (best-effort: failures are logged, not propagated).
pub fn log_plan(store: &Store, plan: &MaintenancePlan) -> Result<()> {
    store.ops_log().record(
        &OpsLogEntry::new(
            "plan_maintenance",
            "MAINT",
            format!(
                "plan_maintenance: source={:?}, {} actions, {} rejected, {} issues",
                plan.source,
                plan.action_count,
                plan.rejected.len(),
                plan.issue_count
            ),
        )
        .at(plan.generated_at)
        .entity(None, "maintenance")
        .payload(
            serde_json::json!({
                "source": plan.source,
                "action_count": plan.action_count,
                "rejected_count": plan.rejected.len(),
                "issue_count": plan.issue_count,
                "llm_enabled": plan.llm_enabled,
                "model": plan.model,
                "actions": plan.actions.iter().map(|a| a.action.as_str()).collect::<Vec<_>>(),
            })
            .to_string(),
        ),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
