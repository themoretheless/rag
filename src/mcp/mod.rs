//! MCP surface: tool router ([`server`]) and schemars param structs ([`tools`]).
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
//! **ISP (params):** [`tools`] groups `*Params` by domain (ingest, search, wiki,
//! graph, …). See that module's cluster map; no mass split yet so re-exports stay
//! flat and tool names / rmcp macro surface unchanged.

pub mod server;
pub mod surface;
pub mod tools;

pub use server::RagServer;
pub use surface::{tool_allowed, ToolSurface, INDEX_FIRST_PLAYBOOK, SPINE_TOOLS_BLURB};
