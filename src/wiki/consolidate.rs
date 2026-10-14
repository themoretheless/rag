//! LLM-assisted consolidation of several sources into one page.

use super::*;
use crate::llm::{parse_consolidate_json, system_user, ChatCompleter};
use crate::util::excerpt_chars as truncate;

/// Optional overrides when consolidating (title/slug/kind/category).
#[derive(Debug, Clone, Default)]
pub struct ConsolidateOpts {
    pub slug: Option<String>,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub category: Option<String>,
    /// Override `RAG_MAINT_MAX_DOCS` for this call (still clamped to config max).
    pub max_docs: Option<usize>,
}

/// Inputs for [`consolidate`].
#[derive(Debug, Clone)]
pub struct ConsolidateRequest<'a> {
    /// Source document ids or uris.
    pub document_ids: &'a [String],
    /// Write the wiki page (`true`) or only return the proposal (`false`).
    pub apply: bool,
    pub opts: ConsolidateOpts,
    pub agent: Option<&'a str>,
}

/// LLM consolidates N document texts into one wiki page proposal.
///
/// - `apply=false` (default for safety): return proposal only (agent may edit).
/// - `apply=true`: write wiki page (chunk+embed+graph+index) + `ops_log`, link
///   each source document node → wiki node with `related`.
///
/// Raw sources stay immutable; only the compiled wiki layer is written.
pub async fn consolidate(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    llm: &dyn ChatCompleter,
    request: ConsolidateRequest<'_>,
) -> Result<ConsolidateResult> {
    let ConsolidateRequest {
        document_ids,
        apply,
        opts,
        agent,
    } = request;
    if document_ids.is_empty() {
        return Err(AppError::config(
            "consolidate requires at least one document id or uri",
        ));
    }

    let max_docs = opts
        .max_docs
        .unwrap_or(config.maint_max_docs)
        .max(1)
        .min(config.maint_max_docs.max(1));

    let mut resolved: Vec<Document> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for key in document_ids {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let doc = crate::util::blocking_section(|| resolve_doc(store, key))?;
        if seen.insert(doc.id.clone()) {
            resolved.push(doc);
        }
        if resolved.len() >= max_docs {
            break;
        }
    }
    if resolved.is_empty() {
        return Err(AppError::config(
            "consolidate: no valid document ids/uris after filtering empties",
        ));
    }
    let capped = document_ids.iter().filter(|s| !s.trim().is_empty()).count() > resolved.len()
        || document_ids.len() > max_docs && resolved.len() >= max_docs;

    let schema = crate::util::blocking_section(|| schema_text(store))?;
    let sources: Vec<(String, String, String)> = resolved
        .iter()
        .map(|d| (d.title.clone(), d.uri.clone(), d.content.clone()))
        .collect();
    let revisions = if apply {
        crate::util::blocking_section(|| wiki_revision_snapshot(store))?
    } else {
        HashMap::new()
    };
    let mut proposed = consolidate_wiki(llm, &schema, &sources).await?;

    // Caller overrides (whitelist fields only).
    if let Some(s) = opts
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        proposed.slug = s.to_string();
    }
    if let Some(t) = opts
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        proposed.title = t.to_string();
    }
    if let Some(k) = opts
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        proposed.kind = k.to_string();
    }
    if let Some(c) = opts
        .category
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        proposed.category = Some(c.to_string());
    }

    let source_ids: Vec<String> = resolved.iter().map(|d| d.id.clone()).collect();
    let source_uris: Vec<String> = resolved.iter().map(|d| d.uri.clone()).collect();
    let source_count = resolved.len();

    if !apply {
        crate::util::blocking_section(|| {
            store.append_ops_log(&OpsLogEntry {
                id: Uuid::new_v4().to_string(),
                seq: 0,
                ts: Utc::now(),
                op: "consolidate".into(),
                prefix: Some("MAINT".into()),
                message: format!(
                    "consolidate dry_run proposed wiki://{} from {} source(s)",
                    proposed.slug, source_count
                ),
                entity_id: None,
                entity_kind: Some(LAYER_WIKI.into()),
                payload_json: serde_json::json!({
                    "apply": false,
                    "dry_run": true,
                    "source_ids": &source_ids,
                    "source_uris": &source_uris,
                    "slug": &proposed.slug,
                    "title": &proposed.title,
                    "kind": &proposed.kind,
                    "capped": capped,
                    "max_docs": max_docs,
                })
                .to_string(),
                agent_name: agent.map(|s| s.to_string()),
            })
        })?;

        return Ok(ConsolidateResult {
            source_ids,
            source_uris,
            source_count,
            capped,
            max_docs,
            applied: false,
            dry_run: true,
            notes: proposed.notes.clone(),
            proposed,
            written: None,
        });
    }

    let expected_revision = revisions
        .get(&format!("wiki://{}", slugify(&proposed.slug)))
        .copied();
    let written = apply_consolidate_proposal_at_revision(
        store,
        embedder,
        config,
        &proposed,
        &resolved,
        agent,
        expected_revision,
    )
    .await?;

    crate::util::blocking_section(|| {
        store.append_ops_log(&OpsLogEntry {
            id: Uuid::new_v4().to_string(),
            seq: 0,
            ts: Utc::now(),
            op: "consolidate".into(),
            prefix: Some("MAINT".into()),
            message: format!(
                "consolidated {} source(s) into wiki://{}",
                source_count, written.slug
            ),
            entity_id: Some(written.document_id.clone()),
            entity_kind: Some(LAYER_WIKI.into()),
            payload_json: serde_json::json!({
                "apply": true,
                "dry_run": false,
                "source_ids": &source_ids,
                "source_uris": &source_uris,
                "slug": &written.slug,
                "document_id": &written.document_id,
                "node_id": &written.node_id,
                "edge_count": written.edge_count,
                "capped": capped,
                "max_docs": max_docs,
            })
            .to_string(),
            agent_name: agent.map(|s| s.to_string()),
        })
    })?;

    Ok(ConsolidateResult {
        source_ids,
        source_uris,
        source_count,
        capped,
        max_docs,
        applied: true,
        dry_run: false,
        notes: proposed.notes.clone(),
        proposed,
        written: Some(written),
    })
}

