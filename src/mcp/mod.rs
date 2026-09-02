//! MCP surface: thin router composition ([`server`]), domain tool clusters,
//! implementations (`facade`), and schemars param structs ([`tools`]).
//!
//! Transports (same tools / same DuckDB):
//! - **stdio** — Claude Desktop/Code local `command` (subprocess)
//! - **streamable HTTP** — `RAG_HTTP_BIND` + path `/mcp` (one shared server for all clients)
//!
//! Domain logic is **not** reimplemented here. Tools call into:
//! - [`crate::db`] — store, search, FTS, graph CRUD, kg
//! - [`crate::graph`] — `rebuild_document_graph` on ingest/wiki write
//! - [`crate::wiki`] — compile layer (pages, schema, file_answer, consolidate, raw)
//! - [`crate::diary`] — agent diary + session checkpoint
//! - [`crate::util`] — content hash, ingest path allowlist
//! - [`crate::search_pack`] — token-budgeted context packing
//!
//! Domain modules own route registration while [`tools`] keeps `*Params` flat and
//! stable for callers. Public tool names and the rmcp schema surface are unchanged.

mod collections;
pub(crate) mod facade;
mod graph;
mod ingest;
mod kg;
mod maintain;
mod recovery;
mod search;
pub mod server;
pub mod surface;
pub mod tools;
mod wiki;

use rmcp::handler::server::tool::ToolRouter;

fn take_routes<S: Send + Sync + 'static>(all: &mut ToolRouter<S>, names: &[&str]) -> ToolRouter<S> {
    let mut cluster = ToolRouter::new();
    for name in names {
        if let Some(route) = all.map.remove(*name) {
            cluster.add_route(route);
        }
    }
    cluster
}

pub use server::RagServer;
pub use surface::{tool_allowed, ToolSurface, INDEX_FIRST_PLAYBOOK, SPINE_TOOLS_BLURB};
