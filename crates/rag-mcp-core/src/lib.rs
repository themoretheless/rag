//! Wire types shared by the rag-mcp server and its clients.
//!
//! Serde-only on purpose: the native UI and other clients depend on this crate
//! instead of the server crate (DuckDB, axum, rmcp, tokio). The server
//! re-exports everything here from `rag_mcp::models`, so paths are unchanged.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
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
    /// LLM HyDE passage generation time (only when `hyde=true` and it ran).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hyde_ms: Option<f64>,
    /// LLM query-rewrite time (only when `rewrite=true` and variants ran).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewrite_ms: Option<f64>,
    /// Rerank stage time (only when `rerank=true` and a provider ran).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchContextChunk {
    pub chunk_id: String,
    pub chunk_index: i32,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_path: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

/// One hit from semantic / hybrid search.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
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
    /// Rerank provider relevance score when `rerank=true` and a provider ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_rerank: Option<f32>,
}

impl SearchHit {
    /// Snippet when the server sent one, else the chunk content.
    pub fn visible_excerpt(&self) -> &str {
        self.snippet.as_deref().unwrap_or(&self.content)
    }
}

/// Lean source-root aggregate for project registry / status surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRootSummary {
    pub canonical_root: String,
    pub file_count: u64,
    pub last_seen: Option<String>,
    pub last_synced: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHome {
    pub project_id: String,
    pub documents: u64,
    pub chunks: u64,
    pub raw_documents: u64,
    pub wiki_documents: u64,
    pub diary_documents: u64,
    pub archived_documents: u64,
    pub rooms: u64,
    pub source_files: u64,
    pub graph_nodes: u64,
    pub graph_edges: u64,
    pub unresolved_links: u64,
    #[serde(default)]
    pub source_roots: Vec<SourceRootSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

// --- Background source-sync jobs (`/v1/jobs`) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSyncError {
    pub path: String,
    pub error: String,
}

/// Structured terminal failure after source-sync mutations have already run.
///
/// This is additive to the existing per-path `errors` list so older clients
/// still classify the run as completed-with-errors, while newer clients can
/// distinguish a derived-index failure from a failed or rolled-back sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSyncFinalizationError {
    pub code: String,
    pub stage: String,
    pub durable_mutation_committed: bool,
    pub retryable: bool,
    pub fallback_dirty_marked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty_marker_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops_log_error: Option<String>,
    pub error: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceSyncCounters {
    pub preflight: u64,
    pub extracted: u64,
    pub embedded: u64,
}

/// Observable phases for a cooperative source synchronization run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSyncPhase {
    Scanning,
    Syncing,
    RemovingDeleted,
    RefreshingFts,
    Completed,
    Cancelled,
    /// A phase this build does not know (newer server).
    #[serde(other)]
    Unknown,
}

/// Progress snapshot emitted while a source tree is synchronized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSyncProgress {
    pub phase: SourceSyncPhase,
    pub total_files: usize,
    pub processed_files: usize,
    #[serde(default)]
    pub current_path: Option<String>,
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
    pub deleted: usize,
    pub errors: usize,
    #[serde(default)]
    pub counters: SourceSyncCounters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    CompletedWithErrors,
    Failed,
    Cancelled,
    /// A status this build does not know (newer server or stored job).
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSyncJobReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_index: Option<IndexReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_index_error: Option<SourceSyncFinalizationError>,
    pub added_count: usize,
    pub updated_count: usize,
    pub skipped_count: usize,
    pub deleted_count: usize,
    pub error_count: usize,
    #[serde(default)]
    pub error_samples: Vec<SourceSyncError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finalization_error: Option<SourceSyncFinalizationError>,
    #[serde(default)]
    pub counters: SourceSyncCounters,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub id: String,
    pub kind: String,
    pub status: JobStatus,
    pub request: SyncJobRequest,
    #[serde(default)]
    pub progress: Option<SourceSyncProgress>,
    #[serde(default)]
    pub report: Option<SourceSyncJobReport>,
    #[serde(default)]
    pub error: Option<String>,
    pub cancel_requested: bool,
    /// True when a new sync with the same request is safe after interrupt/failure.
    /// Source sync is committed-prefix aware (unchanged files skip); mid-batch
    /// embeds are not resumed.
    #[serde(default)]
    pub requeue_safe: bool,
    /// Human-readable resume limits when `requeue_safe` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_limits: Option<String>,
    /// Job id this snapshot was requeued from, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requeued_from: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl JobStatus {
    /// True once the job can no longer change. Unknown statuses count as
    /// terminal so a client never offers to cancel what it cannot classify.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::CompletedWithErrors
                | Self::Failed
                | Self::Cancelled
                | Self::Unknown
        )
    }

    /// Wire string (`queued`, `running`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::CompletedWithErrors => "completed_with_errors",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

