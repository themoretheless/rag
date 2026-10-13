//! Derived-state actions: wiki index, graph, FTS and re-embedding.

use super::*;

pub(super) fn apply_rebuild_index(
    store: &Store,
    index: usize,
    name: &str,
    dry_run: bool,
) -> Result<StepResult> {
    if dry_run {
        let n = store.count_documents_by_layer(crate::models::Layer::Wiki.as_str())?;
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

pub(super) fn apply_rebuild_graph(
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

pub(super) async fn apply_reindex_fts(
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

pub(super) async fn apply_reembed_one(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    index: usize,
    name: &str,
    target: Option<&str>,
    dry_run: bool,
) -> Result<StepResult> {
    let id = require_target(target, name)?;
    let (doc, chunks) = crate::util::blocking_section(|| {
        let doc = store
            .get_document(id)?
            .ok_or_else(|| AppError::not_found(format!("document not found: {id}")))?;
        store.require_embedding_manifest_match(config)?;
        let chunks = store.list_chunks_for_document(&doc.id)?;
        Ok::<_, AppError>((doc, chunks))
    })?;
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
        let embeddings = crate::embeddings::embed_exact(embedder.as_ref(), &texts).await?;
        crate::util::blocking_section(|| {
            store.update_chunk_embeddings_atomic(&doc.id, doc.revision, &chunks, &embeddings, None)
        })?;
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

pub(super) async fn apply_reembed_all(
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
        let n = crate::util::blocking_section(|| store.count_documents())?;
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
