//! Application error type and `Result` alias.

use thiserror::Error;

/// Unified error type for the RAG MCP server.
#[derive(Debug, Error)]
pub enum AppError {
    /// Invalid or missing configuration (env, paths, provider settings).
    #[error("config error: {0}")]
    Config(String),

    /// DuckDB / storage failures.
    #[error("database error: {0}")]
    Db(String),

    /// Embedding provider or vector dimension failures.
    #[error("embeddings error: {0}")]
    Embeddings(String),

    /// Local / remote chat LLM failures (wiki compile, organize).
    #[error("llm error: {0}")]
    Llm(String),

    /// Filesystem and I/O failures.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Chunking pipeline failures.
    #[error("chunking error: {0}")]
    Chunking(String),

    /// Requested resource was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// Full-text search / BM25 (DuckDB FTS or Rust fallback) failures.
    #[error("FTS error: {0}")]
    Fts(String),

    /// Optimistic concurrency / version conflict (e.g. wiki If-Match / etag).
    #[error("conflict: {0}")]
    Conflict(String),

    /// Path or action blocked by policy (e.g. `RAG_INGEST_ROOTS` allowlist).
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// Store is temporarily locked or write-busy (`STORE_BUSY`).
    #[error("busy: {0}")]
    Busy(String),

    /// Catch-all for unexpected failures.
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

/// Library-wide result type using [`AppError`].
pub type Result<T> = std::result::Result<T, AppError>;

impl From<duckdb::Error> for AppError {
    fn from(err: duckdb::Error) -> Self {
        AppError::Db(err.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        // Chat and embeddings share reqwest; callers usually wrap context.
        AppError::Embeddings(err.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError::Other(anyhow::Error::from(err))
    }
}

impl AppError {
    /// Convenience constructor for LLM chat errors.
    pub fn llm(msg: impl Into<String>) -> Self {
        AppError::Llm(msg.into())
    }

    /// Convenience constructor for config errors.
    pub fn config(msg: impl Into<String>) -> Self {
        AppError::Config(msg.into())
    }

    /// Convenience constructor for not-found errors.
    pub fn not_found(msg: impl Into<String>) -> Self {
        AppError::NotFound(msg.into())
    }

    /// Convenience constructor for embedding errors.
    pub fn embeddings(msg: impl Into<String>) -> Self {
        AppError::Embeddings(msg.into())
    }

    /// Convenience constructor for chunking errors.
    pub fn chunking(msg: impl Into<String>) -> Self {
        AppError::Chunking(msg.into())
    }

    /// Convenience constructor for database errors from a message.
    pub fn db(msg: impl Into<String>) -> Self {
        AppError::Db(msg.into())
    }

    /// Convenience constructor for FTS / BM25 errors.
    pub fn fts(msg: impl Into<String>) -> Self {
        AppError::Fts(msg.into())
    }

    /// Convenience constructor for concurrency / version conflicts.
    pub fn conflict(msg: impl Into<String>) -> Self {
        AppError::Conflict(msg.into())
    }

    /// Convenience constructor for allowlist / policy denials.
    pub fn forbidden(msg: impl Into<String>) -> Self {
        AppError::Forbidden(msg.into())
    }

    /// Convenience constructor for store-busy / write-lock errors.
    pub fn busy(msg: impl Into<String>) -> Self {
        AppError::Busy(msg.into())
    }
}
