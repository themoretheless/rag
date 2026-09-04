//! Karpathy-style compiled wiki layer: pages, index, schema, lint, compile.
//!
//! Wiki pages are ordinary [`Document`] rows with `layer = "wiki"`.
//! Raw sources (`layer = "raw"`) are **immutable for content updates**;
//! only re-ingest replace (e.g. [`ingest_raw`]) may overwrite raw content.

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::config::Config;
use crate::db::store::DocumentDerivedWrite;
use crate::db::Store;
use crate::document_indexer::DocumentIndexer;
use crate::embeddings::EmbeddingProvider;
use crate::error::{AppError, Result};
use crate::graph::rebuild_document_graph;
use crate::llm::{ChatClient, CompileResult, ConsolidateProposal};
use crate::models::{Document, OpsLogEntry, WikiIndexEntry};
use crate::util::{content_hash, slugify as shared_slugify, SlugPolicy};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// Document layer for immutable raw sources.
pub const LAYER_RAW: &str = "raw";
/// Document layer for compiled wiki pages.
pub const LAYER_WIKI: &str = "wiki";
/// Document layer for the agent schema conventions file.
pub const LAYER_SCHEMA: &str = "schema";

/// Stable URI for the agent conventions schema document.
pub const SCHEMA_URI: &str = "schema://agents";

/// Default title when creating or updating the schema document.
pub const SCHEMA_TITLE: &str = "Wiki schema";

/// Default schema text when none stored.
pub const DEFAULT_SCHEMA: &str = r#"# Wiki schema (schema://agents)

## Layers
- raw: immutable sources (never edit content after ingest; re-ingest same uri to replace)
- wiki: agent/LLM compiled pages (client owns synthesis)
- schema: this file
- index: catalog table wiki_index (use read_index / rebuild_index)

## Page kinds
- source_summary: one page per ingested source
- entity: people, products, systems
- concept: ideas, patterns
- wiki: general notes / answers

## Index-first query cascade (mandatory)
1. query_with_index — wiki catalog (slug/title/summary)
2. search_wiki — compiled layer only
3. search (prefer mode=hybrid) — chunk/raw escape hatch
4. get_neighbors / get_backlinks — after you have a node or document
5. get_document / get_source — open full text only for 1–2 ids
6. pack_context — budget hits before long answers
Never dump the whole corpus. Never invent facts; cite ids/slugs from tools.

## Compile loop (client LLM)
get_schema → get_source (raw) → write_wiki_page* → rebuild_index → append_log
Optional: file_answer for durable Q&A. Server does not auto-summarize on ingest.

## Conventions
- Titles unique; slug kebab-case
- Cross-link with [[Title]]; tags #tag
- Prefer `source: uri` when summarizing raw
- No invented facts; mark TODO for gaps

## Tool surface
Default MCP list is RAG_TOOLS=spine (compile-first). Advanced MemPalace/maintain tools need RAG_TOOLS=full.
"#;

/// Returns true when `layer` is the immutable raw source layer.
#[inline]
pub fn is_raw_layer(layer: &str) -> bool {
    layer == LAYER_RAW
}

/// Returns true when `layer` is a compiled wiki page.
#[inline]
pub fn is_wiki_layer(layer: &str) -> bool {
    layer == LAYER_WIKI
}

/// Reject content mutation for immutable raw documents.
///
/// Re-ingest via [`ingest_raw`] (same-uri replace) is the only allowed content
/// replacement path for `layer=raw` and does **not** call this helper.
pub fn assert_content_mutable(doc: &Document) -> Result<()> {
    if is_raw_layer(&doc.layer) {
        return Err(AppError::forbidden(
            "raw layer is immutable: content updates are forbidden; re-ingest with the same uri to replace",
        ));
    }
    Ok(())
}

/// Result of `get_schema` / `update_schema` (wiki/schema document at [`SCHEMA_URI`]).
#[derive(Debug, Clone, Serialize)]
pub struct SchemaDocumentView {
    pub document_id: String,
    pub uri: String,
    pub title: String,
    pub content: String,
    pub layer: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// True when content was just written by `update_schema` (or default init).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub created: bool,
}

impl SchemaDocumentView {
    fn from_doc(doc: &Document, created: bool) -> Self {
        Self {
            document_id: doc.id.clone(),
            uri: doc.uri.clone(),
            title: doc.title.clone(),
            content: doc.content.clone(),
            layer: doc.layer.clone(),
            kind: doc.kind.clone(),
            content_hash: doc.content_hash.clone(),
            created_at: doc.created_at.to_rfc3339(),
            updated_at: doc.updated_at.to_rfc3339(),
            created,
        }
    }
}

/// Result of writing a wiki page (with chunk/graph stats).
#[derive(Debug, Clone, Serialize)]
pub struct WikiWriteResult {
    pub document_id: String,
    pub uri: String,
    pub slug: String,
    pub chunk_count: usize,
    pub node_id: String,
    pub edge_count: usize,
    pub index_id: String,
    /// Monotonic revision after write (for multi-LLM If-Match).
    pub revision: i64,
    /// Weak etag `W/"<revision>"`.
    pub etag: String,
}

/// Lint finding.
#[derive(Debug, Clone, Serialize, Default)]
pub struct LintIssue {
    pub code: String,
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    /// For stubs: label that was linked / expected page title or slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_label: Option<String>,
    /// Labels (or ids) of nodes that link to this entity (backlinks).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub referenced_by: Vec<String>,
    /// Graph source node for link-specific findings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// Graph target node for link-specific findings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    /// Graph relation type for link-specific findings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel_type: Option<String>,
    /// Number of equivalent occurrences, primarily for duplicate links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrences: Option<usize>,
}

/// Aggregate wiki/link health counters. Counts are deterministic and make the
/// lint response useful for dashboards without parsing issue messages.
#[derive(Debug, Clone, Serialize, Default)]
pub struct LinkHealthCounts {
    pub documents: usize,
    pub wiki_pages: usize,
    pub graph_nodes: usize,
    pub graph_edges: usize,
    pub wikilinks: usize,
    pub broken_wikilinks: usize,
    pub unresolved_targets: usize,
    pub orphan_documents: usize,
    pub orphan_wiki_pages: usize,
    pub self_links: usize,
    pub duplicate_link_groups: usize,
    pub duplicate_link_occurrences: usize,
}

/// Lint report.
#[derive(Debug, Clone, Serialize)]
pub struct LintReport {
    pub issue_count: usize,
    pub issues: Vec<LintIssue>,
    /// True when no warning/error link-health findings were found.
    pub healthy: bool,
    pub health: LinkHealthCounts,
}

/// Compile apply result.
#[derive(Debug, Clone, Serialize)]
pub struct CompileApplyResult {
    pub source_id: String,
    pub pages_written: usize,
    pub page_ids: Vec<String>,
    pub notes: Option<String>,
    pub dry_run: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed: Option<CompileResult>,
}

