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
        Some(store.chunks_generation()?)
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
        let chunks_generation_after = store.chunks_generation()?;
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
                let failure = store.record_fts_finalization_failure(
                    "apply_maintenance_plan",
                    true,
                    &error_message,
                );
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
        MaintenanceAction::Refile => apply_refile(store, index, name, target, params, dry_run),
        MaintenanceAction::Pin => apply_pin(store, index, name, target, true, dry_run),
        MaintenanceAction::Unpin => apply_pin(store, index, name, target, false, dry_run),
        MaintenanceAction::SetBoost => apply_set_boost(store, index, name, target, params, dry_run),
        MaintenanceAction::SetTags => apply_set_tags(store, index, name, target, params, dry_run),
        MaintenanceAction::Archive => apply_status(store, index, name, target, "archived", dry_run),
        MaintenanceAction::Unarchive => apply_status(store, index, name, target, "active", dry_run),
        MaintenanceAction::RebuildIndex => apply_rebuild_index(store, index, name, dry_run),
        MaintenanceAction::RebuildGraph => {
            apply_rebuild_graph(store, index, name, params, config, dry_run)
        }
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
        MaintenanceAction::MergeExactDup => {
            apply_merge_exact_dup(
                store,
                index,
                name,
                target,
                params,
                dry_run,
                context.remaining_docs,
            )
        }
        MaintenanceAction::MergeNearDup => Ok(StepResult {
            outcome: skip_outcome(
                index,
                name,
                target,
                "merge_near_dup not implemented in apply path; use maintain_compress L2 with confirm",
            ),
            docs_delta: 0,
        }),
        MaintenanceAction::DropTombstones => {
            apply_drop_tombstones(store, index, name, params, config, dry_run)
        }
        MaintenanceAction::Vacuum | MaintenanceAction::Checkpoint => {
            apply_vacuum(store, index, name, dry_run)
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
    match store.append_ops_log(&OpsLogEntry {
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

fn apply_refile(
    store: &Store,
    index: usize,
    name: &str,
    target: Option<&str>,
    params: &serde_json::Value,
    dry_run: bool,
) -> Result<StepResult> {
    let id = require_target(target, name)?;
    let doc = store
        .get_document(id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {id}")))?;

    let update = DocumentMetaUpdate {
        wing: param_str(params, "wing").or_else(|| {
            if params.get("wing").and_then(|v| v.as_str()) == Some("") {
                Some(String::new())
            } else {
                None
            }
        }),
        room: param_str(params, "room").or_else(|| {
            if params.get("room").and_then(|v| v.as_str()) == Some("") {
                Some(String::new())
            } else {
                None
            }
        }),
        layer: param_str(params, "layer"),
        kind: param_str(params, "kind"),
        status: param_str(params, "status"),
        source_file: param_str(params, "source_file"),
        title: param_str(params, "title"),
        ..Default::default()
    };

    if update.wing.is_none()
        && update.room.is_none()
        && update.layer.is_none()
        && update.kind.is_none()
        && update.status.is_none()
        && update.source_file.is_none()
        && update.title.is_none()
    {
        return Ok(StepResult {
            outcome: err_outcome(
                index,
                name,
                Some(id),
                "refile requires at least one of: wing, room, layer, kind, status, source_file, title",
            ),
            docs_delta: 0,
        });
    }

    let detail = json!({
        "before": {
            "wing": doc.wing,
            "room": doc.room,
            "layer": doc.layer,
            "kind": doc.kind,
            "status": doc.status,
        },
        "update": update,
    });

    if dry_run {
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                Some(id),
                true,
                "refile document meta",
                Some(detail),
            ),
            docs_delta: 1,
        });
    }

    let applied = store
        .update_document_meta(id, &update)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {id}")))?;
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            Some(id),
            false,
            format!(
                "refiled {} wing={:?} room={:?}",
                applied.document.id, applied.document.wing, applied.document.room
            ),
            Some(detail),
        ),
        docs_delta: 1,
    })
}

