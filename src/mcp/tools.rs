//! Parameter structs for MCP tools (schemars + serde).
//!
//! Use rmcp's schemars re-export so JsonSchema matches the version rmcp expects (1.x).
//!
//! # ISP: clustered param surfaces
//!
//! Types live in one file but are **section-clustered by tool domain** so a caller
//! only needs the cluster it invokes. The MCP facade maps each `*Params`
//! into domain code and JSON; business logic stays in `db` / `wiki` / `graph` /
//! `search_pack` / `maintain` / etc.
//!
//! Section banners (`// --- … ---`) mark the clusters below:
//!
//! | Cluster | Banner | Example types |
//! |---------|--------|---------------|
//! | **Ingest / raw** | `Ingest / raw layer` | [`IngestTextParams`], [`IngestFileParams`], [`IngestRawParams`], [`AddDrawerParams`], [`ListSourcesParams`], [`GetSourceParams`] |
//! | **Search / pack** | `Search / packing` | [`SearchParams`], [`PackHitParams`], [`PackContextParams`] |
//! | **Documents** | `Documents / integrity` | [`ListDocumentsParams`], [`GetDocumentParams`], [`DeleteDocumentParams`], [`UpdateDocumentMetaParams`], … |
//! | **Taxonomy** | `Taxonomy (wings / rooms)` | [`ListWingsParams`], [`ListRoomsParams`], [`GetTaxonomyParams`] |
//! | **Graph** | `Graph` | [`GetGraphParams`], [`GetNeighborsParams`], [`LinkNodesParams`], … |
//! | **Tunnels** | `Tunnels` | [`CreateTunnelParams`], [`FollowTunnelsParams`], [`GraphExpandSearchParams`], … |
//! | **Diary** | `Diary / checkpoint` | [`DiaryWriteParams`], [`WakeUpParams`], [`CheckpointParams`] |
//! | **Ops log** | `Ops log` | [`AppendLogParams`], [`ReadLogParams`], [`ReconnectParams`] |
//! | **Wiki** | `Karpathy wiki layer` | [`WriteWikiPageParams`], [`SearchWikiParams`], [`QueryWithIndexParams`], [`FileAnswerParams`], … |
//! | **Maintenance** | `Maintenance` | [`AnalyzeCorpusParams`], [`PlanMaintenanceParams`], [`ApplyMaintenancePlanParams`], … |
//! | **KG** | `Temporal knowledge graph` | [`KgAddParams`], [`KgQueryParams`], [`KgTimelineParams`], … |
//!
//! Prefer extending an existing cluster over a flat grab-bag. Shared cross-tool
//! fields (wing/room filters, rank options) should become flattenable JsonSchema
//! fragments later rather than copy-pasted onto every struct.
//!
//! Parameter types remain flat (or can later be re-exported from this module) so
//! `use crate::mcp::tools::…` and the rmcp schema surface stay stable; route
//! registration itself is split into the sibling domain modules.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

