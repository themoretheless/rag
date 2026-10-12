//! Karpathy-style compiled wiki layer: pages, index, schema, lint, compile.
//!
//! Wiki pages are ordinary [`Document`] rows with `layer = "wiki"`.
//! Raw sources (`layer = "raw"`) are **immutable for content updates**;
//! only re-ingest replace (e.g. [`ingest_raw`]) may overwrite raw content.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;
use crate::db::store::DocumentDerivedWrite;
use crate::db::Store;
use crate::document_indexer::DocumentIndexer;
use crate::embeddings::EmbeddingProvider;
use crate::error::{AppError, Result};
use crate::llm::{CompileResult, ConsolidateProposal};
use crate::models::{Document, Layer, OpsLogEntry, WikiIndexEntry};
use crate::util::{content_hash, slugify as shared_slugify, SlugPolicy};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// Document layer for immutable raw sources.
pub const LAYER_RAW: &str = Layer::Raw.as_str();
/// Document layer for compiled wiki pages.
pub const LAYER_WIKI: &str = Layer::Wiki.as_str();
/// Document layer for the agent schema conventions file.
pub const LAYER_SCHEMA: &str = Layer::Schema.as_str();

/// Stable URI for the agent conventions schema document.
pub const SCHEMA_URI: &str = "schema://agents";

/// Default title when creating or updating the schema document.
pub const SCHEMA_TITLE: &str = "Wiki schema";

/// Exact source snapshot used for a generated page. The hash covers the full
/// source body read from the store, before the LLM prompt applies its size cap.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WikiSourceVersion {
    document_id: String,
    uri: String,
    content_hash: String,
}

impl WikiSourceVersion {
    fn from_document(document: &Document) -> Self {
        Self {
            document_id: document.id.clone(),
            uri: document.uri.clone(),
            content_hash: content_hash(&document.content),
        }
    }
}

/// A complete snapshot replaces legacy dependency discovery. Reject the whole
/// array when any entry is malformed, so partial provenance cannot hide a parent.
fn wiki_source_versions(wiki: &Document) -> Option<Vec<WikiSourceVersion>> {
    let versions: Vec<WikiSourceVersion> =
        serde_json::from_value(wiki.metadata_field("source_versions")?).ok()?;
    versions
        .iter()
        .all(|version| {
            (!version.document_id.trim().is_empty() || !version.uri.trim().is_empty())
                && version.content_hash.len() == 64
                && version
                    .content_hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .then_some(versions)
}

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
    Layer::Raw.is(layer)
}

/// Returns true when `layer` is a compiled wiki page.
#[inline]
pub fn is_wiki_layer(layer: &str) -> bool {
    Layer::Wiki.is(layer)
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
            kind: doc.kind.to_string(),
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

crate::models::finding_codes! {
    /// `lint_wiki` finding codes.
    LintCode {
        BrokenWikilink => "broken_wikilink",
        DuplicateWikilink => "duplicate_wikilink",
        MissingIndex => "missing_index",
        OrphanDocument => "orphan_document",
        OrphanWikiPage => "orphan_wiki_page",
        RawUncompiled => "raw_uncompiled",
        SelfLink => "self_link",
        StaleIndex => "stale_index",
        UnresolvedStub => "unresolved_stub",
        WikiEmpty => "wiki_empty",
    }
}

/// Lint finding.
#[derive(Debug, Clone, Serialize, Default)]
pub struct LintIssue {
    pub code: LintCode,
    pub severity: crate::models::IssueSeverity,
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
    /// Raw documents without a related wiki parent (compile debt).
    #[serde(default)]
    pub uncompiled_raw_count: usize,
    /// Sample size included in `raw_uncompiled` / `uncompiled_raw_sample` issues.
    #[serde(default)]
    pub uncompiled_raw_sample_count: usize,
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

mod schema;
mod write;
mod compile;
mod consolidate;
mod stale;
mod file_answer;
mod lint;
mod index;
mod put;

pub use self::schema::*;
pub use self::write::*;
pub use self::compile::*;
pub use self::consolidate::*;
pub use self::stale::*;
pub use self::file_answer::*;
pub use self::lint::*;
use self::index::*;
pub use self::put::*;

#[cfg(test)]
mod tests;
