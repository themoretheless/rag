//! Whitelist of maintenance actions and `plan_maintenance` (LLM or heuristic).
//!
//! Server-side validation only: inventing SQL or shell is never allowed.
//! Design: `docs/LOCAL_LLM_MAINTENANCE.md` M2 + §5 (action whitelist).

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::analyze::AnalysisReport;
use super::prompts::{PLAN_MAINTENANCE_SYSTEM, PLAN_MAINTENANCE_USER_PREFIX};
use crate::config::Config;
use crate::db::Store;
use crate::error::{AppError, Result};
use crate::llm::{ChatClient, ChatMessage};
use crate::models::OpsLogEntry;

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
/// No JsonSchema (result type; DateTime clashes with dual schemars versions).
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
            max_actions: config.maint_max_docs.max(1).min(DEFAULT_MAX_ACTIONS * 2),
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
        return Err(AppError::config(
            "maintenance action params must be a JSON object",
        ));
    }
    if item.params.is_null() {
        item.params = empty_object();
    }

    if let Some(ref tid) = item.target_id {
        if !non_empty_id(tid) {
            return Err(AppError::config(
                "maintenance action target_id must not be empty when set",
            ));
        }
    }

    if item.action.requires_target_id() {
        match item.target_id.as_deref() {
            Some(id) if non_empty_id(id) => {}
            _ => {
                return Err(AppError::config(format!(
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
                    return Err(AppError::config(
                        "maintenance action 'set_boost' params.boost must be a number",
                    ));
                }
                None => {
                    return Err(AppError::config(
                        "maintenance action 'set_boost' requires params.boost",
                    ));
                }
            }
        }
        MaintenanceAction::MergeExactDup | MaintenanceAction::MergeNearDup => {
            // Apply may resolve peers by content_hash / embedding from target_id alone.
            let obj = item.params.as_object().expect("params object");
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
            let has_target = item
                .target_id
                .as_deref()
                .map(non_empty_id)
                .unwrap_or(false);
            if !(has_group || has_target || has_pair) {
                return Err(AppError::config(format!(
                    "maintenance action '{}' needs target_id and/or params.document_ids (or drop/source ids)",
                    item.action
                )));
            }
        }
        MaintenanceAction::ConsolidatePropose | MaintenanceAction::Consolidate => {
            let obj = item.params.as_object().expect("params object");
            let has_sources = obj
                .get("source_ids")
                .or_else(|| obj.get("document_ids"))
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .any(|x| x.as_str().map(non_empty_id).unwrap_or(false))
                })
                .unwrap_or(false);
            let has_target = item
                .target_id
                .as_deref()
                .map(non_empty_id)
                .unwrap_or(false);
            if !has_sources && !has_target {
                return Err(AppError::config(format!(
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
        let raw_json = serde_json::to_value(&serde_json::json!({
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
    let unfenced = strip_json_fences(raw.trim());
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
    if let (Some(start), Some(end)) = (unfenced.find('{'), unfenced.rfind('}')) {
        if end > start {
            let slice = &unfenced[start..=end];
            if let Ok(env) = serde_json::from_str::<PlanEnvelope>(slice) {
                return Ok((env.actions, env.notes));
            }
        }
    }
    if let (Some(start), Some(end)) = (unfenced.find('['), unfenced.rfind(']')) {
        if end > start {
            let slice = &unfenced[start..=end];
            if let Ok(list) = serde_json::from_str::<Vec<RawPlanItem>>(slice) {
                return Ok((list, None));
            }
        }
    }
    Err(AppError::llm(format!(
        "failed to parse maintenance plan JSON from model output: {}",
        truncate(unfenced, 400)
    )))
}

/// Parse + whitelist-filter model output into validated plan items.
pub fn parse_and_filter_plan(
    raw: &str,
) -> Result<(Vec<MaintenancePlanItem>, Vec<RejectedAction>, Option<String>)> {
    let unfenced = strip_json_fences(raw.trim());
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

fn strip_json_fences(trimmed: &str) -> &str {
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest
            .strip_prefix("json")
            .or_else(|| rest.strip_prefix("JSON"))
            .unwrap_or(rest);
        let rest = rest.trim_start_matches('\n').trim_start();
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else {
        trimmed
    }
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
            reason: Some(format!(
                "{} documents share content_hash",
                g.count
            )),
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
            action: MaintenanceAction::RefreshStaleWiki,
            reason: Some(format!(
                "wiki '{}' older than raw '{}'",
                w.wiki_title, w.raw_title
            )),
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
    llm: &ChatClient,
    max_actions: usize,
) -> Result<MaintenancePlan> {
    let analysis_json = serde_json::to_string(report).map_err(|e| {
        AppError::llm(format!("failed to serialize AnalysisReport for planner: {e}"))
    })?;
    let analysis_json = truncate(&analysis_json, ANALYSIS_JSON_MAX_CHARS);

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
    let unfenced = strip_json_fences(raw.trim());
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
    llm: Option<&ChatClient>,
    opts: &PlanOptions,
) -> Result<MaintenancePlan> {
    let max_actions = opts.max_actions.max(1);
    let use_llm = config.llm_enabled && !opts.force_heuristic && llm.is_some();

    let mut plan = if use_llm {
        let client = llm.expect("use_llm implies Some");
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

/// Append `plan_maintenance` to ops_log.
pub fn log_plan(store: &Store, plan: &MaintenancePlan) -> Result<()> {
    let _ = store.append_ops_log(&OpsLogEntry {
        id: Uuid::new_v4().to_string(),
        seq: 0,
        ts: plan.generated_at,
        op: "plan_maintenance".into(),
        prefix: Some("MAINT".into()),
        message: format!(
            "plan_maintenance: source={:?}, {} actions, {} rejected, {} issues",
            plan.source,
            plan.action_count,
            plan.rejected.len(),
            plan.issue_count
        ),
        entity_id: None,
        entity_kind: Some("maintenance".into()),
        payload_json: serde_json::json!({
            "source": plan.source,
            "action_count": plan.action_count,
            "rejected_count": plan.rejected.len(),
            "issue_count": plan.issue_count,
            "llm_enabled": plan.llm_enabled,
            "model": plan.model,
            "actions": plan.actions.iter().map(|a| a.action.as_str()).collect::<Vec<_>>(),
        })
        .to_string(),
        agent_name: None,
    })?;
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maintain::analyze::{
        AnalysisIssue, ArchiveCandidate, CorpusCounts, DuplicateGroup, EmbedMismatchInfo,
        NearDuplicatePair, SizeStats, StaleWikiPage,
    };
    use crate::models::DoctorReport;

    fn empty_report() -> AnalysisReport {
        AnalysisReport {
            generated_at: Utc::now(),
            db_path: ":memory:".into(),
            db_size_bytes: None,
            counts: CorpusCounts::default(),
            size: SizeStats::default(),
            doctor: DoctorReport {
                schema_version: 1,
                expected_schema_version: 1,
                schema_ok: true,
                fts_ready: true,
                document_count: 0,
                chunk_count: 0,
                node_count: 0,
                edge_count: 0,
                embed_dims: 8,
                manifest_dims: None,
                embed_ok: true,
                ready_for_search: false,
                ingest_roots_configured: false,
                db_path: ":memory:".into(),
                wal_bytes: 0,
                wal_warn_bytes: 256 * 1024 * 1024,
                wal_too_large: false,
                documents_without_chunks: 0,
                orphan_chunks: 0,
                orphan_document_nodes: 0,
                orphan_edges: 0,
                unscoped_documents: 0,
                relational_integrity_ok: true,
                repair_hint: None,
                ok: true,
            },
            fts_ready: true,
            embed_mismatch: None,
            exact_duplicates: vec![],
            near_duplicates: vec![],
            orphan_nodes: vec![],
            orphan_wiki_pages: vec![],
            unresolved_stubs: vec![],
            aging_stubs: vec![],
            stale_wiki: vec![],
            archive_candidates: vec![],
            issues: vec![],
            issue_count: 0,
        }
    }

    #[test]
    fn whitelist_accepts_known_ops() {
        assert!(is_allowed_action("refile"));
        assert!(is_allowed_action("vacuum"));
        assert!(is_allowed_action("merge_exact_dup"));
        assert!(is_allowed_action("consolidate_propose"));
        assert!(is_allowed_action("resolve_stub"));
        assert!(is_allowed_action("rebuild_index"));
        assert!(is_allowed_action("reindex_fts"));
        assert!(!is_allowed_action("drop_table"));
        assert!(!is_allowed_action(""));
        assert!(!is_allowed_action("Refile"));
    }

    #[test]
    fn enum_roundtrip_snake_case() {
        for &a in MaintenanceAction::ALL {
            let s = serde_json::to_string(&a).unwrap();
            assert_eq!(s, format!("\"{}\"", a.as_str()));
            let back: MaintenanceAction = serde_json::from_str(&s).unwrap();
            assert_eq!(back, a);
        }
        assert_eq!(MaintenanceAction::ALL.len(), ALLOWED_ACTIONS.len());
        for (i, name) in ALLOWED_ACTIONS.iter().enumerate() {
            assert_eq!(MaintenanceAction::ALL[i].as_str(), *name);
        }
    }

    #[test]
    fn validate_rejects_unknown() {
        let err = action_from_parts("rm -rf", None, None, serde_json::json!({})).unwrap_err();
        assert!(err.to_string().contains("whitelist"));
    }

    #[test]
    fn validate_plan_all_or_nothing() {
        let plan = vec![
            MaintenancePlanItem {
                action: MaintenanceAction::Vacuum,
                reason: Some("L0".into()),
                target_id: None,
                params: serde_json::json!({}),
            },
            MaintenancePlanItem {
                action: MaintenanceAction::Pin,
                reason: None,
                target_id: None,
                params: serde_json::json!({}),
            },
        ];
        assert!(validate_plan(plan).is_err());
    }

    #[test]
    fn filter_plan_keeps_valid_drops_invalid() {
        let (ok, bad) = filter_raw_plan(vec![
            RawPlanItem {
                action: "vacuum".into(),
                reason: None,
                target_id: None,
                params: serde_json::json!({}),
            },
            RawPlanItem {
                action: "drop_table".into(),
                reason: None,
                target_id: None,
                params: serde_json::json!({}),
            },
        ]);
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].action, MaintenanceAction::Vacuum);
        assert_eq!(bad.len(), 1);
    }

    #[test]
    fn parse_plan_envelope_and_array() {
        let raw = r#"{"actions":[{"action":"reindex_fts","reason":"fts","params":{}}],"notes":"n"}"#;
        let (acts, notes) = parse_plan_json(raw).unwrap();
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0]["action"], "reindex_fts");
        assert_eq!(notes.as_deref(), Some("n"));

        let arr = r#"[{"action":"vacuum","params":{}}]"#;
        let (acts, notes) = parse_plan_json(arr).unwrap();
        assert_eq!(acts[0]["action"], "vacuum");
        assert!(notes.is_none());
    }

    #[test]
    fn parse_plan_fenced_json() {
        let raw = "```json\n{\"actions\":[{\"action\":\"noop\",\"params\":{}}]}\n```";
        let (acts, _) = parse_plan_json(raw).unwrap();
        assert_eq!(acts[0]["action"], "noop");
        let (filtered, rejected, _) = parse_and_filter_plan(raw).unwrap();
        assert_eq!(filtered[0].action, MaintenanceAction::Noop);
        assert!(rejected.is_empty());
    }

    #[test]
    fn heuristic_healthy_is_noop() {
        let plan = heuristic_plan(&empty_report(), 10);
        assert_eq!(plan.source, PlanSource::Heuristic);
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].action, MaintenanceAction::Noop);
    }

    #[test]
    fn heuristic_maps_dups_stale_embed_fts() {
        let mut report = empty_report();
        report.counts.chunks = 5;
        report.fts_ready = false;
        report.embed_mismatch = Some(EmbedMismatchInfo {
            config_dims: 8,
            config_provider: "mock".into(),
            config_model: "mock".into(),
            manifest_dims: Some(16),
            manifest_provider: Some("mock".into()),
            manifest_model: Some("old".into()),
            message: "dims mismatch".into(),
        });
        report.exact_duplicates = vec![DuplicateGroup {
            content_hash: "abc".into(),
            document_ids: vec!["d1".into(), "d2".into()],
            titles: vec!["A".into(), "B".into()],
            count: 2,
        }];
        report.stale_wiki = vec![StaleWikiPage {
            wiki_document_id: "w1".into(),
            wiki_title: "Wiki".into(),
            wiki_updated_at: Utc::now(),
            raw_document_id: "r1".into(),
            raw_title: "Raw".into(),
            raw_updated_at: Utc::now(),
        }];
        report.near_duplicates = vec![NearDuplicatePair {
            document_id_a: "n1".into(),
            document_id_b: "n2".into(),
            title_a: "N1".into(),
            title_b: "N2".into(),
            cosine: 0.99,
        }];
        report.archive_candidates = vec![ArchiveCandidate {
            document_id: "a1".into(),
            title: "Old".into(),
            layer: "raw".into(),
            boost: 0.2,
            updated_at: Utc::now(),
            reason: "low boost".into(),
        }];
        report.issues = vec![AnalysisIssue {
            code: "exact_dup".into(),
            severity: "warn".into(),
            message: "dup".into(),
            entity_id: Some("d1".into()),
        }];
        report.issue_count = 6;

        let plan = heuristic_plan(&report, 30);
        let names: Vec<&str> = plan.actions.iter().map(|a| a.action.as_str()).collect();
        assert!(names.contains(&"reembed_all"), "{names:?}");
        assert!(names.contains(&"reindex_fts"), "{names:?}");
        assert!(names.contains(&"merge_exact_dup"), "{names:?}");
        assert!(names.contains(&"refresh_stale_wiki"), "{names:?}");
        assert!(names.contains(&"merge_near_dup"), "{names:?}");
        assert!(names.contains(&"archive"), "{names:?}");
        assert!(names.contains(&"rebuild_index"), "{names:?}");
        assert!(names.contains(&"vacuum"), "{names:?}");
        assert!(plan.rejected.is_empty(), "{:?}", plan.rejected);
        for a in &plan.actions {
            assert!(is_allowed_action(a.action.as_str()));
        }
    }

    #[test]
    fn heuristic_respects_max_actions() {
        let mut report = empty_report();
        for i in 0..20 {
            report.archive_candidates.push(ArchiveCandidate {
                document_id: format!("a{i}"),
                title: format!("T{i}"),
                layer: "raw".into(),
                boost: 0.1,
                updated_at: Utc::now(),
                reason: "old".into(),
            });
        }
        report.issue_count = 20;
        let plan = heuristic_plan(&report, 5);
        assert!(plan.action_count <= 5);
    }

    #[test]
    fn json_schema_is_object() {
        let s = maintenance_plan_json_schema();
        assert!(s.is_object());
        let s2 = maintenance_actions_json_schema();
        assert!(s2.is_object());
    }

    #[test]
    fn validate_named_variants_from_task() {
        let plan = validate_plan(vec![
            MaintenancePlanItem {
                action: MaintenanceAction::Refile,
                reason: None,
                target_id: Some("d1".into()),
                params: serde_json::json!({"wing": "w"}),
            },
            MaintenancePlanItem {
                action: MaintenanceAction::Archive,
                reason: None,
                target_id: Some("d2".into()),
                params: empty_object(),
            },
            MaintenancePlanItem {
                action: MaintenanceAction::Pin,
                reason: None,
                target_id: Some("d3".into()),
                params: empty_object(),
            },
            MaintenancePlanItem {
                action: MaintenanceAction::MergeExactDup,
                reason: None,
                target_id: Some("keep".into()),
                params: serde_json::json!({"document_ids": ["keep", "drop"]}),
            },
            MaintenancePlanItem {
                action: MaintenanceAction::RebuildIndex,
                reason: None,
                target_id: None,
                params: empty_object(),
            },
            MaintenancePlanItem {
                action: MaintenanceAction::Vacuum,
                reason: None,
                target_id: None,
                params: empty_object(),
            },
            MaintenancePlanItem {
                action: MaintenanceAction::ReindexFts,
                reason: None,
                target_id: None,
                params: empty_object(),
            },
            MaintenancePlanItem {
                action: MaintenanceAction::ConsolidatePropose,
                reason: None,
                target_id: None,
                params: serde_json::json!({"source_ids": ["a", "b"]}),
            },
            MaintenancePlanItem {
                action: MaintenanceAction::ResolveStub,
                reason: None,
                target_id: Some("stub".into()),
                params: serde_json::json!({"target_document_id": "doc"}),
            },
        ])
        .unwrap();
        assert_eq!(plan.len(), 9);
    }

    #[tokio::test]
    async fn plan_maintenance_heuristic_when_llm_disabled() {
        let cfg = Config {
            db_path: std::path::PathBuf::from("./x.duckdb"),
            embedding_provider: crate::config::EmbeddingProviderKind::Mock,
            embedding_base_url: "https://api.openai.com/v1".into(),
            embedding_api_key: String::new(),
            embedding_model: "mock".into(),
            embedding_dims: 8,
            chunk_size: 800,
            chunk_overlap: 120,
            default_top_k: 5,
            ingest_roots: vec![],
            max_context_tokens: 4096,
            max_chunks_per_doc: 3,
            fts_stemmer: "porter".into(),
            default_search_mode: crate::models::SearchMode::Vec,
            llm_base_url: "http://127.0.0.1:11434/v1".into(),
            llm_provider: crate::llm::LlmProviderKind::Ollama,
            llm_model: "llama3.2".into(),
            llm_api_key: "ollama".into(),
            llm_enabled: false,
            llm_timeout_secs: 30,
            llm_max_tokens: 512,
            maint_max_docs: 50,
            maint_near_dup_threshold: 0.92,
            tool_surface: crate::mcp::ToolSurface::Full,
            http_bind: None,
            wiki_require_if_match: false,
        };
        let report = empty_report();
        let opts = PlanOptions {
            max_actions: 10,
            force_heuristic: false,
            log_ops: false,
        };
        let plan = plan_maintenance(&report, &cfg, None, &opts)
            .await
            .unwrap();
        assert_eq!(plan.source, PlanSource::Heuristic);
        assert!(!plan.llm_enabled);
        assert_eq!(plan.actions[0].action, MaintenanceAction::Noop);
    }
}