/// Result of [`consolidate`]: LLM merges N docs into one wiki page proposal.
///
/// When `apply=false` (default), only `proposed` is returned. When `apply=true`,
/// writes `layer=wiki`, re-embeds chunks, rebuilds graph + wiki_index, links
/// sources via `related` edges, and appends `ops_log`.
#[derive(Debug, Clone, Serialize)]
pub struct ConsolidateResult {
    /// Resolved source document ids (order preserved, capped by max_docs).
    pub source_ids: Vec<String>,
    pub source_uris: Vec<String>,
    pub source_count: usize,
    /// True when more ids were requested than the budget allowed.
    pub capped: bool,
    pub max_docs: usize,
    /// True when the proposal was written to the store.
    pub applied: bool,
    /// Inverse of `applied` (agent-friendly: propose-only path).
    pub dry_run: bool,
    pub proposed: ConsolidateProposal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub written: Option<WikiWriteResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Ensure `schema://agents` exists; seed with [`DEFAULT_SCHEMA`] when missing.
pub fn ensure_default_schema(store: &Store) -> Result<Document> {
    if let Some(d) = store.get_schema_document()? {
        return Ok(d);
    }
    let now = Utc::now();
    let doc = Document {
        id: Uuid::new_v4().to_string(),
        uri: SCHEMA_URI.into(),
        title: SCHEMA_TITLE.into(),
        content: DEFAULT_SCHEMA.into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        layer: LAYER_SCHEMA.into(),
        kind: LAYER_SCHEMA.into(),
        content_hash: Some(content_hash(DEFAULT_SCHEMA)),
        ..Default::default()
    };
    store.upsert_document(&doc)?;
    // Graph node so schema can participate in wikilink graph if content has links.
    let _ = rebuild_document_graph(store, &doc);
    let _ = store.append_ops_log(&OpsLogEntry {
        id: Uuid::new_v4().to_string(),
        seq: 0,
        ts: now,
        op: "schema_init".into(),
        prefix: Some("SCHEMA".into()),
        message: format!("initialized default {SCHEMA_URI}"),
        entity_id: Some(doc.id.clone()),
        entity_kind: Some("schema".into()),
        payload_json: "{}".into(),
        agent_name: None,
    })?;
    Ok(doc)
}

/// Get or create schema content as string.
pub fn schema_text(store: &Store) -> Result<String> {
    Ok(ensure_default_schema(store)?.content)
}

/// MCP `get_schema`: return the wiki/schema document at [`SCHEMA_URI`].
///
/// Seeds [`DEFAULT_SCHEMA`] when no schema document exists yet.
pub fn get_schema(store: &Store) -> Result<SchemaDocumentView> {
    let doc = ensure_default_schema(store)?;
    Ok(SchemaDocumentView::from_doc(&doc, false))
}

/// MCP `update_schema`: replace content of the schema document at [`SCHEMA_URI`].
///
/// Stored as `layer=schema`, `kind=schema` (wiki compile-layer conventions file).
/// Does not embed chunks (agents read the full document via `get_schema`).
/// Re-runs graph extract for `[[wikilinks]]` / `#tags` in the schema text.
pub fn update_schema(
    store: &Store,
    content: &str,
    title: Option<&str>,
    agent: Option<&str>,
) -> Result<SchemaDocumentView> {
    if content.trim().is_empty() {
        return Err(AppError::config("update_schema: content must not be empty"));
    }

    let now = Utc::now();
    let title = title
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(SCHEMA_TITLE)
        .to_string();

    let (document_id, created_at, created) =
        if let Some(existing) = store.find_by_uri(SCHEMA_URI)? {
            assert_content_mutable(&existing)?;
            (existing.id, existing.created_at, false)
        } else if let Some(existing) = store.get_schema_document()? {
            // Fallback layer=schema row with a non-canonical uri: re-key to SCHEMA_URI.
            assert_content_mutable(&existing)?;
            (existing.id, existing.created_at, false)
        } else {
            (Uuid::new_v4().to_string(), now, true)
        };

    let hash = content_hash(content);
    let doc = Document {
        id: document_id.clone(),
        uri: SCHEMA_URI.into(),
        title: title.clone(),
        content: content.to_string(),
        metadata_json: "{}".into(),
        created_at,
        updated_at: now,
        layer: LAYER_SCHEMA.into(),
        kind: LAYER_SCHEMA.into(),
        content_hash: Some(hash.clone()),
        ..Default::default()
    };
    store.upsert_document(&doc)?;
    let _ = rebuild_document_graph(store, &doc);

    let _ = store.append_ops_log(&OpsLogEntry {
        id: Uuid::new_v4().to_string(),
        seq: 0,
        ts: now,
        op: "schema_update".into(),
        prefix: Some("SCHEMA".into()),
        message: format!("updated {SCHEMA_URI}"),
        entity_id: Some(document_id.clone()),
        entity_kind: Some("schema".into()),
        payload_json: serde_json::json!({
            "uri": SCHEMA_URI,
            "title": title,
            "content_hash": hash,
            "created": created,
        })
        .to_string(),
        agent_name: agent.map(|s| s.to_string()),
    })?;

    Ok(SchemaDocumentView::from_doc(&doc, created))
}

/// Options for wiki page write (log op / extra metadata).
#[derive(Debug, Clone, Default)]
pub struct WriteWikiOpts {
    /// Ops log `op` (default `wiki_write`).
    pub op: Option<String>,
    /// Ops log prefix (default `WIKI`).
    pub prefix: Option<String>,
    /// Ops log message override.
    pub message: Option<String>,
    /// Extra keys merged into document `metadata_json`.
    pub extra_metadata: Option<serde_json::Value>,
    /// Extra keys merged into ops_log `payload_json`.
    pub extra_payload: Option<serde_json::Value>,
    /// Optimistic concurrency: must match existing document revision when set.
    pub if_match_revision: Option<i64>,
}

/// Transport-neutral command for creating or replacing a wiki page.
#[derive(Debug, Clone)]
pub struct WikiWriteCommand {
    pub slug: String,
    pub title: String,
    pub content: String,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub kind: String,
    pub category: Option<String>,
    pub summary: Option<String>,
    pub agent: Option<String>,
    pub options: WriteWikiOpts,
}

/// Write/update a wiki page by slug (`wiki://{slug}`), chunk+embed+graph.
// Compatibility adapter: new integrations should prefer the command API.
#[allow(clippy::too_many_arguments)]
pub async fn write_wiki_page(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    slug: &str,
    title: &str,
    content: &str,
    kind: &str,
    category: Option<&str>,
    summary: Option<&str>,
    agent: Option<&str>,
) -> Result<WikiWriteResult> {
    write_wiki_page_with_opts(
        store,
        embedder,
        config,
        slug,
        title,
        content,
        kind,
        category,
        summary,
        agent,
        WriteWikiOpts::default(),
    )
    .await
}

/// Write/update a wiki page with custom log op and metadata.
// Compatibility adapter: new integrations should prefer the command API.
#[allow(clippy::too_many_arguments)]
pub async fn write_wiki_page_with_opts(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    slug: &str,
    title: &str,
    content: &str,
    kind: &str,
    category: Option<&str>,
    summary: Option<&str>,
    agent: Option<&str>,
    opts: WriteWikiOpts,
) -> Result<WikiWriteResult> {
    write_wiki_page_command(
        store,
        embedder,
        config,
        WikiWriteCommand {
            slug: slug.to_string(),
            title: title.to_string(),
            content: content.to_string(),
            wing: None,
            room: None,
            kind: kind.to_string(),
            category: category.map(str::to_string),
            summary: summary.map(str::to_string),
            agent: agent.map(str::to_string),
            options: opts,
        },
    )
    .await
}

/// Command-object entrypoint used by transports and application services.
pub async fn write_wiki_page_command(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    command: WikiWriteCommand,
) -> Result<WikiWriteResult> {
    let WikiWriteCommand {
        slug,
        title,
        content,
        wing,
        room,
        kind,
        category,
        summary,
        agent,
        options: opts,
    } = command;
    let slug = slugify(&slug);
    if slug.is_empty() {
        return Err(AppError::config("wiki slug must not be empty"));
    }
    let uri = format!("wiki://{slug}");
    let now = Utc::now();
    let kind = if kind.trim().is_empty() {
        LAYER_WIKI
    } else {
        kind.trim()
    };

    let if_match = opts.if_match_revision;
    // Never delete chunks before CAS succeeds: a stale if_match (or a concurrent
    // loser) must leave the previous body+chunks intact for retrieval.
    let existing = store.find_by_uri(&uri)?;
    let (document_id, created_at) = if let Some(existing) = existing.as_ref() {
        // Multi-LLM CAS: when RAG_WIKI_REQUIRE_IF_MATCH=true, updates must pass if_match.
        if config.wiki_require_if_match && if_match.is_none() {
            return Err(AppError::config(format!(
                "if_match_revision (or if_match_etag) is required to update wiki page '{uri}' \
                 when RAG_WIKI_REQUIRE_IF_MATCH=true; call get_wiki_page and pass revision"
            )));
        }
        // Enforce raw immutability: never rewrite raw content via wiki write.
        assert_content_mutable(existing)?;
        // Fail-fast CAS check (store upsert still enforces under the write lock).
        if let Some(expected) = if_match {
            if existing.revision != expected {
                return Err(AppError::conflict(format!(
                    "etag mismatch: if_match_revision={expected} but document {} has revision {}",
                    existing.id, existing.revision
                )));
            }
        }
        (existing.id.clone(), existing.created_at)
    } else {
        if if_match.is_some() {
            return Err(AppError::conflict(format!(
                "etag mismatch: if_match_revision set but wiki page '{uri}' does not exist"
            )));
        }
        (Uuid::new_v4().to_string(), now)
    };

    let title = if title.trim().is_empty() {
        slug.clone()
    } else {
        title.trim().to_string()
    };

    // Updating a page must not silently turn a project-scoped/pinned document
    // into an unscoped default row. Start from the persisted document and
    // replace only fields owned by this command. Optional metadata overlays are
    // merged into the existing object so unrelated application keys survive.
    let metadata_overlay_requested =
        category.is_some() || summary.is_some() || opts.extra_metadata.is_some();
    let metadata_json = merge_wiki_metadata(
        existing
            .as_ref()
            .map(|document| document.metadata_json.as_str()),
        category.as_deref(),
        summary.as_deref(),
        opts.extra_metadata,
        metadata_overlay_requested,
    )?;
    let existing_index = if existing.is_some() {
        store.get_wiki_index_by_slug(&slug)?
    } else {
        None
    };

    let mut doc = existing.clone().unwrap_or_default();
    doc.id = document_id.clone();
    doc.uri = uri.clone();
    doc.title = title.clone();
    doc.content = content.clone();
    if let Some(wing) = wing
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        doc.wing = Some(wing.to_string());
    }
    if let Some(room) = room
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        doc.room = Some(room.to_string());
    }
    doc.metadata_json = metadata_json;
    doc.created_at = created_at;
    doc.updated_at = now;
    doc.layer = LAYER_WIKI.into();
    doc.kind = kind.to_string();
    doc.content_hash = Some(content_hash(&content));
    let chunks = DocumentIndexer::new(embedder.as_ref(), config)
        .build_plain_chunks(&doc)
        .await?;
    let chunk_count = chunks.len();

    let metadata = serde_json::from_str::<serde_json::Value>(&doc.metadata_json).ok();
    let category = category.or_else(|| {
        existing_index
            .as_ref()
            .and_then(|entry| entry.category.clone())
            .or_else(|| metadata_string_field(metadata.as_ref(), "category").map(str::to_string))
    });
    let summary = summary
        .or_else(|| {
            existing_index
                .as_ref()
                .and_then(|entry| entry.summary.clone())
        })
        .or_else(|| metadata_string_field(metadata.as_ref(), "summary").map(str::to_string))
        .unwrap_or_else(|| first_line(&content, 240));
    let index_id = existing_index
        .as_ref()
        .map(|entry| entry.id.clone())
        .unwrap_or_else(|| format!("idx-{document_id}"));
    let index_entry = WikiIndexEntry {
        id: index_id.clone(),
        slug: slug.clone(),
        title: title.clone(),
        kind: kind.to_string(),
        category,
        summary: Some(summary),
        page_id: Some(document_id.clone()),
        updated_at: now,
    };

    let op = opts.op.unwrap_or_else(|| "wiki_write".into());
    let prefix = opts.prefix.unwrap_or_else(|| "WIKI".into());
    let message = opts
        .message
        .unwrap_or_else(|| format!("wrote wiki://{slug}"));
    let mut payload = serde_json::json!({ "slug": slug, "kind": kind });
    if let Some(extra) = opts.extra_payload {
        if let (Some(obj), serde_json::Value::Object(map)) = (payload.as_object_mut(), extra) {
            for (k, v) in map {
                obj.insert(k, v);
            }
        }
    }

    let audit_entry = OpsLogEntry {
        id: Uuid::new_v4().to_string(),
        seq: 0,
        ts: now,
        op,
        prefix: Some(prefix),
        message,
        entity_id: Some(document_id.clone()),
        entity_kind: Some(LAYER_WIKI.into()),
        payload_json: payload.to_string(),
        agent_name: agent.clone(),
    };
    let write =
        store.write_wiki_document_atomic(&doc, if_match, &chunks, &index_entry, &audit_entry)?;
    let revision = write.revision;
    let node_id = write.node_id.unwrap_or_default();
    let edge_count = write.edge_count;

    // Transport-applied changes carry a sync:* agent marker and must not be
    // journalled again, otherwise pull would create an endless echo loop.
    if !agent
        .as_deref()
        .is_some_and(|value| value.starts_with("sync:"))
    {
        let sync_payload = serde_json::json!({
            "slug": slug,
            "title": title,
            "content": content,
            "wing": doc.wing.clone(),
            "room": doc.room.clone(),
            "kind": kind,
            "category": index_entry.category.clone(),
            "summary": index_entry.summary.clone(),
        });
        if let Err(error) = store.journal_local_sync_change(
            "wiki",
            &uri,
            "upsert",
            &sync_payload.to_string(),
            doc.content_hash.as_deref(),
        ) {
            tracing::error!(%error, uri = %uri, "wiki write committed but sync journal append failed");
        }
    }

    Ok(WikiWriteResult {
        document_id,
        uri,
        slug,
        chunk_count,
        node_id,
        edge_count,
        index_id,
        revision,
        etag: crate::models::format_document_etag(revision),
    })
}

fn merge_wiki_metadata(
    existing: Option<&str>,
    category: Option<&str>,
    summary: Option<&str>,
    extra: Option<serde_json::Value>,
    overlay_requested: bool,
) -> Result<String> {
    let existing = existing.unwrap_or("{}");
    if !overlay_requested {
        return Ok(if existing.trim().is_empty() {
            "{}".into()
        } else {
            existing.to_string()
        });
    }

    let mut metadata = if existing.trim().is_empty() {
        serde_json::Map::new()
    } else {
        match serde_json::from_str::<serde_json::Value>(existing) {
            Ok(serde_json::Value::Object(map)) => map,
            Ok(_) => {
                return Err(AppError::config(
                    "existing wiki metadata_json is not an object; refusing to discard it while applying metadata fields",
                ))
            }
            Err(error) => {
                return Err(AppError::config(format!(
                    "existing wiki metadata_json is invalid; refusing to discard it while applying metadata fields: {error}"
                )))
            }
        }
    };
    if let Some(category) = category {
        metadata.insert(
            "category".into(),
            serde_json::Value::String(category.to_string()),
        );
    }
    if let Some(summary) = summary {
        metadata.insert(
            "summary".into(),
            serde_json::Value::String(summary.to_string()),
        );
    }
    if let Some(extra) = extra {
        if let serde_json::Value::Object(map) = extra {
            metadata.extend(map);
        } else {
            metadata.insert("extra".into(), extra);
        }
    }
    Ok(serde_json::Value::Object(metadata).to_string())
}

fn metadata_string_field<'a>(
    metadata: Option<&'a serde_json::Value>,
    field: &str,
) -> Option<&'a str> {
    metadata?.as_object()?.get(field)?.as_str()
}

/// Update an existing wiki page (by document id, `wiki://slug`, or slug).
///
/// Requires an existing `layer=wiki` document. Re-chunks, re-embeds, and
/// re-runs wikilink/tag graph extract. Rejects raw-layer documents.
// Compatibility adapter: preserve the established public call shape.
#[allow(clippy::too_many_arguments)]
pub async fn update_wiki_page(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    id_or_slug: &str,
    title: Option<&str>,
    content: &str,
    kind: Option<&str>,
    category: Option<&str>,
    summary: Option<&str>,
    agent: Option<&str>,
) -> Result<WikiWriteResult> {
    update_wiki_page_cas(
        store, embedder, config, id_or_slug, title, content, kind, category, summary, agent, None,
    )
    .await
}

