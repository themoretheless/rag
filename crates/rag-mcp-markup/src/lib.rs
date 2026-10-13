//! Pure text parsing shared by the rag-mcp server and its clients.
//!
//! - [`extract`]: `[[wikilinks]]` and `#tags` with char offsets, code spans masked
//! - [`normalize`]: the one `label_key` normalization used to bind link targets
//!
//! The server builds graph edges from these links and the native UI renders
//! them, so both agree on what counts as a link.

pub mod extract;
pub mod normalize;

pub use extract::{
    extract_links, extract_links_with, ExtractOptions, ExtractedLink, DEFAULT_MAX_LINKS_PER_DOC,
    REL_TAGGED, REL_TUNNEL, REL_WIKILINK,
};
pub use normalize::{display_label, label_key, tag_key};
