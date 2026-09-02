//! Domain types for documents, chunks, search results, graph, and store stats.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::AppError;

fn default_layer() -> String {
    "raw".into()
}

fn default_kind() -> String {
    "document".into()
}

fn default_doc_status() -> String {
    "active".into()
}

fn default_boost() -> f64 {
    1.0
}

fn default_revision() -> i64 {
    1
}

fn default_false() -> bool {
    false
}

fn default_kg_status() -> String {
    "active".into()
}

fn default_empty_object_json() -> String {
    "{}".into()
}

/// Search backend selection for hybrid / lexical / vector retrieval.
///
/// Wire names: `lex` | `vec` | `hybrid` (see env `RAG_DEFAULT_SEARCH_MODE` and search tool `mode`).
///
/// # Extending
///
/// Add a variant here, then update every exhaustive `SearchMode` match (compiler-guided):
/// `as_str`, `parse`, [`Self::needs_embedding`], [`Self::needs_query_text`], and
/// `db::search::search` (prefer a new `run_*` free fn; keep the match a pure dispatch).
/// Do not add a `_` arm on those matches so new modes fail to compile until handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Full-text / BM25 (or term-frequency fallback).
    Lex,
    /// Dense vector cosine similarity.
    #[default]
    Vec,
    /// Reciprocal rank fusion of lex + vec.
    Hybrid,
}

impl SearchMode {
    /// Wire name used in env and tool parameters.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vec => "vec",
            Self::Lex => "lex",
            Self::Hybrid => "hybrid",
        }
    }

    /// Parse tool/env wire name: `vec` | `lex` | `hybrid` (case-insensitive).
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "vec" => Ok(Self::Vec),
            "lex" => Ok(Self::Lex),
            "hybrid" => Ok(Self::Hybrid),
            other => Err(format!(
                "invalid search mode '{other}': expected 'vec', 'lex', or 'hybrid'"
            )),
        }
    }

    /// Whether this mode needs a dense query embedding before ranking.
    ///
    /// Use for MCP/HTTP prep (embed only when true). Keep exhaustive when extending.
    pub fn needs_embedding(self) -> bool {
        match self {
            Self::Vec | Self::Hybrid => true,
            Self::Lex => false,
        }
    }

    /// Whether this mode needs free-text query input for ranking.
    ///
    /// Use for MCP/HTTP prep (require `query_text` when true). Keep exhaustive when extending.
    pub fn needs_query_text(self) -> bool {
        match self {
            Self::Lex | Self::Hybrid => true,
            Self::Vec => false,
        }
    }
}

/// A stored document (full text + metadata).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub uri: String,
    pub title: String,
    pub content: String,
    pub metadata_json: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Hierarchical scope (MemPalace-style wing).
    #[serde(default)]
    pub wing: Option<String>,
    /// Hierarchical scope (MemPalace-style room).
    #[serde(default)]
    pub room: Option<String>,
    /// Origin path or source identifier when known.
    #[serde(default)]
    pub source_file: Option<String>,
    /// `raw` | `wiki` | `diary` | `schema` | `index` | `log`
    #[serde(default = "default_layer")]
    pub layer: String,
    /// `document` | `wiki` | `entity` | `concept` | `source_summary` | `diary` | `schema`
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Content fingerprint for idempotent ingest / dedupe.
    #[serde(default)]
    pub content_hash: Option<String>,
    /// Lifecycle: `active` | `draft` | `consolidated` | `archived` | `tombstone`.
    #[serde(default = "default_doc_status")]
    pub status: String,
    /// Surface first in lists / wake_up when true.
    #[serde(default = "default_false")]
    pub pinned: bool,
    /// Manual relevance multiplier (default 1.0).
    #[serde(default = "default_boost")]
    pub boost: f64,
    /// Monotonic optimistic-concurrency counter (starts at 1; +1 each successful write).
    #[serde(default = "default_revision")]
    pub revision: i64,
}

impl Document {
    /// Weak entity-tag for multi-LLM writers: `W/"<revision>"`.
    pub fn etag(&self) -> String {
        format_document_etag(self.revision)
    }
}

