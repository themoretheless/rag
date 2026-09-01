//! DuckDB data plane: schema, store CRUD, vector/hybrid search, FTS, graph, temporal KG.
//!
//! # Submodules
//!
//! | Module | Role |
//! |--------|------|
//! | [`schema`] | DDL / migrate / open connection |
//! | [`store`] | Document & chunk CRUD, wing/room, ops_log, wiki_index, manifest |
//! | [`search`] | Vector, lexical, hybrid RRF + diversity |
//! | [`fts`] | DuckDB FTS / BM25 (or term-frequency fallback) |
//! | [`graph`] | Graph node/edge CRUD, neighbors BFS, backlinks (methods on [`Store`]) |
//! | [`kg`] | Temporal knowledge-graph facts (P1 surface) |
//!
//! Graph *extract/resolve* (wikilinks, tags, stubs) lives in crate-root [`crate::graph`].

pub mod fts;
pub mod collections;
pub mod graph;
pub mod kg;
pub mod recovery;
mod rows;
pub mod schema;
pub mod search;
pub mod store;
pub mod vault;

pub use fts::{
    ensure_fts, ensure_fts_with_stemmer, fts_index_present, fts_status, load_fts_extension,
    probe_ready, reindex, reindex_with_stemmer, search_bm25, search_bm25_with_stemmer,
    FtsBackend, FtsState, LexFilters,
};
pub use store::Store;
