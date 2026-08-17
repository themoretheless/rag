//! Local HTTP gateway for rag-mcp (same process as DuckDB).
//!
//! Bind with `RAG_HTTP_BIND=127.0.0.1:7432`.
//!
//! | Path | Role |
//! |------|------|
//! | `/health`, `/v1/graph`, `/v1/neighbors`, `/v1/find`, `/v1/document` | Graph + document UI |
//! | `GET /v1/wiki`, `PUT /v1/wiki`, `GET /v1/backlinks` | Wiki catalog, write (CAS), backlinks |
//! | `/mcp` | Streamable HTTP MCP (Claude, ChatGPT, any remote MCP client) |
//!
//! Does **not** open a second DuckDB connection; shares the server's [`Store`].
//! UI: `rag-mcp-ui --http http://127.0.0.1:7432`.
//! MCP URL: `http://127.0.0.1:7432/mcp`.
//!
//! `PUT /v1/wiki` accepts optional `if_match_revision` / `if_match_etag` (same semantics as MCP
//! `write_wiki_page`). Stale CAS returns **409 Conflict**.
//!
//! # Submodules
//!
//! | Module | Role |
//! |--------|------|
//! | [`bind`] | `parse_bind` + loopback gate |
//! | [`error`] | `api_ok` / `api_err` / `status_for` |
//! | [`health`] | `/health` |
//! | [`graph`] | Graph + document handlers |
//! | [`wiki`] | Wiki list/put/backlinks |

mod bind;
mod error;
mod graph;
mod health;
mod wiki;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager,
    tower::{StreamableHttpServerConfig, StreamableHttpService},
};
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::db::Store;
use crate::embeddings::{build_provider, EmbeddingProvider};
use crate::mcp::RagServer;
use crate::models::GraphView;

pub use bind::parse_bind;

/// Shared state for graph/wiki HTTP handlers (clone of MCP store).
#[derive(Clone)]
pub struct HttpState {
    pub store: Arc<Store>,
    /// True when streamable MCP is mounted at `/mcp`.
    pub mcp_http: bool,
    /// Config for wiki write (chunking / embed dims). Built in [`serve`] from env.
    pub config: Config,
    /// Embedder for wiki write re-chunk path.
    pub embedder: Arc<dyn EmbeddingProvider>,
}

/// Build streamable-HTTP MCP service that shares `store` / embedder / config.
///
/// Each MCP session gets a fresh [`RagServer`] clone of the same DuckDB `Store`.
pub fn mcp_http_service(
    store: Store,
    embedder: Arc<dyn EmbeddingProvider>,
    config: Config,
    cancellation_token: CancellationToken,
) -> StreamableHttpService<RagServer, LocalSessionManager> {
    StreamableHttpService::new(
        move || Ok(RagServer::new(store.clone(), embedder.clone(), config.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig {
            stateful_mode: true,
            sse_keep_alive: Some(std::time::Duration::from_secs(15)),
            cancellation_token,
        },
    )
}

/// Start HTTP server (graph API + MCP `/mcp`); runs until process exit.
///
/// Builds embedder/config from env for `PUT /v1/wiki` (same process env as MCP).
/// Route tables come from [`health::routes`], [`graph::routes`], [`wiki::routes`] (merge-only).
pub async fn serve(
    bind: SocketAddr,
    store: Arc<Store>,
    mcp: Option<StreamableHttpService<RagServer, LocalSessionManager>>,
) -> Result<(), std::io::Error> {
    let mcp_http = mcp.is_some();
    let config = Config::from_env().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("HTTP wiki write config: {e}"),
        )
    })?;
    let embedder = build_provider(&config).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("HTTP wiki write embedder: {e}"),
        )
    })?;
    let api = Router::new()
        .merge(health::routes())
        .merge(graph::routes())
        .merge(wiki::routes())
        .with_state(HttpState {
            store,
            mcp_http,
            config,
            embedder,
        });

    let app = if let Some(mcp_svc) = mcp {
        Router::new()
            .merge(api)
            .nest_service("/mcp", mcp_svc)
    } else {
        api
    };

    tracing::info!(
        %bind,
        mcp_http,
        mcp_path = if mcp_http { "/mcp" } else { "(disabled)" },
        "rag-mcp HTTP gateway listening (graph UI + optional streamable MCP)"
    );
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[allow(dead_code)]
pub type GraphJson = GraphView;
