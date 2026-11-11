//! Test fixtures of this crate.

use std::path::Path;

use rag_store_duckdb::{Result, Store};

/// [`Store::open`] with this crate's schema extensions registered, as the
/// composition root does before opening a store.
pub(crate) fn open_test_store(path: &Path) -> Result<Store> {
    crate::register_schema_extensions();
    Store::open(path)
}