// --- Backup / recovery ---

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BackupDbParams {
    /// Destination DuckDB file under `RAG_INGEST_ROOTS`.
    pub path: String,
    /// Report the intended copy without writing it.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Replace an existing destination. Defaults to false.
    #[serde(default)]
    pub overwrite: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExportBundleParams {
    /// Destination `.json` or `.jsonl` file under `RAG_INGEST_ROOTS`.
    pub path: String,
    /// `json` or `jsonl`; inferred from the extension when omitted.
    #[serde(default)]
    pub format: Option<String>,
    /// Report counts without writing a file.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Replace an existing destination. Defaults to false.
    #[serde(default)]
    pub overwrite: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExportVaultParams {
    /// Destination directory under `RAG_INGEST_ROOTS`.
    pub path: String,
    /// Preview document/file counts without writing. Defaults to true.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Replace an existing vault, retaining it as a dated `.previous-*` sibling.
    #[serde(default)]
    pub overwrite: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImportBundleParams {
    /// Source `.json` or `.jsonl` file under `RAG_INGEST_ROOTS`.
    pub path: String,
    /// `json` or `jsonl`; inferred from the extension when omitted.
    #[serde(default)]
    pub format: Option<String>,
    /// `error` (default), `skip`, or `overwrite` for document id/URI conflicts.
    #[serde(default)]
    pub conflict_policy: Option<String>,
    /// Parse and report the import without changing DuckDB. Defaults to true.
    #[serde(default)]
    pub dry_run: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct DoctorRepairParams {
    /// Preview only by default. Set false to reingest missing chunks and prune orphan rows.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Maximum documents without chunks to repair in one call.
    #[serde(default)]
    pub max_docs: Option<usize>,
}

// --- Collections / outlines / dependency order ---

/// One ordered document entry supplied to collection create/update.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CollectionEntryParams {
    /// Existing document primary key.
    pub document_id: String,
    /// Optional parent document in the same collection (outline nesting).
    #[serde(default)]
    pub parent_document_id: Option<String>,
    /// Optional prerequisite documents in the same collection.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CollectionCreateParams {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub metadata_json: Option<String>,
    /// Reading order; array order becomes the durable position.
    #[serde(default)]
    pub entries: Vec<CollectionEntryParams>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct CollectionListParams {}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CollectionGetParams {
    pub collection_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CollectionUpdateParams {
    pub collection_id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Empty string clears the description.
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub metadata_json: Option<String>,
    /// When present, replaces membership, reading order, outline parents, and dependencies.
    #[serde(default)]
    pub entries: Option<Vec<CollectionEntryParams>>,
}

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
}

// --- Search / packing ---

/// Parameters for `search`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SearchParams {
    /// Natural-language query to embed and rank against chunks.
    pub query: String,
    /// Max hits to return (defaults to config `RAG_DEFAULT_TOP_K`).
    #[serde(default)]
    pub top_k: Option<u32>,
    /// Optional filter: only search chunks of this document id.
    #[serde(default)]
    pub document_id: Option<String>,
    /// Search backend: `lex` | `vec` | `hybrid` (defaults to `RAG_DEFAULT_SEARCH_MODE`).
    #[serde(default)]
    pub mode: Option<String>,
    /// Drop hits whose primary score is strictly below this threshold.
    #[serde(default)]
    pub min_score: Option<f32>,
    /// Filter: only documents with this wing.
    #[serde(default)]
    pub wing: Option<String>,
    /// Filter: only documents with this room.
    #[serde(default)]
    pub room: Option<String>,
    /// Filter: only documents with this layer (`raw`, `wiki`, …).
    #[serde(default)]
    pub layer: Option<String>,
    /// Filter: exact `source_file` provenance match.
    #[serde(default)]
    pub source_file: Option<String>,
    /// When true, include archived/tombstone documents (default false).
    #[serde(default)]
    pub include_archived: Option<bool>,
    /// Result diversity: `mmr` | `collapse_by_document`.
    #[serde(default)]
    pub diversity: Option<String>,
    /// Alias for document-level grouping. `document` selects collapse_by_document.
    #[serde(default)]
    pub group_by: Option<String>,
    /// Optional exponential freshness boost in days. Disabled when omitted.
    #[serde(default)]
    pub recency_half_life_days: Option<f64>,
    /// Token budget when packing hit content (approx chars/4; default `RAG_MAX_CONTEXT_TOKENS`).
    #[serde(default)]
    pub max_context_tokens: Option<u32>,
    /// Cap retained chunks per document under diversity (default `RAG_MAX_CHUNKS_PER_DOC`).
    #[serde(default)]
    pub max_chunks_per_document: Option<u32>,
    /// Optional source context: `neighbors` or `parent_section`.
    #[serde(default)]
    pub context_expansion: Option<String>,
    /// Chunks on each side for neighbor expansion (default 1).
    #[serde(default)]
    pub neighbor_chunks: Option<u32>,
    /// End-to-end synchronous retrieval budget (default 5000 ms).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Explicit multi-query rewrite/RRF. Callers provide rewrites; no LLM is invoked by default.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MultiQuerySearchParams {
    /// Original query followed by optional deterministic/manual rewrites.
    pub queries: Vec<String>,
    #[serde(default)]
    pub top_k: Option<u32>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub wing: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub source_file: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// One hit passed into `pack_context` (same shape as `search` results).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PackHitParams {
    pub chunk_id: String,
    pub document_id: String,
    pub document_title: String,
    pub document_uri: String,
    pub chunk_index: i32,
    pub content: String,
    pub score: f32,
    #[serde(default)]
    pub score_vec: Option<f32>,
    #[serde(default)]
    pub score_lex: Option<f32>,
    #[serde(default)]
    pub score_rrf: Option<f32>,
    #[serde(default)]
    pub snippet: Option<String>,
    #[serde(default)]
    pub char_start: Option<i32>,
    #[serde(default)]
    pub char_end: Option<i32>,
    #[serde(default)]
    pub heading_path: Option<Vec<String>>,
    #[serde(default)]
    pub section: Option<String>,
}

/// Parameters for `pack_context`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PackContextParams {
    /// Ranked search hits to pack (highest score first).
    pub hits: Vec<PackHitParams>,
    /// Token budget (~4 chars/token). Defaults to `RAG_MAX_CONTEXT_TOKENS`.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Optional source context: `neighbors` or `parent_section`.
    #[serde(default)]
    pub context_expansion: Option<String>,
    /// Chunks on each side for neighbor expansion (default 1).
    #[serde(default)]
    pub neighbor_chunks: Option<u32>,
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
    /// Max rows to return.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Parameters for `get_document`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetDocumentParams {
    /// Document primary key.
    pub document_id: String,
    /// When true, include chunk texts (no embeddings).
    #[serde(default)]
    pub include_chunks: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MultiGetParams {
    pub document_ids: Vec<String>,
    #[serde(default)]
    pub include_chunks: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FindSimilarParams {
    pub document_id: String,
    #[serde(default)]
    pub top_k: Option<u32>,
    #[serde(default)]
    pub wing: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExpandChunksParams {
    pub document_id: String,
    pub chunk_index: i32,
    #[serde(default)]
    pub radius: Option<u32>,
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

/// Parameters for `reembed_document`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ReembedDocumentParams {
    /// Document primary key whose chunks should be re-embedded with the live config.
    pub document_id: String,
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

// --- Graph ---

/// Parameters for `get_graph`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetGraphParams {
    /// Optional node kinds to include (`document`, `tag`, `stub`, `entity`).
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
    /// Optional edge relation types (`wikilink`, `tagged`, `related`, `mentions`, `tunnel`).
    #[serde(default)]
    pub rel_types: Option<Vec<String>>,
    /// Optional seed node ids (export only those nodes and edges between them).
    #[serde(default)]
    pub seed_ids: Option<Vec<String>>,
    /// Max nodes to return (default 500).
    #[serde(default)]
    pub max_nodes: Option<u32>,
}

/// Parameters for `export_graph_snapshot` (write GraphView JSON for rag-mcp-ui while MCP holds the DB).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExportGraphSnapshotParams {
    /// Output file path. Default: `<parent of RAG_DB_PATH>/graph.json`.
    /// Must be under the DB directory or `RAG_INGEST_ROOTS`.
    #[serde(default)]
    pub path: Option<String>,
    /// Max nodes (default 500, PKB-oriented export).
    #[serde(default)]
    pub max_nodes: Option<u32>,
    /// Include tag nodes/edges (default false).
    #[serde(default)]
    pub include_tags: Option<bool>,
}

/// Parameters for `get_neighbors`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetNeighborsParams {
    /// Seed graph node id.
    pub node_id: String,
    /// BFS hop depth (default 1).
    #[serde(default)]
    pub depth: Option<u32>,
    /// Max nodes in the local subgraph (default 100).
    #[serde(default)]
    pub max_nodes: Option<u32>,
}

/// Parameters for `get_backlinks`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetBacklinksParams {
    /// Resolve target by graph node id.
    #[serde(default)]
    pub node_id: Option<String>,
    /// Resolve target by document id.
    #[serde(default)]
    pub document_id: Option<String>,
    /// Resolve target by exact node label.
    #[serde(default)]
    pub label: Option<String>,
}

/// Parameters for `link_nodes`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LinkNodesParams {
    /// Source graph node id.
    pub source_id: String,
    /// Target graph node id.
    pub target_id: String,
    /// Edge type (default `related`; `tunnel` allowed).
    #[serde(default)]
    pub rel_type: Option<String>,
    /// Edge weight (default 1.0).
    #[serde(default)]
    pub weight: Option<f64>,
}

// --- Tunnels (`rel_type=tunnel`) ---

/// Parameters for `create_tunnel`.
///
/// Creates an explicit undirected bridge edge (`rel_type=tunnel`) between two
/// graph nodes. Re-creating the same pair (either order) updates weight/context.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CreateTunnelParams {
    /// Source graph node id.
    pub source_id: String,
    /// Target graph node id.
    pub target_id: String,
    /// Edge weight (default 1.0; must be finite and > 0).
    #[serde(default)]
    pub weight: Option<f64>,
    /// Optional label / note stored in edge `context`.
    #[serde(default)]
    pub context: Option<String>,
    /// Alias for `context` (MemPalace-style label).
    #[serde(default)]
    pub label: Option<String>,
}

