//! DuckDB store of rag-mcp and the domain types it persists.
//!
//! | Module | Layer | Role |
//! |--------|-------|------|
//! | [`models`], [`error`], [`config`], [`util`], [`graph`], [`search_pack`], [`tool_spec`] | domain | plain types, `AppError`, config values, pure helpers |
//! | [`db`] | store | schema, migrations, repositories, vector/FTS search |
//!
//! The domain modules live here rather than in a crate of their own because
//! the store implements DuckDB traits for them (`ToSql`/`FromSql` for the
//! string-coded enums, `From<duckdb::Error>` for `AppError`, see
//! `db/sql_types.rs`); the orphan rule allows that only in the crate that owns
//! the types. Domain modules must not import `db` (checked by the
//! architecture test in the root crate). The root crate `rag-mcp` re-exports
//! every module, so `rag_mcp::db::…`, `rag_mcp::models::…` keep working.

pub mod config;
pub mod db;
pub mod error;
pub mod graph;
pub mod models;
pub mod search_pack;
/// Test fixtures; compiled only for tests or with the `testkit` feature.
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
pub mod tool_spec;
pub mod util;

pub use config::Config;
pub use db::Store;
pub use error::{AppError, Result};