fn apply_pin(
    store: &Store,
    index: usize,
    name: &str,
    target: Option<&str>,
    pinned: bool,
    dry_run: bool,
) -> Result<StepResult> {
    let id = require_target(target, name)?;
    let _doc = store
        .get_document(id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {id}")))?;
    if dry_run {
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                Some(id),
                true,
                format!("set pinned={pinned}"),
                Some(json!({"pinned": pinned})),
            ),
            docs_delta: 1,
        });
    }
    store.update_document_meta(
        id,
        &DocumentMetaUpdate {
            pinned: Some(pinned),
            ..Default::default()
        },
    )?;
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            Some(id),
            false,
            format!("pinned={pinned}"),
            Some(json!({"pinned": pinned})),
        ),
        docs_delta: 1,
    })
}

fn apply_set_boost(
    store: &Store,
    index: usize,
    name: &str,
    target: Option<&str>,
    params: &serde_json::Value,
    dry_run: bool,
) -> Result<StepResult> {
    let id = require_target(target, name)?;
    let boost = param_f64(params, "boost")
        .ok_or_else(|| AppError::config("set_boost requires params.boost (finite number > 0)"))?;
    if !boost.is_finite() || boost <= 0.0 {
        return Err(AppError::config(format!(
            "boost must be finite and > 0 (got {boost})"
        )));
    }
    let _doc = store
        .get_document(id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {id}")))?;
    if dry_run {
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                Some(id),
                true,
                format!("set boost={boost}"),
                Some(json!({"boost": boost})),
            ),
            docs_delta: 1,
        });
    }
    store.update_document_meta(
        id,
        &DocumentMetaUpdate {
            boost: Some(boost),
            ..Default::default()
        },
    )?;
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            Some(id),
            false,
            format!("boost={boost}"),
            Some(json!({"boost": boost})),
        ),
        docs_delta: 1,
    })
}

fn apply_status(
    store: &Store,
    index: usize,
    name: &str,
    target: Option<&str>,
    status: &str,
    dry_run: bool,
) -> Result<StepResult> {
    let id = require_target(target, name)?;
    let doc = store
        .get_document(id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {id}")))?;
    if dry_run {
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                Some(id),
                true,
                format!("set status={status} (was {})", doc.status),
                Some(json!({"status": status, "before": doc.status})),
            ),
            docs_delta: 1,
        });
    }
    store.update_document_meta(
        id,
        &DocumentMetaUpdate {
            status: Some(status.into()),
            ..Default::default()
        },
    )?;
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            Some(id),
            false,
            format!("status={status}"),
            Some(json!({"status": status})),
        ),
        docs_delta: 1,
    })
}

fn apply_set_tags(
    store: &Store,
    index: usize,
    name: &str,
    target: Option<&str>,
    params: &serde_json::Value,
    dry_run: bool,
) -> Result<StepResult> {
    let id = require_target(target, name)?;
    let mut tags = param_str_list(params, "tags");
    if tags.is_empty() {
        tags = param_str_list(params, "tag");
    }
    if tags.is_empty() {
        if let Some(t) = param_str(params, "tag") {
            tags.push(t);
        }
    }
    if tags.is_empty() {
        return Err(AppError::config(
            "set_tags requires params.tags (array of strings) or params.tag",
        ));
    }
    apply_set_tags_inner(store, index, name, id, &tags, params, dry_run)
}

fn apply_set_tags_inner(
    store: &Store,
    index: usize,
    name: &str,
    id: &str,
    tags: &[String],
    params: &serde_json::Value,
    dry_run: bool,
) -> Result<StepResult> {
    let _doc = store
        .get_document(id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {id}")))?;
    let mode = param_str(params, "mode").unwrap_or_else(|| "replace".into());
    let replace = mode != "add";

    if dry_run {
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                Some(id),
                true,
                format!("set_tags mode={mode} tags={tags:?}"),
                Some(json!({"tags": tags, "mode": mode})),
            ),
            docs_delta: 1,
        });
    }

    let doc_node = match store.find_node_by_document_id(id)? {
        Some(n) => n,
        None => {
            let doc = store
                .get_document(id)?
                .ok_or_else(|| AppError::not_found(format!("document not found: {id}")))?;
            let (node_id, _) = rebuild_document_graph(store, &doc)?;
            store.find_node_by_id(&node_id)?.ok_or_else(|| {
                AppError::db(format!("graph node missing after rebuild: {node_id}"))
            })?
        }
    };

    let labels: Vec<String> = tags
        .iter()
        .map(|tag| tag.trim().trim_start_matches('#').to_string())
        .collect();
    store.set_document_tags(&doc_node.id, &labels, replace)?;
    let linked = tags;

    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            Some(id),
            false,
            format!("set_tags mode={mode} count={}", linked.len()),
            Some(json!({"tags": linked, "mode": mode})),
        ),
        docs_delta: 1,
    })
}

