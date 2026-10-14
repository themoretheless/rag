//! Execute a validated maintenance plan (`apply_maintenance_plan`).
//!
//! - Whitelist-only actions ([`MaintenanceAction`] enum).
//! - `dry_run` defaults to **true** (preview without mutation).
//! - Document-scoped ops are capped by `RAG_MAINT_MAX_DOCS` / `max_docs`.
//! - Every action (and the plan summary) is written to `ops_log`.
//! - Never silently deletes `layer=raw` (needs `allow_raw_delete` in params).

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::plan::{validate_action, MaintenanceAction, MaintenancePlanItem};
use super::refresh::{
    ensure_fts_nonblocking, rebuild_graph_for_all_or_dirty, rebuild_wiki_index, reembed_all,
    reembed_all_incomplete, reembed_all_incomplete_message, reindex_fts_nonblocking,
    REEMBED_ALL_INCOMPLETE_CODE,
};
use crate::config::Config;
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::error::{AppError, Result};
use crate::graph::rebuild_document_graph;
use crate::models::{DocumentMetaUpdate, OpsLogEntry};
use crate::util::blocking_section;
use crate::wiki::{self, FileAnswerCitation};

/// Options for [`apply_maintenance_plan`].
#[derive(Debug, Clone)]
pub struct ApplyPlanOptions {
    /// When true (default), preview without mutating store (except ops_log).
    pub dry_run: bool,
    /// Cap on document-scoped mutations (default: `config.maint_max_docs`).
    pub max_docs: Option<usize>,
    /// Optional agent name for ops_log rows.
    pub agent: Option<String>,
}

impl Default for ApplyPlanOptions {
    fn default() -> Self {
        Self {
            dry_run: true,
            max_docs: None,
            agent: None,
        }
    }
}

/// Outcome status for one planned step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcomeKind {
    /// Mutation applied (or dry_run would apply).
    Applied,
    /// Intentionally not run (cap, missing support, noop conditions).
    Skipped,
    /// Failed while validating/executing this step.
    Error,
}

/// One step result inside an apply report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionOutcome {
    pub index: usize,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    pub outcome: ActionOutcomeKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops_log_id: Option<String>,
}

/// Aggregate report for `apply_maintenance_plan`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyMaintenancePlanReport {
    pub dry_run: bool,
    pub max_docs: usize,
    /// Document-scoped touches counted against the cap.
    pub docs_touched: usize,
    pub applied: Vec<ActionOutcome>,
    pub skipped: Vec<ActionOutcome>,
    pub errors: Vec<ActionOutcome>,
    pub total_actions: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops_log_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops_log_seq: Option<i64>,
}

