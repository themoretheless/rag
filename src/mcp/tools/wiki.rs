//! Wiki, schema, diary and ops log tool parameters.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

/// Parameters for `list_wiki_pages` (lean wiki catalog inventory).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListWikiPagesParams {
    /// Case-insensitive substring on title, slug, URI, summary, category, or kind.
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub wing: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
    /// Max rows to return (default 50, hard cap 200).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Rows to skip after filters (default 0).
    #[serde(default)]
    pub offset: Option<u32>,
}

// --- Diary / checkpoint ---

/// Parameters for `diary_write` (per-agent chronological note).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DiaryWriteParams {
    /// Agent identity (normalized to lowercase; wing becomes `agents/<name>` unless `wing` set).
    #[serde(alias = "agent")]
    pub agent_name: String,
    /// Verbatim diary body (never summarized on write).
    pub content: String,
    /// Optional wing override (default `agents/<agent_name>`).
    #[serde(default)]
    pub wing: Option<String>,
    /// Optional topic tag (default `general`); stored in metadata.
    #[serde(default)]
    pub topic: Option<String>,
    /// Optional display title (default: topic + timestamp).
    #[serde(default)]
    pub title: Option<String>,
}

/// Parameters for `diary_read` (recent entries for one agent).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DiaryReadParams {
    /// Agent identity (case-insensitive).
    #[serde(alias = "agent")]
    pub agent_name: String,
    /// Max entries, newest first (default 10, max 100). Alias: `limit`.
    #[serde(default, alias = "limit")]
    pub last_n: Option<u32>,
}

/// Parameters for `wake_up` (session bootstrap: status + diary + pinned + schema).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct WakeUpParams {
    /// When set, scope recent diary to this agent; omit for all diary entries.
    #[serde(default, alias = "agent")]
    pub agent_name: Option<String>,
    /// Max diary entries, newest first (default 5, max 100).
    #[serde(default)]
    pub diary_limit: Option<u32>,
    /// Max pinned documents (default 20).
    #[serde(default)]
    pub pinned_limit: Option<u32>,
}

/// Parameters for `checkpoint` (session savepoint: ops_log + optional diary).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CheckpointParams {
    /// Required session summary recorded in `ops_log` (`op=checkpoint`). Alias: `message`.
    #[serde(alias = "message")]
    pub summary: String,
    /// Optional diary body; when set, also calls diary_write under the agent wing.
    #[serde(default)]
    pub diary: Option<String>,
    /// Optional agent name for ops_log and diary (defaults to `agent` when diary is set).
    #[serde(default, alias = "agent")]
    pub agent_name: Option<String>,
}

// --- Ops log ---

/// Parameters for `append_log` (append-only ops_log / log.md timeline).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AppendLogParams {
    /// Operation name, e.g. `ingest`, `wiki_write`, `file_answer`, `lint`, `delete`.
    pub op: String,
    /// Human-readable log message.
    #[serde(default)]
    pub message: Option<String>,
    /// Parseable short code, e.g. `INGEST` / `WIKI` / `LINT`.
    #[serde(default)]
    pub prefix: Option<String>,
    /// Related entity id (document, wiki page, …).
    #[serde(default)]
    pub entity_id: Option<String>,
    /// Related entity kind (`document`, `wiki`, `chunk`, …).
    #[serde(default)]
    pub entity_kind: Option<String>,
    /// Optional JSON object string payload (default `{}`).
    #[serde(default)]
    pub payload_json: Option<String>,
    /// Optional agent name that produced the op.
    #[serde(default)]
    pub agent_name: Option<String>,
}

/// Parameters for `read_log`.
///
/// Resolve one entry by `id` or `seq`; when neither is set, list recent rows
/// (same shape as `list_recent_ops`, default limit 50).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ReadLogParams {
    /// Ops log row primary key.
    #[serde(default)]
    pub id: Option<String>,
    /// Monotonic sequence number.
    #[serde(default)]
    pub seq: Option<i64>,
    /// When listing (no id/seq): max rows, newest first (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Parameters for `list_recent_ops`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ListRecentOpsParams {
    /// Max rows to return, newest first (default 20).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Parameters for `memories_filed_away` (recent memory-filing ops from ops_log).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct MemoriesFiledAwayParams {
    /// Max filing ops to return, newest first (default 20).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Parameters for `reconnect` (DuckDB no-op; accepted for MemPalace parity).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ReconnectParams {}

// --- Karpathy wiki layer ---