/// Write a consolidate proposal (wiki page + graph links from sources).
///
/// Separated so tests can exercise apply without a live LLM.
pub async fn apply_consolidate_proposal(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    proposal: &ConsolidateProposal,
    sources: &[Document],
    agent: Option<&str>,
) -> Result<WikiWriteResult> {
    let uri = format!("wiki://{}", slugify(&proposal.slug));
    let expected_revision =
        crate::util::blocking_section(|| store.find_by_uri(&uri))?.map(|page| page.revision);
    apply_consolidate_proposal_at_revision(
        store,
        embedder,
        config,
        proposal,
        sources,
        agent,
        expected_revision,
    )
    .await
}

pub(super) async fn apply_consolidate_proposal_at_revision(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    proposal: &ConsolidateProposal,
    sources: &[Document],
    agent: Option<&str>,
    expected_revision: Option<i64>,
) -> Result<WikiWriteResult> {
    let source_ids: Vec<String> = sources.iter().map(|d| d.id.clone()).collect();
    let source_uris: Vec<String> = sources.iter().map(|d| d.uri.clone()).collect();
    let source_titles: Vec<String> = sources.iter().map(|d| d.title.clone()).collect();
    let source_versions: Vec<WikiSourceVersion> = sources
        .iter()
        .map(WikiSourceVersion::from_document)
        .collect();

    let summary = proposal
        .summary
        .clone()
        .unwrap_or_else(|| first_line(&proposal.content, 240));

    let wr = write_generated_wiki_page(
        store,
        embedder,
        config,
        WikiWriteCommand {
            slug: proposal.slug.clone(),
            title: proposal.title.clone(),
            content: proposal.content.clone(),
            wing: None,
            room: None,
            kind: proposal.kind.clone(),
            category: proposal.category.clone(),
            summary: Some(summary),
            agent: agent.map(str::to_string),
            options: WriteWikiOpts {
                op: Some("consolidate_write".into()),
                metadata_json_override: None,
                prefix: Some("MAINT".into()),
                message: Some(format!(
                    "consolidated wiki://{} from {} source(s)",
                    proposal.slug,
                    sources.len()
                )),
                extra_metadata: Some(serde_json::json!({
                    "consolidated_from": &source_ids,
                    "source_ids": &source_ids,
                    "source_uris": &source_uris,
                    "source_titles": &source_titles,
                    "source_versions": &source_versions,
                    "suggested_links": &proposal.suggested_links,
                    "filed_as": "consolidate",
                })),
                extra_payload: Some(serde_json::json!({
                    "source_ids": &source_ids,
                    "source_count": sources.len(),
                    "suggested_links": &proposal.suggested_links,
                })),
                if_match_revision: None,
            },
        },
        expected_revision,
    )
    .await?;

    // Link each source document → consolidated wiki (related), when nodes exist.
    crate::util::blocking_section(|| {
        if let Ok(Some(wiki_node)) = store.find_node_by_document_id(&wr.document_id) {
            for src in sources {
                if let Ok(Some(src_node)) = store.find_node_by_document_id(&src.id) {
                    if let Err(error) = store.link_nodes(&src_node.id, &wiki_node.id, "related", 1.0) {
                        tracing::warn!(%error, "failed to link consolidated wiki node");
                    }
                }
            }
        }
    });

    Ok(wr)
}