/// Apply (or dry-run) a list of whitelist maintenance plan items.
///
/// Invalid items become `errors` and are not executed. Document-scoped actions
/// stop once `max_docs` is reached (remaining are `skipped`).
pub async fn apply_maintenance_plan(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    llm: Option<&dyn crate::llm::ChatCompleter>,
    actions: Vec<MaintenancePlanItem>,
    opts: &ApplyPlanOptions,
) -> Result<ApplyMaintenancePlanReport> {
    let dry_run = opts.dry_run;
    let max_docs = opts
        .max_docs
        .unwrap_or(config.maint_max_docs)
        .max(1)
        .min(config.maint_max_docs.max(1));
    let agent = opts.agent.as_deref();

    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    let mut errors = Vec::new();
    let mut docs_touched = 0usize;
    let total_actions = actions.len();
    let chunks_generation_before = if dry_run {
        None
    } else {
        Some(blocking_section(|| store.chunks_generation())?)
    };

    for (index, raw) in actions.into_iter().enumerate() {
        let action_name = raw.action.as_str().to_string();
        let target_preview = raw.target_id.clone();

        let action = match validate_action(raw) {
            Ok(a) => a,
            Err(e) => {
                let outcome = ActionOutcome {
                    index,
                    action: action_name,
                    target_id: target_preview,
                    outcome: ActionOutcomeKind::Error,
                    message: e.to_string(),
                    detail: None,
                    ops_log_id: None,
                };
                let logged = log_action(store, &outcome, dry_run, agent);
                let mut outcome = outcome;
                outcome.ops_log_id = logged;
                errors.push(outcome);
                continue;
            }
        };

        if is_doc_scoped(action.action) && docs_touched >= max_docs {
            let outcome = ActionOutcome {
                index,
                action: action.action.as_str().into(),
                target_id: action.target_id.clone(),
                outcome: ActionOutcomeKind::Skipped,
                message: format!(
                    "skipped: docs_touched cap reached ({docs_touched}>={max_docs} RAG_MAINT_MAX_DOCS)"
                ),
                detail: Some(json!({"docs_touched": docs_touched, "max_docs": max_docs})),
                ops_log_id: None,
            };
            let logged = log_action(store, &outcome, dry_run, agent);
            let mut outcome = outcome;
            outcome.ops_log_id = logged;
            skipped.push(outcome);
            continue;
        }

        let context = MaintenanceExecutionContext {
            store,
            embedder,
            config,
            llm,
            dry_run,
            agent,
            remaining_docs: max_docs.saturating_sub(docs_touched),
        };
        let step = execute_one(&context, &action, index).await;

        let mut outcome = step.outcome;
        let docs_delta = step.docs_delta;
        let logged = log_action(store, &outcome, dry_run, agent);
        outcome.ops_log_id = logged;

        match outcome.outcome {
            ActionOutcomeKind::Applied => {
                if docs_delta > 0 {
                    docs_touched = docs_touched.saturating_add(docs_delta);
                }
                applied.push(outcome);
            }
            ActionOutcomeKind::Skipped => skipped.push(outcome),
            ActionOutcomeKind::Error => errors.push(outcome),
        }
    }

    if let Some(chunks_generation_before) = chunks_generation_before {
        let chunks_generation_after = blocking_section(|| store.chunks_generation())?;
        if chunks_generation_after != chunks_generation_before {
            // A plan may combine reindex with later delete/re-embed/write
            // actions. Reconcile once at the plan boundary, independent of
            // action ordering, before returning the aggregate result.
            if let Err(error) = finalize_fts_after_plan(store, &config.fts_stemmer).await {
                // Durable mutations have already committed. Preserve their
                // aggregate outcomes and make the derived-index failure an
                // additive result that the next lexical read can retry.
                let error_message = error.to_string();
                // Same safety record as every other corpus workflow: dirty
                // marker for the next lexical read plus the canonical
                // `fts_finalization_failed` audit row.
                let failure = blocking_section(|| {
                    store.record_fts_finalization_failure(
                        "apply_maintenance_plan",
                        true,
                        &error_message,
                    )
                });
                let mut outcome = ActionOutcome {
                    index: total_actions,
                    action: "finalize_fts".into(),
                    target_id: None,
                    outcome: ActionOutcomeKind::Error,
                    message: format!(
                        "durable corpus mutation committed, but final FTS refresh failed: {error_message}"
                    ),
                    detail: Some(json!({
                        "code": "FTS_FINALIZATION_FAILED",
                        "stage": "finalize_fts",
                        "durable_mutation_committed": true,
                        "retryable": true,
                        "fallback_dirty_marked": failure.dirty_marker_written,
                        "dirty_marker_error": failure.dirty_marker_error,
                        "ops_log_error": failure.ops_log_error,
                        "error": error_message,
                    })),
                    ops_log_id: None,
                };
                outcome.ops_log_id = log_action(store, &outcome, false, agent);
                errors.push(outcome);
            }
        }
    }

    let mut report = ApplyMaintenancePlanReport {
        dry_run,
        max_docs,
        docs_touched,
        applied,
        skipped,
        errors,
        total_actions,
        ops_log_id: None,
        ops_log_seq: None,
    };

    let op = if dry_run {
        "apply_maintenance_plan_dry_run"
    } else {
        "apply_maintenance_plan"
    };
    let message = format!(
        "{op}: applied={} skipped={} errors={} docs_touched={docs_touched}/{max_docs}",
        report.applied.len(),
        report.skipped.len(),
        report.errors.len(),
    );
    let written = super::audit::record_run(
        store,
        "apply_maintenance_plan",
        dry_run,
        "maintenance",
        message,
        agent.map(str::to_string),
        &report,
    )?;
    report.ops_log_id = Some(written.id);
    report.ops_log_seq = Some(written.seq);

    Ok(report)
}

async fn finalize_fts_after_plan(store: &Store, stemmer: &str) -> Result<()> {
    #[cfg(test)]
    crate::db::fts::test_fault::check("apply", stemmer)?;

    ensure_fts_nonblocking(store, stemmer, "apply_maintenance_plan final refresh").await
}

struct StepResult {
    outcome: ActionOutcome,
    docs_delta: usize,
}

struct MaintenanceExecutionContext<'a> {
    store: &'a Store,
    embedder: &'a Arc<dyn EmbeddingProvider>,
    config: &'a Config,
    llm: Option<&'a dyn crate::llm::ChatCompleter>,
    dry_run: bool,
    agent: Option<&'a str>,
    remaining_docs: usize,
}