/// Lean immutable revision row for timelines. Full bodies are deliberately
/// excluded and loaded one-at-a-time through the revision snapshot endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocumentRevisionSummary {
    pub document_id: String,
    pub uri: String,
    pub title: String,
    #[serde(default)]
    pub wing: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
    pub layer: String,
    pub kind: String,
    pub status: String,
    pub updated_at: String,
    pub superseded_at: String,
    pub revision: i64,
    pub content_chars: u64,
    pub content_lines: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentRevisionPage {
    pub items: Vec<DocumentRevisionSummary>,
    pub total: u64,
}

/// Format a document etag from revision (`W/"3"`).
pub fn format_document_etag(revision: i64) -> String {
    format!("W/\"{revision}\"")
}

/// Parse etag / bare revision from tool params (`W/"3"`, `"3"`, `3`, `r3`).
pub fn parse_document_etag(raw: &str) -> Option<i64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let s = s
        .strip_prefix("W/")
        .or_else(|| s.strip_prefix("w/"))
        .unwrap_or(s)
        .trim();
    let s = s.trim_matches('"').trim();
    let s = s
        .strip_prefix('r')
        .or_else(|| s.strip_prefix('R'))
        .unwrap_or(s);
    s.parse::<i64>().ok().filter(|n| *n >= 0)
}

/// Resolve `if_match_revision` from optional revision and/or etag (revision wins).
///
/// Shared by HTTP `PUT /v1/wiki` and MCP `write_wiki_page` so CAS policy has one owner.
pub fn resolve_if_match(
    revision: Option<i64>,
    etag: Option<&str>,
) -> Result<Option<i64>, AppError> {
    if let Some(r) = revision {
        if r < 0 {
            return Err(AppError::config("if_match_revision must be >= 0"));
        }
        return Ok(Some(r));
    }
    if let Some(raw) = etag.map(str::trim).filter(|s| !s.is_empty()) {
        let parsed = parse_document_etag(raw).ok_or_else(|| {
            AppError::config(format!(
                "invalid if_match_etag '{raw}': expected W/\"n\", \"n\", or n"
            ))
        })?;
        return Ok(Some(parsed));
    }
    Ok(None)
}

impl Default for Document {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            id: String::new(),
            uri: String::new(),
            title: String::new(),
            content: String::new(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            wing: None,
            room: None,
            source_file: None,
            layer: default_layer(),
            kind: default_kind(),
            content_hash: None,
            status: default_doc_status(),
            pinned: false,
            boost: default_boost(),
            revision: default_revision(),
        }
    }
}

/// Optional filters for document listing (wing / room / status / layer / kind).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    /// When set, only documents with this status (`active`, `archived`, …).
    /// Takes precedence over [`Self::include_archived`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    /// When `false` / omitted and `status` is unset, exclude `archived` and `tombstone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Partial document metadata update (`update_document_meta` / placement / rank).
///
/// `None` fields are left unchanged. For nullable columns (`wing`, `room`,
/// `source_file`), pass `Some("")` to clear (store NULL).
///
/// When `content` is set and differs from the stored body, the store updates
/// text + content_hash and reports `content_changed`; the caller must re-chunk
/// and re-embed. Meta-only updates never touch chunks/embeddings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentMetaUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_json: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boost: Option<f64>,
    /// Optional body replace. Store does not re-embed; server does when content changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Alias kept for placement-only call sites (`refile`-style updates).
pub type PlacementUpdate = DocumentMetaUpdate;

/// Result of applying [`DocumentMetaUpdate`] at the store layer (no re-embed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetaApplyResult {
    pub document: Document,
    /// True when body text was replaced (caller should re-chunk / re-embed).
    pub content_changed: bool,
    /// True when `title` field was written.
    pub title_changed: bool,
}

/// One wing with document count (for `list_wings`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WingCount {
    pub wing: String,
    pub document_count: u64,
}

/// Stable project scope identifier. Persisted as the legacy `wing` value until
/// the v2 wire format removes that compatibility alias.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn parse(value: impl Into<String>) -> Result<Self, AppError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.len() > 128 {
            return Err(AppError::config("project_id must contain 1..=128 bytes"));
        }
        if trimmed.chars().any(char::is_control) {
            return Err(AppError::config(
                "project_id must not contain control characters",
            ));
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Project catalog item exposed to native and HTTP clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub project_id: ProjectId,
    pub document_count: u64,
    pub rooms: Vec<TaxonomyRoom>,
}

