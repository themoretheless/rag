//! Wire types shared by the rag-mcp server and its clients.
//!
//! Serde-only on purpose: the native UI and other clients depend on this crate
//! instead of the server crate (DuckDB, axum, rmcp, tokio). The server
//! re-exports everything here from `rag_mcp::models`, so paths are unchanged.

mod graph;
mod index;
mod jobs;
mod revisions;
mod search;
mod sync;

pub use graph::*;
pub use index::*;
pub use jobs::*;
pub use revisions::*;
pub use search::*;
pub use sync::*;
