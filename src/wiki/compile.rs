//! LLM-assisted compile of one raw source into wiki pages.

use super::*;
use crate::llm::{parse_compile_json, system_user, ChatCompleter};
use crate::util::excerpt_chars as truncate;

/// Compile raw document with local LLM into wiki pages.
pub async fn compile_source(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    llm: &dyn ChatCompleter,
    source_id_or_uri: &str,
    dry_run: bool,
    agent: Option<&str>,
) -> Result<CompileApplyResult> {
    let source = crate::util::blocking_section(|| resolve_doc(store, source_id_or_uri))?;
    if !matches!(
        Layer::parse(&source.layer),
        Some(Layer::Raw | Layer::Document)
    ) {
        // Allow compile from any non-wiki if needed, but prefer raw.
        tracing::warn!(layer = %source.layer, "compile_source: source is not layer=raw");
    }
    let schema = crate::util::blocking_section(|| schema_text(store))?;
    // Output slugs are chosen by the model, so capture the lean catalog before
    // generation rather than reading the chosen page's revision after the LLM.
    let revisions = if dry_run {
        HashMap::new()
    } else {
        crate::util::blocking_section(|| wiki_revision_snapshot(store))?
    };
    let proposed = compile_wiki(llm, &schema, &source.title, &source.uri, &source.content).await?;

    if dry_run {
        return Ok(CompileApplyResult {
            source_id: source.id,
            pages_written: 0,
            page_ids: vec![],
            notes: proposed.notes.clone(),
            dry_run: true,
            proposed: Some(proposed),
        });
    }

    let mut page_ids = Vec::new();
    for page in &proposed.pages {
        let expected_revision = revisions
            .get(&format!("wiki://{}", slugify(&page.slug)))
            .copied();
        let wr = write_generated_wiki_page(
            store,
            embedder,
            config,
            WikiWriteCommand {
                slug: page.slug.clone(),
                title: page.title.clone(),
                content: page.content.clone(),
                wing: None,
                room: None,
                kind: page.kind.clone(),
                category: page.category.clone(),
                summary: page.summary.clone(),
                agent: agent.map(str::to_string),
                options: WriteWikiOpts {
                    op: Some("compile_source".into()),
                    metadata_json_override: None,
                    prefix: Some("COMPILE".into()),
                    message: Some(format!(
                        "compiled page wiki://{} from {}",
                        page.slug, source.uri
                    )),
                    extra_metadata: Some(serde_json::json!({
                        "source_id": source.id,
                        "source_uri": source.uri,
                        "source_versions": [WikiSourceVersion::from_document(&source)],
                    })),
                    extra_payload: Some(serde_json::json!({
                        "source_id": source.id,
                        "source_uri": source.uri,
                    })),
                    if_match_revision: None,
                },
            },
            expected_revision,
        )
        .await?;
        page_ids.push(wr.document_id);
    }

    crate::util::blocking_section(|| {
        // Link source -> summaries via graph related edges when possible
        if let (Ok(Some(src_node)), true) = (
            store.find_node_by_document_id(&source.id),
            !page_ids.is_empty(),
        ) {
            for pid in &page_ids {
                if let Ok(Some(dst)) = store.find_node_by_document_id(pid) {
                    if let Err(error) = store.link_nodes(&src_node.id, &dst.id, "related", 1.0) {
                        tracing::warn!(%error, "failed to link wiki graph nodes");
                    }
                }
            }
        }

        store.append_ops_log(&OpsLogEntry {
        id: Uuid::new_v4().to_string(),
        seq: 0,
        ts: Utc::now(),
        op: "compile_source".into(),
        prefix: Some("COMPILE".into()),
        message: format!("compiled {} into {} wiki pages", source.uri, page_ids.len()),
        entity_id: Some(source.id.clone()),
        entity_kind: Some("raw".into()),
        payload_json: serde_json::json!({ "page_ids": page_ids }).to_string(),
        agent_name: agent.map(|s| s.to_string()),
    })
    })?;

    Ok(CompileApplyResult {
        source_id: source.id,
        pages_written: page_ids.len(),
        page_ids,
        notes: proposed.notes,
        dry_run: false,
        proposed: None,
    })
}

/// A model may choose any existing slug; only lightweight revision metadata is
/// needed to detect edits made while generation is in flight.
pub(super) fn wiki_revision_snapshot(store: &Store) -> Result<HashMap<String, i64>> {
    Ok(store
        .list_wiki_page_metas()?
        .into_iter()
        .map(|page| (page.uri, page.revision))
        .collect())
}

/// Generated writes always preserve the state observed before generation, even
/// when callers have not enabled mandatory CAS for ordinary manual edits.
pub(super) async fn write_generated_wiki_page(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    mut command: WikiWriteCommand,
    expected_revision: Option<i64>,
) -> Result<WikiWriteResult> {
    command.options.if_match_revision = expected_revision;
    if expected_revision.is_some() {
        write_wiki_page_command(store, embedder, config, command).await
    } else {
        create_wiki_page_command(store, embedder, config, command).await
    }
}

/// Compile a raw source into wiki pages (JSON object in model output).
pub(crate) async fn compile_wiki(
    llm: &dyn ChatCompleter,
    schema_md: &str,
    source_title: &str,
    source_uri: &str,
    source_body: &str,
) -> Result<CompileResult> {
    let system = r#"You are a knowledge compiler for a local LLM wiki (Karpathy-style).
Given an immutable raw source, produce structured wiki pages.

Rules:
- Never invent facts not grounded in the source; mark gaps as TODO.
- Use markdown. Cross-link with [[Page Title]] and tags as #tag.
- Prefer few high-quality pages over many stubs.
- Return ONLY valid JSON (no markdown fences) matching:
{
  "pages": [
{
  "slug": "kebab-case",
  "title": "Human Title",
  "kind": "source_summary|entity|concept|wiki",
  "category": "optional",
  "content": "markdown body",
  "summary": "one-line"
}
  ],
  "notes": "optional compiler notes"
}
"#;
    let user = format!(
        "## Wiki schema / conventions\n\n{schema}\n\n## Source\n- title: {title}\n- uri: {uri}\n\n```\n{body}\n```\n",
        schema = truncate(schema_md, 6000),
        title = source_title,
        uri = source_uri,
        body = truncate(source_body, 24_000),
    );
    let messages = system_user(system, user);
    let raw = llm.complete(&messages).await?;
    parse_compile_json(&raw)
}