impl SourceSyncPhase {
    /// Wire string (`scanning`, `syncing`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scanning => "scanning",
            Self::Syncing => "syncing",
            Self::RemovingDeleted => "removing_deleted",
            Self::RefreshingFts => "refreshing_fts",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

/// Known document layers. Stored and sent as plain strings (`raw`, `wiki`, ...);
/// this type only gives comparisons a name. Unknown stored values parse to `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layer {
    /// Immutable ingested source; only same-uri re-ingest replaces content.
    Raw,
    /// Compiled wiki page.
    Wiki,
    /// Agent conventions document (`schema://agents`).
    Schema,
    /// Agent diary entry.
    Diary,
    /// Legacy generic document layer, compiled like `raw`.
    Document,
}

impl Layer {
    /// Wire string for this layer (`raw`, `wiki`, `schema`, `diary`, `document`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Wiki => "wiki",
            Self::Schema => "schema",
            Self::Diary => "diary",
            Self::Document => "document",
        }
    }

    /// Known layer named by `value`, ignoring ASCII case.
    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::Raw,
            Self::Wiki,
            Self::Schema,
            Self::Diary,
            Self::Document,
        ]
        .into_iter()
        .find(|layer| layer.is(value))
    }

    /// The spelling to store: a known layer in lowercase, anything else
    /// trimmed as given. SQL filters compare exactly, so writes canonicalize.
    pub fn canonicalize(value: &str) -> std::borrow::Cow<'_, str> {
        let value = value.trim();
        match Self::parse(value) {
            Some(layer) => std::borrow::Cow::Borrowed(layer.as_str()),
            None => std::borrow::Cow::Borrowed(value),
        }
    }

    /// True when `value` names this layer, ignoring ASCII case. One rule for
    /// every check, so raw-layer immutability cannot depend on letter case.
    pub fn is(self, value: &str) -> bool {
        value.eq_ignore_ascii_case(self.as_str())
    }
}

impl std::fmt::Display for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One `/v1/activity` feed entry (HTTP request or MCP call).
///
/// `kind` is a `Cow` so the server records static labels without allocating
/// while clients deserialize owned strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub seq: u64,
    pub at: String,
    /// `http` | `mcp` | ...
    pub kind: std::borrow::Cow<'static, str>,
    pub client: Option<String>,
    pub action: String,
    pub status: Option<u16>,
    pub elapsed_ms: Option<f64>,
    pub request_id: Option<String>,
}

/// Graph node kinds. Stored and sent as plain strings; this type names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    /// A stored document (raw, wiki, ...).
    Document,
    /// A `#tag` hub.
    Tag,
    /// An unresolved `[[link]]` target.
    Stub,
    /// A named entity from the knowledge graph.
    Entity,
}

