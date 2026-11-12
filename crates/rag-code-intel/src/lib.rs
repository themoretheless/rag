//! Code intelligence of rag-mcp (the `intel` layer): the code index, external
//! database checks, the analysis gate, confidence calibration and analysis
//! tasks. Depends on the store crate (`rag-store-duckdb`) only; talks to the
//! store through repository types over `Store::with_conn` / `with_tx`.
//!
//! The root crate `rag-mcp` re-exports each module under its old path
//! (`rag_mcp::code_index`, ...).

pub mod analysis_tasks;
pub mod calibration;
pub mod code_index;
pub mod external_db;
pub mod gate;
#[cfg(test)]
mod testkit;

// Domain and store modules under the crate-root names the moved code uses
// (`crate::db::…`, `crate::models::…`): the layer below, not re-exported.
#[cfg(test)]
use rag_mcp::{access, embeddings, file_ingest, source_sync};
#[cfg(test)]
use rag_store_duckdb::Config;
use rag_store_duckdb::{Store, config, db, error, models, search_pack, tool_spec, util};

/// Register the tables owned by this crate (analysis tasks, gate,
/// calibration, external DB snapshots) as store schema extensions, so every
/// store opened afterwards has them. Idempotent. The composition root calls it
/// (`rag_mcp::storage::register_schema_extensions`).
pub fn register_schema_extensions() {
    code_index::source::install_document_write_hook();
    for extension in [
        analysis_tasks::SCHEMA,
        gate::SCHEMA,
        calibration::SCHEMA,
        external_db::SCHEMA,
    ] {
        db::schema::register_schema_extension(extension);
    }
}
