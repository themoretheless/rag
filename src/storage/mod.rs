//! Backend-neutral storage boundary.
//!
//! DuckDB remains the default and only adapter for now.  The deliberately
//! small document slice lets callers migrate behind [`Storage`] gradually
//! without changing the existing `db::Store` API.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{AppError, Document, Store};

pub mod duckdb;
pub mod markdown;

/// Stable identifier used by configuration and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    DuckDb,
    Sqlite,
    Postgres,
    Markdown,
    Memory,
}

impl BackendKind {
    pub fn parse(value: &str) -> Result<Self, AppError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "duckdb" | "duck_db" => Ok(Self::DuckDb),
            "sqlite" => Ok(Self::Sqlite),
            "postgres" | "postgresql" => Ok(Self::Postgres),
            "markdown" | "vault" => Ok(Self::Markdown),
            "memory" => Ok(Self::Memory),
            other => Err(AppError::config(format!(
                "unknown RAG_STORAGE_BACKEND '{other}'; expected duckdb, sqlite, postgres, markdown, or memory"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DuckDb => "duckdb", Self::Sqlite => "sqlite", Self::Postgres => "postgres",
            Self::Markdown => "markdown", Self::Memory => "memory",
        }
    }
}

/// Operations a backend can implement natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageCapability {
    Documents,
    FullTextSearch,
    VectorSearch,
    Transactions,
    Graph,
    TemporalKnowledgeGraph,
}

impl StorageCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Documents => "documents", Self::FullTextSearch => "full_text_search",
            Self::VectorSearch => "vector_search", Self::Transactions => "transactions",
            Self::Graph => "graph", Self::TemporalKnowledgeGraph => "temporal_knowledge_graph",
        }
    }
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

pub fn configured_backend() -> Result<BackendKind, AppError> {
    BackendKind::parse(&std::env::var("RAG_STORAGE_BACKEND").unwrap_or_else(|_| "duckdb".into()))
}

/// Open the configured adapter. Unsupported adapters fail at startup rather
/// than silently pointing another backend name at DuckDB.
pub fn open_configured(path: &Path) -> Result<Store, AppError> {
    let backend = configured_backend()?;
    match backend {
        BackendKind::DuckDb => Store::open(path),
        other => Err(AppError::config(format!(
            "storage backend '{}' is recognized but not implemented; export_bundle/export_vault before migrating",
            other.as_str()
        ))),
    }
}

/// Open the configured backend through the backend-neutral document contract.
///
/// The Markdown adapter is deliberately opt-in and requires an explicit
/// `RAG_VAULT_PATH`; `RAG_DB_PATH` is never reinterpreted as a vault root.
pub fn open_configured_storage(path: &Path) -> Result<Box<dyn Storage>, AppError> {
    match configured_backend()? {
        BackendKind::DuckDb => Ok(Box::new(Store::open(path)?)),
        BackendKind::Markdown => {
            let root = std::env::var_os("RAG_VAULT_PATH")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| {
                    AppError::config(
                        "RAG_VAULT_PATH is required when RAG_STORAGE_BACKEND=markdown",
                    )
                })?;
            Ok(Box::new(markdown::MarkdownVaultStorage::open(root)?))
        }
        other => Err(AppError::config(format!(
            "storage backend '{}' is recognized but not implemented; export_bundle/export_vault before migrating",
            other.as_str()
        ))),
    }
}

pub fn markdown_capability_names() -> Vec<String> {
    markdown::CAPABILITIES.iter().map(|cap| cap.as_str().to_string()).collect()
}

pub fn duckdb_capability_names() -> Vec<String> {
    duckdb::CAPABILITIES.iter().map(|cap| cap.as_str().to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_parser_is_explicit_and_capabilities_are_stable() {
        assert_eq!(BackendKind::parse("vault").unwrap(), BackendKind::Markdown);
        assert!(BackendKind::parse("surprise").is_err());
        let caps = duckdb_capability_names();
        assert!(caps.contains(&"documents".to_string()));
        assert!(caps.contains(&"graph".to_string()));
    }
}