/// Parameters for `write_wiki_page` / `update_wiki_page`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WriteWikiPageParams {
    /// Kebab-case slug → `wiki://{slug}`.
    pub slug: String,
    pub title: String,
    pub content: String,
    /// Project placement for newly created pages; omitted updates preserve it.
    #[serde(default)]
    pub wing: Option<String>,
    /// Optional project room/module placement.
    #[serde(default)]
    pub room: Option<String>,
    /// `wiki` | `entity` | `concept` | `source_summary`
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    /// Optimistic concurrency: revision from last `get_wiki_page` (integer).
    /// When set, write fails with conflict if another agent already advanced revision.
    #[serde(default)]
    pub if_match_revision: Option<i64>,
    /// Same as `if_match_revision` but as etag string (`W/"3"`, `"3"`, `r3`).
    #[serde(default)]
    pub if_match_etag: Option<String>,
}

/// Parameters for `get_wiki_page`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetWikiPageParams {
    /// Slug, `wiki://slug`, or document id.
    pub id_or_slug: String,
}

/// Parameters for `query_with_index` (Karpathy index-first navigation).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct QueryWithIndexParams {
    /// Free-text query matched against wiki index catalog fields (slug/title/summary/…).
    pub query: String,
    /// Max catalog matches (defaults to `RAG_DEFAULT_TOP_K`).
    #[serde(default)]
    pub top_k: Option<u32>,
    /// When true, also load full wiki page content for matched `page_id`s.
    #[serde(default)]
    pub include_content: Option<bool>,
}

/// Parameters for `search_wiki` (search restricted to `layer=wiki`).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SearchWikiParams {
    /// Natural-language query to rank against wiki-layer chunks.
    pub query: String,
    /// Max hits to return (defaults to config `RAG_DEFAULT_TOP_K`).
    #[serde(default)]
    pub top_k: Option<u32>,
    /// Search backend: `lex` | `vec` | `hybrid` (defaults to `RAG_DEFAULT_SEARCH_MODE`).
    #[serde(default)]
    pub mode: Option<String>,
    /// Drop hits whose primary score is strictly below this threshold.
    #[serde(default)]
    pub min_score: Option<f32>,
    /// Optional filter: only documents with this wing.
    #[serde(default)]
    pub wing: Option<String>,
    /// Optional filter: only documents with this room.
    #[serde(default)]
    pub room: Option<String>,
    /// Result diversity: `mmr` | `collapse_by_document`.
    #[serde(default)]
    pub diversity: Option<String>,
    /// Complete citation-block budget including headers and expansion (~chars/4; 0 returns no hits).
    #[serde(default)]
    pub max_context_tokens: Option<u32>,
    /// Cap retained chunks per document under diversity (default `RAG_MAX_CHUNKS_PER_DOC`).
    #[serde(default)]
    pub max_chunks_per_document: Option<u32>,
    /// When true and `RAG_RERANK_PROVIDER` is configured, rerank the candidate
    /// pool before returning `top_k` hits (fallback keeps the original ranking).
    /// Default false.
    #[serde(default)]
    pub rerank: Option<bool>,
}

/// One citation attached to a `file_answer` wiki page.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileAnswerCitationParams {
    /// Source document id when known.
    #[serde(default)]
    pub document_id: Option<String>,
    /// Source document URI.
    #[serde(default)]
    pub uri: Option<String>,
    /// Source title for display.
    #[serde(default)]
    pub title: Option<String>,
    /// Chunk id when the citation is from a search hit.
    #[serde(default)]
    pub chunk_id: Option<String>,
    /// Quoted span used in the answer.
    #[serde(default)]
    pub quote: Option<String>,
    /// Freeform note about how the source supports the answer.
    #[serde(default)]
    pub note: Option<String>,
}

/// Parameters for `file_answer` (persist cited answer as wiki page).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileAnswerParams {
    /// Page title (also used to derive slug when `slug` is omitted).
    pub title: String,
    /// Answer markdown body (citations section may be appended).
    pub body: String,
    /// Optional wiki slug (`wiki://{slug}`); defaults to slugified title.
    #[serde(default)]
    pub slug: Option<String>,
    /// Citation records stored in page metadata and listed under `## Citations`.
    #[serde(default)]
    pub citations: Option<Vec<FileAnswerCitationParams>>,
    /// Optional agent name recorded in ops_log.
    #[serde(default)]
    pub agent: Option<String>,
}

