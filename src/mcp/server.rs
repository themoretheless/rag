//! Composition root for the domain-clustered MCP tool router.

use rmcp::handler::server::tool::ToolRouter;

pub use super::facade::RagServer;

impl RagServer {
    /// Compose the public MCP facade from domain-owned tool routers.
    ///
    /// Each domain module registers its own `#[tool_router]`; this is the only
    /// place that knows the full set. Tool names stay unique across clusters
    /// because rmcp keys routes by name.
    pub(super) fn all_tools_router() -> ToolRouter<Self> {
        Self::analysis_router()
            + Self::ingest_router()
            + Self::collections_router()
            + Self::search_router()
            + Self::graph_router()
            + Self::wiki_router()
            + Self::kg_router()
            + Self::maintain_router()
            + Self::recovery_router()
    }

    /// Number of tools registered on the full surface, computed once.
    ///
    /// Status endpoints need the count without building a server (and its
    /// store/embedder handles) per request.
    pub fn registered_tool_count() -> usize {
        static COUNT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *COUNT.get_or_init(|| Self::all_tools_router().map.len())
    }
}