async fn execute_one(
    context: &MaintenanceExecutionContext<'_>,
    action: &MaintenancePlanItem,
    index: usize,
) -> StepResult {
    let MaintenanceExecutionContext {
        store,
        embedder,
        config,
        dry_run,
        ..
    } = context;
    let dry_run = *dry_run;
    let name = action.action.as_str();
    let target = action.target_id.as_deref();
    let params = &action.params;

    let result = match action.action {
        MaintenanceAction::Noop => Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                target,
                dry_run,
                "noop",
                Some(json!({"reason": action.reason})),
            ),
            docs_delta: 0,
        }),
        MaintenanceAction::Refile => {
            blocking_section(|| apply_refile(store, index, name, target, params, dry_run))
        }
        MaintenanceAction::Pin => {
            blocking_section(|| apply_pin(store, index, name, target, true, dry_run))
        }
        MaintenanceAction::Unpin => {
            blocking_section(|| apply_pin(store, index, name, target, false, dry_run))
        }
        MaintenanceAction::SetBoost => {
            blocking_section(|| apply_set_boost(store, index, name, target, params, dry_run))
        }
        MaintenanceAction::SetTags => {
            blocking_section(|| apply_set_tags(store, index, name, target, params, dry_run))
        }
        MaintenanceAction::Archive => {
            blocking_section(|| apply_status(store, index, name, target, "archived", dry_run))
        }
        MaintenanceAction::Unarchive => {
            blocking_section(|| apply_status(store, index, name, target, "active", dry_run))
        }
        MaintenanceAction::RebuildIndex => {
            blocking_section(|| apply_rebuild_index(store, index, name, dry_run))
        }
        MaintenanceAction::RebuildGraph => blocking_section(|| {
            apply_rebuild_graph(store, index, name, params, config, dry_run)
        }),
        MaintenanceAction::ReindexFts => {
            apply_reindex_fts(store, index, name, config, dry_run).await
        }
        MaintenanceAction::Reembed => {
            apply_reembed_one(store, embedder, config, index, name, target, dry_run).await
        }
        MaintenanceAction::ReembedAll => {
            apply_reembed_all(store, embedder, config, index, name, params, dry_run).await
        }
        MaintenanceAction::CompileSource => {
            apply_compile_source(context, index, name, target, params).await
        }
        MaintenanceAction::ConsolidatePropose => Ok(StepResult {
            outcome: skip_outcome(
                index,
                name,
                target,
                "consolidate_propose is agent/LLM-side; call consolidate MCP tool or pass pre-synthesized title+content via consolidate",
            ),
            docs_delta: 0,
        }),
        MaintenanceAction::Consolidate => {
            apply_consolidate(context, index, name, target, params).await
        }
        MaintenanceAction::RefreshStaleWiki => {
            apply_refresh_stale(context, index, name, params).await
        }
        MaintenanceAction::FileAnswer => {
            apply_file_answer(context, index, name, params).await
        }
        MaintenanceAction::ResolveStub
        | MaintenanceAction::LintAndFixLinks
        | MaintenanceAction::BuildOutline => Ok(StepResult {
            outcome: skip_outcome(
                index,
                name,
                target,
                format!("{name} not implemented in apply path yet"),
            ),
            docs_delta: 0,
        }),
        MaintenanceAction::MergeExactDup => blocking_section(|| {
            apply_merge_exact_dup(
                store,
                index,
                name,
                target,
                params,
                dry_run,
                context.remaining_docs,
            )
        }),
        MaintenanceAction::MergeNearDup => Ok(StepResult {
            outcome: skip_outcome(
                index,
                name,
                target,
                "merge_near_dup not implemented in apply path; use maintain_compress L2 with confirm",
            ),
            docs_delta: 0,
        }),
        MaintenanceAction::DropTombstones => blocking_section(|| {
            apply_drop_tombstones(store, index, name, params, config, dry_run)
        }),
        MaintenanceAction::Vacuum | MaintenanceAction::Checkpoint => {
            blocking_section(|| apply_vacuum(store, index, name, dry_run))
        }
    };

    match result {
        Ok(s) => s,
        Err(e) => StepResult {
            outcome: err_outcome(index, name, target, e.to_string()),
            docs_delta: 0,
        },
    }
}

// --- helpers: outcomes ---

fn ok_outcome(
    index: usize,
    action: &str,
    target: Option<&str>,
    dry_run: bool,
    message: impl Into<String>,
    detail: Option<serde_json::Value>,
) -> ActionOutcome {
    let msg = if dry_run {
        format!("would_apply: {}", message.into())
    } else {
        message.into()
    };
    ActionOutcome {
        index,
        action: action.into(),
        target_id: target.map(|s| s.to_string()),
        outcome: ActionOutcomeKind::Applied,
        message: msg,
        detail,
        ops_log_id: None,
    }
}

