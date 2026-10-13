//! Metadata actions: refile, pin, boost, status, tags.

use super::*;

pub(super) fn apply_refile(
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

pub(super) fn apply_set_boost(
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

pub(super) fn apply_set_tags(
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

pub(super) fn apply_set_tags_inner(
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