/// Parameters for `list_tunnels`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListTunnelsParams {
    /// When set, only tunnels incident to this graph node id.
    #[serde(default)]
    pub node_id: Option<String>,
}

/// Parameters for `delete_tunnel`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DeleteTunnelParams {
    /// Tunnel edge primary key (`graph_edges.id`).
    pub tunnel_id: String,
}

/// Parameters for `follow_tunnels` (multi-hop along tunnel edges only).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FollowTunnelsParams {
    /// Seed graph node id.
    pub node_id: String,
    /// BFS hop depth along tunnels (default 2).
    #[serde(default)]
    pub depth: Option<u32>,
    /// Max nodes in the returned subgraph (default 100).
    #[serde(default)]
    pub max_nodes: Option<u32>,
}

/// Parameters for `find_tunnels` (filter/discover tunnel edges).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct FindTunnelsParams {
    /// Tunnels incident to this node.
    #[serde(default)]
    pub node_id: Option<String>,
    /// With `node_id`: only tunnels bridging these two nodes (undirected).
    #[serde(default)]
    pub other_node_id: Option<String>,
    /// Either endpoint document is in this wing.
    #[serde(default)]
    pub wing: Option<String>,
    /// Max edges to return (0 / omit = unlimited).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Parameters for `find_node`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FindNodeParams {
    /// Exact label match.
    #[serde(default)]
    pub label: Option<String>,
    /// Document id of a document-kind node.
    #[serde(default)]
    pub document_id: Option<String>,
    /// Graph node primary key.
    #[serde(default)]
    pub node_id: Option<String>,
}

