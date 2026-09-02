//! Temporal knowledge-graph tools.

use super::facade::RagServer;
use super::take_routes;
use rmcp::handler::server::tool::ToolRouter;

const TOOLS: &[&str] = &[
    "kg_add",
    "kg_query",
    "kg_invalidate",
    "kg_supersede",
    "kg_timeline",
    "kg_stats",
];

pub(super) fn router(all: &mut ToolRouter<RagServer>) -> ToolRouter<RagServer> {
    take_routes(all, TOOLS)
}
