//! Retrieval, embedding lifecycle, and context-packing tools.

use super::facade::RagServer;
use super::take_routes;
use rmcp::handler::server::tool::ToolRouter;

const TOOLS: &[&str] = &[
    "search",
    "multi_query_search",
    "get_embedding_manifest",
    "reembed_document",
    "pack_context",
    "multi_get",
    "expand_chunks",
    "find_similar",
];

pub(super) fn router(all: &mut ToolRouter<RagServer>) -> ToolRouter<RagServer> {
    take_routes(all, TOOLS)
}