impl NodeKind {
    /// Accepted wire strings (`GRAPH_DESIGN.md` §1.1).
    pub const WIRE: &'static [&'static str] = &["document", "tag", "stub", "entity"];

    /// Wire string for this node kind (`document`, `tag`, `stub`, `entity`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Tag => "tag",
            Self::Stub => "stub",
            Self::Entity => "entity",
        }
    }

    /// Parse a node kind from its exact wire string.
    pub fn parse(value: &str) -> Option<Self> {
        [Self::Document, Self::Tag, Self::Stub, Self::Entity]
            .into_iter()
            .find(|kind| kind.as_str() == value)
    }

    /// True when `value` is exactly this kind's wire string.
    pub fn is(self, value: &str) -> bool {
        self.as_str() == value
    }
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

/// Relations a person authors by hand with `link_nodes`, mostly in wiki maps:
/// `детализирует`, `реализует`, `проверяет`, …
///
/// They are part of the §1.1 vocabulary but no extractor can produce them, so
/// they are always `origin = explicit` (§1.6) and are always document → document.
/// They exist in live graphs (118 edges measured 2026-09-25 on an auto-backup of
/// the gateway database), so the store boundary must accept them and §7.1 must
/// read them; a client that wants only the structural set names it explicitly.
pub const WIKI_SEMANTIC_REL_TYPES: &[&str] = &[
    "детализирует",
    "зависит от",
    "компенсируется",
    "вызывает",
    "публикует",
    "обновляет",
    "хранит",
    "проверяет",
    "использует",
    "изменяет схему",
    "реализует",
];

/// Optional filters for full/partial graph export.
#[derive(Debug, Clone, Default)]
pub struct GraphFilter {
    pub kinds: Option<Vec<String>>,
    pub rel_types: Option<Vec<String>>,
    pub seed_ids: Option<Vec<String>>,
    pub max_nodes: Option<u32>,
}

/// Maximum number of nodes returned by a UI-oriented graph export.
///
/// Rendering applies its own, much smaller viewport cap; this limit only keeps
/// the server/query boundary finite while allowing the complete current corpus
/// to be searched for a useful local focus.
pub const UI_GRAPH_EXPORT_MAX_NODES: u32 = 100_000;

/// Structural relations the PKB default read set walks (§7.1).
pub const PKB_CORE_REL_TYPES: &[&str] = &["wikilink", "related"];

/// Default node kinds for UI export (tags off unless explicitly included).
pub const PKB_NODE_KINDS: &[&str] = &["document", "stub", "entity"];

/// The PKB literary relation set (§7.1), shared by every PKB-defaulted read path.
///
/// `include_tags` adds the `tagged` edge, the only way tag hubs stay reachable.
/// This is the single source of that choice: the UI projections, the MCP
/// `get_neighbors` walk and `get_graph` all default through here, and the store
/// boundary accepts exactly this set plus the rest of `RelType::WIRE` (server).
pub fn pkb_rel_types(include_tags: bool) -> Vec<String> {
    let mut rel_types: Vec<String> = PKB_CORE_REL_TYPES
        .iter()
        .chain(WIKI_SEMANTIC_REL_TYPES.iter())
        .map(|rel| (*rel).to_string())
        .collect();
    if include_tags {
        rel_types.push("tagged".into());
    }
    rel_types
}

/// PKB node kinds (§7.1): tags are opt-in everywhere.
pub fn pkb_node_kinds(include_tags: bool) -> Vec<String> {
    let mut kinds: Vec<String> = PKB_NODE_KINDS
        .iter()
        .map(|kind| (*kind).to_string())
        .collect();
    if include_tags {
        kinds.push("tag".into());
    }
    kinds
}

impl GraphFilter {
    /// PKB defaults for exclusive `--db` inspector and Mode C snapshot dumps.
    ///
    /// - `rel_types`: [`pkb_rel_types`] — wikilink + related + the §1.1 wiki
    ///   semantic set (`include_tags` adds `tagged`)
    /// - kinds: document + stub + entity (`include_tags` adds `tag`)
    /// - `max_nodes`: `max_nodes` or [`UI_GRAPH_EXPORT_MAX_NODES`] (100,000)
    ///
    /// Topology only; no layout positions.
    pub fn pkb_ui_export(max_nodes: Option<u32>, include_tags: bool) -> Self {
        Self {
            kinds: Some(pkb_node_kinds(include_tags)),
            rel_types: Some(pkb_rel_types(include_tags)),
            seed_ids: None,
            max_nodes: Some(max_nodes.unwrap_or(UI_GRAPH_EXPORT_MAX_NODES)),
        }
    }
}