/// One room (optionally under a wing) with document count (for `list_rooms`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomCount {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wing: Option<String>,
    pub room: String,
    pub document_count: u64,
}

/// Room node inside a taxonomy wing tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxonomyRoom {
    pub room: String,
    pub document_count: u64,
}

/// Wing node with nested rooms and counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxonomyWing {
    pub wing: String,
    pub document_count: u64,
    pub rooms: Vec<TaxonomyRoom>,
}

/// Full palace taxonomy: wings → rooms with document counts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Taxonomy {
    pub wings: Vec<TaxonomyWing>,
    /// Documents with no wing set.
    pub unscoped_count: u64,
    pub total_documents: u64,
}

/// One document matched by [`DuplicateCheckResult`] (`content_hash` and/or `uri`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateMatch {
    pub id: String,
    pub uri: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    /// Why this row matched: `content_hash`, `uri`, or `content_hash+uri`.
    pub match_reason: String,
}

/// Result of `check_duplicate` (content-hash / uri idempotency probe).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DuplicateCheckResult {
    /// True when at least one document matches the hash and/or uri probe.
    pub is_duplicate: bool,
    /// Fingerprint used for the hash probe (from `content` or provided hash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// Documents that matched (deduped by id).
    #[serde(default)]
    pub matches: Vec<DuplicateMatch>,
}

/// A text chunk with its embedding vector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub content: String,
    pub embedding: Vec<f32>,
    pub char_start: i32,
    pub char_end: i32,
    /// Additive per-chunk metadata (currently Markdown `heading_path` / `section`).
    #[serde(default = "default_empty_object_json")]
    pub metadata_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchContextChunk {
    pub chunk_id: String,
    pub chunk_index: i32,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_path: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchExplanation {
    #[serde(default)]
    pub reasons: Vec<String>,
    pub retrieval_ms: f64,
    pub postprocess_ms: f64,
    pub total_ms: f64,
    /// Query embedding time (vec / hybrid only; measured by the adapter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed_ms: Option<f64>,
    /// Dense candidate retrieval time (vec / hybrid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vec_ms: Option<f64>,
    /// Lexical candidate retrieval time (lex / hybrid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lex_ms: Option<f64>,
    /// RRF constant used for fusion (hybrid only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rrf_k: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deduplication: Option<String>,
}

/// One hit from semantic / hybrid search.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchHit {
    pub chunk_id: String,
    pub document_id: String,
    pub document_title: String,
    pub document_uri: String,
    pub chunk_index: i32,
    pub content: String,
    /// Primary ranking score (cosine for vec; fused for hybrid; lex for lex-only).
    pub score: f32,
    /// Dense vector cosine when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_vec: Option<f32>,
    /// Lexical / BM25 (or TF fallback) score when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_lex: Option<f32>,
    /// Reciprocal rank fusion score when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_rrf: Option<f32>,
    /// 1-based rank in the dense candidate list before fusion (vec / hybrid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_vec: Option<u32>,
    /// 1-based rank in the lexical candidate list before fusion (lex / hybrid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_lex: Option<u32>,
    /// Optional short excerpt for citations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Character offset of the hit span in the source document (start).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub char_start: Option<i32>,
    /// Character offset of the hit span in the source document (end).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub char_end: Option<i32>,
    /// Markdown heading ancestry for this chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_path: Option<Vec<String>>,
    /// Leaf Markdown heading for this chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// Opt-in neighboring or same-section chunks, kept in source order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Vec<SearchContextChunk>>,
    /// Ranking factors and stage timings for audit/debugging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<SearchExplanation>,
}

/// Lean wiki catalog row for list/sidebar surfaces (`GET /v1/wiki`, UI).
///
/// ISP: deliberately **no** `content` / body / `metadata_json`. Full page text
/// belongs on [`Document`] via `get_wiki_page` / document get, never on list rows.
/// `summary` is optional metadata blurb only (not the page body).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiPageListItem {
    pub id: String,
    pub uri: String,
    pub slug: String,
    pub title: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub revision: i64,
    pub etag: String,
    pub updated_at: String,
}

