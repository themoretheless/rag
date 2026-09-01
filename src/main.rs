use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use rag_mcp::config::Config;
use rag_mcp::db::schema::SCHEMA_VERSION;
use rag_mcp::db::Store;
use rag_mcp::embeddings::{build_provider, EmbeddingProvider};
use rag_mcp::http_api;
use rag_mcp::mcp::RagServer;
use rmcp::{transport::stdio, ServiceExt};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let startup_started = Instant::now();
    rag_mcp::ops::set_startup_phase("loading_config");

    let config = Config::from_env().context("failed to load config from environment")?;
    rag_mcp::ops::configure_runtime(config.tool_surface.as_str(), config.http_bind.clone());
    rag_mcp::ops::set_startup_phase("validating_paths");
    rag_mcp::ops::validate_runtime_paths(&config.db_path, &config.ingest_roots)
        .context("runtime path validation failed")?;
    rag_mcp::ops::set_startup_phase("opening_store");
    let store = rag_mcp::storage::open_configured(&config.db_path)
        .context("failed to open configured storage backend")?;

    if env_truthy_default("RAG_CHECKPOINT_ON_START", true) {
        rag_mcp::ops::set_startup_phase("checkpoint");
        let phase = Instant::now();
        store.checkpoint().context(
            "startup DuckDB CHECKPOINT failed; run db_repair offline if this is an ART/index error",
        )?;
        rag_mcp::ops::set_startup_timing("checkpoint", phase.elapsed());
    }

    // FTS / BM25 (or TF fallback) before serving tools so lex/hybrid is ready.
    rag_mcp::ops::set_startup_phase("fts");
    let phase = Instant::now();
    let fts_state = store
        .ensure_fts(&config.fts_stemmer)
        .context("failed to ensure FTS index")?;
    rag_mcp::ops::set_startup_timing("fts", phase.elapsed());

    // Corpus embedding fingerprint (does not overwrite an existing row).
    rag_mcp::ops::set_startup_phase("embedding_manifest");
    let phase = Instant::now();
    let manifest = store
        .ensure_embedding_manifest(&config)
        .context("failed to record embedding_manifest")?;
    rag_mcp::ops::set_startup_timing("manifest", phase.elapsed());
    // Persist migration, FTS DDL, and manifest writes before accepting traffic.
    // Otherwise a SIGTERM/restart can leave FTS catalog DDL in WAL that DuckDB
    // cannot replay on the next open because extension objects depend on it.
    store.checkpoint().context("post-initialization DuckDB CHECKPOINT failed")?;

    let schema_version = store.schema_version().context("schema_version")?.unwrap_or(0);
    let fts_ready = store.fts_ready().context("fts_ready")?;
    let (document_count, chunk_count, node_count, edge_count) =
        store.stats().context("store stats")?;
    let ready_for_search = chunk_count > 0 && schema_version >= SCHEMA_VERSION;

    // Tracing is configured for stderr only (MCP owns stdout when using stdio).
    tracing::info!(
        db_path = %store.path().display(),
        schema_version,
        fts_backend = fts_state.backend.as_str(),
        fts_stemmer = %fts_state.stemmer,
        fts_ready,
        document_count,
        chunk_count,
        node_count,
        edge_count,
        embed_provider = %manifest.provider,
        embed_model = %manifest.model,
        embed_dims = manifest.dims,
        ready_for_search,
        "rag-mcp store ready"
    );

    rag_mcp::ops::set_startup_phase("embedding_provider");
    let embedder = build_provider(&config).context("failed to build embedding provider")?;
    rag_mcp::ops::set_startup_timing("total", startup_started.elapsed());
    rag_mcp::ops::mark_ready();
    tracing::info!(startup = %serde_json::to_string(&rag_mcp::ops::runtime_snapshot()).unwrap_or_default(), "startup summary");
    let result = run_serve_modes(store.clone(), embedder, config).await;
    rag_mcp::ops::set_startup_phase("shutdown_checkpoint");
    let checkpointed = match store.checkpoint() {
        Ok(()) => true,
        Err(error) => { tracing::error!(error = %error, "shutdown checkpoint failed"); false }
    };
    rag_mcp::ops::mark_shutdown(checkpointed);
    tracing::info!(checkpointed, shutdown = %serde_json::to_string(&rag_mcp::ops::runtime_snapshot()).unwrap_or_default(), "rag-mcp shutdown complete");
    result
}

/// HTTP-only, dual (HTTP + stdio), or stdio-only selection and hang-on-stdio-fail.
async fn run_serve_modes(
    store: Store,
    embedder: Arc<dyn EmbeddingProvider>,
    config: Config,
) -> anyhow::Result<()> {
    rag_mcp::ops::spawn_auto_backup(store.clone());
    RagServer::new(store.clone(), embedder.clone(), config.clone()).spawn_auto_sync();
    let http_only = env_flag("RAG_HTTP_ONLY");
    // When HTTP is bound, mount streamable MCP at /mcp (set RAG_MCP_HTTP=false to disable).
    let mcp_http_enabled = env_truthy_default("RAG_MCP_HTTP", true);

    let http_bind = config.http_bind.clone();
    if let Some(ref bind_str) = http_bind {
        let addr = http_api::parse_bind(bind_str)
            .context("RAG_HTTP_BIND")?
            .expect("non-empty bind validated");

        let mcp_svc = if mcp_http_enabled {
            Some(http_api::mcp_http_service(
                store.clone(),
                embedder.clone(),
                config.clone(),
                CancellationToken::new(),
            ))
        } else {
            None
        };

        let store_http = Arc::new(store.clone());
        if http_only {
            tracing::info!(
                %addr,
                mcp_http = mcp_svc.is_some(),
                "RAG_HTTP_ONLY: HTTP gateway only (no stdio); MCP URL http://{}/mcp",
                addr
            );
            tokio::select! {
                result = http_api::serve(addr, store_http, mcp_svc) => {
                    result.context("HTTP API server error")?;
                }
                signal = shutdown_signal() => {
                    signal.context("shutdown signal handler failed")?;
                    tracing::info!("shutdown signal received");
                }
            }
            return Ok(());
        }

        tokio::spawn(async move {
            if let Err(e) = http_api::serve(addr, store_http, mcp_svc).await {
                tracing::error!(error = %e, "HTTP API exited");
            }
        });
    } else if http_only {
        anyhow::bail!("RAG_HTTP_ONLY=true requires RAG_HTTP_BIND (e.g. 127.0.0.1:7432)");
    }

    let has_http = http_bind.is_some();
    let server = RagServer::new(store, embedder, config);

    tracing::info!("starting rag-mcp on stdio");
    match server.serve(stdio()).await {
        Ok(service) => {
            service
                .waiting()
                .await
                .context("MCP server terminated with error")?;
        }
        Err(e) if has_http => {
            // No MCP client on stdin (manual service start): keep HTTP alive for UI/MCP HTTP.
            tracing::warn!(
                error = %e,
                "stdio MCP not connected; HTTP gateway still running (Ctrl+C to stop)"
            );
            std::future::pending::<()>().await;
        }
        Err(e) => {
            return Err(e).context("failed to start MCP server on stdio");
        }
    }

    Ok(())
}

async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(true)
        .init();
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .and_then(|v| rag_mcp::config::parse_env_truthy(&v))
        .unwrap_or(false)
}

/// Bool env with default when unset (`false`/`0`/`off` force off).
fn env_truthy_default(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => rag_mcp::config::parse_env_truthy(&v).unwrap_or(default),
        Err(_) => default,
    }
}
