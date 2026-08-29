//! Object-graph navigation, tunnels, and graph-expanded retrieval tools.

use super::facade::RagServer;
use super::take_routes;
use rmcp::handler::server::tool::ToolRouter;

const TOOLS: &[&str] = &[
    "get_graph", "export_graph_snapshot", "get_neighbors", "get_backlinks",
    "link_nodes", "create_tunnel", "list_tunnels", "delete_tunnel",
    "follow_tunnels", "find_tunnels", "graph_stats", "find_node",
    "graph_expand_search",
];

pub(super) fn router(all: &mut ToolRouter<RagServer>) -> ToolRouter<RagServer> {
    take_routes(all, TOOLS)
}
