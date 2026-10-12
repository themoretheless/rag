//! LLM-backed actions: compile, consolidate, refresh stale pages, file answers.

use super::*;

pub(super) async fn apply_compile_source(
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

pub(super) async fn apply_consolidate(
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
    let kind = param_str(params, "kind")
        .unwrap_or_else(|| crate::models::DocumentKind::Concept.as_str().into());
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

pub(super) async fn apply_refresh_stale(
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

pub(super) async fn apply_file_answer(
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