/// Result of an ingest operation (includes graph node/edge summary).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestResult {
    pub document_id: String,
    pub chunk_count: usize,
    /// Graph node id for this document (empty when graph is not yet wired).
    #[serde(default)]
    pub node_id: String,
    /// Edges written for this document during ingest.
    #[serde(default)]
    pub edge_count: usize,
    /// Content fingerprint of the stored body (blake3 hex).
    #[serde(default)]
    pub content_hash: String,
    /// `inserted` | `updated` | `unchanged` (idempotent raw re-register).
    #[serde(default)]
    pub op: String,
    /// Document revision after this op (0 when unchanged without load).
    #[serde(default)]
    pub revision: i64,
    /// Weak etag `W/"<revision>"` after this op.
    #[serde(default)]
    pub etag: String,
}

/// Store-level statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    pub document_count: u64,
    pub chunk_count: u64,
    pub node_count: u64,
    pub edge_count: u64,
    pub db_path: String,
}

/// Result of `vacuum_store`: DuckDB `CHECKPOINT` plus optional main-file size stats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VacuumStoreReport {
    pub db_path: String,
    /// Main database file size in bytes before `CHECKPOINT` (absent if unreadable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_before: Option<u64>,
    /// Main database file size in bytes after `CHECKPOINT` (absent if unreadable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_after: Option<u64>,
    /// `bytes_after - bytes_before` when both are known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_delta: Option<i64>,
    /// True when `CHECKPOINT` completed without error.
    pub checkpointed: bool,
}

/// Extended health snapshot for the `status` MCP tool (MemPalace + vision §5.5).
///
/// Layer health (not only undifferentiated row counts): raw/wiki counts,
/// index coverage, uncompiled raw debt, embedding manifest match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    /// Storage backend kind (`duckdb` today).
    pub backend: String,
    #[serde(default)]
    pub storage_capabilities: Vec<String>,
    pub schema_version: i32,
    pub fts_ready: bool,
    pub document_count: u64,
    pub chunk_count: u64,
    pub node_count: u64,
    pub edge_count: u64,
    /// Documents with `layer=raw`.
    #[serde(default)]
    pub raw_count: u64,
    /// Documents with `layer=wiki`.
    #[serde(default)]
    pub wiki_count: u64,
    /// Rows in `wiki_index`.
    #[serde(default)]
    pub index_entry_count: u64,
    /// Wiki pages present in `wiki_index` / `wiki_count` (0 when no wiki).
    #[serde(default)]
    pub index_coverage: f64,
    /// Raw sources still treated as compile debt (raw without wiki corpus coverage).
    /// Currently equals `raw_count` when `wiki_count == 0`, else raw docs not yet
    /// referenced by any wiki graph edge (soft); falls back to `raw_count` if
    /// graph walk is empty.
    #[serde(default)]
    pub uncompiled_raw_count: u64,
    /// Live embedding identity matches stored `embedding_manifest`.
    /// Persisted canonical fingerprints are authoritative; legacy rows without
    /// one are unverifiable and remain fail-closed until a complete re-embed.
    #[serde(default)]
    pub embedding_manifest_match: bool,
    /// Configured embedding provider name.
    #[serde(default)]
    pub embed_provider: String,
    /// Configured embedding model id.
    #[serde(default)]
    pub embed_model: String,
    /// Distinct non-empty wings with document counts.
    pub wings: Vec<WingCount>,
    pub embed_dims: usize,
    pub ready_for_search: bool,
    pub ingest_roots_configured: bool,
    pub db_path: String,
    /// Gateway process id (single writer).
    #[serde(default)]
    pub pid: u32,
    /// Seconds since process start.
    #[serde(default)]
    pub uptime_seconds: u64,
    /// Main DuckDB file size in bytes when readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_file_bytes: Option<u64>,
    /// WAL file size in bytes (0 when absent).
    #[serde(default)]
    pub wal_bytes: u64,
    /// WAL warning threshold (`RAG_WAL_WARN_BYTES`).
    #[serde(default)]
    pub wal_warn_bytes: u64,
}

