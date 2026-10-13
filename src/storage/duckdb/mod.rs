//! DuckDB implementation of the document-lifecycle [`Storage`] contract.
//!
//! `crate::db::Store` stays the DuckDB façade; this module only adapts it.

use crate::db::store::DocumentDerivedWrite;
use crate::db::Store;
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

    /// Same write boundary as every other generic document write: the row,
    /// its graph-node label and the wiki change journal commit together.
    fn upsert_document(&self, document: &Document) -> Result<(), AppError> {
        self.write_document_atomic(document, None, DocumentDerivedWrite::RefreshGraphLabel)
            .map(|_| ())
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

