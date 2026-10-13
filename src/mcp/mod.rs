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
//! Each domain module owns its tool implementations behind one `#[tool_router]`
//! (`ingest`, `collections`, `search`, `graph`, `wiki`, `kg`, `maintain`,
//! `recovery`); [`server`] adds those routers together, [`facade`] holds the
//! shared `RagServer` state, service adapters and `ServerHandler` dispatch, and
//! [`tools`] keeps `*Params` flat and stable for callers. Public tool names and
//! the rmcp schema surface are unchanged.

mod collections;
mod error;
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
mod views;
mod wiki;

pub use server::RagServer;
pub use surface::{tool_allowed, ToolSurface, INDEX_FIRST_PLAYBOOK, SPINE_TOOLS_BLURB};
