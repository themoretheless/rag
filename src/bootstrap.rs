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

// Process-wide knobs live in the subsystems that read them (link ceiling in
// `db::graph::resolve`, call-log size in `telemetry`, sync identity in
// `db::sync`). Pushing them is composition work, so it sits here rather than in
// the value-only `config` module. `Config` lives in the store crate, so the
// method is an extension trait (import it to call `config.apply_process_limits()`).
pub trait ApplyProcessLimits {
    /// Push process-global settings (graph link ceiling, call-log size, sync identity) into the
    /// subsystems that hold them. Call once at start-up, before first use.
    fn apply_process_limits(&self);
}

impl ApplyProcessLimits for Config {
    fn apply_process_limits(&self) {
        crate::db::graph::resolve::configure_max_links(self.max_links_per_doc);
        crate::telemetry::configure_capacity(self.call_log_capacity);
        crate::db::sync::configure_identity(
            self.sync_node_id.clone(),
            self.sync_primary_url.clone(),
        );
    }
}

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
    config.apply_process_limits();
    let store = crate::storage::open_configured(&config).context("open DuckDB")?;
    store
        .embedding_manifest()
        .ensure_embedding_manifest(&config)
        .context("validate embedding manifest")?;
    let embedder = build_provider(&config).context("embedding provider")?;
    Ok(OfflineRuntime {
        config,
        store,
        embedder,
    })
}
