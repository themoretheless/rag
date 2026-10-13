//! Graph and tunnels tool parameters.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

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
    /// Add tag nodes and `tagged` edges to the PKB defaults of §7.1 (tags are off
    /// unless asked for). Ignored when `kinds` / `rel_types` are supplied.
    #[serde(default)]
    pub include_tags: Option<bool>,
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
    /// BFS hop depth (default 1, hard cap 5).
    #[serde(default)]
    pub depth: Option<u32>,
    /// Max nodes in the local subgraph (default 100).
    #[serde(default)]
    pub max_nodes: Option<u32>,
    /// Edge relation types to follow. Default is the PKB literary set
    /// (`wikilink`, `related`, …); pass a list to opt into `tunnel`,
    /// `mentions`, `derived_from` and the rest explicitly.
    #[serde(default)]
    pub rel_types: Option<Vec<String>>,
    /// Add tag hubs to the default walk (`tagged` edges). Ignored when
    /// `rel_types` is given. Default false.
    #[serde(default)]
    pub include_tags: Option<bool>,
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
    /// Edge type (default `related`). Accepts the structural names plus the
    /// hand-authored wiki relations in §1.1 (`реализует`, `проверяет`, …);
    /// unknown names are rejected at the store boundary.
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
