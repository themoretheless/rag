//! Streamable-HTTP MCP service mounted at `/mcp` by the composition root.

use std::sync::Arc;

use axum::Router;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager,
    tower::{StreamableHttpServerConfig, StreamableHttpService},
};
use tokio_util::sync::CancellationToken;

use crate::config::{configured_http_allowed_hosts, Config};
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;

use super::RagServer;

/// Keep-alive ping interval on streamable-HTTP MCP SSE responses.
const MCP_SSE_KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(15);

/// A router with the MCP service nested at `/mcp`, ready for the gateway.
pub fn mcp_http_router(
    context: crate::context::AppContext,
    cancellation_token: CancellationToken,
) -> Router {
    Router::new().nest_service("/mcp", mcp_http_service_for(context, cancellation_token))
}

/// Build streamable-HTTP MCP service that shares `store` / embedder / config.
///
/// Each MCP request gets a fresh [`RagServer`] clone of the same DuckDB `Store`.
///
/// The gateway intentionally runs without server-side MCP sessions. All durable
/// state belongs to DuckDB and every tool call is self-contained, so retaining
/// an in-memory session only makes reconnects fragile: after a client closes a
/// session, a delayed SSE `GET /mcp` must receive 404 by the MCP specification.
/// Stateless Streamable HTTP avoids that stale-session loop while keeping SSE
/// request/response framing for clients that do not support JSON responses.
pub fn mcp_http_service(
    store: Store,
    embedder: Arc<dyn EmbeddingProvider>,
    config: Config,
    cancellation_token: CancellationToken,
) -> StreamableHttpService<RagServer, LocalSessionManager> {
    mcp_http_service_for(
        crate::context::AppContext::new(store, embedder, config),
        cancellation_token,
    )
}

/// Streamable-HTTP MCP over the shared [`crate::context::AppContext`].
pub fn mcp_http_service_for(
    context: crate::context::AppContext,
    cancellation_token: CancellationToken,
) -> StreamableHttpService<RagServer, LocalSessionManager> {
    let allowed_hosts = configured_http_allowed_hosts(
        &context.config,
        context.config.http_allowed_hosts.as_deref(),
    );
    // Build once: the context already holds the checked manifest and the chat
    // client; each stateless request gets a cheap clone of this server.
    let prototype = RagServer::from_context(context).with_transport("http-mcp");
    mcp_service_from(prototype, allowed_hosts, cancellation_token)
}

fn mcp_service_from(
    prototype: RagServer,
    allowed_hosts: Vec<String>,
    cancellation_token: CancellationToken,
) -> StreamableHttpService<RagServer, LocalSessionManager> {
    StreamableHttpService::new(
        move || Ok(prototype.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_sse_keep_alive(Some(MCP_SSE_KEEP_ALIVE))
            .with_cancellation_token(cancellation_token)
            .with_allowed_hosts(allowed_hosts),
    )
}