/// Like [`update_wiki_page`] with optional `if_match_revision` (multi-LLM CAS).
// Compatibility adapter: preserve the established public call shape.
#[allow(clippy::too_many_arguments)]
pub async fn update_wiki_page_cas(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    id_or_slug: &str,
    title: Option<&str>,
    content: &str,
    kind: Option<&str>,
    category: Option<&str>,
    summary: Option<&str>,
    agent: Option<&str>,
    if_match_revision: Option<i64>,
) -> Result<WikiWriteResult> {
    let existing = get_wiki_page(store, id_or_slug)?;
    assert_content_mutable(&existing)?;

    let slug = slug_from_wiki_doc(&existing);
    let title = title
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(existing.title.as_str());
    let kind = kind
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(existing.kind.as_str());

    write_wiki_page_with_opts(
        store,
        embedder,
        config,
        &slug,
        title,
        content,
        kind,
        category,
        summary,
        agent,
        WriteWikiOpts {
            if_match_revision,
            ..Default::default()
        },
    )
    .await
}

/// Fetch a wiki page by document id, `wiki://slug`, or bare slug.
///
/// Returns [`AppError::NotFound`] when missing; [`AppError::Forbidden`] when
/// the document exists but is not `layer=wiki` (including raw sources).
pub fn get_wiki_page(store: &Store, id_or_uri_or_slug: &str) -> Result<Document> {
    let s = id_or_uri_or_slug.trim();
    if s.is_empty() {
        return Err(AppError::config(
            "get_wiki_page: id_or_slug must not be empty",
        ));
    }

    let doc = if let Some(d) = store.get_document(s)? {
        d
    } else if let Some(d) = store.find_by_uri(s)? {
        d
    } else {
        let slug = if let Some(rest) = s.strip_prefix("wiki://") {
            slugify(rest)
        } else {
            slugify(s)
        };
        if slug.is_empty() {
            return Err(AppError::not_found(format!(
                "wiki page not found: {id_or_uri_or_slug}"
            )));
        }
        let uri = format!("wiki://{slug}");
        if let Some(d) = store.find_by_uri(&uri)? {
            d
        } else if let Some(entry) = store.get_wiki_index_by_slug(&slug)? {
            if let Some(pid) = entry.page_id.as_deref() {
                store.get_document(pid)?.ok_or_else(|| {
                    AppError::not_found(format!(
                        "wiki page not found for index slug '{slug}' (stale page_id)"
                    ))
                })?
            } else {
                return Err(AppError::not_found(format!(
                    "wiki page not found: {id_or_uri_or_slug}"
                )));
            }
        } else {
            return Err(AppError::not_found(format!(
                "wiki page not found: {id_or_uri_or_slug}"
            )));
        }
    };

    if !is_wiki_layer(&doc.layer) {
        if is_raw_layer(&doc.layer) {
            return Err(AppError::forbidden(format!(
                "document {} is layer=raw (immutable source); use get_source / list_sources, not wiki page APIs",
                doc.id
            )));
        }
        return Err(AppError::forbidden(format!(
            "document {} has layer='{}'; expected layer=wiki",
            doc.id, doc.layer
        )));
    }
    Ok(doc)
}

/// List all documents with `layer=wiki`, ordered by `created_at` ascending.
pub fn list_wiki_pages(store: &Store) -> Result<Vec<Document>> {
    store.list_documents_by_layer(LAYER_WIKI)
}

/// Extract kebab slug from a wiki document uri (`wiki://slug`) or title.
fn slug_from_wiki_doc(doc: &Document) -> String {
    if let Some(rest) = doc.uri.strip_prefix("wiki://") {
        let s = slugify(rest);
        if !s.is_empty() {
            return s;
        }
    }
    slugify(&doc.title)
}

/// Ingest immutable raw text (layer=raw). Re-ingest same uri replaces chunks but keeps raw policy.
// Compatibility adapter: preserve the established public call shape.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_raw(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    text: String,
    title: Option<String>,
    uri: Option<String>,
    wing: Option<String>,
    room: Option<String>,
    source_file: Option<String>,
) -> Result<crate::models::IngestResult> {
    store.ensure_embedding_manifest(config)?;
    store.require_embedding_manifest_match(config)?;
    let now = Utc::now();
    let uri = uri
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| format!("raw://{}", Uuid::new_v4()));
    let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| {
        uri.rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("untitled")
            .to_string()
    });

    let (document_id, created_at) = if let Some(existing) = store.find_by_uri(&uri)? {
        (existing.id, existing.created_at)
    } else {
        (Uuid::new_v4().to_string(), now)
    };

    let doc = Document {
        id: document_id.clone(),
        uri: uri.clone(),
        title,
        content: text,
        metadata_json: "{}".into(),
        created_at,
        updated_at: now,
        wing,
        room,
        source_file,
        layer: LAYER_RAW.into(),
        kind: "document".into(),
        content_hash: None,
        ..Default::default()
    };
    let chunks = DocumentIndexer::new(embedder.as_ref(), config)
        .build_plain_chunks(&doc)
        .await?;
    let chunk_count = chunks.len();
    // Re-ingest replace path: intentionally does not call assert_content_mutable.
    let write = store.write_document_atomic(
        &doc,
        None,
        DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
    )?;
    let _ = store.append_ops_log(&OpsLogEntry {
        id: Uuid::new_v4().to_string(),
        seq: 0,
        ts: now,
        op: "ingest_raw".into(),
        prefix: Some("INGEST".into()),
        message: format!("ingested raw {uri}"),
        entity_id: Some(document_id.clone()),
        entity_kind: Some(LAYER_RAW.into()),
        payload_json: "{}".into(),
        agent_name: None,
    })?;
    let rev = write.revision;
    Ok(crate::models::IngestResult {
        document_id,
        chunk_count,
        node_id: write.node_id.unwrap_or_default(),
        edge_count: write.edge_count,
        content_hash: content_hash(&doc.content),
        op: "inserted".into(),
        revision: rev,
        etag: crate::models::format_document_etag(rev),
    })
}