/// Local chat LLM + embedding config snapshot for the `llm_status` MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmStatusReport {
    /// Whether chat tools are enabled (`RAG_LLM_ENABLED`).
    pub llm_enabled: bool,
    /// Provider preset (`ollama`|`openai`|`codex`|`claude`|`kimi`|`deepseek`|`custom`).
    #[serde(default)]
    pub provider: String,
    /// Wire dialect: `openai_compat` or `anthropic_messages`.
    #[serde(default)]
    pub dialect: String,
    /// Chat API base URL (`RAG_LLM_BASE_URL` or provider default).
    pub base_url: String,
    /// Chat model tag (`RAG_LLM_MODEL`).
    pub model: String,
    /// True when a short probe against the chat endpoint succeeds.
    pub reachable: bool,
    /// Probe error detail when `reachable` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Embedding provider name (`mock` | `openai` | …).
    pub embed_provider: String,
    /// Embedding model id.
    pub embed_model: String,
    /// Configured embedding dimensionality.
    pub embed_dims: usize,
    /// Embedding API base URL when relevant (openai / local compat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed_base_url: Option<String>,
}

/// Minimal integrity report for the `doctor` MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub backend: String,
    #[serde(default)]
    pub storage_capabilities: Vec<String>,
    pub schema_version: i32,
    pub expected_schema_version: i32,
    pub schema_ok: bool,
    pub fts_ready: bool,
    pub document_count: u64,
    pub chunk_count: u64,
    pub node_count: u64,
    pub edge_count: u64,
    pub embed_dims: usize,
    /// Dims from `embedding_manifest` when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_dims: Option<i32>,
    /// True when the stored embedding identity matches live config (or when
    /// both the manifest and corpus chunks are absent).
    pub embed_ok: bool,
    pub ready_for_search: bool,
    pub ingest_roots_configured: bool,
    pub db_path: String,
    pub wal_bytes: u64,
    pub wal_warn_bytes: u64,
    pub wal_too_large: bool,
    pub documents_without_chunks: u64,
    pub orphan_chunks: u64,
    pub orphan_document_nodes: u64,
    pub orphan_edges: u64,
    pub unscoped_documents: u64,
    pub relational_integrity_ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair_hint: Option<String>,
    /// Critical integrity gate: schema current and embed dims consistent.
    pub ok: bool,
}

/// Object-graph node (document, tag, stub, or entity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    /// `document` | `tag` | `stub` | `entity`
    pub kind: String,
    pub label: String,
    pub document_id: Option<String>,
    pub uri: Option<String>,
    pub resolved: bool,
    pub metadata_json: String,
}

/// Directed edge between graph nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    /// `wikilink` | `tagged` | `related` | `mentions` | `tunnel`
    pub rel_type: String,
    pub weight: f64,
    pub context: Option<String>,
}

/// Topology payload for graph UI / LLM consumption.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphView {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// Aggregate counts for the object graph (`graph_stats` tool).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphStats {
    pub total_nodes: u64,
    pub total_edges: u64,
    /// Counts keyed by node kind (`document`, `tag`, `stub`, `entity`, …).
    pub nodes_by_kind: BTreeMap<String, u64>,
    /// Counts keyed by edge `rel_type` (`wikilink`, `tagged`, `related`, `tunnel`, …).
    pub edges_by_rel_type: BTreeMap<String, u64>,
}

/// Optional filters for full/partial graph export.
#[derive(Debug, Clone, Default)]
pub struct GraphFilter {
    pub kinds: Option<Vec<String>>,
    pub rel_types: Option<Vec<String>>,
    pub seed_ids: Option<Vec<String>>,
    pub max_nodes: Option<u32>,
}

/// Hard layout cap for the optional inspector / UI-oriented export (EGUI_GRAPH_VIEW §8.1).
pub const UI_GRAPH_EXPORT_MAX_NODES: u32 = 300;

/// Default PKB relation types for local graph and UI export
/// (GRAPH_DESIGN §7.1: `wikilink` + `related`; tags / tunnel / Dep opt-in).
pub const PKB_REL_TYPES: &[&str] = &["wikilink", "related"];

