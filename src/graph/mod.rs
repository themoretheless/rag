//! Object-graph helpers: label normalization, link extraction and document graph rebuild.
//!
//! - [`extract`]: pure text parsing of `[[wikilinks]]` and `#tags`
//! - [`normalize`]: single `label_key` normalization used by resolve and store lookups
//! - [`resolve`]: match targets to nodes / stubs and write edges

pub mod extract;
pub mod normalize;
pub mod resolve;

pub use extract::{
    extract_links, extract_links_with, ExtractOptions, ExtractedLink, DEFAULT_MAX_LINKS_PER_DOC,
    REL_TAGGED, REL_TUNNEL, REL_WIKILINK,
};
pub use normalize::{display_label, label_key, tag_key};
pub use resolve::rebuild_document_graph;
