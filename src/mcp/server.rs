//! Thin composition layer for the domain-clustered MCP tool router.

use rmcp::handler::server::tool::ToolRouter;

use super::{collections, graph, ingest, kg, maintain, search, wiki};
pub use super::facade::RagServer;

/// Compose the public MCP facade from domain-owned tool registrations.
pub(super) fn compose_tool_router(mut all: ToolRouter<RagServer>) -> ToolRouter<RagServer> {
    let mut router = ToolRouter::new();
    router.merge(ingest::router(&mut all));
    router.merge(collections::router(&mut all));
    router.merge(search::router(&mut all));
    router.merge(graph::router(&mut all));
    router.merge(wiki::router(&mut all));
    router.merge(kg::router(&mut all));
    router.merge(maintain::router(&mut all));

    debug_assert!(all.map.is_empty(), "every MCP tool must belong to a domain cluster");
    router.merge(all);
    router
}
