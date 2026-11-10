//! Application layer: one use case per operation, shared by every transport.
//!
//! MCP tools (`crate::mcp`) and HTTP handlers (`crate::http_api`) are thin
//! adapters: they decode their wire parameters into a typed command, call the
//! use case here and wrap the typed result in their own envelope (MCP: the bare
//! result; HTTP: `{ok: true, …}`). Parameter normalization, limit clamps,
//! defaults and the error rules live here once (docs/refactor/PLAN.md, Stage 4).

pub mod admin;
pub mod graph;
pub mod ingest;
pub mod search;
pub mod wiki;

pub use admin::{AdminUseCase, KgQueryCommand, KgQueryPage};
pub use graph::{GraphUseCase, NeighborsCommand};
pub use ingest::IngestUseCase;
pub use search::{
    MultiGetCommand, MultiGetItem, MultiGetResponse, PackContextRequest, PackContextResponse,
    SearchUseCase, TraceOptions,
};
pub use wiki::{WikiUseCase, WikiWriteCommand, WikiWriteMode};