fn apply_rebuild_index(
    store: &Store,
    index: usize,
    name: &str,
    dry_run: bool,
) -> Result<StepResult> {
    if dry_run {
        let n = store.count_documents_by_layer("wiki")?;
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                None,
                true,
                format!("rebuild wiki_index from {n} wiki docs"),
                Some(json!({"wiki_docs": n})),
            ),
            docs_delta: 0,
        });
    }
    let report = rebuild_wiki_index(store)?;
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            None,
            false,
            format!("rebuilt wiki_index entries={}", report.entry_count),
            Some(json!({"entry_count": report.entry_count})),
        ),
        docs_delta: 0,
    })
}

fn apply_rebuild_graph(
    store: &Store,
    index: usize,
    name: &str,
    params: &serde_json::Value,
    config: &Config,
    dry_run: bool,
) -> Result<StepResult> {
    let dirty_only = param_bool(params, "dirty_only").unwrap_or(true);
    let max_docs = param_f64(params, "max_docs")
        .map(|n| n as usize)
        .unwrap_or(config.maint_max_docs)
        .max(1)
        .min(config.maint_max_docs.max(1));

    if dry_run {
        let candidate_count = store.count_graph_rebuild_candidates(dirty_only)?;
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                None,
                true,
                format!(
                    "rebuild_graph dirty_only={dirty_only} candidates={candidate_count} cap={max_docs}"
                ),
                Some(json!({
                    "dirty_only": dirty_only,
                    "candidate_count": candidate_count,
                    "max_docs": max_docs,
                })),
            ),
            docs_delta: 0,
        });
    }

    let report = rebuild_graph_for_all_or_dirty(store, dirty_only, max_docs)?;
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            None,
            false,
            format!(
                "rebuild_graph processed={} succeeded={} failed={}",
                report.processed, report.succeeded, report.failed
            ),
            Some(serde_json::to_value(&report).unwrap_or(json!({}))),
        ),
        docs_delta: 0,
    })
}

async fn apply_reindex_fts(
    store: &Store,
    index: usize,
    name: &str,
    config: &Config,
    dry_run: bool,
) -> Result<StepResult> {
    if dry_run {
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                None,
                true,
                format!("reindex_fts stemmer={}", config.fts_stemmer),
                Some(json!({"stemmer": config.fts_stemmer})),
            ),
            docs_delta: 0,
        });
    }
    let report =
        reindex_fts_nonblocking(store, &config.fts_stemmer, "apply_maintenance_plan reindex")
            .await?;
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            None,
            false,
            format!(
                "reindex_fts backend={} stemmer={}",
                report.backend, report.stemmer
            ),
            Some(serde_json::to_value(&report).unwrap_or(json!({}))),
        ),
        docs_delta: 0,
    })
}

