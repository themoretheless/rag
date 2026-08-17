//! Object-graph helpers: link extraction and document graph rebuild.
//!
//! - [`extract`]: pure text parsing of `[[wikilinks]]` and `#tags`
//! - [`resolve`]: match targets to nodes / stubs and write edges

pub mod extract;
pub mod resolve;

pub use extract::{extract_links, ExtractedLink, REL_TAGGED, REL_TUNNEL, REL_WIKILINK};
pub use resolve::rebuild_document_graph;
