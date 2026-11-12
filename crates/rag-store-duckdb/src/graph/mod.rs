//! Object-graph helpers: label normalization and link extraction (domain only).
//!
//! - [`extract`]: pure text parsing of `[[wikilinks]]` and `#tags`
//! - [`normalize`]: single `label_key` normalization used by resolve and store lookups
//!
//! Graph persistence (node upsert, wikilink/tag resolution, document graph
//! rebuild) lives in the store: `crate::db::graph::resolve`.

/// Pure link parsing lives in the `rag-mcp-markup` crate, shared with the UI.
pub use rag_mcp_markup::{extract, normalize};

pub use extract::{
    DEFAULT_MAX_LINKS_PER_DOC, ExtractOptions, ExtractedLink, REL_TAGGED, REL_TUNNEL, REL_WIKILINK,
    extract_links, extract_links_with,
};
pub use normalize::{display_label, label_key, tag_key};