async fn apply_reembed_one(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    index: usize,
    name: &str,
    target: Option<&str>,
    dry_run: bool,
) -> Result<StepResult> {
    let id = require_target(target, name)?;
    let doc = store
        .get_document(id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {id}")))?;
    store.require_embedding_manifest_match(config)?;
    let chunks = store.list_chunks_for_document(&doc.id)?;
    if dry_run {
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                Some(id),
                true,
                format!("reembed {} chunks", chunks.len()),
                Some(json!({"chunk_count": chunks.len()})),
            ),
            docs_delta: 1,
        });
    }

    if !chunks.is_empty() {
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings = embedder.embed(&texts).await?;
        if embeddings.len() != chunks.len() {
            return Err(AppError::embeddings(format!(
                "embedder returned {} vectors for {} chunks",
                embeddings.len(),
                chunks.len()
            )));
        }
        store.update_chunk_embeddings_atomic(&doc.id, doc.revision, &chunks, &embeddings, None)?;
    }
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            Some(id),
            false,
            format!("reembedded {} chunks", chunks.len()),
            Some(json!({"chunk_count": chunks.len()})),
        ),
        docs_delta: 1,
    })
}

async fn apply_reembed_all(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    index: usize,
    name: &str,
    params: &serde_json::Value,
    dry_run: bool,
) -> Result<StepResult> {
    let max_docs = param_f64(params, "max_docs")
        .map(|n| n as usize)
        .unwrap_or(config.maint_max_docs)
        .max(1)
        .min(config.maint_max_docs.max(1));

    if dry_run {
        let n = store.count_documents()?;
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                None,
                true,
                format!("reembed_all docs={n} cap={max_docs}"),
                Some(json!({"documents": n, "max_docs": max_docs})),
            ),
            docs_delta: 0,
        });
    }

    let report = reembed_all(store, embedder, config, max_docs).await?;
    if reembed_all_incomplete(&report) {
        return Ok(StepResult {
            outcome: ActionOutcome {
                index,
                action: name.into(),
                target_id: None,
                outcome: ActionOutcomeKind::Error,
                message: reembed_all_incomplete_message(&report),
                detail: Some(json!({
                    "code": REEMBED_ALL_INCOMPLETE_CODE,
                    "retryable": true,
                    "report": report,
                })),
                ops_log_id: None,
            },
            docs_delta: 0,
        });
    }
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            None,
            false,
            format!(
                "reembed_all processed={} succeeded={} chunks={}",
                report.documents_processed, report.documents_succeeded, report.chunks_reembedded
            ),
            Some(serde_json::to_value(&report).unwrap_or(json!({}))),
        ),
        docs_delta: 0,
    })
}

async fn apply_compile_source(
    context: &MaintenanceExecutionContext<'_>,
    index: usize,
    name: &str,
    target: Option<&str>,
    params: &serde_json::Value,
) -> Result<StepResult> {
    let store = context.store;
    let embedder = context.embedder;
    let config = context.config;
    let llm = context.llm;
    let dry_run = context.dry_run;
    let agent = context.agent;
    let id = target
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| param_str(params, "source_id_or_uri"))
        .or_else(|| param_str(params, "source_id"))
        .ok_or_else(|| {
            AppError::config("compile_source requires target_id or params.source_id_or_uri")
        })?;

    if dry_run {
        if let Some(client) = llm.filter(|_| config.llm_enabled) {
            let res =
                wiki::compile_source(store, embedder, config, client, &id, true, agent).await?;
            return Ok(StepResult {
                outcome: ok_outcome(
                    index,
                    name,
                    Some(&id),
                    true,
                    format!(
                        "compile_source proposed {} pages",
                        res.proposed.as_ref().map(|p| p.pages.len()).unwrap_or(0)
                    ),
                    Some(serde_json::to_value(&res).unwrap_or(json!({}))),
                ),
                docs_delta: 1,
            });
        }
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                Some(&id),
                true,
                "compile_source (LLM dry_run preview requires ChatClient)",
                Some(json!({"source": id})),
            ),
            docs_delta: 1,
        });
    }

    if !config.llm_enabled {
        return Ok(StepResult {
            outcome: skip_outcome(
                index,
                name,
                Some(&id),
                "compile_source skipped: RAG_LLM_ENABLED=false",
            ),
            docs_delta: 0,
        });
    }
    let client = llm.ok_or_else(|| {
        AppError::llm("compile_source requires ChatClient; check RAG_LLM_BASE_URL / model")
    })?;
    let res = wiki::compile_source(store, embedder, config, client, &id, false, agent).await?;
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            Some(&id),
            false,
            format!("compiled {} pages", res.pages_written),
            Some(serde_json::to_value(&res).unwrap_or(json!({}))),
        ),
        docs_delta: 1,
    })
}

