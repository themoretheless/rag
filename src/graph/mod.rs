//! Object-graph helpers: label normalization, link extraction and document graph rebuild.
//!
//! - [`extract`]: pure text parsing of `[[wikilinks]]` and `#tags`
//! - [`normalize`]: single `label_key` normalization used by resolve and store lookups
//! - [`resolve`]: match targets to nodes / stubs and write edges

/// Pure link parsing lives in the `rag-mcp-markup` crate, shared with the UI.
pub use rag_mcp_markup::{extract, normalize};
/// Graph persistence (node upsert, wikilink/tag resolution) lives in `db`.
pub use crate::db::graph::resolve;

pub use extract::{
    extract_links, extract_links_with, ExtractOptions, ExtractedLink, DEFAULT_MAX_LINKS_PER_DOC,
    REL_TAGGED, REL_TUNNEL, REL_WIKILINK,
};
pub use normalize::{display_label, label_key, tag_key};
pub use resolve::rebuild_document_graph;
