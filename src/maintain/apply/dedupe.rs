//! Destructive cleanup: exact-duplicate merge, tombstone drop, vacuum.

use super::*;

/// Keep `target_id` (or params.keep_id); archive/tombstone other same-hash docs.
pub(super) fn apply_merge_exact_dup(
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

pub(super) fn apply_drop_tombstones(
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
        .filter(|(_, layer)| allow_raw_delete || !crate::models::Layer::Raw.is(layer))
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

pub(super) fn apply_vacuum(
    store: &Store,
    index: usize,
    name: &str,
    dry_run: bool,
) -> Result<StepResult> {
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