fn skip_outcome(
    index: usize,
    action: &str,
    target: Option<&str>,
    message: impl Into<String>,
) -> ActionOutcome {
    ActionOutcome {
        index,
        action: action.into(),
        target_id: target.map(|s| s.to_string()),
        outcome: ActionOutcomeKind::Skipped,
        message: message.into(),
        detail: None,
        ops_log_id: None,
    }
}

fn err_outcome(
    index: usize,
    action: &str,
    target: Option<&str>,
    message: impl Into<String>,
) -> ActionOutcome {
    ActionOutcome {
        index,
        action: action.into(),
        target_id: target.map(|s| s.to_string()),
        outcome: ActionOutcomeKind::Error,
        message: message.into(),
        detail: None,
        ops_log_id: None,
    }
}

fn require_target<'a>(target: Option<&'a str>, action: &str) -> Result<&'a str> {
    target
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::config(format!("{action} requires target_id")))
}

fn is_doc_scoped(action: MaintenanceAction) -> bool {
    matches!(
        action,
        MaintenanceAction::Refile
            | MaintenanceAction::Pin
            | MaintenanceAction::Unpin
            | MaintenanceAction::SetBoost
            | MaintenanceAction::SetTags
            | MaintenanceAction::Archive
            | MaintenanceAction::Unarchive
            | MaintenanceAction::Reembed
            | MaintenanceAction::CompileSource
            | MaintenanceAction::Consolidate
            | MaintenanceAction::FileAnswer
            | MaintenanceAction::MergeExactDup
            | MaintenanceAction::MergeNearDup
            | MaintenanceAction::ResolveStub
    )
}

fn log_action(
    store: &Store,
    outcome: &ActionOutcome,
    dry_run: bool,
    agent: Option<&str>,
) -> Option<String> {
    let op = format!(
        "maintain_{}",
        match outcome.outcome {
            ActionOutcomeKind::Applied if dry_run => "would_apply",
            ActionOutcomeKind::Applied => "apply",
            ActionOutcomeKind::Skipped => "skip",
            ActionOutcomeKind::Error => "error",
        }
    );
    let payload = serde_json::to_string(outcome).unwrap_or_else(|_| "{}".into());
    // One autocommit per plan step, called from the async plan loop.
    match blocking_section(|| {
        store.append_ops_log(&OpsLogEntry {
            id: Uuid::new_v4().to_string(),
            seq: 0,
            ts: Utc::now(),
            op,
            prefix: Some("MAINT".into()),
            message: format!(
                "{} [{}] {}",
                outcome.action,
                match outcome.outcome {
                    ActionOutcomeKind::Applied => {
                        if dry_run {
                            "dry_run"
                        } else {
                            "applied"
                        }
                    }
                    ActionOutcomeKind::Skipped => "skipped",
                    ActionOutcomeKind::Error => "error",
                },
                outcome.message
            ),
            entity_id: outcome.target_id.clone(),
            entity_kind: Some("maintenance_action".into()),
            payload_json: payload,
            agent_name: agent.map(|s| s.to_string()),
        })
    }) {
        Ok(e) => Some(e.id),
        Err(e) => {
            tracing::warn!(error = %e, "failed to append ops_log for maintenance action");
            None
        }
    }
}

// --- param helpers ---

fn param_str(params: &serde_json::Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn param_f64(params: &serde_json::Value, key: &str) -> Option<f64> {
    params.get(key).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_i64().map(|i| i as f64))
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

fn param_bool(params: &serde_json::Value, key: &str) -> Option<bool> {
    params.get(key).and_then(|v| {
        v.as_bool().or_else(|| match v.as_str() {
            Some("true") | Some("1") | Some("yes") => Some(true),
            Some("false") | Some("0") | Some("no") => Some(false),
            _ => None,
        })
    })
}

fn param_str_list(params: &serde_json::Value, key: &str) -> Vec<String> {
    let Some(v) = params.get(key) else {
        return Vec::new();
    };
    if let Some(arr) = v.as_array() {
        return arr
            .iter()
            .filter_map(|x| x.as_str().map(str::trim).filter(|s| !s.is_empty()))
            .map(|s| s.to_string())
            .collect();
    }
    if let Some(s) = v.as_str() {
        return s
            .split(|c: char| c == ',' || c.is_whitespace())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .collect();
    }
    Vec::new()
}

// --- action executors ---

mod dedupe;
mod index;
mod llm;
mod metadata;

use self::{dedupe::*, index::*, llm::*, metadata::*};

#[cfg(test)]
mod tests;
