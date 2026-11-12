//! `lin` data plane: schema, store CRUD, vector/hybrid search, FTS, graph, temporal KG.
//!
//! # Submodules
//!
//! | Module | Role |
//! |--------|------|
//! | [`lin_engine`] | Embedded `lin` engine adapter, `BridgeEmbedder`, RwLock guards, ACID `LinTx` |
//! | [`schema`] | DDL / migrate / open connection |
//! | [`store`] | Document & chunk CRUD, wing/room, ops_log, wiki_index, manifest |
//! | [`search`] | Vector, lexical, hybrid RRF + diversity |
//! | [`fts`] | BM25 + stemmer lexical search (or term-frequency fallback) |
//! | [`graph`] | Graph node/edge CRUD, neighbors BFS, backlinks (methods on [`Store`]) |
//! | [`kg`] | Temporal knowledge-graph facts (P1 surface) |
//!
//! Graph *extract/resolve* (wikilinks, tags, stubs) lives in crate-root [`crate::graph`].

pub mod analytics;
mod catalog;
pub mod chunk_text;
pub mod collections;
pub mod embedding_identity;
pub mod fts;
pub mod graph;
pub mod kg;
pub mod knowledge;
pub mod lin_engine;
/// Payload key/value tables shared with the root crate; internal plumbing.
#[doc(hidden)]
pub mod payload_kv;
pub mod recovery;
pub mod repos;
pub mod review;
pub mod rows;
pub mod schema;
pub mod search;
mod source_dedupe;
pub mod source_manifest;
pub mod store;
pub mod sync;
pub mod vault;
pub mod write_hook;

pub use catalog::{
    DEFAULT_CATALOG_PAGE_SIZE, DocumentCatalogFilter, DocumentCatalogPage, MAX_CATALOG_PAGE_SIZE,
    ProjectHome,
};
pub use embedding_identity::EmbeddingIdentity;
pub use fts::{
    FtsBackend, FtsGenerationState, FtsState, LexFilters, chunks_generation, ensure_fts,
    ensure_fts_with_stemmer, fts_generation_state, fts_index_present, fts_status,
    load_fts_extension, mark_fts_dirty, probe_ready, refresh_fts_if_stale,
    refresh_fts_if_stale_with_stemmer, reindex, reindex_with_stemmer, search_bm25,
    search_bm25_with_stemmer,
};
pub use kg::KgFactInput;
pub use source_dedupe::{
    SourceDuplicateCleanupReport, SourceDuplicateDocument, SourceDuplicateGroup,
};
pub use source_manifest::{SourceManifestEntry, SourceManifestWrite, SourceRootSummary};
pub use store::Store;
pub use write_hook::{DocumentWriteHook, HookWriter, register_document_write_hook};
