//! Process start-up shared by the server and the offline command-line tools.
//!
//! Binaries used to repeat "load config → open store → check the embedding
//! manifest → build the provider" and the tracing set-up; each copy drifted
//! from `main` (backend selection, manifest checks). This is the one sequence.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::db::Store;
use crate::embeddings::{build_provider, EmbeddingProvider};

/// Install the stderr `tracing` subscriber; `RUST_LOG` overrides `default_filter`.
pub fn init_tracing(default_filter: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(true)
        .init();
}

/// Everything an offline tool needs to read or write one database file.
pub struct OfflineRuntime {
    pub config: Config,
    pub store: Store,
    pub embedder: Arc<dyn EmbeddingProvider>,
}

/// Load configuration from the environment for `db_path`, open the configured
/// storage backend, verify (or record) the corpus embedding manifest, and build
/// the embedding provider.
pub fn open_offline(db_path: PathBuf) -> Result<OfflineRuntime> {
    let mut config = Config::from_env().context("Config::from_env")?;
    config.db_path = db_path;
    let store = crate::storage::open_configured(&config).context("open DuckDB")?;
    store
        .ensure_embedding_manifest(&config)
        .context("validate embedding manifest")?;
    let embedder = build_provider(&config).context("embedding provider")?;
    Ok(OfflineRuntime {
        config,
        store,
        embedder,
    })
}
