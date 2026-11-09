//! Object graph: layers, node kinds, topology payloads and PKB read defaults.

use serde::{Deserialize, Serialize};

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

/// Edge relations a client styles by name. Stored and sent as plain strings;
/// this type names the ones with dedicated drawing rules. Hand-authored wiki
/// relations ([`WIKI_SEMANTIC_REL_TYPES`]) and future names parse to `None`
/// and are drawn with the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelType {
    Wikilink,
    Related,
    Similar,
    Contains,
    Imports,
    Describes,
    Tunnel,
    Tagged,
    Mentions,
    DependsOn,
    DerivedFrom,
    Cites,
    Supports,
    Contradicts,
    Supersedes,
}

impl RelType {
    /// Every named relation.
    pub const ALL: [Self; 15] = [
        Self::Wikilink,
        Self::Related,
        Self::Similar,
        Self::Contains,
        Self::Imports,
        Self::Describes,
        Self::Tunnel,
        Self::Tagged,
        Self::Mentions,
        Self::DependsOn,
        Self::DerivedFrom,
        Self::Cites,
        Self::Supports,
        Self::Contradicts,
        Self::Supersedes,
    ];

    /// Stroke width of a relation without dedicated styling.
    pub const DEFAULT_BASE_WIDTH: f32 = 1.2;

    /// Wire string for this relation (`wikilink`, `depends_on`, ...).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wikilink => "wikilink",
            Self::Related => "related",
            Self::Similar => "similar",
            Self::Contains => "contains",
            Self::Imports => "imports",
            Self::Describes => "describes",
            Self::Tunnel => "tunnel",
            Self::Tagged => "tagged",
            Self::Mentions => "mentions",
            Self::DependsOn => "depends_on",
            Self::DerivedFrom => "derived_from",
            Self::Cites => "cites",
            Self::Supports => "supports",
            Self::Contradicts => "contradicts",
            Self::Supersedes => "supersedes",
        }
    }

    /// Parse a relation from its exact wire string.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|rel| rel.as_str() == value)
    }

    /// True when `value` is exactly this relation's wire string.
    pub fn is(self, value: &str) -> bool {
        self.as_str() == value
    }

    /// True when the relation is drawn with an arrowhead at its target.
    pub const fn is_directed(self) -> bool {
        matches!(
            self,
            Self::Wikilink | Self::DependsOn | Self::DerivedFrom | Self::Supersedes
        )
    }

    /// True when the relation is drawn dashed (weak associations).
    pub const fn is_dashed(self) -> bool {
        matches!(self, Self::Tagged | Self::Mentions)
    }

    /// Stroke width before weight and multi-edge scaling.
    pub const fn base_width(self) -> f32 {
        match self {
            Self::Related => 0.8,
            Self::Tunnel => 2.2,
            Self::Tagged | Self::Mentions => 1.0,
            _ => Self::DEFAULT_BASE_WIDTH,
        }
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

#[cfg(test)]
mod tests {
    use super::{Layer, NodeKind, RelType, WIKI_SEMANTIC_REL_TYPES};

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
    fn pkb_relation_counts_bound_graph_edge_queries() {
        // db::graph sizes its bounded edge queries with these counts.
        assert_eq!(super::pkb_rel_types(true).len(), 14);
        assert_eq!(super::pkb_rel_types(false).len(), 13);
    }

    #[test]
    fn node_kind_round_trips_wire_strings() {
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

    #[test]
    fn rel_type_round_trips_wire_strings_and_rejects_unknown_names() {
        for rel in RelType::ALL {
            assert_eq!(RelType::parse(rel.as_str()), Some(rel));
            assert!(rel.is(rel.as_str()));
        }
        assert_eq!(RelType::parse("Wikilink"), None);
        assert_eq!(RelType::parse(""), None);
        for semantic in WIKI_SEMANTIC_REL_TYPES {
            assert_eq!(RelType::parse(semantic), None);
        }
        // Every structural relation the PKB read set walks has a name here.
        for wire in super::PKB_CORE_REL_TYPES {
            assert!(RelType::parse(wire).is_some());
        }
    }

    #[test]
    fn rel_type_drawing_rules_match_the_canvas_contract() {
        let directed: Vec<_> = RelType::ALL
            .into_iter()
            .filter(|rel| rel.is_directed())
            .map(RelType::as_str)
            .collect();
        assert_eq!(
            directed,
            ["wikilink", "depends_on", "derived_from", "supersedes"]
        );
        let dashed: Vec<_> = RelType::ALL
            .into_iter()
            .filter(|rel| rel.is_dashed())
            .map(RelType::as_str)
            .collect();
        assert_eq!(dashed, ["tagged", "mentions"]);
        // A dashed stroke never carries an arrowhead.
        assert!(RelType::ALL
            .into_iter()
            .all(|rel| !(rel.is_dashed() && rel.is_directed())));

        assert_eq!(RelType::Related.base_width(), 0.8);
        assert_eq!(RelType::Tunnel.base_width(), 2.2);
        assert_eq!(RelType::Tagged.base_width(), 1.0);
        assert_eq!(RelType::Mentions.base_width(), 1.0);
        assert_eq!(RelType::Wikilink.base_width(), RelType::DEFAULT_BASE_WIDTH);
        assert_eq!(RelType::Cites.base_width(), 1.2);
    }
}
