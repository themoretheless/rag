//! Gateway loaders for graph topology, documents, wiki pages and activity.

mod activity;
mod document;
mod graph;
#[cfg(test)]
pub(crate) mod testing;
mod wiki;

pub use activity::fetch_activity_http;
pub use document::{fetch_backlinks_http, fetch_document_http};
pub use graph::{expand_neighbors_http, fetch_project_catalog_http, load_http};
pub use wiki::{fetch_wiki_list_http, put_wiki_http};
