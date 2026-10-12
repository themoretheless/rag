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
}

impl SearchHit {
    /// Snippet when the server sent one, else the chunk content.
    pub fn visible_excerpt(&self) -> &str {
        self.snippet.as_deref().unwrap_or(&self.content)
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
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Wiki => "wiki",
            Self::Schema => "schema",
            Self::Diary => "diary",
            Self::Document => "document",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::Raw,
            Self::Wiki,
            Self::Schema,
            Self::Diary,
            Self::Document,
        ]
        .into_iter()
        .find(|layer| layer.as_str() == value)
    }

    /// True when `value` is exactly this layer's wire string.
    pub fn is(self, value: &str) -> bool {
        self.as_str() == value
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

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Tag => "tag",
            Self::Stub => "stub",
            Self::Entity => "entity",
        }
    }

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
        assert_eq!(Layer::parse("Wiki"), None);
        assert!(Layer::Raw.is("raw"));
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
