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
        self.documents()
            .write_document_atomic(document, None, DocumentDerivedWrite::RefreshGraphLabel)
            .map(|_| ())
    }

    fn get_document(&self, id: &str) -> Result<Option<Document>, AppError> {
        self.documents().get_document(id)
    }

    fn list_documents(&self) -> Result<Vec<Document>, AppError> {
        self.documents().list_documents()
    }

    fn delete_document(&self, id: &str) -> Result<bool, AppError> {
        self.documents().delete_document(id)
    }
}
