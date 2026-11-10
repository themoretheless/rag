//! Storage composition: backend selection, store hooks and capability names.
//!
//! DuckDB (`db::Store`) is the only backend; other configured names fail at
//! startup instead of silently pointing at DuckDB.

use crate::config::Config;

use serde::{Deserialize, Serialize};

use crate::{AppError, Store};

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
    /// Parse a backend name (as in `RAG_STORAGE_BACKEND`).
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

    /// Wire string for this backend.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DuckDb => "duckdb",
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
            Self::Markdown => "markdown",
            Self::Memory => "memory",
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
    /// Wire string for this capability.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Documents => "documents",
            Self::FullTextSearch => "full_text_search",
            Self::VectorSearch => "vector_search",
            Self::Transactions => "transactions",
            Self::Graph => "graph",
            Self::TemporalKnowledgeGraph => "temporal_knowledge_graph",
        }
    }
}

/// What the DuckDB backend implements natively.
const DUCKDB_CAPABILITIES: &[StorageCapability] = &[
    StorageCapability::Documents,
    StorageCapability::FullTextSearch,
    StorageCapability::VectorSearch,
    StorageCapability::Transactions,
    StorageCapability::Graph,
    StorageCapability::TemporalKnowledgeGraph,
];

/// Backend kind named by the configuration.
pub fn configured_backend(config: &Config) -> Result<BackendKind, AppError> {
    BackendKind::parse(&config.storage_backend)
}

/// Open the configured adapter. Unsupported adapters fail at startup rather
/// than silently pointing another backend name at DuckDB.
pub fn open_configured(config: &Config) -> Result<Store, AppError> {
    install_store_hooks();
    let backend = configured_backend(config)?;
    match backend {
        BackendKind::DuckDb => open_store(&config.db_path),
        other => Err(AppError::config(format!(
            "storage backend '{}' is recognized but not implemented; export_bundle/export_vault before migrating",
            other.as_str()
        ))),
    }
}

/// Open a DuckDB store with the composition hooks installed.
///
/// The one way binaries and test helpers open a store: it registers the
/// document write hook (moves `code_source_text` into
/// `code_index_source_blobs`) and the code-intelligence schema extensions
/// before the first write, so no caller can forget them. `Store::open` stays
/// the raw `db`-layer constructor.
pub fn open_store(path: &std::path::Path) -> Result<Store, AppError> {
    install_store_hooks();
    Store::open(path)
}

/// Composition: register the store write hooks owned by higher layers.
/// Idempotent; called by [`open_store`].
pub(crate) fn install_store_hooks() {
    crate::code_index::source::install_document_write_hook();
    register_schema_extensions();
}

/// Composition: register the tables owned by the code-intelligence modules,
/// so every store opened afterwards has them from the start.
pub fn register_schema_extensions() {
    for extension in [
        crate::analysis_tasks::SCHEMA,
        crate::gate::SCHEMA,
        crate::calibration::SCHEMA,
        crate::external_db::SCHEMA,
    ] {
        crate::db::schema::register_schema_extension(extension);
    }
}

/// Capability names of the DuckDB backend.
pub fn duckdb_capability_names() -> Vec<String> {
    DUCKDB_CAPABILITIES
        .iter()
        .map(|cap| cap.as_str().to_string())
        .collect()
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

#[cfg(test)]
mod open_store_tests {
    use super::*;
    use crate::models::Document;

    #[test]
    fn open_store_moves_code_source_text_into_the_blob_table() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(&dir.path().join("hooks.duckdb")).unwrap();
        let now = chrono::Utc::now();
        store
            .documents()
            .upsert_document(&Document {
                id: "code-doc".into(),
                uri: "file:///tmp/code-doc.rs".into(),
                title: "code-doc.rs".into(),
                content: "fn main() {}".into(),
                metadata_json: serde_json::json!({ "code_source_text": "fn main() {}\r\n" })
                    .to_string(),
                created_at: now,
                updated_at: now,
                ..Default::default()
            })
            .unwrap();
        let stored = store.documents().get_document("code-doc").unwrap().unwrap();
        let metadata: serde_json::Value = serde_json::from_str(&stored.metadata_json).unwrap();
        assert!(metadata.get("code_source_text").is_none(), "{metadata}");
        assert!(metadata.get("code_source_hash").is_some(), "{metadata}");
    }
}