/// Default node kinds for UI export (tags off unless explicitly included).
pub const PKB_NODE_KINDS: &[&str] = &["document", "stub", "entity"];

impl GraphFilter {
    /// PKB defaults for exclusive `--db` inspector and Mode C snapshot dumps.
    ///
    /// - `rel_types`: wikilink + related (`include_tags` adds `tagged`)
    /// - kinds: document + stub + entity (`include_tags` adds `tag`)
    /// - `max_nodes`: `max_nodes` or [`UI_GRAPH_EXPORT_MAX_NODES`] (300)
    ///
    /// Topology only; no layout positions.
    pub fn pkb_ui_export(max_nodes: Option<u32>, include_tags: bool) -> Self {
        let mut kinds: Vec<String> = PKB_NODE_KINDS.iter().map(|s| (*s).to_string()).collect();
        let mut rel_types: Vec<String> = PKB_REL_TYPES.iter().map(|s| (*s).to_string()).collect();
        if include_tags {
            kinds.push("tag".into());
            rel_types.push("tagged".into());
        }
        Self {
            kinds: Some(kinds),
            rel_types: Some(rel_types),
            seed_ids: None,
            max_nodes: Some(max_nodes.unwrap_or(UI_GRAPH_EXPORT_MAX_NODES)),
        }
    }
}

/// Append-only ops / wiki log row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpsLogEntry {
    pub id: String,
    /// Monotonic sequence number.
    pub seq: i64,
    pub ts: DateTime<Utc>,
    /// Operation name, e.g. `ingest` | `wiki_write` | `file_answer` | `lint` | `delete`.
    pub op: String,
    /// Parseable short code, e.g. `INGEST` / `WIKI` / `LINT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_kind: Option<String>,
    #[serde(default = "default_empty_object_json")]
    pub payload_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
}

/// Named ordered set of documents, optionally shaped as an outline and dependency graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub metadata_json: String,
    pub created_at: String,
    pub updated_at: String,
}

/// One document's placement and prerequisites inside a collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionEntry {
    pub document_id: String,
    pub position: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_document_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

/// Full collection including its ordered outline entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionDetail {
    #[serde(flatten)]
    pub collection: Collection,
    pub entries: Vec<CollectionEntry>,
    /// Stable topological order (reading order breaks ties). Empty when dependencies cycle.
    pub dependency_order: Vec<String>,
    /// Members left in dependency cycles; empty when the dependency graph is acyclic.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_cycle_members: Vec<String>,
}

/// Catalog row for the wiki `index.md`-style content index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiIndexEntry {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_id: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// One ranked match from index-first query (`query_with_index`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexQueryMatch {
    pub entry: WikiIndexEntry,
    /// Simple term-overlap score (higher is better).
    pub score: f32,
}

/// Optional page body attached to an index match when `include_content` is set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexQueryPage {
    pub document_id: String,
    pub uri: String,
    pub title: String,
    pub layer: String,
    pub kind: String,
    pub content: String,
}

/// Response body for `query_with_index`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexQueryResult {
    pub query: String,
    pub match_count: usize,
    pub matches: Vec<IndexQueryMatch>,
    /// Loaded wiki page bodies when requested (aligned by `page_id` / document id).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<IndexQueryPage>,
}

/// Corpus embedding fingerprint; vec/hybrid search refuses mismatch with live config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingManifest {
    /// Singleton id, typically `"default"`.
    pub id: String,
    pub provider: String,
    pub model: String,
    pub dims: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Optional hash of embedding-related config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_fingerprint: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Temporal knowledge-graph fact (subject–predicate–object with validity window).
///
/// Status: `active` | `invalidated` | `superseded`.
/// Validity is half-open `[valid_from, valid_to)` when bounds are set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KgFact {
    pub id: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<DateTime<Utc>>,
    /// `active` | `invalidated` | `superseded`
    #[serde(default = "default_kg_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_document_id: Option<String>,
    /// Free-text provenance (legacy / MemPalace `source` field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default = "default_empty_object_json")]
    pub metadata_json: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidated_at: Option<DateTime<Utc>>,
}