async fn apply_consolidate(
    context: &MaintenanceExecutionContext<'_>,
    index: usize,
    name: &str,
    target: Option<&str>,
    params: &serde_json::Value,
) -> Result<StepResult> {
    let store = context.store;
    let embedder = context.embedder;
    let config = context.config;
    let dry_run = context.dry_run;
    let agent = context.agent;
    // Persist a pre-synthesized page (title+content). Full LLM consolidate is
    // agent/MCP-side; apply only writes when body is present.
    let title = match param_str(params, "title") {
        Some(t) => t,
        None => {
            return Ok(StepResult {
                outcome: skip_outcome(
                    index,
                    name,
                    target,
                    "consolidate requires params.title + params.content (or body); use consolidate MCP tool for LLM synthesis",
                ),
                docs_delta: 0,
            });
        }
    };
    let content = match param_str(params, "content").or_else(|| param_str(params, "body")) {
        Some(c) => c,
        None => {
            return Ok(StepResult {
                outcome: skip_outcome(
                    index,
                    name,
                    target,
                    "consolidate requires params.content or params.body",
                ),
                docs_delta: 0,
            });
        }
    };
    let slug = param_str(params, "slug").unwrap_or_else(|| title.clone());
    let kind = param_str(params, "kind").unwrap_or_else(|| "concept".into());
    let category = param_str(params, "category");
    let summary = param_str(params, "summary");

    if dry_run {
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                target,
                true,
                format!("write consolidated wiki page slug={slug}"),
                Some(json!({
                    "slug": slug,
                    "title": title,
                    "kind": kind,
                    "content_chars": content.len(),
                })),
            ),
            docs_delta: 1,
        });
    }

    let wr = wiki::write_wiki_page(
        store,
        embedder,
        config,
        &slug,
        &title,
        &content,
        &kind,
        category.as_deref(),
        summary.as_deref(),
        agent,
    )
    .await?;
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            Some(&wr.document_id),
            false,
            format!("consolidated wiki page {}", wr.document_id),
            Some(serde_json::to_value(&wr).unwrap_or(json!({}))),
        ),
        docs_delta: 1,
    })
}

async fn apply_refresh_stale(
    context: &MaintenanceExecutionContext<'_>,
    index: usize,
    name: &str,
    params: &serde_json::Value,
) -> Result<StepResult> {
    let store = context.store;
    let embedder = context.embedder;
    let config = context.config;
    let llm = context.llm;
    let dry_run = context.dry_run;
    let agent = context.agent;
    let max_docs = param_f64(params, "max_docs").map(|n| n as usize);
    // A nested action may opt into preview inside an applying plan, but it may
    // never override the plan-wide safety boundary in the mutating direction.
    let nested_dry = dry_run || param_bool(params, "dry_run").unwrap_or(false);
    let llm_ref = if !nested_dry && config.llm_enabled {
        llm
    } else {
        None
    };
    let res = wiki::refresh_stale_wiki(
        store, embedder, config, llm_ref, nested_dry, max_docs, agent,
    )
    .await?;
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            None,
            nested_dry,
            format!(
                "refresh_stale_wiki stale={} applied_nested={}",
                res.stale_count, !nested_dry
            ),
            Some(serde_json::to_value(&res).unwrap_or(json!({}))),
        ),
        docs_delta: 0,
    })
}