/// Compile raw document with local LLM into wiki pages.
pub async fn compile_source(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    llm: &ChatClient,
    source_id_or_uri: &str,
    dry_run: bool,
    agent: Option<&str>,
) -> Result<CompileApplyResult> {
    let source = resolve_doc(store, source_id_or_uri)?;
    if source.layer != "raw" && source.layer != "document" {
        // Allow compile from any non-wiki if needed, but prefer raw.
        tracing::warn!(layer = %source.layer, "compile_source: source is not layer=raw");
    }
    let schema = schema_text(store)?;
    let proposed = llm
        .compile_wiki(&schema, &source.title, &source.uri, &source.content)
        .await?;

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
        let wr = write_wiki_page_with_opts(
            store,
            embedder,
            config,
            &page.slug,
            &page.title,
            &page.content,
            &page.kind,
            page.category.as_deref(),
            page.summary.as_deref(),
            agent,
            WriteWikiOpts {
                op: Some("compile_source".into()),
                prefix: Some("COMPILE".into()),
                message: Some(format!(
                    "compiled page wiki://{} from {}",
                    page.slug, source.uri
                )),
                extra_metadata: Some(serde_json::json!({
                    "source_id": source.id,
                    "source_uri": source.uri,
                })),
                extra_payload: Some(serde_json::json!({
                    "source_id": source.id,
                    "source_uri": source.uri,
                })),
                if_match_revision: None,
            },
        )
        .await?;
        page_ids.push(wr.document_id);
    }

    // Link source -> summaries via graph related edges when possible
    if let (Ok(Some(src_node)), true) = (
        store.find_node_by_document_id(&source.id),
        !page_ids.is_empty(),
    ) {
        for pid in &page_ids {
            if let Ok(Some(dst)) = store.find_node_by_document_id(pid) {
                let _ = store.link_nodes(&src_node.id, &dst.id, "related", 1.0);
            }
        }
    }

    let _ = store.append_ops_log(&OpsLogEntry {
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

/// LLM consolidates N document texts into one wiki page proposal.
///
/// - `apply=false` (default for safety): return proposal only (agent may edit).
/// - `apply=true`: write wiki page (chunk+embed+graph+index) + `ops_log`, link
///   each source document node → wiki node with `related`.
///
/// Raw sources stay immutable; only the compiled wiki layer is written.
// Compatibility adapter: preserve the established public call shape.
#[allow(clippy::too_many_arguments)]
pub async fn consolidate(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    llm: &ChatClient,
    document_ids: &[String],
    apply: bool,
    opts: ConsolidateOpts,
    agent: Option<&str>,
) -> Result<ConsolidateResult> {
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
        let doc = resolve_doc(store, key)?;
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

    let schema = schema_text(store)?;
    let sources: Vec<(String, String, String)> = resolved
        .iter()
        .map(|d| (d.title.clone(), d.uri.clone(), d.content.clone()))
        .collect();
    let mut proposed = llm.consolidate_wiki(&schema, &sources).await?;

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
        let _ = store.append_ops_log(&OpsLogEntry {
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

    let written =
        apply_consolidate_proposal(store, embedder, config, &proposed, &resolved, agent).await?;

    let _ = store.append_ops_log(&OpsLogEntry {
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
    let source_ids: Vec<String> = sources.iter().map(|d| d.id.clone()).collect();
    let source_uris: Vec<String> = sources.iter().map(|d| d.uri.clone()).collect();
    let source_titles: Vec<String> = sources.iter().map(|d| d.title.clone()).collect();

    let summary = proposal
        .summary
        .clone()
        .unwrap_or_else(|| first_line(&proposal.content, 240));

    let wr = write_wiki_page_with_opts(
        store,
        embedder,
        config,
        &proposal.slug,
        &proposal.title,
        &proposal.content,
        &proposal.kind,
        proposal.category.as_deref(),
        Some(&summary),
        agent,
        WriteWikiOpts {
            op: Some("consolidate_write".into()),
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
    )
    .await?;

    // Link each source document → consolidated wiki (related), when nodes exist.
    if let Ok(Some(wiki_node)) = store.find_node_by_document_id(&wr.document_id) {
        for src in sources {
            if let Ok(Some(src_node)) = store.find_node_by_document_id(&src.id) {
                let _ = store.link_nodes(&src_node.id, &wiki_node.id, "related", 1.0);
            }
        }
    }

    Ok(wr)
}

/// One wiki page older than a linked raw source (stale compiled layer).
#[derive(Debug, Clone, Serialize)]
pub struct StaleWikiItem {
    pub wiki_id: String,
    pub wiki_uri: String,
    pub wiki_title: String,
    pub wiki_kind: String,
    pub wiki_updated_at: String,
    pub raw_id: String,
    pub raw_uri: String,
    pub raw_title: String,
    pub raw_updated_at: String,
    /// `graph_related` | `metadata` | `citation` | `content_source`
    pub link_kind: String,
}

/// Unique raw source that should be recompiled, with linked stale wiki ids.
#[derive(Debug, Clone, Serialize)]
pub struct RawRefreshTarget {
    pub raw_id: String,
    pub raw_uri: String,
    pub raw_title: String,
    pub stale_wiki_ids: Vec<String>,
}

/// Per-source recompile failure (apply path).
#[derive(Debug, Clone, Serialize)]
pub struct RefreshError {
    pub raw_id: String,
    pub message: String,
}

/// Result of [`find_stale_wiki`] / [`refresh_stale_wiki`].
#[derive(Debug, Clone, Serialize)]
pub struct RefreshStaleWikiResult {
    pub stale_count: usize,
    pub stale: Vec<StaleWikiItem>,
    /// Unique raw parents (agent mark-list / recompile queue), order stable by raw_uri.
    pub raw_sources: Vec<RawRefreshTarget>,
    /// Compile results when apply ran with an LLM (empty on list-only / dry_run).
    pub recompiled: Vec<CompileApplyResult>,
    pub errors: Vec<RefreshError>,
    pub dry_run: bool,
    /// True when at least one `compile_source` write was attempted.
    pub applied: bool,
    /// True when more unique raws exist than `max_docs` budget.
    pub capped: bool,
    pub max_docs: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Find wiki pages whose `updated_at` is older than a linked raw parent.
///
/// Links are discovered from:
/// 1. graph `related` edges between raw and wiki document nodes (`compile_source`)
/// 2. wiki metadata `source_id` / `source_uri`
/// 3. wiki metadata `citations[].document_id` / `citations[].uri`
/// 4. body lines `source: <uri-or-id>`
pub fn find_stale_wiki(store: &Store) -> Result<Vec<StaleWikiItem>> {
    let raws = store.list_documents_by_layer(LAYER_RAW)?;
    let mut raw_by_id: std::collections::HashMap<String, Document> =
        std::collections::HashMap::with_capacity(raws.len());
    let mut raw_by_uri: std::collections::HashMap<String, Document> =
        std::collections::HashMap::with_capacity(raws.len());
    for r in &raws {
        raw_by_id.insert(r.id.clone(), r.clone());
        if !r.uri.is_empty() {
            raw_by_uri.insert(r.uri.clone(), r.clone());
        }
    }

    // (wiki_id, raw_id) -> item (prefer more specific link_kind if already present)
    let mut pairs: std::collections::BTreeMap<(String, String), StaleWikiItem> =
        std::collections::BTreeMap::new();

    // 1) Graph related edges from raw → wiki (and undirected neighbors).
    for raw in &raws {
        let Some(src_node) = store.find_node_by_document_id(&raw.id)? else {
            continue;
        };
        let view = store.neighbors(&src_node.id, 1, 500)?;
        for e in &view.edges {
            if e.rel_type != "related" {
                continue;
            }
            let other = if e.source_id == src_node.id {
                e.target_id.as_str()
            } else if e.target_id == src_node.id {
                e.source_id.as_str()
            } else {
                continue;
            };
            let Some(other_node) = view.nodes.iter().find(|n| n.id == other) else {
                continue;
            };
            let Some(doc_id) = other_node.document_id.as_deref() else {
                continue;
            };
            let Some(wiki) = store.get_document(doc_id)? else {
                continue;
            };
            if !is_wiki_layer(&wiki.layer) {
                continue;
            }
            if wiki.updated_at < raw.updated_at {
                insert_stale_pair(&mut pairs, &wiki, raw, "graph_related");
            }
        }
    }

    // 2–4) Metadata + content markers on wiki pages.
    let wiki_docs = store.list_documents_by_layer(LAYER_WIKI)?;
    for wiki in &wiki_docs {
        let linked = linked_raws_for_wiki(store, wiki, &raw_by_id, &raw_by_uri)?;
        for (raw, link_kind) in linked {
            if wiki.updated_at < raw.updated_at {
                insert_stale_pair(&mut pairs, wiki, &raw, link_kind);
            }
        }
    }

    let mut out: Vec<StaleWikiItem> = pairs.into_values().collect();
    out.sort_by(|a, b| {
        b.raw_updated_at
            .cmp(&a.raw_updated_at)
            .then_with(|| a.wiki_uri.cmp(&b.wiki_uri))
            .then_with(|| a.raw_uri.cmp(&b.raw_uri))
    });
    Ok(out)
}

/// List stale wiki↔raw pairs; optionally recompile unique raw parents via LLM.
///
/// - `dry_run=true` (default for callers): list only (agent mark-list).
/// - `dry_run=false` + `llm=Some`: re-run [`compile_source`] per unique raw (capped by
///   `max_docs` / `config.maint_max_docs`).
/// - `dry_run=false` + `llm=None`: same list + notes; no error (agent recompiles).
pub async fn refresh_stale_wiki(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    llm: Option<&ChatClient>,
    dry_run: bool,
    max_docs: Option<usize>,
    agent: Option<&str>,
) -> Result<RefreshStaleWikiResult> {
    let max_docs = max_docs
        .unwrap_or(config.maint_max_docs)
        .max(1)
        .min(config.maint_max_docs.max(1));

    let stale = find_stale_wiki(store)?;
    let mut raw_sources = group_raw_targets(&stale);
    let total_raws = raw_sources.len();
    let capped = total_raws > max_docs;
    if capped {
        raw_sources.truncate(max_docs);
    }

    let mut recompiled = Vec::new();
    let mut errors = Vec::new();
    let mut applied = false;

    let notes = if dry_run {
        Some(if stale.is_empty() {
            "no stale wiki pages".into()
        } else {
            format!(
                "dry_run: {} stale page(s) over {} unique raw source(s); recompile with dry_run=false + LLM, or call compile_source per raw_sources entry",
                stale.len(),
                total_raws
            )
        })
    } else if let Some(client) = llm {
        applied = true;
        for target in &raw_sources {
            match compile_source(
                store,
                embedder,
                config,
                client,
                &target.raw_id,
                false,
                agent,
            )
            .await
            {
                Ok(res) => recompiled.push(res),
                Err(e) => errors.push(RefreshError {
                    raw_id: target.raw_id.clone(),
                    message: e.to_string(),
                }),
            }
        }
        Some(format!(
            "applied recompile for {} raw source(s); {} ok, {} errors{}",
            raw_sources.len(),
            recompiled.len(),
            errors.len(),
            if capped {
                format!(" (capped at max_docs={max_docs}; {total_raws} total)")
            } else {
                String::new()
            }
        ))
    } else {
        Some(format!(
            "listed {} stale page(s) / {} unique raw(s); no ChatClient - agent should call compile_source on each raw_sources entry",
            stale.len(),
            total_raws
        ))
    };

    let _ = store.append_ops_log(&OpsLogEntry {
        id: Uuid::new_v4().to_string(),
        seq: 0,
        ts: Utc::now(),
        op: "refresh_stale_wiki".into(),
        prefix: Some("MAINT".into()),
        message: format!(
            "refresh_stale_wiki dry_run={dry_run} stale={} raws={} applied={applied}",
            stale.len(),
            total_raws
        ),
        entity_id: None,
        entity_kind: Some("wiki".into()),
        payload_json: serde_json::json!({
            "stale_count": stale.len(),
            "raw_count": total_raws,
            "dry_run": dry_run,
            "applied": applied,
            "capped": capped,
            "max_docs": max_docs,
            "recompiled": recompiled.len(),
            "errors": errors.len(),
        })
        .to_string(),
        agent_name: agent.map(|s| s.to_string()),
    })?;

    Ok(RefreshStaleWikiResult {
        stale_count: stale.len(),
        stale,
        raw_sources,
        recompiled,
        errors,
        dry_run,
        applied,
        capped,
        max_docs,
        notes,
    })
}

fn insert_stale_pair(
    pairs: &mut std::collections::BTreeMap<(String, String), StaleWikiItem>,
    wiki: &Document,
    raw: &Document,
    link_kind: &str,
) {
    let key = (wiki.id.clone(), raw.id.clone());
    // Prefer graph_related when already present; otherwise first write wins.
    if let Some(existing) = pairs.get(&key) {
        if existing.link_kind == "graph_related" {
            return;
        }
        if link_kind != "graph_related" && existing.link_kind == link_kind {
            return;
        }
        if link_kind != "graph_related" {
            return;
        }
    }
    pairs.insert(
        key,
        StaleWikiItem {
            wiki_id: wiki.id.clone(),
            wiki_uri: wiki.uri.clone(),
            wiki_title: wiki.title.clone(),
            wiki_kind: wiki.kind.clone(),
            wiki_updated_at: wiki.updated_at.to_rfc3339(),
            raw_id: raw.id.clone(),
            raw_uri: raw.uri.clone(),
            raw_title: raw.title.clone(),
            raw_updated_at: raw.updated_at.to_rfc3339(),
            link_kind: link_kind.to_string(),
        },
    );
}

fn group_raw_targets(stale: &[StaleWikiItem]) -> Vec<RawRefreshTarget> {
    let mut map: std::collections::BTreeMap<String, RawRefreshTarget> =
        std::collections::BTreeMap::new();
    for s in stale {
        let entry = map
            .entry(s.raw_id.clone())
            .or_insert_with(|| RawRefreshTarget {
                raw_id: s.raw_id.clone(),
                raw_uri: s.raw_uri.clone(),
                raw_title: s.raw_title.clone(),
                stale_wiki_ids: Vec::new(),
            });
        if !entry.stale_wiki_ids.contains(&s.wiki_id) {
            entry.stale_wiki_ids.push(s.wiki_id.clone());
        }
    }
    // Stable order by raw_uri then id.
    let mut out: Vec<RawRefreshTarget> = map.into_values().collect();
    out.sort_by(|a, b| {
        a.raw_uri
            .cmp(&b.raw_uri)
            .then_with(|| a.raw_id.cmp(&b.raw_id))
    });
    out
}

/// Resolve raw parents linked from a wiki page (metadata + content).
fn linked_raws_for_wiki(
    store: &Store,
    wiki: &Document,
    raw_by_id: &std::collections::HashMap<String, Document>,
    raw_by_uri: &std::collections::HashMap<String, Document>,
) -> Result<Vec<(Document, &'static str)>> {
    let mut out: Vec<(Document, &'static str)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&wiki.metadata_json) {
        if let Some(sid) = meta.get("source_id").and_then(|v| v.as_str()) {
            try_push_raw(
                store, sid, "metadata", raw_by_id, raw_by_uri, &mut seen, &mut out,
            )?;
        }
        if let Some(suri) = meta.get("source_uri").and_then(|v| v.as_str()) {
            try_push_raw(
                store, suri, "metadata", raw_by_id, raw_by_uri, &mut seen, &mut out,
            )?;
        }
        if let Some(arr) = meta.get("citations").and_then(|v| v.as_array()) {
            for c in arr {
                if let Some(did) = c.get("document_id").and_then(|v| v.as_str()) {
                    try_push_raw(
                        store, did, "citation", raw_by_id, raw_by_uri, &mut seen, &mut out,
                    )?;
                }
                if let Some(uri) = c.get("uri").and_then(|v| v.as_str()) {
                    try_push_raw(
                        store, uri, "citation", raw_by_id, raw_by_uri, &mut seen, &mut out,
                    )?;
                }
            }
        }
    }

    for line in wiki.content.lines() {
        let t = line.trim();
        let rest = t
            .strip_prefix("source:")
            .or_else(|| t.strip_prefix("source_uri:"))
            .or_else(|| t.strip_prefix("Source:"));
        if let Some(rest) = rest {
            let ref_s = rest.trim().trim_matches('`').trim();
            if !ref_s.is_empty() {
                try_push_raw(
                    store,
                    ref_s,
                    "content_source",
                    raw_by_id,
                    raw_by_uri,
                    &mut seen,
                    &mut out,
                )?;
            }
        }
    }

    Ok(out)
}

fn try_push_raw(
    store: &Store,
    id_or_uri: &str,
    kind: &'static str,
    raw_by_id: &std::collections::HashMap<String, Document>,
    raw_by_uri: &std::collections::HashMap<String, Document>,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<(Document, &'static str)>,
) -> Result<()> {
    let id = id_or_uri.trim();
    if id.is_empty() {
        return Ok(());
    }
    let raw = if let Some(r) = raw_by_id.get(id) {
        r.clone()
    } else if let Some(r) = raw_by_uri.get(id) {
        r.clone()
    } else if let Some(d) = store.get_document(id)? {
        if is_raw_layer(&d.layer) {
            d
        } else {
            return Ok(());
        }
    } else if let Some(d) = store.find_by_uri(id)? {
        if is_raw_layer(&d.layer) {
            d
        } else {
            return Ok(());
        }
    } else {
        return Ok(());
    };
    if seen.insert(raw.id.clone()) {
        out.push((raw, kind));
    }
    Ok(())
}

/// One citation attached to a filed answer (metadata + optional body lines).
#[derive(Debug, Clone, Serialize, serde::Deserialize, Default)]
pub struct FileAnswerCitation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl FileAnswerCitation {
    /// Human-readable bullet for the page body.
    pub fn display_line(&self) -> String {
        let label = self
            .title
            .as_deref()
            .or(self.uri.as_deref())
            .or(self.document_id.as_deref())
            .unwrap_or("source");
        let mut line = label.to_string();
        if let Some(uri) = self.uri.as_deref().filter(|u| !u.is_empty()) {
            if self.title.is_some() {
                line.push_str(&format!(" ({uri})"));
            }
        }
        if let Some(q) = self.quote.as_deref().filter(|s| !s.is_empty()) {
            let short = if q.chars().count() > 160 {
                let mut s: String = q.chars().take(159).collect();
                s.push('…');
                s
            } else {
                q.to_string()
            };
            line.push_str(&format!(": \"{short}\""));
        }
        if let Some(n) = self.note.as_deref().filter(|s| !s.is_empty()) {
            line.push_str(&format!(" - {n}"));
        }
        line
    }
}

/// Persist a cited answer as a wiki page: body + citations metadata, graph rebuild, ops log, index touch.
// Compatibility adapter: preserve the established public call shape.
#[allow(clippy::too_many_arguments)]
pub async fn file_answer(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    title: &str,
    body: &str,
    slug: Option<&str>,
    citations: Option<Vec<FileAnswerCitation>>,
    agent: Option<&str>,
) -> Result<WikiWriteResult> {
    if title.trim().is_empty() {
        return Err(AppError::config("file_answer requires a non-empty title"));
    }
    if body.trim().is_empty() {
        return Err(AppError::config("file_answer requires a non-empty body"));
    }

    let slug_src = slug
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(title);
    let slug = slugify(slug_src);
    if slug.is_empty() {
        return Err(AppError::config("file_answer could not derive a wiki slug"));
    }

    let cites = citations.unwrap_or_default();
    let mut page_body = body.trim_end().to_string();
    if !cites.is_empty() {
        // Avoid duplicating an existing Citations section.
        if !page_body.to_ascii_lowercase().contains("## citations") {
            page_body.push_str("\n\n## Citations\n");
            for c in &cites {
                page_body.push_str(&format!("- {}\n", c.display_line()));
            }
        }
    }

    let cites_json = serde_json::to_value(&cites).unwrap_or_else(|_| serde_json::json!([]));
    let summary = first_line(body, 240);

    write_wiki_page_with_opts(
        store,
        embedder,
        config,
        &slug,
        title,
        &page_body,
        "wiki",
        Some("answers"),
        Some(&summary),
        agent,
        WriteWikiOpts {
            op: Some("file_answer".into()),
            prefix: Some("FILE".into()),
            message: Some(format!("filed answer wiki://{slug}")),
            extra_metadata: Some(serde_json::json!({
                "citations": cites_json,
                "filed_as": "answer",
            })),
            extra_payload: Some(serde_json::json!({
                "citation_count": cites.len(),
            })),
            if_match_revision: None,
        },
    )
    .await
}

/// Lint wiki catalog and graph link health.
pub fn lint_wiki(store: &Store) -> Result<LintReport> {
    let mut issues = Vec::new();
    let documents = store.list_documents()?;
    let wiki_docs = store.list_documents_by_layer("wiki")?;
    let index = store.list_wiki_index()?;
    let graph = store.get_graph_view(crate::models::GraphFilter {
        max_nodes: Some(u32::MAX),
        ..Default::default()
    })?;
    let all_edges = store.list_graph_edges()?;
    let node_by_id: HashMap<&str, &crate::models::GraphNode> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut incident_nodes = HashSet::new();
    let mut duplicate_wikilinks: BTreeMap<(&str, &str), Vec<&crate::models::GraphEdge>> =
        BTreeMap::new();
    let mut health = LinkHealthCounts {
        documents: documents.len(),
        wiki_pages: wiki_docs.len(),
        graph_nodes: graph.nodes.len(),
        graph_edges: all_edges.len(),
        ..Default::default()
    };

    for edge in &all_edges {
        let source = node_by_id.get(edge.source_id.as_str()).copied();
        let target = node_by_id.get(edge.target_id.as_str()).copied();
        if source.is_some() {
            incident_nodes.insert(edge.source_id.as_str());
        }
        if target.is_some() {
            incident_nodes.insert(edge.target_id.as_str());
        }
        if edge.rel_type != crate::graph::REL_WIKILINK {
            continue;
        }
        health.wikilinks += 1;
        if source.is_none() || target.is_none() {
            health.broken_wikilinks += 1;
            let missing = match (source.is_none(), target.is_none()) {
                (true, true) => "source and target nodes",
                (true, false) => "source node",
                (false, true) => "target node",
                (false, false) => unreachable!(),
            };
            issues.push(LintIssue {
                code: "broken_wikilink".into(),
                severity: "error".into(),
                message: format!(
                    "wikilink edge {} references missing {}; rebuild or remove this edge",
                    edge.id, missing
                ),
                entity_id: Some(edge.id.clone()),
                source_id: Some(edge.source_id.clone()),
                target_id: Some(edge.target_id.clone()),
                rel_type: Some(edge.rel_type.clone()),
                ..Default::default()
            });
        }
        if edge.source_id == edge.target_id {
            health.self_links += 1;
            let label = source.map(|n| n.label.as_str()).unwrap_or(&edge.source_id);
            issues.push(LintIssue {
                code: "self_link".into(),
                severity: "warn".into(),
                message: format!("'{label}' links to itself; remove the redundant wikilink"),
                entity_id: Some(edge.id.clone()),
                source_id: Some(edge.source_id.clone()),
                target_id: Some(edge.target_id.clone()),
                rel_type: Some(edge.rel_type.clone()),
                ..Default::default()
            });
        }
        duplicate_wikilinks
            .entry((edge.source_id.as_str(), edge.target_id.as_str()))
            .or_default()
            .push(edge);
    }

    for ((source_id, target_id), edges) in duplicate_wikilinks {
        if edges.len() < 2 {
            continue;
        }
        health.duplicate_link_groups += 1;
        health.duplicate_link_occurrences += edges.len() - 1;
        let source_label = node_by_id
            .get(source_id)
            .map(|n| n.label.as_str())
            .unwrap_or(source_id);
        let target_label = node_by_id
            .get(target_id)
            .map(|n| n.label.as_str())
            .unwrap_or(target_id);
        issues.push(LintIssue {
            code: "duplicate_wikilink".into(),
            severity: "warn".into(),
            message: format!(
                "'{source_label}' links to '{target_label}' {} times; keep one occurrence unless repetition is intentional",
                edges.len()
            ),
            entity_id: Some(edges[0].id.clone()),
            source_id: Some(source_id.to_string()),
            target_id: Some(target_id.to_string()),
            rel_type: Some(crate::graph::REL_WIKILINK.into()),
            occurrences: Some(edges.len()),
            ..Default::default()
        });
    }

    if wiki_docs.is_empty() {
        issues.push(LintIssue {
            code: "wiki_empty".into(),
            severity: "info".into(),
            message: "no wiki pages yet".into(),
            ..Default::default()
        });
    }

    // Pages missing from index
    for d in &wiki_docs {
        let in_index = index
            .iter()
            .any(|e| e.page_id.as_deref() == Some(d.id.as_str()));
        if !in_index {
            issues.push(LintIssue {
                code: "missing_index".into(),
                severity: "warn".into(),
                message: format!("wiki page {} not in wiki_index", d.uri),
                entity_id: Some(d.id.clone()),
                ..Default::default()
            });
        }
    }

    // Index entries pointing nowhere
    for e in &index {
        if let Some(pid) = &e.page_id {
            if store.get_document(pid)?.is_none() {
                issues.push(LintIssue {
                    code: "stale_index".into(),
                    severity: "warn".into(),
                    message: format!("index entry {} points to missing page {}", e.slug, pid),
                    entity_id: Some(e.id.clone()),
                    expected_label: Some(e.slug.clone()),
                    ..Default::default()
                });
            }
        }
    }

    // Unresolved stubs in graph (include who links + expected label)
    for n in graph.nodes.iter().filter(|n| n.kind == "stub") {
        if !n.resolved {
            health.unresolved_targets += 1;
            let bl = store
                .backlinks(&n.id)
                .unwrap_or_else(|_| crate::models::GraphView {
                    nodes: vec![],
                    edges: vec![],
                });
            let referenced_by: Vec<String> = bl
                .nodes
                .iter()
                .filter(|src| src.id != n.id)
                .map(|src| {
                    if src.label.is_empty() {
                        src.id.clone()
                    } else {
                        src.label.clone()
                    }
                })
                .collect();
            let refs_note = if referenced_by.is_empty() {
                "no incoming edges".to_string()
            } else {
                format!("referenced by: {}", referenced_by.join(", "))
            };
            issues.push(LintIssue {
                code: "unresolved_stub".into(),
                severity: "info".into(),
                message: format!(
                    "stub node '{}' unresolved ({}); write wiki/document with title or wiki:// slug matching this label",
                    n.label, refs_note
                ),
                entity_id: Some(n.id.clone()),
                expected_label: Some(n.label.clone()),
                referenced_by,
                ..Default::default()
            });
        }
    }

    // Documents with no graph node, or document nodes with no incident edges.
    let node_by_document: HashMap<&str, &crate::models::GraphNode> = graph
        .nodes
        .iter()
        .filter_map(|node| node.document_id.as_deref().map(|id| (id, node)))
        .collect();
    for doc in &documents {
        let node = node_by_document.get(doc.id.as_str()).copied();
        if node
            .map(|n| incident_nodes.contains(n.id.as_str()))
            .unwrap_or(false)
        {
            continue;
        }
        let is_wiki = doc.layer == LAYER_WIKI;
        health.orphan_documents += 1;
        if is_wiki {
            health.orphan_wiki_pages += 1;
        }
        issues.push(LintIssue {
            code: if is_wiki {
                "orphan_wiki_page"
            } else {
                "orphan_document"
            }
            .into(),
            severity: "info".into(),
            message: format!(
                "{} has no incoming or outgoing graph links; add a relevant wikilink or archive it",
                doc.uri
            ),
            entity_id: Some(doc.id.clone()),
            ..Default::default()
        });
    }

    // Raw without any related wiki (soft)
    let raws = store.list_documents_by_layer("raw")?;
    if !raws.is_empty() && wiki_docs.is_empty() {
        issues.push(LintIssue {
            code: "raw_uncompiled".into(),
            severity: "info".into(),
            message: format!(
                "{} raw sources present but no wiki pages; run compile_source",
                raws.len()
            ),
            entity_id: None,
            expected_label: None,
            referenced_by: vec![],
            ..Default::default()
        });
    }

    let healthy = !issues
        .iter()
        .any(|issue| issue.severity == "warn" || issue.severity == "error");
    Ok(LintReport {
        issue_count: issues.len(),
        issues,
        healthy,
        health,
    })
}

/// Render index catalog as markdown (index.md analogue).
pub fn render_index_markdown(store: &Store) -> Result<String> {
    let entries = store.list_wiki_index()?;
    let mut out = String::from("# Wiki index\n\n");
    if entries.is_empty() {
        out.push_str("_empty catalog_\n");
        return Ok(out);
    }
    // group by category
    let mut by_cat: std::collections::BTreeMap<String, Vec<&WikiIndexEntry>> =
        std::collections::BTreeMap::new();
    for e in &entries {
        let cat = e.category.clone().unwrap_or_else(|| "uncategorized".into());
        by_cat.entry(cat).or_default().push(e);
    }
    for (cat, rows) in by_cat {
        out.push_str(&format!("## {cat}\n\n"));
        for e in rows {
            let sum = e.summary.as_deref().unwrap_or("");
            out.push_str(&format!(
                "- [[{}]] (`{}`, {}) - {}\n",
                e.title, e.slug, e.kind, sum
            ));
        }
        out.push('\n');
    }
    Ok(out)
}

fn resolve_doc(store: &Store, id_or_uri: &str) -> Result<Document> {
    let s = id_or_uri.trim();
    if let Some(d) = store.get_document(s)? {
        return Ok(d);
    }
    if let Some(d) = store.find_by_uri(s)? {
        return Ok(d);
    }
    Err(AppError::not_found(format!(
        "document not found: {id_or_uri}"
    )))
}

fn slugify(s: &str) -> String {
    shared_slugify(s, SlugPolicy::WikiPage)
}

fn first_line(content: &str, max: usize) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let mut s: String = line.chars().take(max.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

// silence unused import if CompilePage only used via CompileResult
#[allow(dead_code)]
fn _touch_compile_page(_: &crate::llm::CompilePage) {}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tempfile::tempdir;

    struct FailingEmbedder {
        dims: usize,
    }

    #[derive(Debug, PartialEq)]
    struct ChunkState {
        id: String,
        index: i32,
        content: String,
        embedding: Vec<f32>,
        char_start: i32,
        char_end: i32,
        metadata_json: String,
    }

    #[derive(Debug, PartialEq)]
    struct WikiIndexState {
        id: String,
        title: String,
        kind: String,
        category: Option<String>,
        summary: Option<String>,
        page_id: Option<String>,
        updated_at: chrono::DateTime<Utc>,
    }

    #[async_trait]
    impl EmbeddingProvider for FailingEmbedder {
        async fn embed(&self, _texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
            Err(AppError::embeddings("injected wiki embedding failure"))
        }

        fn dimensions(&self) -> usize {
            self.dims
        }
    }

    fn open_store() -> Store {
        let dir = tempdir().unwrap();
        let path = dir.path().join("schema.duckdb");
        // Keep tempdir alive by leaking path under store open only for test process lifetime.
        // Store owns the path; tempdir drop would remove file while open on some OSes,
        // so convert to owned path inside store (file remains until process ends via open handle).
        let _persist = Box::leak(Box::new(dir));
        Store::open(&path).unwrap()
    }

    #[test]
    fn get_schema_seeds_default_at_canonical_uri() {
        let store = open_store();
        assert!(store.get_schema_document().unwrap().is_none());

        let view = get_schema(&store).unwrap();
        assert_eq!(view.uri, SCHEMA_URI);
        assert_eq!(view.layer, "schema");
        assert_eq!(view.kind, "schema");
        assert!(view.content.contains("Wiki schema"));
        assert!(!view.created);

        let again = store.get_schema_document().unwrap().expect("stored");
        assert_eq!(again.uri, SCHEMA_URI);
        assert_eq!(again.id, view.document_id);
    }

    #[test]
    fn update_schema_roundtrip_stable_id() {
        let store = open_store();
        let first = update_schema(
            &store,
            "# Custom\n\n- rule one\n",
            Some("Agents schema"),
            Some("test-agent"),
        )
        .unwrap();
        assert!(first.created);
        assert_eq!(first.uri, SCHEMA_URI);
        assert_eq!(first.title, "Agents schema");
        assert_eq!(first.layer, "schema");
        assert_eq!(first.kind, "schema");
        assert!(first.content.contains("rule one"));

        let second = update_schema(&store, "# Custom\n\n- rule two\n", None, None).unwrap();
        assert!(!second.created);
        assert_eq!(second.document_id, first.document_id);
        assert_eq!(second.title, SCHEMA_TITLE);
        assert!(second.content.contains("rule two"));

        let got = get_schema(&store).unwrap();
        assert_eq!(got.document_id, first.document_id);
        assert!(got.content.contains("rule two"));
    }

    #[test]
    fn update_schema_rejects_empty_content() {
        let store = open_store();
        let err = update_schema(&store, "   ", None, None).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    fn sample_config(dims: usize) -> Config {
        Config {
            db_path: std::path::PathBuf::from("./rag.duckdb"),
            embedding_provider: crate::config::EmbeddingProviderKind::Mock,
            embedding_base_url: "https://api.openai.com/v1".into(),
            embedding_api_key: String::new(),
            embedding_model: "mock".into(),
            embedding_dims: dims,
            chunk_size: 200,
            chunk_overlap: 20,
            default_top_k: 5,
            ingest_roots: Vec::new(),
            max_context_tokens: 4096,
            max_chunks_per_doc: 3,
            fts_stemmer: "porter".into(),
            default_search_mode: crate::models::SearchMode::Vec,
            llm_base_url: "http://127.0.0.1:11434/v1".into(),
            llm_provider: crate::llm::LlmProviderKind::Ollama,
            llm_model: "llama3.2".into(),
            llm_api_key: "ollama".into(),
            llm_enabled: false,
            llm_timeout_secs: 120,
            llm_max_tokens: 4096,
            maint_max_docs: 50,
            maint_near_dup_threshold: 0.92,
            tool_surface: crate::mcp::ToolSurface::Full,
            http_bind: None,
            wiki_require_if_match: false,
        }
    }

    fn chunk_state(store: &Store, document_id: &str) -> Vec<ChunkState> {
        store
            .list_chunks_for_document(document_id)
            .expect("chunks")
            .into_iter()
            .map(|chunk| ChunkState {
                id: chunk.id,
                index: chunk.chunk_index,
                content: chunk.content,
                embedding: chunk.embedding,
                char_start: chunk.char_start,
                char_end: chunk.char_end,
                metadata_json: chunk.metadata_json,
            })
            .collect()
    }

    fn outgoing_edge_state(
        store: &Store,
        node_id: &str,
    ) -> Vec<(String, String, String, Option<String>)> {
        let mut state = store
            .list_graph_edges()
            .expect("graph edges")
            .into_iter()
            .filter(|edge| edge.source_id == node_id)
            .map(|edge| (edge.id, edge.target_id, edge.rel_type, edge.context))
            .collect::<Vec<_>>();
        state.sort();
        state
    }

    fn wiki_index_state(store: &Store, slug: &str) -> Option<WikiIndexState> {
        store
            .get_wiki_index_by_slug(slug)
            .expect("wiki index")
            .map(|entry| WikiIndexState {
                id: entry.id,
                title: entry.title,
                kind: entry.kind,
                category: entry.category,
                summary: entry.summary,
                page_id: entry.page_id,
                updated_at: entry.updated_at,
            })
    }

    #[tokio::test]
    async fn wiki_crud_write_get_list_update() {
        let store = open_store();
        let dims = 16usize;
        let config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));

        let written = write_wiki_page(
            &store,
            &embedder,
            &config,
            "cats",
            "Cats",
            "Felines are [[Mammals]] #animals",
            "entity",
            Some("biology"),
            Some("About cats"),
            None,
        )
        .await
        .expect("write");
        assert_eq!(written.uri, "wiki://cats");
        assert_eq!(written.slug, "cats");

        let by_slug = get_wiki_page(&store, "cats").expect("get by slug");
        assert_eq!(by_slug.layer, LAYER_WIKI);
        assert_eq!(by_slug.id, written.document_id);
        assert!(by_slug.content.contains("Felines"));

        let by_uri = get_wiki_page(&store, "wiki://cats").expect("get by uri");
        assert_eq!(by_uri.id, written.document_id);

        let by_id = get_wiki_page(&store, &written.document_id).expect("get by id");
        assert_eq!(by_id.title, "Cats");

        let listed = list_wiki_pages(&store).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, written.document_id);

        let updated = update_wiki_page(
            &store,
            &embedder,
            &config,
            "cats",
            Some("Domestic Cats"),
            "Updated body about cats #pets",
            None,
            None,
            Some("updated summary"),
            None,
        )
        .await
        .expect("update");
        assert_eq!(updated.document_id, written.document_id);

        let after = get_wiki_page(&store, "cats").expect("get after update");
        assert_eq!(after.title, "Domestic Cats");
        assert!(after.content.contains("Updated body"));
        assert_eq!(after.layer, LAYER_WIKI);
    }

    #[tokio::test]
    async fn wiki_update_preserves_omitted_placement_state_and_arbitrary_metadata() {
        let store = open_store();
        let dims = 16usize;
        let config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));

        let written = write_wiki_page(
            &store,
            &embedder,
            &config,
            "preserved-page",
            "Preserved page",
            "original body",
            "concept",
            Some("architecture"),
            Some("original summary"),
            None,
        )
        .await
        .expect("create wiki page");
        let original_index = store
            .get_wiki_index_by_slug("preserved-page")
            .expect("read index")
            .expect("index exists");

        let mut persisted = store
            .get_document(&written.document_id)
            .expect("read document")
            .expect("document exists");
        persisted.wing = Some("alpha".into());
        persisted.room = Some("design".into());
        persisted.source_file = Some("/sources/alpha/design.md".into());
        persisted.status = "archived".into();
        persisted.pinned = true;
        persisted.boost = 2.75;
        persisted.metadata_json =
            r#"{"category":"architecture","summary":"original summary","custom":{"retained":true},"rank":7}"#
                .into();
        let prior_revision = persisted.revision;
        persisted.revision = store
            .upsert_document_cas(&persisted, Some(prior_revision))
            .expect("seed placement and state");
        let metadata_before = persisted.metadata_json.clone();

        let updated = update_wiki_page_cas(
            &store,
            &embedder,
            &config,
            "preserved-page",
            Some("Edited title"),
            "edited body",
            None,
            None,
            None,
            None,
            Some(persisted.revision),
        )
        .await
        .expect("update with omitted optional fields");

        let after = store
            .get_document(&updated.document_id)
            .expect("read updated document")
            .expect("updated document exists");
        assert_eq!(after.wing.as_deref(), Some("alpha"));
        assert_eq!(after.room.as_deref(), Some("design"));
        assert_eq!(
            after.source_file.as_deref(),
            Some("/sources/alpha/design.md")
        );
        assert_eq!(after.status, "archived");
        assert!(after.pinned);
        assert_eq!(after.boost, 2.75);
        assert_eq!(after.kind, "concept");
        assert_eq!(after.metadata_json, metadata_before);
        let after_index = store
            .get_wiki_index_by_slug("preserved-page")
            .expect("read updated index")
            .expect("updated index exists");
        assert_eq!(after_index.id, original_index.id);
        assert_eq!(after_index.kind, "concept");
        assert_eq!(after_index.category.as_deref(), Some("architecture"));
        assert_eq!(after_index.summary.as_deref(), Some("original summary"));

        let overlaid = update_wiki_page_cas(
            &store,
            &embedder,
            &config,
            "preserved-page",
            None,
            "edited body again",
            None,
            Some("systems"),
            Some("new summary"),
            None,
            Some(after.revision),
        )
        .await
        .expect("metadata overlay update");
        let overlaid = store
            .get_document(&overlaid.document_id)
            .expect("read overlaid document")
            .expect("overlaid document exists");
        let metadata: serde_json::Value =
            serde_json::from_str(&overlaid.metadata_json).expect("valid metadata object");
        assert_eq!(metadata["custom"]["retained"], true);
        assert_eq!(metadata["rank"], 7);
        assert_eq!(metadata["category"], "systems");
        assert_eq!(metadata["summary"], "new summary");
        assert_eq!(overlaid.wing.as_deref(), Some("alpha"));
        assert!(overlaid.pinned);
    }

    #[tokio::test]
    async fn wiki_require_if_match_blocks_update_without_revision() {
        let store = open_store();
        let dims = 16usize;
        let mut config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));

        let written = write_wiki_page(
            &store,
            &embedder,
            &config,
            "cas-page",
            "CAS Page",
            "first body",
            "wiki",
            None,
            None,
            None,
        )
        .await
        .expect("create without if_match is allowed");
        assert_eq!(written.revision, 1);

        config.wiki_require_if_match = true;
        let err = write_wiki_page(
            &store,
            &embedder,
            &config,
            "cas-page",
            "CAS Page",
            "second body without if_match",
            "wiki",
            None,
            None,
            None,
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("if_match_revision") && msg.contains("RAG_WIKI_REQUIRE_IF_MATCH"),
            "expected require-if-match error, got {msg}"
        );

        let ok = write_wiki_page_with_opts(
            &store,
            &embedder,
            &config,
            "cas-page",
            "CAS Page",
            "second body with if_match",
            "wiki",
            None,
            None,
            None,
            WriteWikiOpts {
                if_match_revision: Some(written.revision),
                ..Default::default()
            },
        )
        .await
        .expect("update with matching revision");
        assert_eq!(ok.revision, 2);
    }

    /// Stale if_match must fail with Conflict and leave body+chunks intact
    /// (CAS before delete_chunks / re-embed).
    #[tokio::test]
    async fn stale_if_match_preserves_body_and_chunks() {
        let store = open_store();
        let dims = 16usize;
        let config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));

        let original_body = "CAS preserve body: unique retrieval text for chunks";
        let written = write_wiki_page(
            &store,
            &embedder,
            &config,
            "cas-preserve",
            "CAS Preserve",
            original_body,
            "wiki",
            None,
            None,
            None,
        )
        .await
        .expect("create");
        assert_eq!(written.revision, 1);

        let chunks_before = store
            .list_chunks_for_document(&written.document_id)
            .expect("list chunks");
        assert!(
            !chunks_before.is_empty(),
            "expected chunks after successful wiki write"
        );
        let chunk_ids_before: Vec<String> = chunks_before.iter().map(|c| c.id.clone()).collect();

        let err = write_wiki_page_with_opts(
            &store,
            &embedder,
            &config,
            "cas-preserve",
            "CAS Preserve",
            "should not replace body or wipe chunks",
            "wiki",
            None,
            None,
            None,
            WriteWikiOpts {
                // Stale: page is at revision 1.
                if_match_revision: Some(0),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, AppError::Conflict(_)),
            "expected Conflict on stale if_match, got {err:?}"
        );

        let doc = store
            .get_document(&written.document_id)
            .expect("get")
            .expect("exists");
        assert_eq!(doc.revision, 1, "revision must not advance on CAS failure");
        assert_eq!(
            doc.content, original_body,
            "body must be preserved on stale if_match"
        );

        let chunks_after = store
            .list_chunks_for_document(&written.document_id)
            .expect("list chunks after conflict");
        assert_eq!(
            chunks_after.len(),
            chunks_before.len(),
            "chunk count must not change on CAS failure"
        );
        let chunk_ids_after: Vec<String> = chunks_after.iter().map(|c| c.id.clone()).collect();
        assert_eq!(
            chunk_ids_after, chunk_ids_before,
            "chunks must not be deleted on stale if_match"
        );
        assert!(
            chunks_after
                .iter()
                .any(|c| c.content.contains("unique retrieval")),
            "chunk text must still reflect original body"
        );
    }

    #[tokio::test]
    async fn raw_layer_immutable_rejects_wiki_overwrite_and_content_update() {
        let store = open_store();
        let dims = 16usize;
        let config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));

        let raw = ingest_raw(
            &store,
            &embedder,
            &config,
            "raw source text".into(),
            Some("Raw Title".into()),
            Some("wiki://stolen-slug".into()),
            None,
            None,
            None,
        )
        .await
        .expect("ingest raw at wiki-like uri");

        let raw_doc = store.get_document(&raw.document_id).unwrap().expect("raw");
        assert_eq!(raw_doc.layer, LAYER_RAW);
        assert!(assert_content_mutable(&raw_doc).is_err());

        // write_wiki_page must not overwrite raw content at same uri.
        let err = write_wiki_page(
            &store,
            &embedder,
            &config,
            "stolen-slug",
            "Wiki Title",
            "trying to overwrite raw",
            "wiki",
            None,
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, AppError::Forbidden(_)),
            "expected Forbidden, got {err}"
        );

        // get_wiki_page refuses raw documents.
        let get_err = get_wiki_page(&store, &raw.document_id).unwrap_err();
        assert!(matches!(get_err, AppError::Forbidden(_)));

        // Re-ingest replace is allowed (does not call assert_content_mutable).
        let again = ingest_raw(
            &store,
            &embedder,
            &config,
            "replaced raw content".into(),
            Some("Raw Title".into()),
            Some("wiki://stolen-slug".into()),
            None,
            None,
            None,
        )
        .await
        .expect("re-ingest replace");
        assert_eq!(again.document_id, raw.document_id);
        let replaced = store.get_document(&raw.document_id).unwrap().unwrap();
        assert_eq!(replaced.content, "replaced raw content");
        assert_eq!(replaced.layer, LAYER_RAW);
    }

    #[test]
    fn update_wiki_page_missing_is_not_found() {
        let store = open_store();
        let err = get_wiki_page(&store, "no-such-page").unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));
    }

    #[tokio::test]
    async fn file_answer_writes_wiki_graph_index_and_ops_log() {
        let store = open_store();
        let dims = 16usize;
        let config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));

        let cites = vec![FileAnswerCitation {
            document_id: Some("doc-src".into()),
            uri: Some("raw://alpha".into()),
            title: Some("Alpha source".into()),
            chunk_id: Some("c1".into()),
            quote: Some("alpha fact".into()),
            note: Some("supports claim".into()),
        }];

        let wr = file_answer(
            &store,
            &embedder,
            &config,
            "Why Alpha Matters",
            "Alpha is important because of [[Beta]] #research",
            None,
            Some(cites),
            Some("agent-test"),
        )
        .await
        .expect("file_answer");

        assert_eq!(wr.slug, "why-alpha-matters");
        assert_eq!(wr.uri, "wiki://why-alpha-matters");
        assert!(!wr.node_id.is_empty());
        assert!(wr.chunk_count >= 1);
        assert!(wr.edge_count >= 1, "wikilink/tag edges expected");

        let doc = store
            .get_document(&wr.document_id)
            .unwrap()
            .expect("wiki doc");
        assert_eq!(doc.layer, "wiki");
        assert_eq!(doc.kind, "wiki");
        assert!(doc.content.contains("Alpha is important"));
        assert!(doc.content.contains("## Citations"));
        assert!(doc.content.contains("Alpha source"));
        let meta: serde_json::Value = serde_json::from_str(&doc.metadata_json).unwrap();
        assert_eq!(meta["category"], "answers");
        assert_eq!(meta["filed_as"], "answer");
        assert!(meta["citations"].as_array().unwrap().len() == 1);

        let idx = store
            .get_wiki_index_by_slug("why-alpha-matters")
            .unwrap()
            .expect("index entry");
        assert_eq!(idx.page_id.as_deref(), Some(wr.document_id.as_str()));
        assert_eq!(idx.category.as_deref(), Some("answers"));

        let ops = store.list_ops_log(20).unwrap();
        let fa = ops
            .iter()
            .find(|e| e.op == "file_answer")
            .expect("ops_log file_answer row");
        assert_eq!(fa.prefix.as_deref(), Some("FILE"));
        assert_eq!(fa.entity_id.as_deref(), Some(wr.document_id.as_str()));
        assert_eq!(fa.agent_name.as_deref(), Some("agent-test"));
    }

    #[tokio::test]
    async fn file_answer_embedding_failure_preserves_document_chunks_graph_and_side_effects() {
        let store = open_store();
        let dims = 16usize;
        let config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));

        let written = file_answer(
            &store,
            &embedder,
            &config,
            "Atomic Answer",
            "Original answer links to [[Old target]].",
            Some("atomic-answer"),
            None,
            Some("agent-test"),
        )
        .await
        .expect("initial file_answer");
        let original_document = store
            .get_document(&written.document_id)
            .unwrap()
            .expect("original document");
        let original_chunks = chunk_state(&store, &written.document_id);
        let original_node = store
            .find_node_by_document_id(&written.document_id)
            .unwrap()
            .expect("original graph node");
        let original_edges = outgoing_edge_state(&store, &original_node.id);
        let original_index = wiki_index_state(&store, "atomic-answer");
        let original_ops_count = store.list_ops_log(100).unwrap().len();

        let failing_embedder: Arc<dyn EmbeddingProvider> = Arc::new(FailingEmbedder { dims });
        let error = file_answer(
            &store,
            &failing_embedder,
            &config,
            "Changed Answer",
            "Changed answer links to [[New target]].",
            Some("atomic-answer"),
            None,
            Some("agent-test"),
        )
        .await
        .expect_err("embedding must fail before persistence");
        assert!(error
            .to_string()
            .contains("injected wiki embedding failure"));

        let persisted_document = store
            .get_document(&written.document_id)
            .unwrap()
            .expect("document after embedding failure");
        assert_eq!(persisted_document.revision, original_document.revision);
        assert_eq!(persisted_document.title, original_document.title);
        assert_eq!(persisted_document.content, original_document.content);
        assert_eq!(chunk_state(&store, &written.document_id), original_chunks);
        let persisted_node = store
            .find_node_by_document_id(&written.document_id)
            .unwrap()
            .expect("graph node after embedding failure");
        assert_eq!(persisted_node.id, original_node.id);
        assert_eq!(persisted_node.label, original_node.label);
        assert_eq!(
            outgoing_edge_state(&store, &persisted_node.id),
            original_edges
        );
        assert!(store.find_nodes_by_label("New target").unwrap().is_empty());
        assert_eq!(wiki_index_state(&store, "atomic-answer"), original_index);
        assert_eq!(store.list_ops_log(100).unwrap().len(), original_ops_count);
    }

    #[tokio::test]
    async fn wiki_graph_fault_rolls_back_document_chunks_graph_and_side_effects() {
        let store = open_store();
        let dims = 16usize;
        let config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));

        let written = write_wiki_page(
            &store,
            &embedder,
            &config,
            "atomic-wiki-update",
            "Original Wiki Title",
            "Original body links to [[Old target]].",
            "wiki",
            Some("tests"),
            Some("original summary"),
            Some("agent-test"),
        )
        .await
        .expect("initial wiki write");
        let original_document = store
            .get_document(&written.document_id)
            .unwrap()
            .expect("original document");
        let original_chunks = chunk_state(&store, &written.document_id);
        let original_node = store
            .find_node_by_document_id(&written.document_id)
            .unwrap()
            .expect("original graph node");
        let original_edges = outgoing_edge_state(&store, &original_node.id);
        let original_index = wiki_index_state(&store, "atomic-wiki-update");
        let original_ops_count = store.list_ops_log(100).unwrap().len();

        // Test-only fault injector: the graph rebuild inserts the first edge,
        // then the duplicate violates this constraint after doc/chunk writes.
        store
            .lock()
            .unwrap()
            .execute(
                "CREATE UNIQUE INDEX wiki_rollback_edge_key ON graph_edges(source_id, target_id, rel_type)",
                [],
            )
            .expect("fault index");

        let error = update_wiki_page_cas(
            &store,
            &embedder,
            &config,
            "atomic-wiki-update",
            Some("Changed Wiki Title"),
            "Changed body [[Repeated target]] and [[Repeated target]].",
            None,
            None,
            Some("changed summary"),
            Some("agent-test"),
            Some(written.revision),
        )
        .await
        .expect_err("duplicate graph edge must fail the wiki transaction");
        assert!(
            error.to_string().contains("duplicate")
                || error.to_string().contains("constraint")
                || error.to_string().contains("unique"),
            "unexpected fault: {error}"
        );

        let persisted_document = store
            .get_document(&written.document_id)
            .unwrap()
            .expect("document after graph rollback");
        assert_eq!(persisted_document.revision, original_document.revision);
        assert_eq!(persisted_document.title, original_document.title);
        assert_eq!(persisted_document.content, original_document.content);
        assert_eq!(chunk_state(&store, &written.document_id), original_chunks);
        let persisted_node = store
            .find_node_by_document_id(&written.document_id)
            .unwrap()
            .expect("graph node after rollback");
        assert_eq!(persisted_node.id, original_node.id);
        assert_eq!(persisted_node.label, original_node.label);
        assert_eq!(
            outgoing_edge_state(&store, &persisted_node.id),
            original_edges
        );
        assert!(store
            .find_nodes_by_label("Repeated target")
            .unwrap()
            .is_empty());
        assert!(store
            .list_document_revisions(&written.document_id)
            .unwrap()
            .is_empty());
        assert_eq!(
            wiki_index_state(&store, "atomic-wiki-update"),
            original_index
        );
        assert_eq!(store.list_ops_log(100).unwrap().len(), original_ops_count);
    }

    #[tokio::test]
    async fn late_catalog_fault_rolls_back_new_wiki_page_and_audit() {
        let store = open_store();
        let dims = 16usize;
        let config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));
        let ops_before = store.list_ops_log(100).unwrap().len();

        let error = write_wiki_page_with_opts(
            &store,
            &embedder,
            &config,
            "late-create-rollback",
            "Late create rollback",
            "New body links to [[Never committed]].",
            "wiki",
            Some("tests"),
            Some("must roll back"),
            Some("agent-test"),
            WriteWikiOpts {
                op: Some(crate::db::store::TEST_FAIL_WIKI_WRITE_AFTER_INDEX_OP.into()),
                ..Default::default()
            },
        )
        .await
        .expect_err("late catalog fault must roll back the complete wiki write");

        assert!(error
            .to_string()
            .contains("injected wiki write failure after catalog update"));
        assert!(store
            .find_by_uri("wiki://late-create-rollback")
            .unwrap()
            .is_none());
        assert!(wiki_index_state(&store, "late-create-rollback").is_none());
        assert!(store
            .find_nodes_by_label("Never committed")
            .unwrap()
            .is_empty());
        assert_eq!(store.list_ops_log(100).unwrap().len(), ops_before);
    }

    #[tokio::test]
    async fn late_catalog_fault_rolls_back_existing_wiki_revision_and_derived_state() {
        let store = open_store();
        let dims = 16usize;
        let config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));
        let written = write_wiki_page(
            &store,
            &embedder,
            &config,
            "late-update-rollback",
            "Original title",
            "Original body links to [[Old target]].",
            "concept",
            Some("original"),
            Some("original summary"),
            Some("agent-test"),
        )
        .await
        .expect("initial wiki write");
        let original_document = store
            .get_document(&written.document_id)
            .unwrap()
            .expect("original document");
        let original_chunks = chunk_state(&store, &written.document_id);
        let original_node = store
            .find_node_by_document_id(&written.document_id)
            .unwrap()
            .expect("original node");
        let original_edges = outgoing_edge_state(&store, &original_node.id);
        let original_index = wiki_index_state(&store, "late-update-rollback");
        let original_ops_count = store.list_ops_log(100).unwrap().len();

        let error = write_wiki_page_with_opts(
            &store,
            &embedder,
            &config,
            "late-update-rollback",
            "Changed title",
            "Changed body links to [[New target]].",
            "concept",
            Some("changed"),
            Some("changed summary"),
            Some("agent-test"),
            WriteWikiOpts {
                op: Some(crate::db::store::TEST_FAIL_WIKI_WRITE_AFTER_INDEX_OP.into()),
                if_match_revision: Some(written.revision),
                ..Default::default()
            },
        )
        .await
        .expect_err("late catalog fault must roll back the existing wiki update");

        assert!(error
            .to_string()
            .contains("injected wiki write failure after catalog update"));
        let persisted = store
            .get_document(&written.document_id)
            .unwrap()
            .expect("document survives rollback");
        assert_eq!(persisted.revision, original_document.revision);
        assert_eq!(persisted.title, original_document.title);
        assert_eq!(persisted.content, original_document.content);
        assert_eq!(persisted.metadata_json, original_document.metadata_json);
        assert_eq!(chunk_state(&store, &written.document_id), original_chunks);
        let persisted_node = store
            .find_node_by_document_id(&written.document_id)
            .unwrap()
            .expect("node survives rollback");
        assert_eq!(persisted_node.id, original_node.id);
        assert_eq!(persisted_node.label, original_node.label);
        assert_eq!(
            outgoing_edge_state(&store, &persisted_node.id),
            original_edges
        );
        assert!(store.find_nodes_by_label("New target").unwrap().is_empty());
        assert_eq!(
            wiki_index_state(&store, "late-update-rollback"),
            original_index
        );
        assert!(store
            .list_document_revisions(&written.document_id)
            .unwrap()
            .is_empty());
        assert_eq!(store.list_ops_log(100).unwrap().len(), original_ops_count);
    }

    #[tokio::test]
    async fn find_stale_wiki_via_metadata_and_graph() {
        use chrono::Duration;

        let store = open_store();
        let dims = 16usize;
        let config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));

        let older = Utc::now() - Duration::days(5);
        let newer = Utc::now();

        let raw = ingest_raw(
            &store,
            &embedder,
            &config,
            "version one of the source".into(),
            Some("Alpha".into()),
            Some("raw://alpha".into()),
            None,
            None,
            None,
        )
        .await
        .expect("ingest raw");

        // Pin raw to older timestamp so wiki (now) is fresh.
        let mut raw_doc = store.get_document(&raw.document_id).unwrap().unwrap();
        raw_doc.updated_at = older;
        store.upsert_document(&raw_doc).unwrap();

        let wiki = write_wiki_page_with_opts(
            &store,
            &embedder,
            &config,
            "alpha-summary",
            "Alpha summary",
            "Summary of Alpha.\nsource: raw://alpha\n",
            "source_summary",
            Some("sources"),
            Some("Alpha summary"),
            None,
            WriteWikiOpts {
                op: Some("wiki_write".into()),
                prefix: Some("WIKI".into()),
                message: None,
                extra_metadata: Some(serde_json::json!({
                    "source_id": raw.document_id,
                    "source_uri": "raw://alpha",
                })),
                extra_payload: None,
                if_match_revision: None,
            },
        )
        .await
        .expect("write wiki");

        let raw_node = store
            .find_node_by_document_id(&raw.document_id)
            .unwrap()
            .expect("raw node");
        let wiki_node = store
            .find_node_by_document_id(&wiki.document_id)
            .unwrap()
            .expect("wiki node");
        store
            .link_nodes(&raw_node.id, &wiki_node.id, "related", 1.0)
            .unwrap();

        let empty = find_stale_wiki(&store).unwrap();
        assert!(
            empty.is_empty(),
            "expected no stale while wiki newer than raw, got {empty:?}"
        );

        // Bump raw past wiki without re-ingest (keeps related edge).
        let mut raw_doc = store.get_document(&raw.document_id).unwrap().unwrap();
        raw_doc.updated_at = newer;
        raw_doc.content = "version two of the source - changed".into();
        store.upsert_document(&raw_doc).unwrap();

        let mut wiki_doc = store.get_document(&wiki.document_id).unwrap().unwrap();
        wiki_doc.updated_at = older;
        store.upsert_document(&wiki_doc).unwrap();

        let stale = find_stale_wiki(&store).unwrap();
        assert_eq!(stale.len(), 1, "expected one stale pair, got {stale:?}");
        assert_eq!(stale[0].wiki_id, wiki.document_id);
        assert_eq!(stale[0].raw_id, raw.document_id);
        assert!(
            stale[0].link_kind == "metadata"
                || stale[0].link_kind == "content_source"
                || stale[0].link_kind == "graph_related",
            "link_kind={}",
            stale[0].link_kind
        );

        let report =
            refresh_stale_wiki(&store, &embedder, &config, None, true, None, Some("tester"))
                .await
                .expect("refresh dry_run");
        assert_eq!(report.stale_count, 1);
        assert_eq!(report.raw_sources.len(), 1);
        assert_eq!(report.raw_sources[0].raw_id, raw.document_id);
        assert!(!report.applied);
        assert!(report.dry_run);
        assert!(report.recompiled.is_empty());

        let ops = store.list_ops_log(10).unwrap();
        assert!(
            ops.iter().any(|e| e.op == "refresh_stale_wiki"),
            "ops_log should record refresh_stale_wiki"
        );
    }

    #[tokio::test]
    async fn apply_consolidate_proposal_writes_wiki_graph_index_and_ops() {
        let store = open_store();
        let dims = 16usize;
        let config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));

        let a = ingest_raw(
            &store,
            &embedder,
            &config,
            "Alpha talks about hybrid search and BM25.".into(),
            Some("Alpha note".into()),
            Some("raw://alpha-note".into()),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let b = ingest_raw(
            &store,
            &embedder,
            &config,
            "Beta covers vector embeddings and RRF fusion.".into(),
            Some("Beta note".into()),
            Some("raw://beta-note".into()),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let doc_a = store.get_document(&a.document_id).unwrap().unwrap();
        let doc_b = store.get_document(&b.document_id).unwrap().unwrap();

        let proposal = ConsolidateProposal {
            slug: "hybrid-search".into(),
            title: "Hybrid Search".into(),
            kind: "concept".into(),
            category: Some("retrieval".into()),
            content: "Hybrid search fuses BM25 and vectors via [[RRF]] #search\n\n## Sources\n- Alpha note\n- Beta note\n".into(),
            summary: Some("BM25 + vec fusion".into()),
            suggested_links: vec!["RRF".into(), "[[FTS]]".into()],
            notes: Some("merged alpha+beta".into()),
        };

        let wr = apply_consolidate_proposal(
            &store,
            &embedder,
            &config,
            &proposal,
            &[doc_a, doc_b],
            Some("agent-c"),
        )
        .await
        .expect("apply_consolidate_proposal");

        assert_eq!(wr.uri, "wiki://hybrid-search");
        assert_eq!(wr.slug, "hybrid-search");
        assert!(wr.chunk_count >= 1);
        assert!(!wr.node_id.is_empty());

        let wiki = get_wiki_page(&store, "hybrid-search").unwrap();
        assert_eq!(wiki.layer, LAYER_WIKI);
        assert_eq!(wiki.kind, "concept");
        assert!(wiki.content.contains("BM25"));
        let meta: serde_json::Value = serde_json::from_str(&wiki.metadata_json).unwrap();
        assert_eq!(meta["filed_as"], "consolidate");
        assert_eq!(meta["category"], "retrieval");
        let from = meta["source_ids"].as_array().expect("source_ids");
        assert_eq!(from.len(), 2);

        let idx = store
            .get_wiki_index_by_slug("hybrid-search")
            .unwrap()
            .expect("index");
        assert_eq!(idx.page_id.as_deref(), Some(wr.document_id.as_str()));
        assert_eq!(idx.category.as_deref(), Some("retrieval"));

        // Graph related edges from both sources.
        let wiki_node = store
            .find_node_by_document_id(&wr.document_id)
            .unwrap()
            .expect("wiki node");
        for src_id in [&a.document_id, &b.document_id] {
            let src_node = store
                .find_node_by_document_id(src_id)
                .unwrap()
                .expect("src node");
            let view = store.neighbors(&src_node.id, 1, 50).unwrap();
            let linked = view.edges.iter().any(|e| {
                e.rel_type == "related"
                    && ((e.source_id == src_node.id && e.target_id == wiki_node.id)
                        || (e.target_id == src_node.id && e.source_id == wiki_node.id))
            });
            assert!(linked, "expected related edge from {src_id} to wiki");
        }

        let ops = store.list_ops_log(20).unwrap();
        assert!(
            ops.iter().any(|e| e.op == "consolidate_write"),
            "ops_log should record consolidate_write"
        );
    }

    #[tokio::test]
    async fn consolidate_rejects_empty_ids() {
        let store = open_store();
        let dims = 16usize;
        let config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));
        let llm = ChatClient::with_limits("http://127.0.0.1:9/v1", "x", "m", 1, 16).unwrap();
        let err = consolidate(
            &store,
            &embedder,
            &config,
            &llm,
            &[],
            false,
            ConsolidateOpts::default(),
            None,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("at least one"));
    }

    #[tokio::test]
    async fn refresh_stale_wiki_apply_without_llm_lists_only() {
        use chrono::Duration;

        let store = open_store();
        let dims = 16usize;
        let config = sample_config(dims);
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::embeddings::MockEmbedder::new(dims));

        let older = Utc::now() - Duration::days(3);
        let newer = Utc::now();

        let raw = ingest_raw(
            &store,
            &embedder,
            &config,
            "body".into(),
            Some("B".into()),
            Some("raw://b".into()),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let wiki = write_wiki_page_with_opts(
            &store,
            &embedder,
            &config,
            "b-sum",
            "B sum",
            "source: raw://b",
            "source_summary",
            None,
            None,
            None,
            WriteWikiOpts {
                extra_metadata: Some(serde_json::json!({
                    "source_uri": "raw://b",
                    "source_id": raw.document_id,
                })),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let mut wiki_doc = store.get_document(&wiki.document_id).unwrap().unwrap();
        wiki_doc.updated_at = older;
        store.upsert_document(&wiki_doc).unwrap();

        let mut raw_doc = store.get_document(&raw.document_id).unwrap().unwrap();
        raw_doc.updated_at = newer;
        store.upsert_document(&raw_doc).unwrap();

        let report = refresh_stale_wiki(&store, &embedder, &config, None, false, Some(10), None)
            .await
            .unwrap();
        assert_eq!(report.stale_count, 1);
        assert!(!report.applied);
        assert!(report
            .notes
            .as_deref()
            .unwrap_or("")
            .contains("no ChatClient"));
    }
}