/// Aggregate counts for the temporal knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KgStats {
    pub total_facts: u64,
    pub active_facts: u64,
    pub invalidated_facts: u64,
    pub superseded_facts: u64,
    pub distinct_subjects: u64,
    pub distinct_predicates: u64,
    /// Distinct predicate labels, sorted.
    pub relationship_types: Vec<String>,
}

/// Per-agent chronological diary note (`kind=diary`, wing `agents/<name>`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiaryEntry {
    /// Underlying document id.
    pub id: String,
    pub agent_name: String,
    pub title: String,
    pub content: String,
    /// Placement wing, typically `agents/<agent_name>`.
    pub wing: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl DiaryEntry {
    /// Build a diary view from a stored document (`kind=diary`).
    ///
    /// Resolves `agent_name` from `room`, then `metadata_json.agent_name`, then
    /// the suffix of wing `agents/<name>`, else `"unknown"`.
    pub fn from_document(doc: &Document) -> Self {
        let agent_name = doc
            .room
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| {
                serde_json::from_str::<serde_json::Value>(&doc.metadata_json)
                    .ok()
                    .and_then(|v| {
                        v.get("agent_name")
                            .and_then(|x| x.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                    })
            })
            .or_else(|| {
                doc.wing.as_deref().and_then(|w| {
                    w.strip_prefix("agents/")
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                })
            })
            .unwrap_or_else(|| "unknown".into());
        let wing = doc
            .wing
            .clone()
            .unwrap_or_else(|| format!("agents/{agent_name}"));
        Self {
            id: doc.id.clone(),
            agent_name,
            title: doc.title.clone(),
            content: doc.content.clone(),
            wing,
            content_hash: doc.content_hash.clone(),
            created_at: doc.created_at,
            updated_at: doc.updated_at,
        }
    }
}

/// Session bootstrap payload for `wake_up` (MemPalace parity).
///
/// Combines store [`StatusReport`], recent diary notes, pinned documents, and
/// an optional schema document (only when already present — not auto-seeded).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeUpReport {
    pub status: StatusReport,
    /// Recent diary entries for the requested agent (or all agents when unscoped).
    #[serde(default)]
    pub diary: Vec<DiaryEntry>,
    /// Pinned active documents (list order: boost desc, updated_at desc).
    #[serde(default)]
    pub pinned: Vec<DrawerListItem>,
    /// Schema conventions document when `schema://agents` (or layer=schema) exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<WakeUpSchemaSnippet>,
    /// Agent name used for diary scope (`None` = all diary entries).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
}

/// Compact schema view embedded in [`WakeUpReport`] (no auto-create).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeUpSchemaSnippet {
    pub document_id: String,
    pub uri: String,
    pub title: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl WakeUpSchemaSnippet {
    pub fn from_document(doc: &Document) -> Self {
        Self {
            document_id: doc.id.clone(),
            uri: doc.uri.clone(),
            title: doc.title.clone(),
            content: doc.content.clone(),
            content_hash: doc.content_hash.clone(),
            updated_at: doc.updated_at,
        }
    }
}

/// Generic wing / room taxonomy tree node (nested alternative to [`Taxonomy`]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaxonomyNode {
    /// Wing name (top-level) or room name when nested.
    pub name: String,
    /// `wing` | `room`
    pub kind: String,
    /// Documents in this scope.
    pub document_count: u64,
    /// Child rooms when `kind = "wing"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TaxonomyNode>,
}

/// Document list row without full body (MemPalace drawer inventory / `list_documents`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawerListItem {
    pub id: String,
    pub uri: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    #[serde(default = "default_layer")]
    pub layer: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default = "default_false")]
    pub pinned: bool,
    #[serde(default = "default_boost")]
    pub boost: f64,
    #[serde(default = "default_doc_status")]
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&Document> for DrawerListItem {
    fn from(d: &Document) -> Self {
        Self {
            id: d.id.clone(),
            uri: d.uri.clone(),
            title: d.title.clone(),
            wing: d.wing.clone(),
            room: d.room.clone(),
            source_file: d.source_file.clone(),
            layer: d.layer.clone(),
            kind: d.kind.clone(),
            content_hash: d.content_hash.clone(),
            pinned: d.pinned,
            boost: d.boost,
            status: d.status.clone(),
            created_at: d.created_at,
            updated_at: d.updated_at,
        }
    }
}

