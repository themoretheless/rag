//! Metadata actions: refile, pin, boost, status, tags.

use super::*;

pub(super) fn apply_refile(
    store: &Store,
    index: usize,
    name: &str,
    target: Option<&str>,
    params: &RefileParams,
    dry_run: bool,
) -> Result<StepResult> {
    let id = require_target(target, name)?;
    let doc = store
        .documents()
        .get_document(id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {id}")))?;

    let update = params.to_update();

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
        .documents()
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

pub(super) fn apply_pin(
    store: &Store,
    index: usize,
    name: &str,
    target: Option<&str>,
    pinned: bool,
    dry_run: bool,
) -> Result<StepResult> {
    let id = require_target(target, name)?;
    let _doc = store
        .documents()
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
    store.documents().update_document_meta(
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

pub(super) fn apply_set_boost(
    store: &Store,
    index: usize,
    name: &str,
    target: Option<&str>,
    params: &SetBoostParams,
    dry_run: bool,
) -> Result<StepResult> {
    let id = require_target(target, name)?;
    let boost = params
        .boost
        .ok_or_else(|| AppError::invalid("set_boost requires params.boost (finite number > 0)"))?;
    if !boost.is_finite() || boost <= 0.0 {
        return Err(AppError::invalid(format!(
            "boost must be finite and > 0 (got {boost})"
        )));
    }
    let _doc = store
        .documents()
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
    store.documents().update_document_meta(
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

pub(super) fn apply_status(
    store: &Store,
    index: usize,
    name: &str,
    target: Option<&str>,
    status: &str,
    dry_run: bool,
) -> Result<StepResult> {
    let id = require_target(target, name)?;
    let doc = store
        .documents()
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
    store.documents().update_document_meta(
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

pub(super) fn apply_set_tags(
    store: &Store,
    index: usize,
    name: &str,
    target: Option<&str>,
    params: &SetTagsParams,
    dry_run: bool,
) -> Result<StepResult> {
    let id = require_target(target, name)?;
    if params.tags.is_empty() {
        return Err(AppError::invalid(
            "set_tags requires params.tags (array of strings) or params.tag",
        ));
    }
    apply_set_tags_inner(store, index, name, id, &params.tags, &params.mode, dry_run)
}

pub(super) fn apply_set_tags_inner(
    store: &Store,
    index: usize,
    name: &str,
    id: &str,
    tags: &[String],
    mode: &str,
    dry_run: bool,
) -> Result<StepResult> {
    let _doc = store
        .documents()
        .get_document(id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {id}")))?;
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

    let doc_node = match store.graph().find_node_by_document_id(id)? {
        Some(n) => n,
        None => {
            let doc = store
                .documents()
                .get_document(id)?
                .ok_or_else(|| AppError::not_found(format!("document not found: {id}")))?;
            let (node_id, _) = rebuild_document_graph(store, &doc)?;
            store.graph().find_node_by_id(&node_id)?.ok_or_else(|| {
                AppError::db(format!("graph node missing after rebuild: {node_id}"))
            })?
        }
    };

    let labels: Vec<String> = tags
        .iter()
        .map(|tag| tag.trim().trim_start_matches('#').to_string())
        .collect();
    store
        .graph()
        .set_document_tags(&doc_node.id, &labels, replace)?;
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
