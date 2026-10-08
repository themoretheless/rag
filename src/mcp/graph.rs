//! Object-graph navigation, tunnels, and graph-expanded retrieval tools.

use std::path::Path;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router, ErrorData as McpError};
use serde::Serialize;

use super::facade::RagServer;
use super::tools::{
    CreateTunnelParams, DeleteTunnelParams, ExportGraphSnapshotParams, FindNodeParams,
    FindTunnelsParams, FollowTunnelsParams, GetBacklinksParams, GetGraphParams, GetNeighborsParams,
    GraphExpandSearchParams, LinkNodesParams, ListTunnelsParams,
};
use crate::db::search::{acquire_corpus_search_guard, search_with_corpus_guard, SearchQuery};
use crate::error::AppError;
use crate::models::{pkb_node_kinds, pkb_rel_types};
use crate::models::{GraphEdge, GraphFilter, GraphNode, GraphView, SearchHit, SearchMode};
use crate::util::run_blocking;

#[tool_router(router = graph_router, vis = "pub(super)")]
impl RagServer {
    #[tool(
        name = "get_graph",
        description = "Export object graph topology {nodes, edges}. Defaults are the PKB set (§7.1): document/stub/entity kinds and the literary relations; pass include_tags=true for tag hubs, or kinds/rel_types/seed_ids to filter explicitly."
    )]
    pub(super) async fn get_graph(
        &self,
        Parameters(params): Parameters<GetGraphParams>,
    ) -> Result<CallToolResult, McpError> {
        let include_tags = params.include_tags.unwrap_or(false);
        let kinds = params.kinds.or_else(|| Some(pkb_node_kinds(include_tags)));
        // Naming tag nodes as a kind means the caller wants the edges that reach them.
        let wants_tags = include_tags
            || kinds
                .as_deref()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "tag"));
        let filter = GraphFilter {
            max_nodes: params.max_nodes,
            rel_types: params.rel_types.or_else(|| Some(pkb_rel_types(wants_tags))),
            seed_ids: params.seed_ids,
            kinds,
        };
        let view = self.store.get_graph_view(filter)?;
        Self::json_result(&view)
    }

    /// Write GraphView JSON for rag-mcp-ui while this process holds the DuckDB lock.
    /// Concurrent UI must use `--snapshot` (not a second `--db` open).
    #[tool(
        name = "export_graph_snapshot",
        description = "Write GraphView topology JSON to disk for rag-mcp-ui (Mode C). Use while MCP holds the DB; then run: rag-mcp-ui --snapshot PATH --seed LABEL. Path defaults to <db_dir>/graph.json; must be under DB directory or RAG_INGEST_ROOTS."
    )]
    pub(super) async fn export_graph_snapshot(
        &self,
        Parameters(params): Parameters<ExportGraphSnapshotParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_nodes = params.max_nodes.unwrap_or(500);
        let include_tags = params.include_tags.unwrap_or(false);
        #[derive(Serialize)]
        struct ExportSnapResult {
            path: String,
            node_count: usize,
            edge_count: usize,
            max_nodes: u32,
            include_tags: bool,
            ui_hint: String,
        }
        let requested_path = params.path;
        let store = self.store.clone();
        let config = self.config.clone();
        let report = run_blocking("export_graph_snapshot", move || {
            let view = store.export_graph_for_ui(Some(max_nodes), include_tags)?;
            let default_path = store
                .path()
                .parent()
                .map(|parent| parent.join("graph.json"))
                .unwrap_or_else(|| Path::new("graph.json").to_path_buf());
            let out = requested_path
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(Path::new)
                .map(Path::to_path_buf)
                .unwrap_or(default_path);
            let out = std::fs::canonicalize(out.parent().unwrap_or(Path::new(".")))
                .map(|parent| {
                    parent.join(
                        out.file_name()
                            .unwrap_or_else(|| std::ffi::OsStr::new("graph.json")),
                    )
                })
                .unwrap_or(out.clone());

            let mut allowed = config.ingest_roots;
            if let Some(parent) = store.path().parent() {
                allowed.push(parent.to_path_buf());
            }
            let out_canon = if out.exists() {
                std::fs::canonicalize(&out).unwrap_or(out.clone())
            } else if let Some(parent) = out.parent() {
                let parent_c =
                    std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
                parent_c.join(out.file_name().unwrap_or_default())
            } else {
                out.clone()
            };
            if !allowed.iter().any(|root| {
                let root_c = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
                out_canon.starts_with(&root_c)
            }) {
                return Err(AppError::forbidden(format!(
                    "export path {} is outside DB directory and RAG_INGEST_ROOTS",
                    out.display()
                )));
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    AppError::db(format!("create parent for {}: {error}", out.display()))
                })?;
            }
            let json = serde_json::to_vec_pretty(&view)
                .map_err(|error| AppError::db(format!("serialize GraphView: {error}")))?;
            std::fs::write(&out, json)
                .map_err(|error| AppError::db(format!("write {}: {error}", out.display())))?;

            Ok(ExportSnapResult {
                path: out.display().to_string(),
                node_count: view.nodes.len(),
                edge_count: view.edges.len(),
                max_nodes,
                include_tags,
                ui_hint: format!(
                    "Run: rag-mcp-ui --snapshot {} --seed <title_or_node_id>",
                    out.display()
                ),
            })
        })
        .await?;
        Self::json_result(&report)
    }

    #[tool(
        name = "get_neighbors",
        description = "Local undirected BFS subgraph around a node (Obsidian local graph). Default edges are the PKB literary set (wikilink, related, and the hand-authored wiki relations such as реализует/проверяет); pass include_tags=true to also walk tag hubs, or rel_types to name the relations you want (tunnel, mentions, derived_from, depends_on are opt-in there)."
    )]
    pub(super) async fn get_neighbors(
        &self,
        Parameters(params): Parameters<GetNeighborsParams>,
    ) -> Result<CallToolResult, McpError> {
        let depth = params.depth.unwrap_or(1);
        let max_nodes = params.max_nodes.unwrap_or(100);
        let rel_types = match params.rel_types.as_deref() {
            Some(requested) if !requested.is_empty() => requested.to_vec(),
            _ => pkb_rel_types(params.include_tags.unwrap_or(false)),
        };
        let view =
            self.store
                .neighbors_filtered(&params.node_id, depth, max_nodes, Some(&rel_types))?;
        Self::json_result(&view)
    }

    #[tool(
        name = "get_backlinks",
        description = "Incoming edges and source nodes for a node (resolve by node_id, document_id, or label)."
    )]
    pub(super) async fn get_backlinks(
        &self,
        Parameters(params): Parameters<GetBacklinksParams>,
    ) -> Result<CallToolResult, McpError> {
        let target = if let Some(id) = params
            .node_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.store.find_node_by_id(id)?
        } else if let Some(doc_id) = params
            .document_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.store.find_node_by_document_id(doc_id)?
        } else if let Some(label) = params
            .label
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.store.find_nodes_by_label(label)?.into_iter().next()
        } else {
            return Err(McpError::from(AppError::config(
                "get_backlinks requires node_id, document_id, or label",
            )));
        }
        .ok_or_else(|| {
            McpError::from(AppError::not_found(
                "graph node not found (provide node_id, document_id, or label)",
            ))
        })?;
        let view = self.store.backlinks(&target.id)?;
        Self::json_result(&view)
    }

    #[tool(
        name = "link_nodes",
        description = "Create an explicit graph edge (default rel_type=related). Accepted rel_type: the structural names (wikilink, tagged, mentions, related, tunnel, depends_on, derived_from, supersedes) or a hand-authored wiki relation (реализует, проверяет, детализирует, зависит от, ...); anything else is rejected. The edge is origin=explicit, so rebuild never owns it."
    )]
    pub(super) async fn link_nodes(
        &self,
        Parameters(params): Parameters<LinkNodesParams>,
    ) -> Result<CallToolResult, McpError> {
        let rel = params.rel_type.as_deref().unwrap_or("related");
        let weight = params.weight.unwrap_or(1.0);
        let edge = self
            .store
            .link_nodes(&params.source_id, &params.target_id, rel, weight)?;
        Self::json_result(&edge)
    }

    #[tool(
        name = "create_tunnel",
        description = "Create an explicit tunnel edge (rel_type=tunnel) between two graph nodes. Same pair either order updates weight/context."
    )]
    pub(super) async fn create_tunnel(
        &self,
        Parameters(params): Parameters<CreateTunnelParams>,
    ) -> Result<CallToolResult, McpError> {
        let weight = params.weight.unwrap_or(1.0);
        let context = params
            .context
            .as_deref()
            .or(params.label.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let edge =
            self.store
                .create_tunnel(&params.source_id, &params.target_id, weight, context)?;
        Self::json_result(&edge)
    }

    #[tool(
        name = "list_tunnels",
        description = "List tunnel edges (rel_type=tunnel). Optional node_id filters to incident tunnels."
    )]
    pub(super) async fn list_tunnels(
        &self,
        Parameters(params): Parameters<ListTunnelsParams>,
    ) -> Result<CallToolResult, McpError> {
        let edges = self.store.list_tunnels(params.node_id.as_deref())?;
        Self::json_result(&ListTunnelsResult {
            count: edges.len() as u64,
            tunnels: edges,
        })
    }

    #[tool(
        name = "delete_tunnel",
        description = "Delete a tunnel edge by id (only rows with rel_type=tunnel)."
    )]
    pub(super) async fn delete_tunnel(
        &self,
        Parameters(params): Parameters<DeleteTunnelParams>,
    ) -> Result<CallToolResult, McpError> {
        let deleted = self.store.delete_tunnel(&params.tunnel_id)?;
        if !deleted {
            return Err(McpError::from(AppError::not_found(format!(
                "tunnel not found: {}",
                params.tunnel_id
            ))));
        }
        Self::json_result(&DeleteTunnelResult {
            deleted: true,
            tunnel_id: params.tunnel_id,
        })
    }

    #[tool(
        name = "follow_tunnels",
        description = "Multi-hop undirected BFS along tunnel edges only (node_id, depth?=2, max_nodes?=100)."
    )]
    pub(super) async fn follow_tunnels(
        &self,
        Parameters(params): Parameters<FollowTunnelsParams>,
    ) -> Result<CallToolResult, McpError> {
        let depth = params.depth.unwrap_or(2);
        let max_nodes = params.max_nodes.unwrap_or(100);
        let view = self
            .store
            .follow_tunnels(&params.node_id, depth, max_nodes)?;
        Self::json_result(&view)
    }

    #[tool(
        name = "find_tunnels",
        description = "Find tunnel edges: node_id?, other_node_id? (pair), wing? (endpoint document wing), limit?."
    )]
    pub(super) async fn find_tunnels(
        &self,
        Parameters(params): Parameters<FindTunnelsParams>,
    ) -> Result<CallToolResult, McpError> {
        let edges = self.store.find_tunnels(
            params.node_id.as_deref(),
            params.other_node_id.as_deref(),
            params.wing.as_deref(),
            params.limit,
        )?;
        Self::json_result(&ListTunnelsResult {
            count: edges.len() as u64,
            tunnels: edges,
        })
    }

    #[tool(
        name = "graph_stats",
        description = "Object graph aggregates: total nodes/edges, node counts by kind, edge counts by rel_type (includes tunnel), and edge counts cut by (rel_type, origin, context) so hand-authored relations stay separate from rebuild-owned membership and tag edges."
    )]
    pub(super) async fn graph_stats(&self) -> Result<CallToolResult, McpError> {
        let body = self.store.graph_stats()?;
        Self::json_result(&body)
    }

    #[tool(
        name = "find_node",
        description = "Resolve graph node metadata by node_id, document_id, or exact label."
    )]
    pub(super) async fn find_node(
        &self,
        Parameters(params): Parameters<FindNodeParams>,
    ) -> Result<CallToolResult, McpError> {
        if params
            .node_id
            .as_ref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
            && params
                .document_id
                .as_ref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
            && params
                .label
                .as_ref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
        {
            return Err(McpError::from(AppError::config(
                "find_node requires node_id, document_id, or label",
            )));
        }

        // When label is the only key, return all label matches; otherwise single resolve.
        let only_label = params
            .node_id
            .as_ref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
            && params
                .document_id
                .as_ref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
            && params
                .label
                .as_ref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);

        if only_label {
            let label = params.label.as_deref().unwrap_or("").trim();
            let nodes = self.store.find_nodes_by_label(label)?;
            return Self::json_result(&FindNodeResult {
                node: nodes.first().cloned(),
                matches: nodes,
            });
        }

        let node = self.store.resolve_node(
            params.node_id.as_deref(),
            params.document_id.as_deref(),
            params.label.as_deref(),
        )?;
        let matches = match &node {
            Some(n) => vec![n.clone()],
            None => Vec::new(),
        };
        Self::json_result(&FindNodeResult { node, matches })
    }

    #[tool(
        name = "graph_expand_search",
        description = "Semantic search then expand neighbor subgraph from hit document nodes."
    )]
    pub(super) async fn graph_expand_search(
        &self,
        Parameters(params): Parameters<GraphExpandSearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let corpus_guard = acquire_corpus_search_guard(&self.store, SearchMode::Vec)?;
        self.require_vec_compatible()?;

        let top_k = params
            .top_k
            .map(|k| k as usize)
            .unwrap_or(self.config.default_top_k);
        let depth = params.depth.unwrap_or(1);
        let max_nodes = params.max_nodes.unwrap_or(100);

        let query_vecs = self.embedder.embed(&[params.query]).await?;
        let query_emb = query_vecs.into_iter().next().ok_or_else(|| {
            McpError::internal_error("embedder returned no vector for query", None)
        })?;

        let hits = search_with_corpus_guard(
            &self.store,
            &SearchQuery {
                mode: SearchMode::Vec,
                top_k,
                query_embedding: Some(query_emb),
                document_id: params.document_id,
                ..SearchQuery::default()
            },
            &corpus_guard,
        )?;

        let mut merged_nodes: std::collections::HashMap<String, GraphNode> =
            std::collections::HashMap::new();
        let mut merged_edges: std::collections::HashMap<String, crate::models::GraphEdge> =
            std::collections::HashMap::new();

        let mut seen_doc_nodes = std::collections::HashSet::new();
        for hit in &hits {
            let Some(node) = self.store.find_node_by_document_id(&hit.document_id)? else {
                continue;
            };
            if !seen_doc_nodes.insert(node.id.clone()) {
                continue;
            }
            let local = self.store.neighbors(&node.id, depth, max_nodes)?;
            for n in local.nodes {
                merged_nodes.entry(n.id.clone()).or_insert(n);
            }
            for e in local.edges {
                merged_edges.entry(e.id.clone()).or_insert(e);
            }
            if merged_nodes.len() >= max_nodes as usize {
                break;
            }
        }

        // Cap nodes if over max_nodes after merge.
        let mut nodes: Vec<GraphNode> = merged_nodes.into_values().collect();
        if nodes.len() > max_nodes as usize {
            nodes.truncate(max_nodes as usize);
        }
        let keep: std::collections::HashSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
        let edges: Vec<_> = merged_edges
            .into_values()
            .filter(|e| keep.contains(&e.source_id) && keep.contains(&e.target_id))
            .collect();

        let body = GraphExpandSearchResult {
            hits,
            graph: GraphView { nodes, edges },
        };
        Self::json_result(&body)
    }
}

#[derive(Debug, Serialize)]
struct FindNodeResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    node: Option<GraphNode>,
    matches: Vec<GraphNode>,
}

#[derive(Debug, Serialize)]
struct GraphExpandSearchResult {
    hits: Vec<SearchHit>,
    graph: GraphView,
}

#[derive(Debug, Serialize)]
struct ListTunnelsResult {
    count: u64,
    tunnels: Vec<GraphEdge>,
}

#[derive(Debug, Serialize)]
struct DeleteTunnelResult {
    deleted: bool,
    tunnel_id: String,
}
