//! Shared fixtures for unit tests and, behind the `testkit` feature, for the
//! integration tests in `tests/it`.
//!
//! Store-level fixtures (`SeedDoc`, `scratch_dir`) live in
//! `rag_store_duckdb::testkit`; this module re-exports them and adds the ones
//! that need the composition root.
//!
//! Not part of the supported API: the module only exists under `cfg(test)` or
//! `--features testkit`.

use std::path::PathBuf;

use crate::config::Config;
use crate::db::Store;

pub use rag_store_duckdb::testkit::{SeedDoc, scratch_dir};

/// A fresh Store in a [`scratch_dir`], removed when the test's thread ends.
///
/// Unit tests get the code-intelligence tables a composed process has; the
/// integration tests (feature `testkit`) open the raw store as before.
pub fn temp_store() -> Store {
    #[cfg(test)]
    crate::storage::register_schema_extensions();
    Store::open(&scratch_dir().join("test.duckdb")).expect("open temp store")
}

/// [`Store::open`] with the code-intelligence tables registered, for unit
/// tests that open a store at a path of their own.
#[cfg(test)]
pub fn open_test_store(path: &std::path::Path) -> crate::error::Result<Store> {
    crate::storage::register_schema_extensions();
    Store::open(path)
}

/// A fresh Store plus the guard that owns its directory, for tests that also
/// need the directory (extra files, a second open, `Config::db_path`).
pub fn open_temp_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = crate::storage::open_store(&dir.path().join("test.duckdb")).expect("open store");
    (dir, store)
}

/// Baseline config for tests that embed with the mock provider and must not
/// reach an LLM: `embedding_model = "mock"`, `llm_enabled = false`, everything
/// else from [`Config::for_tests`]. Override the rest with struct-update syntax.
pub fn test_config(db_path: PathBuf, dims: usize) -> Config {
    Config {
        db_path,
        embedding_model: "mock".into(),
        embedding_dims: dims,
        llm_enabled: false,
        ..Config::for_tests()
    }
}