/// Merge N document texts into one consolidated wiki page proposal (JSON).
///
/// Used by the `consolidate` maintenance tool: propose first; callers write
/// only when `apply=true`. Never invents facts beyond the provided sources.
pub(crate) async fn consolidate_wiki(
    llm: &dyn ChatCompleter,
    schema_md: &str,
    sources: &[(String, String, String)],
) -> Result<ConsolidateProposal> {
    if sources.is_empty() {
        return Err(AppError::llm(
            "consolidate_wiki requires at least one source document",
        ));
    }
    let system = r#"You are a knowledge consolidator for a local LLM wiki (Karpathy-style).
Merge the given source documents into ONE durable wiki page (concept or entity).

Rules:
- Never invent facts not grounded in the sources; mark gaps as TODO.
- Deduplicate overlapping claims; prefer precise wording over length.
- Use markdown. Cross-link with [[Page Title]] and tags as #tag.
- List provenance under a ## Sources section (titles / uris).
- Return ONLY valid JSON (no markdown fences) matching:
{
  "slug": "kebab-case",
  "title": "Human Title",
  "kind": "concept|entity|wiki",
  "category": "optional",
  "content": "markdown body",
  "summary": "one-line",
  "suggested_links": ["Other Page", "[[Yet Another]]"],
  "notes": "optional consolidator notes"
}
"#;
    // Budget: schema + N sources within a reasonable prompt size.
    let per_body = (20_000usize / sources.len().max(1)).clamp(1_500, 8_000);
    let mut blocks = String::new();
    for (i, (title, uri, body)) in sources.iter().enumerate() {
        blocks.push_str(&format!(
            "### Source {}\n- title: {}\n- uri: {}\n\n```\n{}\n```\n\n",
            i + 1,
            title,
            uri,
            truncate(body, per_body)
        ));
    }
    let user = format!(
        "## Wiki schema / conventions\n\n{schema}\n\n## Sources to consolidate ({n})\n\n{blocks}",
        schema = truncate(schema_md, 4_000),
        n = sources.len(),
        blocks = blocks,
    );
    let messages = system_user(system, user);
    let raw = llm.complete(&messages).await?;
    parse_consolidate_json(&raw)
}
