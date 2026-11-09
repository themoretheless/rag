//! Transitional facade over the loader modules; removed once callers import
//! `cli`, `dto`, `http`, `snapshot` and `topology` directly.

pub use crate::cli::*;
pub use crate::direct_db::{
    expand_neighbors_store, export_graph_snapshot, fetch_document_db, fetch_wiki_list_db,
    load_live_db,
};
pub use crate::dto::*;
pub use crate::http::*;
pub use crate::snapshot::*;
pub use crate::topology::*;

/// Load from a resolved CLI source (snapshot XOR read-only db XOR http).
pub fn load_cli_source(
    source: &CliSource,
    seed: Option<&str>,
    depth: u32,
    project: Option<&str>,
    include_tags: bool,
) -> Result<LoadedGraph, String> {
    match source {
        CliSource::Snapshot(path) => load_snapshot_path(path),
        CliSource::Db(path) => load_live_db(path, seed, depth, project, include_tags),
        CliSource::Http(base) => load_http(base, seed, depth, project, include_tags),
    }
}
