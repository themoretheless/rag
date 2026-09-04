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

mod catalog;
pub mod collections;
pub mod fts;
pub mod graph;
pub mod kg;
pub mod recovery;
mod rows;
pub mod schema;
pub mod search;
mod source_dedupe;
mod source_manifest;
pub mod store;
pub mod sync;
pub mod vault;

pub use catalog::{
    DocumentCatalogFilter, DocumentCatalogPage, ProjectHome, DEFAULT_CATALOG_PAGE_SIZE,
    MAX_CATALOG_PAGE_SIZE,
};
pub use fts::{
    chunks_generation, ensure_fts, ensure_fts_with_stemmer, fts_generation_state,
    fts_index_present, fts_status, load_fts_extension, mark_fts_dirty, probe_ready,
    refresh_fts_if_stale, refresh_fts_if_stale_with_stemmer, reindex, reindex_with_stemmer,
    search_bm25, search_bm25_with_stemmer, FtsBackend, FtsGenerationState, FtsState, LexFilters,
};
pub use source_dedupe::{
    SourceDuplicateCleanupReport, SourceDuplicateDocument, SourceDuplicateGroup,
};
pub use source_manifest::{SourceManifestEntry, SourceManifestWrite, SourceRootSummary};
pub use store::Store;
