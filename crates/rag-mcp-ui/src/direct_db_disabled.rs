//! Stand-in for `direct_db.rs` when the UI is built without `direct-db`.
//!
//! Same signatures; every call reports that `--db` needs the feature.

use crate::load::*;
use rag_mcp_core::GraphView;
use std::path::Path;

const DISABLED: &str =
    "rag-mcp-ui was built without the `direct-db` feature; use --http with the gateway";

/// Direct DB access is compiled out; always returns the disabled error.
#[allow(unused_variables)]
pub fn fetch_wiki_list_db(
    db_path: &Path,
    project: Option<&str>,
) -> Result<Vec<WikiPageMeta>, String> {
    Err(DISABLED.into())
}

/// Direct DB access is compiled out; always returns the disabled error.
#[allow(unused_variables)]
pub fn fetch_document_db(
    db_path: &Path,
    document_id: Option<&str>,
    uri: Option<&str>,
) -> Result<DocumentBody, String> {
    Err(DISABLED.into())
}

/// Direct DB access is compiled out; always returns the disabled error.
#[allow(unused_variables)]
pub fn load_live_db(
    path: &Path,
    seed: Option<&str>,
    depth: u32,
    project: Option<&str>,
    include_tags: bool,
) -> Result<LoadedGraph, String> {
    Err(DISABLED.into())
}

/// Direct DB access is compiled out; always returns the disabled error.
#[allow(unused_variables)]
pub fn export_graph_snapshot(args: &ExportArgs) -> Result<ExportResult, String> {
    Err(DISABLED.into())
}

/// Direct DB access is compiled out; always returns the disabled error.
#[allow(unused_variables)]
pub fn expand_neighbors_store(
    db_path: &Path,
    current: &GraphView,
    selected_id: &str,
    max_nodes: u32,
    project: Option<&str>,
    include_tags: bool,
) -> Result<GraphView, String> {
    Err(DISABLED.into())
}

/// Direct DB access is compiled out; always returns the disabled backlinks error.
#[allow(unused_variables)]
pub fn fetch_backlinks_db(
    path: &Path,
    document_id: &str,
    project: Option<&str>,
) -> Result<Vec<BacklinkItem>, String> {
    Err(format!("backlinks unavailable: {DISABLED}"))
}
