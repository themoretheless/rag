//! DuckDB storage adapter and compatibility seam.
//!
//! Re-exporting the current data plane here provides the target module path
//! for gradual migration while `crate::db` and `crate::Store` stay valid.

pub use crate::db::*;

use crate::{AppError, Document};

use super::{BackendKind, BackendMetadata, Storage, StorageCapability};

pub(super) const CAPABILITIES: &[StorageCapability] = &[
    StorageCapability::Documents,
    StorageCapability::FullTextSearch,
    StorageCapability::VectorSearch,
    StorageCapability::Transactions,
    StorageCapability::Graph,
    StorageCapability::TemporalKnowledgeGraph,
];

impl Storage for Store {
    fn metadata(&self) -> BackendMetadata {
        BackendMetadata {
            kind: BackendKind::DuckDb,
            name: "duckdb",
            capabilities: CAPABILITIES,
        }
    }

    fn upsert_document(&self, document: &Document) -> Result<(), AppError> {
        Store::upsert_document(self, document)
    }

    fn get_document(&self, id: &str) -> Result<Option<Document>, AppError> {
        Store::get_document(self, id)
    }

    fn list_documents(&self) -> Result<Vec<Document>, AppError> {
        Store::list_documents(self)
    }

    fn delete_document(&self, id: &str) -> Result<bool, AppError> {
        Store::delete_document(self, id)
    }
}

/// Explicit adapter name for new code; existing `Store` imports remain valid.
pub type DuckDbStore = Store;