/// Parameters for `graph_expand_search`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GraphExpandSearchParams {
    /// Natural-language query to embed and rank against chunks.
    pub query: String,
    /// Max search hits (defaults to config `RAG_DEFAULT_TOP_K`).
    #[serde(default)]
    pub top_k: Option<u32>,
    /// Optional filter: only search chunks of this document id.
    #[serde(default)]
    pub document_id: Option<String>,
    /// Neighbor BFS depth from hit document nodes (default 1).
    #[serde(default)]
    pub depth: Option<u32>,
    /// Max nodes in the expanded subgraph (default 100).
    #[serde(default)]
    pub max_nodes: Option<u32>,
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
    /// Token budget when packing hit content (approx chars/4; default `RAG_MAX_CONTEXT_TOKENS`).
    #[serde(default)]
    pub max_context_tokens: Option<u32>,
    /// Cap retained chunks per document under diversity (default `RAG_MAX_CHUNKS_PER_DOC`).
    #[serde(default)]
    pub max_chunks_per_document: Option<u32>,
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

// --- Maintenance ---

/// Parameters for `analyze_corpus` (deterministic health report; no LLM).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct AnalyzeCorpusParams {
    /// Unresolved stubs older than this many days are flagged as aging (default 14).
    #[serde(default)]
    pub stub_age_days: Option<u32>,
    /// When true, scan first-chunk embeddings for near-duplicates (default false).
    #[serde(default)]
    pub include_near_dups: Option<bool>,
    /// Cosine threshold θ for near-dups (default `RAG_MAINT_NEAR_DUP_THRESHOLD`).
    #[serde(default)]
    pub near_dup_threshold: Option<f64>,
    /// Min age (days) for low-boost archive candidates (default 90).
    #[serde(default)]
    pub archive_min_age_days: Option<u32>,
    /// When false, skip appending `ops_log` for this run (default true).
    #[serde(default)]
    pub log_ops: Option<bool>,
}

/// Parameters for `plan_maintenance` (LLM or heuristic action plan from analysis).
///
/// When `RAG_LLM_ENABLED=false` (or `force_heuristic=true`), returns a deterministic
/// plan from the analysis signals with no network call. When LLM is enabled, calls
/// the local chat model with the analysis JSON + system prompt, parses JSON actions,
/// and validates against the server whitelist.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanMaintenanceParams {
    /// Optional AnalysisReport JSON object (from `analyze_corpus`). When omitted,
    /// runs `analyze_corpus` first with default options (no near-dups unless set).
    #[serde(default)]
    pub analysis: Option<serde_json::Value>,
    /// Cap validated actions in the plan (default min(30, `RAG_MAINT_MAX_DOCS`)).
    #[serde(default)]
    pub max_actions: Option<u32>,
    /// When true, skip LLM even if `RAG_LLM_ENABLED=true` (tests / offline).
    #[serde(default)]
    pub force_heuristic: Option<bool>,
    /// When false, skip `ops_log` for this plan (default true).
    #[serde(default)]
    pub log_ops: Option<bool>,
    /// When analysis is omitted: pass through to analyze (near-dups).
    #[serde(default)]
    pub include_near_dups: Option<bool>,
}

