//! Wire types shared by the rag-mcp server and its clients.
//!
//! Serde-only on purpose: the native UI and other clients depend on this crate
//! instead of the server crate (DuckDB, axum, rmcp, tokio). The server
//! re-exports everything here from `rag_mcp::models`, so paths are unchanged.

use serde::{Deserialize, Serialize};

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
