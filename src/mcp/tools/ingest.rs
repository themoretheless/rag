//! Ingest, documents and taxonomy tool parameters.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

// --- Ingest / raw layer ---

/// Parameters for `ingest_text`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct IngestTextParams {
    /// Raw text content to ingest.
    pub text: String,
    /// Optional document title (defaults to uri or "untitled").
    #[serde(default)]
    pub title: Option<String>,
    /// Optional stable URI for upsert-by-uri re-ingest.
    #[serde(default)]
    pub uri: Option<String>,
    /// Optional metadata as a JSON object string (default `{}`).
    #[serde(default)]
    pub metadata_json: Option<String>,
}

/// Parameters for `ingest_file`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct IngestFileParams {
    /// Filesystem path to text, Markdown, HTML, PDF, or a common source-code file.
    pub path: String,
    /// Optional document title (defaults to file name).
    #[serde(default)]
    pub title: Option<String>,
    /// Optional stable URI (defaults to `file://` + absolute path).
    #[serde(default)]
    pub uri: Option<String>,
    /// Optional metadata as a JSON object string (default `{}`).
    #[serde(default)]
    pub metadata_json: Option<String>,
    /// Project shelf (MemPalace wing), e.g. `rag` or `loyalty`.
    #[serde(default)]
    pub wing: Option<String>,
    /// Sub-area under wing.
    #[serde(default)]
    pub room: Option<String>,
}

/// Parameters for `sync_sources` (incremental directory ingest).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SyncSourcesParams {
    /// Directory to scan recursively. Must be under `RAG_INGEST_ROOTS`.
    pub path: String,
    /// Remove records whose previously ingested source files disappeared.
    /// Defaults to false; deletion never happens implicitly.
    #[serde(default)]
    pub remove_deleted: Option<bool>,
    /// Optional project shelf applied to newly added or updated files.
    #[serde(default)]
    pub wing: Option<String>,
    /// Optional room applied to newly added or updated files.
    #[serde(default)]
    pub room: Option<String>,
    /// Maximum bytes accepted per source file. Defaults to 512 KiB.
    #[serde(default)]
    pub max_file_bytes: Option<u64>,
}

/// Parameters for `ingest_raw` (immutable raw layer register).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct IngestRawParams {
    /// Raw source text content (stored verbatim; layer=`raw`).
    pub text: String,
    /// Optional document title (defaults to uri or "untitled").
    #[serde(default)]
    pub title: Option<String>,
    /// Optional stable URI for idempotent re-register (same hash = no-op).
    #[serde(default)]
    pub uri: Option<String>,
    /// Optional metadata as a JSON object string (default `{}`).
    #[serde(default)]
    pub metadata_json: Option<String>,
    /// Hierarchical scope (MemPalace-style wing).
    #[serde(default)]
    pub wing: Option<String>,
    /// Hierarchical scope (MemPalace-style room).
    #[serde(default)]
    pub room: Option<String>,
    /// Origin path or source identifier when known.
    #[serde(default)]
    pub source_file: Option<String>,
}

/// Parameters for `add_drawer` (MemPalace-style verbatim ingest with placement).
///
/// Stores content as-is (no summarize). Required: `content`, `wing`, `room`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AddDrawerParams {
    /// Verbatim drawer body (exact words; never summarized on ingest).
    pub content: String,
    /// Top-level shelf / project (required).
    pub wing: String,
    /// Sub-area under the wing (required).
    pub room: String,
    /// Origin path or provenance id when known.
    #[serde(default)]
    pub source_file: Option<String>,
    /// Optional document title (defaults to uri or "untitled").
    #[serde(default)]
    pub title: Option<String>,
    /// Optional stable URI for upsert-by-uri re-ingest.
    #[serde(default)]
    pub uri: Option<String>,
    /// Optional metadata as a JSON object string (default `{}`).
    #[serde(default)]
    pub metadata_json: Option<String>,
}

/// Parameters for `list_sources` (raw layer inventory).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ListSourcesParams {
    /// Optional filter: only sources in this wing.
    #[serde(default)]
    pub wing: Option<String>,
    /// Optional filter: only sources in this room.
    #[serde(default)]
    pub room: Option<String>,
    /// Max rows to return (default 50, hard cap 200).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Rows to skip after filters (default 0).
    #[serde(default)]
    pub offset: Option<u32>,
}

/// Parameters for `get_source` (fetch one raw-layer document).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetSourceParams {
    /// Document primary key (provide document_id or uri).
    #[serde(default)]
    pub document_id: Option<String>,
    /// Stable URI of the raw source (provide document_id or uri).
    #[serde(default)]
    pub uri: Option<String>,
    /// When true, include chunk texts (no embeddings).
    #[serde(default)]
    pub include_chunks: Option<bool>,
    /// Per-document chunk cap when `include_chunks=true` (default 100, hard cap 500).
    #[serde(default)]
    pub chunk_limit: Option<u32>,
}

// --- Documents / integrity ---