/// Parameters for `maintain_organize` (suggest/apply refiles for docs missing wing).
///
/// Heuristic path/title/embedding suggestions by default; optional local LLM.
/// `dry_run` defaults to **true** (preview only). Set `dry_run=false` to apply
/// whitelist `refile` actions. Cap: `RAG_MAINT_MAX_DOCS` / `max_docs`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct MaintainOrganizeParams {
    /// When true (default), report suggestions without mutating placement.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// `heuristic` (default) | `llm` | `auto` (heuristic then LLM fill-in).
    #[serde(default)]
    pub mode: Option<String>,
    /// Cap unscoped documents considered / applied (default `RAG_MAINT_MAX_DOCS`).
    #[serde(default)]
    pub max_docs: Option<u32>,
    /// Only act on suggestions with confidence ≥ this (default 0.5).
    #[serde(default)]
    pub min_confidence: Option<f64>,
    /// When true (and not dry_run), rebuild wiki_index after refiles.
    #[serde(default)]
    pub rebuild_index: Option<bool>,
    /// Optional agent name for ops_log.
    #[serde(default)]
    pub agent: Option<String>,
}

/// Parameters for `maintain_refresh` (actualize FTS / graph / wiki index / reembed).
///
/// Whitelist actions only (`reindex_fts`, `rebuild_graph`, `rebuild_wiki_index`,
/// `reembed_all`). When no action flag is set, safe defaults run: FTS reindex +
/// dirty graph rebuild + wiki index rebuild (no reembed). When any action is
/// set explicitly, unspecified actions default to false.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct MaintainRefreshParams {
    /// Rebuild DuckDB FTS / BM25 index on `chunks` (after bulk deletes).
    #[serde(default)]
    pub reindex_fts: Option<bool>,
    /// Rebuild object-graph slices from document content (wikilinks/tags).
    #[serde(default)]
    pub rebuild_graph: Option<bool>,
    /// When rebuilding graph: only docs missing a document node (default true).
    /// Set false to rebuild graph for all documents (capped by max_docs).
    #[serde(default)]
    pub graph_dirty_only: Option<bool>,
    /// Clear and rebuild `wiki_index` from all `layer=wiki` documents.
    #[serde(default)]
    pub rebuild_wiki_index: Option<bool>,
    /// Opt-in: re-embed all document chunks with the live embedder and refresh
    /// `embedding_manifest` (use after model/dims change). Cap: `RAG_MAINT_MAX_DOCS`.
    #[serde(default)]
    pub reembed_all: Option<bool>,
    /// When true, report planned work without mutating store (still logs dry_run).
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Override `RAG_MAINT_MAX_DOCS` for this call (graph rebuild / reembed cap).
    #[serde(default)]
    pub max_docs: Option<u32>,
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

/// One action in an `apply_maintenance_plan` request (mirrors maintain plan JSON).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MaintenanceActionParams {
    /// Whitelisted op name (refile, pin, archive, rebuild_index, …).
    pub action: String,
    /// Human rationale from planner (optional).
    #[serde(default)]
    pub reason: Option<String>,
    /// Target document / node / slug id when applicable.
    #[serde(default)]
    pub target_id: Option<String>,
    /// Free-form parameters (wing, room, boost, tags, …) as a JSON object.
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// Parameters for `apply_maintenance_plan`.
///
/// Executes a whitelist-only action list. **`dry_run` defaults to true** (preview
/// without mutation; still logs). Document-scoped ops are capped by
/// `max_docs` / `RAG_MAINT_MAX_DOCS`. Every action is ops_logged; returns
/// `applied` / `skipped` / `errors`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ApplyMaintenancePlanParams {
    /// Ordered whitelist actions to apply (or preview).
    pub actions: Vec<MaintenanceActionParams>,
    /// When true (default), preview without mutating store.
    /// Set `false` to execute.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Override `RAG_MAINT_MAX_DOCS` for document-scoped steps this run.
    #[serde(default)]
    pub max_docs: Option<u32>,
    /// Optional agent name for ops_log.
    #[serde(default)]
    pub agent: Option<String>,
}