impl Default for KgFact {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            id: String::new(),
            subject: String::new(),
            predicate: String::new(),
            object: String::new(),
            valid_from: None,
            valid_to: None,
            status: default_kg_status(),
            superseded_by: None,
            source_document_id: None,
            source: None,
            confidence: None,
            metadata_json: default_empty_object_json(),
            created_at: now,
            updated_at: now,
            invalidated_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{format_document_etag, parse_document_etag, Document, ProjectId, WikiPageListItem};

    #[test]
    fn project_id_is_trimmed_and_rejects_ambiguous_values() {
        assert_eq!(ProjectId::parse("  rag-mcp  ").unwrap().as_str(), "rag-mcp");
        assert_eq!(
            ProjectId::parse("agents/claude").unwrap().as_str(),
            "agents/claude"
        );
        assert!(ProjectId::parse("").is_err());
        assert_eq!(
            ProjectId::parse("two projects (2026)").unwrap().as_str(),
            "two projects (2026)"
        );
        assert!(ProjectId::parse("two\nprojects").is_err());
    }

    #[test]
    fn wiki_page_list_item_serde_stays_lean_no_content() {
        let item = WikiPageListItem {
            id: "w1".into(),
            uri: "wiki://page".into(),
            slug: "page".into(),
            title: "Page".into(),
            kind: "wiki".into(),
            summary: Some("one-line blurb".into()),
            category: Some("docs".into()),
            revision: 2,
            etag: format_document_etag(2),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        let v = serde_json::to_value(&item).expect("serialize");
        let obj = v.as_object().expect("object");
        assert!(
            !obj.contains_key("content"),
            "WikiPageListItem must not expose body content"
        );
        assert!(
            !obj.contains_key("metadata_json"),
            "WikiPageListItem must not expose raw metadata_json"
        );
        // Known catalog keys only (identity + light meta + CAS headers).
        for key in obj.keys() {
            assert!(
                matches!(
                    key.as_str(),
                    "id" | "uri"
                        | "slug"
                        | "title"
                        | "kind"
                        | "summary"
                        | "category"
                        | "revision"
                        | "etag"
                        | "updated_at"
                ),
                "unexpected fat or unknown list field: {key}"
            );
        }
    }

    #[test]
    fn format_document_etag_weak_quoted_revision() {
        assert_eq!(format_document_etag(1), "W/\"1\"");
        assert_eq!(format_document_etag(0), "W/\"0\"");
        assert_eq!(format_document_etag(42), "W/\"42\"");
    }

    #[test]
    fn parse_document_etag_accepts_wire_forms() {
        assert_eq!(parse_document_etag("W/\"3\""), Some(3));
        assert_eq!(parse_document_etag("w/\"3\""), Some(3));
        assert_eq!(parse_document_etag("\"3\""), Some(3));
        assert_eq!(parse_document_etag("3"), Some(3));
        assert_eq!(parse_document_etag("r3"), Some(3));
        assert_eq!(parse_document_etag("R7"), Some(7));
        assert_eq!(parse_document_etag("  W/\"9\"  "), Some(9));
        assert_eq!(parse_document_etag("W/  \"12\"  "), Some(12));
    }

    #[test]
    fn parse_document_etag_rejects_invalid() {
        assert_eq!(parse_document_etag(""), None);
        assert_eq!(parse_document_etag("   "), None);
        assert_eq!(parse_document_etag("not-a-rev"), None);
        assert_eq!(parse_document_etag("-1"), None);
        assert_eq!(parse_document_etag("W/\"-2\""), None);
        assert_eq!(parse_document_etag("W/"), None);
    }

    #[test]
    fn document_etag_roundtrips_through_parse() {
        let doc = Document {
            revision: 5,
            ..Document::default()
        };
        let tag = doc.etag();
        assert_eq!(tag, format_document_etag(5));
        assert_eq!(parse_document_etag(&tag), Some(5));
        assert_eq!(parse_document_etag(&format_document_etag(0)), Some(0));
    }
}