/// Parameters for `list_documents` (filtered inventory without full body).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListDocumentsParams {
    /// Filter: only documents with this wing.
    #[serde(default)]
    pub wing: Option<String>,
    /// Filter: only documents with this room.
    #[serde(default)]
    pub room: Option<String>,
    /// Filter: exact `source_file` match.
    #[serde(default)]
    pub source_file: Option<String>,
    /// When true, include archived/tombstone (default false).
    #[serde(default)]
    pub include_archived: Option<bool>,
    /// Filter: only documents with this layer (`raw`, `wiki`, …).
    #[serde(default)]
    pub layer: Option<String>,
    /// Filter: only documents with this kind.
    #[serde(default)]
    pub kind: Option<String>,
    /// Filter on the compile link: `true` keeps documents a resolved wiki page links to,
    /// `false` keeps the compile debt - raw sources no wiki page cites yet.
    #[serde(default)]
    pub compiled: Option<bool>,
    /// Max rows to return.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Rows to skip after filters (default 0).
    #[serde(default)]
    pub offset: Option<u32>,
}

/// Parameters for `get_document`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetDocumentParams {
    /// Document primary key.
    pub document_id: String,
    /// When true, include chunk texts (no embeddings).
    #[serde(default)]
    pub include_chunks: Option<bool>,
    /// Per-document chunk cap when `include_chunks=true` (default 100, hard cap 500).
    #[serde(default)]
    pub chunk_limit: Option<u32>,
}

/// Parameters for `delete_document`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DeleteDocumentParams {
    /// Document primary key to delete (and its chunks).
    pub document_id: String,
}

/// Parameters for `check_duplicate` (content-hash / uri idempotency probe).
///
/// Provide at least one of `content`, `content_hash` / `hash`, or `uri`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CheckDuplicateParams {
    /// Raw content to fingerprint (blake3) when `content_hash`/`hash` is omitted.
    #[serde(default)]
    pub content: Option<String>,
    /// Precomputed content fingerprint (blake3 hex).
    #[serde(default, alias = "hash")]
    pub content_hash: Option<String>,
    /// Stable URI to probe for an existing document.
    #[serde(default)]
    pub uri: Option<String>,
}

/// Parameters for `delete_by_source` (bulk delete by `source_file`).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DeleteBySourceParams {
    /// Exact `documents.source_file` value to match.
    #[serde(alias = "source")]
    pub source_file: String,
    /// When true, report match count and ids without deleting (default false).
    #[serde(default)]
    pub dry_run: Option<bool>,
}

/// Parameters for `update_document_meta` (MemPalace `update_drawer` analogue).
///
/// Partial update: omitted fields are left unchanged. Meta-only updates do **not**
/// re-embed; optional `content` triggers re-chunk + re-embed only when body changes
/// (refused for immutable `layer=raw`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct UpdateDocumentMetaParams {
    /// Document primary key (required).
    pub document_id: String,
    /// Hierarchical scope (wing). Pass empty string to clear.
    #[serde(default)]
    pub wing: Option<String>,
    /// Hierarchical scope (room). Pass empty string to clear.
    #[serde(default)]
    pub room: Option<String>,
    /// Display title.
    #[serde(default)]
    pub title: Option<String>,
    /// Metadata as a JSON object string (replaces entire `metadata_json` when set).
    #[serde(default)]
    pub metadata_json: Option<String>,
    /// Pin document for list / wake_up priority.
    #[serde(default)]
    pub pinned: Option<bool>,
    /// Manual relevance multiplier (must be finite and > 0).
    #[serde(default)]
    pub boost: Option<f64>,
    /// Lifecycle: `active` | `draft` | `consolidated` | `archived` | `tombstone`.
    #[serde(default)]
    pub status: Option<String>,
    /// Optional layer override (`raw` | `wiki` | `diary` | …).
    #[serde(default)]
    pub layer: Option<String>,
    /// Optional kind override (`document` | `wiki` | `diary` | …).
    #[serde(default)]
    pub kind: Option<String>,
    /// Origin path / provenance. Pass empty string to clear.
    #[serde(default)]
    pub source_file: Option<String>,
    /// Optional body replace. When set and different from stored content,
    /// server re-chunks and re-embeds (refused for immutable `layer=raw`).
    #[serde(default)]
    pub content: Option<String>,
}

// --- Taxonomy (wings / rooms) ---

/// Parameters for `list_wings` (returns all wings with document counts).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListWingsParams {
    /// Reserved; list always returns every non-empty wing.
    #[serde(default)]
    pub _unused: Option<bool>,
}

/// Parameters for `list_rooms` (optional wing filter).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListRoomsParams {
    /// When set, only rooms under this wing; omit for all rooms globally.
    #[serde(default)]
    pub wing: Option<String>,
}

/// Parameters for `get_taxonomy` (full wing → room tree with counts).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct GetTaxonomyParams {
    /// Reserved; taxonomy always covers the full document placement tree.
    #[serde(default)]
    pub _unused: Option<bool>,
}