/// Parameters for `maintain_compress` (L0 checkpoint+FTS, L1 exact hash merge,
/// L2 near-dup list; merge only with `confirm`).
///
/// Never hard-deletes `layer=raw` without `allow_raw_delete=true` (tombstones
/// instead). Default `dry_run=true` for safety.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct MaintainCompressParams {
    /// Compression level: 0=checkpoint+FTS, 1=+exact hash merge, 2=+near-dup list
    /// (near-dup merge only when `confirm=true`). Default 0.
    #[serde(default)]
    pub level: Option<u32>,
    /// When true (default), plan only — no mutations. Set false to apply.
    #[serde(default)]
    pub dry_run: Option<bool>,
    /// Required to apply near-duplicate merges at level ≥ 2. List always runs.
    #[serde(default)]
    pub confirm: Option<bool>,
    /// When true, hard-delete raw-layer duplicates. Default false (tombstone).
    #[serde(default)]
    pub allow_raw_delete: Option<bool>,
    /// Cosine threshold θ for near-dups (default `RAG_MAINT_NEAR_DUP_THRESHOLD`).
    #[serde(default)]
    pub near_dup_threshold: Option<f64>,
    /// Cap docs removed/tombstoned per run (default `RAG_MAINT_MAX_DOCS`).
    #[serde(default)]
    pub max_docs: Option<u32>,
}

// --- Temporal knowledge graph (`kg_facts`) ---

/// Parameters for `kg_add` (insert temporal SPO fact; open-active SPO is idempotent).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct KgAddParams {
    /// Fact subject (entity / name).
    pub subject: String,
    /// Relation label (normalized: lowercased, spaces → underscores).
    pub predicate: String,
    /// Fact object / value.
    pub object: String,
    /// Optional validity start (RFC3339 or `YYYY-MM-DD[ HH:MM:SS]`); null = open past.
    #[serde(default)]
    pub valid_from: Option<String>,
    /// Optional validity end (half-open upper bound); null = open future.
    #[serde(default)]
    pub valid_to: Option<String>,
    /// Optional provenance document id (also stored as free-text `source`).
    #[serde(default)]
    pub source_document_id: Option<String>,
    /// Confidence score, typically 0.0–1.0 (default 1.0).
    #[serde(default)]
    pub confidence: Option<f64>,
    /// Optional metadata JSON object string (default `{}`).
    #[serde(default)]
    pub metadata_json: Option<String>,
}

/// Parameters for `kg_query` (filter active facts, or all statuses valid at `at_time`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct KgQueryParams {
    /// Filter by subject.
    #[serde(default)]
    pub subject: Option<String>,
    /// Filter by predicate (case-insensitive; spaces → underscores).
    #[serde(default)]
    pub predicate: Option<String>,
    /// Filter by object.
    #[serde(default)]
    pub object: Option<String>,
    /// Point-in-time query (RFC3339 / date). When set, uses half-open
    /// `[valid_from, valid_to)` instead of `status = active`.
    #[serde(default)]
    pub at_time: Option<String>,
}

/// Parameters for `kg_invalidate` (close open active fact(s) matching SPO).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct KgInvalidateParams {
    /// Fact subject.
    pub subject: String,
    /// Relation label.
    pub predicate: String,
    /// Fact object.
    pub object: String,
    /// End of validity (default now). Must not be before `valid_from`.
    #[serde(default)]
    pub ended: Option<String>,
}

/// Parameters for `kg_supersede` (close old object, open successor at shared boundary).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct KgSupersedeParams {
    /// Fact subject.
    pub subject: String,
    /// Relation label.
    pub predicate: String,
    /// Object value being replaced.
    pub old_object: String,
    /// Successor object value.
    pub new_object: String,
    /// Shared boundary instant (default now); old gets `valid_to=at`, new `valid_from=at`.
    #[serde(default)]
    pub at: Option<String>,
    /// Optional provenance document id for the successor fact.
    #[serde(default)]
    pub source_document_id: Option<String>,
    /// Confidence for the successor (default 1.0).
    #[serde(default)]
    pub confidence: Option<f64>,
}

/// Parameters for `kg_timeline` (all facts for a subject, any status, chronological).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct KgTimelineParams {
    /// Subject whose fact history to return.
    pub subject: String,
}