async fn apply_file_answer(
    context: &MaintenanceExecutionContext<'_>,
    index: usize,
    name: &str,
    params: &serde_json::Value,
) -> Result<StepResult> {
    let store = context.store;
    let embedder = context.embedder;
    let config = context.config;
    let dry_run = context.dry_run;
    let agent = context.agent;
    let title = param_str(params, "title")
        .ok_or_else(|| AppError::config("file_answer requires params.title"))?;
    let body = param_str(params, "body")
        .or_else(|| param_str(params, "content"))
        .ok_or_else(|| AppError::config("file_answer requires params.body or params.content"))?;
    let slug = param_str(params, "slug");

    let citations: Option<Vec<FileAnswerCitation>> = params
        .get("citations")
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    if dry_run {
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                None,
                true,
                format!("file_answer title={title}"),
                Some(json!({
                    "title": title,
                    "body_chars": body.len(),
                    "slug": slug,
                    "citation_count": citations.as_ref().map(|c| c.len()).unwrap_or(0),
                })),
            ),
            docs_delta: 1,
        });
    }

    let wr = wiki::file_answer(
        store,
        embedder,
        config,
        &title,
        &body,
        slug.as_deref(),
        citations,
        agent,
    )
    .await?;
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            Some(&wr.document_id),
            false,
            format!("filed answer {}", wr.document_id),
            Some(serde_json::to_value(&wr).unwrap_or(json!({}))),
        ),
        docs_delta: 1,
    })
}

/// Keep `target_id` (or params.keep_id); archive/tombstone other same-hash docs.
fn apply_merge_exact_dup(
    store: &Store,
    index: usize,
    name: &str,
    target: Option<&str>,
    params: &serde_json::Value,
    dry_run: bool,
    remaining_docs: usize,
) -> Result<StepResult> {
    let keep_id = target
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| param_str(params, "keep_id"))
        .ok_or_else(|| {
            AppError::config("merge_exact_dup requires target_id or params.keep_id (canonical)")
        })?;

    let keep = store
        .get_document(&keep_id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {keep_id}")))?;
    let expected_hash = crate::util::content_hash(&keep.content);
    let declared_hash = param_str(params, "content_hash");
    if declared_hash
        .as_deref()
        .is_some_and(|declared| declared != expected_hash)
    {
        return Err(AppError::conflict(format!(
            "merge_exact_dup declared content_hash does not match canonical document {keep_id}"
        )));
    }

    let mut source_ids = param_str_list(params, "source_ids");
    if source_ids.is_empty() {
        source_ids = param_str_list(params, "document_ids");
    }
    if let Some(b) = param_str(params, "document_id_b")
        .or_else(|| param_str(params, "drop_id"))
        .or_else(|| param_str(params, "source_id"))
    {
        if !source_ids.contains(&b) {
            source_ids.push(b);
        }
    }
    let mut seen = std::collections::HashSet::new();
    source_ids = source_ids
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty() && id != &keep_id && seen.insert(id.clone()))
        .collect();
    // If document_ids included keep, drop it; if only keep remains empty, expand via hash.
    let requested_sources = if source_ids.is_empty() {
        let (candidate_count, candidates) =
            store.exact_duplicate_source_ids(&expected_hash, &keep_id, remaining_docs)?;
        source_ids = candidates;
        candidate_count
    } else {
        source_ids.len()
    };

    if source_ids.is_empty() {
        return Ok(StepResult {
            outcome: skip_outcome(index, name, Some(&keep_id), "no duplicate sources to merge"),
            docs_delta: 0,
        });
    }

    source_ids.truncate(remaining_docs);
    let skipped_cap = requested_sources.saturating_sub(source_ids.len());
    if source_ids.is_empty() {
        return Ok(StepResult {
            outcome: skip_outcome(
                index,
                name,
                Some(&keep_id),
                "no remaining document budget for duplicate sources",
            ),
            docs_delta: 0,
        });
    }

    let allow_raw_delete = param_bool(params, "allow_raw_delete").unwrap_or(false);
    let hard_delete = param_bool(params, "delete").unwrap_or(false);

    if dry_run {
        // Dry-run uses the same fail-closed body proof without invoking the
        // mutating transaction.
        for source_id in &source_ids {
            let source = store
                .get_document(source_id)?
                .ok_or_else(|| AppError::not_found(format!("document not found: {source_id}")))?;
            if crate::util::content_hash(&source.content) != expected_hash {
                return Err(AppError::conflict(format!(
                    "merge_exact_dup source {source_id} is not an exact content duplicate of {keep_id}"
                )));
            }
        }
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                Some(&keep_id),
                true,
                format!(
                    "merge_exact_dup keep={keep_id} sources={} hard_delete={hard_delete}",
                    source_ids.len()
                ),
                Some(json!({
                    "keep_id": keep_id,
                    "source_ids": source_ids,
                    "skipped_cap": skipped_cap,
                    "hard_delete": hard_delete,
                    "allow_raw_delete": allow_raw_delete,
                })),
            ),
            docs_delta: source_ids.len(),
        });
    }

    let merge = store.merge_exact_duplicates_atomic(
        &keep_id,
        &source_ids,
        declared_hash.as_deref(),
        hard_delete,
        allow_raw_delete,
    )?;
    let blocked = merge
        .raw_protected
        .iter()
        .map(|id| {
            json!({
                "id": id,
                "note": "raw not hard-deleted (allow_raw_delete=false); archived instead",
            })
        })
        .collect::<Vec<_>>();
    let merged = merge.merged;

    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            Some(&keep_id),
            false,
            format!("merged {} duplicate(s) into {keep_id}", merged.len()),
            Some(json!({
                "keep_id": keep_id,
                "merged": merged,
                "blocked": blocked,
                "skipped_cap": skipped_cap,
                "hard_delete": hard_delete,
            })),
        ),
        docs_delta: merged.len(),
    })
}