/// Parameters for `read_index` (wiki content catalog).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ReadIndexParams {
    /// Output shape: `json` (default) returns `{entries, count}`; `markdown` also
    /// includes an `index.md`-style `markdown` field grouped by category.
    #[serde(default)]
    pub format: Option<String>,
}

/// Parameters for `update_index_entry` (create/merge one catalog row).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct UpdateIndexEntryParams {
    /// Catalog slug (normalized to kebab-case).
    pub slug: String,
    /// Display title (optional on update; defaults to slug on insert).
    #[serde(default)]
    pub title: Option<String>,
    /// Page kind: `wiki` | `entity` | `concept` | `source_summary`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Optional category for index grouping.
    #[serde(default)]
    pub category: Option<String>,
    /// One-line summary for navigation.
    #[serde(default)]
    pub summary: Option<String>,
    /// Linked wiki document id when known.
    #[serde(default)]
    pub page_id: Option<String>,
}

/// Parameters for `rebuild_index` (no fields required).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct RebuildIndexParams {
    /// Reserved; rebuild always rebuilds from all `layer=wiki` documents.
    #[serde(default)]
    pub _unused: Option<bool>,
}

/// Parameters for `compile_source` / `ingest_source`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CompileSourceParams {
    /// Raw document id or uri.
    pub source_id_or_uri: String,
    /// When true, return proposed pages without writing.
    #[serde(default)]
    pub dry_run: Option<bool>,
    #[serde(default)]
    pub agent: Option<String>,
}

/// Parameters for `consolidate` (LLM merges N docs → one wiki page proposal).
///
/// Default is propose-only (`apply=false`). Set `apply=true` to write the wiki
/// page, rebuild graph/index, and append ops_log. Capped by `RAG_MAINT_MAX_DOCS`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ConsolidateParams {
    /// Document ids and/or uris to merge (at least one; duplicates dropped).
    pub document_ids: Vec<String>,
    /// When true, write the proposed page (chunk+embed+graph+index+ops_log).
    /// Default false: return proposal only for agent review.
    #[serde(default)]
    pub apply: Option<bool>,
    /// Optional override slug (`wiki://{slug}`).
    #[serde(default)]
    pub slug: Option<String>,
    /// Optional override page title.
    #[serde(default)]
    pub title: Option<String>,
    /// Optional kind: `concept` | `entity` | `wiki`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Optional category for wiki_index grouping.
    #[serde(default)]
    pub category: Option<String>,
    /// Cap sources for this call (default / max: `RAG_MAINT_MAX_DOCS`).
    #[serde(default)]
    pub max_docs: Option<u32>,
    /// Optional agent name for ops_log.
    #[serde(default)]
    pub agent: Option<String>,
}

/// Parameters for deterministic lifecycle candidate listing.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListMemoryLifecycleCandidatesParams {
    /// Lifecycle status to list (default `active`).
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    /// Maximum rows (default 100, max 500).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Mark selected memory items consolidated into an existing durable output.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ConsolidateMemoryItemsParams {
    pub document_ids: Vec<String>,
    pub output_document_id: String,
    #[serde(default)]
    pub agent: Option<String>,
}

/// Archive selected memory items idempotently.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ArchiveMemoryItemsParams {
    pub document_ids: Vec<String>,
    #[serde(default)]
    pub agent: Option<String>,
}

/// Parameters for `get_schema`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct GetSchemaParams {
    /// When true, do not seed default schema if missing (return not found).
    #[serde(default)]
    pub no_default: Option<bool>,
}

/// Parameters for `update_schema`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct UpdateSchemaParams {
    /// Full schema markdown / conventions text (agent AGENTS.md analogue).
    pub content: String,
    /// Optional display title (default "Wiki schema").
    #[serde(default)]
    pub title: Option<String>,
    /// Optional agent name for ops_log.
    #[serde(default)]
    pub agent: Option<String>,
}

/// Parameters for `refresh_stale_wiki`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct RefreshStaleWikiParams {
    /// When true (default), only list stale wiki↔raw pairs for the agent.
    /// When false and a chat LLM is configured + enabled, re-run `compile_source`
    /// per unique raw parent (capped by `max_docs` / `RAG_MAINT_MAX_DOCS`).
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Max unique raw sources to recompile when applying (default `RAG_MAINT_MAX_DOCS`).
    #[serde(default)]
    pub max_docs: Option<u32>,
    /// Optional agent name for ops_log.
    #[serde(default)]
    pub agent: Option<String>,
}