/// Body of `POST /v1/jobs/sync`: synchronize one allowlisted source root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncJobRequest {
    pub path: String,
    #[serde(default)]
    pub remove_deleted: bool,
    #[serde(default, alias = "project", skip_serializing_if = "Option::is_none")]
    pub wing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_bytes: Option<u64>,
}

/// Outcome of restoring an older revision as the new head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreRevisionResult {
    pub document_id: String,
    pub restored_from_revision: i64,
    pub revision: i64,
    pub etag: String,
    pub chunk_count: usize,
    pub node_id: String,
    pub edge_count: usize,
}

/// One changed line in a bounded revision diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionLineChange {
    /// `added` or `removed`.
    pub kind: String,
    pub line: usize,
    pub content: String,
    pub content_truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexReport {
    pub indexed: usize,
    pub unchanged: usize,
    pub unsupported: usize,
    pub parse_errors: Vec<String>,
    #[serde(default)]
    pub index_revision: Option<i64>,
    #[serde(default)]
    pub revision_created: bool,
}

#[cfg(test)]
mod tests {
    use super::Layer;

    #[test]
    fn layer_round_trips_wire_strings() {
        for layer in [
            Layer::Raw,
            Layer::Wiki,
            Layer::Schema,
            Layer::Diary,
            Layer::Document,
        ] {
            assert_eq!(Layer::parse(layer.as_str()), Some(layer));
        }
        assert_eq!(Layer::parse("Wiki"), Some(Layer::Wiki));
        assert_eq!(Layer::canonicalize(" RAW "), "raw");
        assert_eq!(Layer::canonicalize("custom"), "custom");
        assert!(Layer::Raw.is("raw") && Layer::Raw.is("RAW") && !Layer::Raw.is("raw2"));
    }

    #[test]
    fn job_snapshot_tolerates_newer_servers() {
        use super::{JobSnapshot, JobStatus, SourceSyncPhase};
        let job: JobSnapshot = serde_json::from_value(serde_json::json!({
            "id": "j", "kind": "source_sync", "status": "paused_by_operator",
            "request": {"path": "/vault"},
            "progress": {"phase": "rebalancing", "total_files": 2, "processed_files": 1,
                         "added": 0, "updated": 0, "skipped": 0, "deleted": 0, "errors": 0},
            "cancel_requested": false,
            "created_at": "2026-10-01T12:00:00Z"
        }))
        .unwrap();
        assert_eq!(job.status, JobStatus::Unknown);
        assert!(job.status.is_terminal());
        assert_eq!(job.progress.unwrap().phase, SourceSyncPhase::Unknown);
        assert!(job.report.is_none() && job.started_at.is_none());
        assert_eq!(
            serde_json::to_value(JobStatus::CompletedWithErrors).unwrap(),
            JobStatus::CompletedWithErrors.as_str()
        );
    }

    #[test]
    fn pkb_relation_counts_bound_graph_edge_queries() {
        // db::graph sizes its bounded edge queries with these counts.
        assert_eq!(super::pkb_rel_types(true).len(), 14);
        assert_eq!(super::pkb_rel_types(false).len(), 13);
    }

    #[test]
    fn node_kind_round_trips_wire_strings() {
        use super::NodeKind;
        for kind in [
            NodeKind::Document,
            NodeKind::Tag,
            NodeKind::Stub,
            NodeKind::Entity,
        ] {
            assert_eq!(NodeKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(NodeKind::parse("wiki"), None);
    }
}