fn apply_drop_tombstones(
    store: &Store,
    index: usize,
    name: &str,
    params: &serde_json::Value,
    config: &Config,
    dry_run: bool,
) -> Result<StepResult> {
    let allow_raw_delete = param_bool(params, "allow_raw_delete").unwrap_or(false);
    let max_docs = param_f64(params, "max_docs")
        .map(|n| n as usize)
        .unwrap_or(config.maint_max_docs)
        .max(1)
        .min(config.maint_max_docs.max(1));

    let candidates = crate::db::analytics::tombstone_candidates(store, max_docs)?;

    if candidates.is_empty() {
        return Ok(StepResult {
            outcome: skip_outcome(index, name, None, "no tombstone documents"),
            docs_delta: 0,
        });
    }

    if dry_run {
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                None,
                true,
                format!("drop_tombstones count={}", candidates.len()),
                Some(json!({
                    "candidates": candidates.iter().map(|(id, layer)| json!({"id": id, "layer": layer})).collect::<Vec<_>>(),
                    "allow_raw_delete": allow_raw_delete,
                })),
            ),
            docs_delta: 0,
        });
    }

    let delete_ids = candidates
        .iter()
        .filter(|(_, layer)| allow_raw_delete || !layer.eq_ignore_ascii_case("raw"))
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    let skipped_raw = candidates.len().saturating_sub(delete_ids.len());
    let deleted = if delete_ids.is_empty() {
        0
    } else {
        let (deleted, _, _) = store.apply_document_dispositions_atomic(&delete_ids, &[])?;
        usize::try_from(deleted).unwrap_or(usize::MAX)
    };

    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            None,
            false,
            format!("drop_tombstones deleted={deleted} skipped_raw={skipped_raw}"),
            Some(json!({
                "deleted": deleted,
                "skipped_raw": skipped_raw,
                "candidates": candidates.len(),
                "max_docs": max_docs,
            })),
        ),
        docs_delta: 0,
    })
}

fn apply_vacuum(store: &Store, index: usize, name: &str, dry_run: bool) -> Result<StepResult> {
    if dry_run {
        let bytes = store.db_file_size_bytes();
        return Ok(StepResult {
            outcome: ok_outcome(
                index,
                name,
                None,
                true,
                "DuckDB CHECKPOINT",
                Some(json!({"bytes_before": bytes})),
            ),
            docs_delta: 0,
        });
    }
    let report = store.vacuum_store()?;
    Ok(StepResult {
        outcome: ok_outcome(
            index,
            name,
            None,
            false,
            "checkpoint complete",
            Some(serde_json::to_value(&report).unwrap_or(json!({}))),
        ),
        docs_delta: 0,
    })
}

#[cfg(test)]
mod tests;
