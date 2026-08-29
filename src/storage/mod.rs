//! Backend-neutral storage boundary.
//!
//! DuckDB remains the default and only adapter for now.  The deliberately
//! small document slice lets callers migrate behind [`Storage`] gradually
//! without changing the existing `db::Store` API.

use crate::{AppError, Document};

pub mod duckdb;

/// Stable identifier used by configuration and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    DuckDb,
}

/// Operations a backend can implement natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageCapability {
    Documents,
    FullTextSearch,
    VectorSearch,
    Transactions,
    Graph,
    TemporalKnowledgeGraph,
}

/// Backend identity and an honest capability declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendMetadata {
    pub kind: BackendKind,
    pub name: &'static str,
    pub capabilities: &'static [StorageCapability],
}

impl BackendMetadata {
    pub fn supports(self, capability: StorageCapability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// Domain-facing storage contract.
///
/// This first vertical slice covers document lifecycle operations. Search,
/// graph, wiki, and maintenance APIs continue to use the compatible DuckDB
/// surface until they are migrated in later commits.
pub trait Storage: Send + Sync {
    fn metadata(&self) -> BackendMetadata;

    fn upsert_document(&self, document: &Document) -> Result<(), AppError>;
    fn get_document(&self, id: &str) -> Result<Option<Document>, AppError>;
    fn list_documents(&self) -> Result<Vec<Document>, AppError>;
    fn delete_document(&self, id: &str) -> Result<bool, AppError>;
}
