//! Environment-backed configuration for the RAG MCP server.

use std::env;
use std::path::PathBuf;
use std::str::FromStr;

use crate::error::AppError;
use crate::models::SearchMode;

/// Embedding backend selected via `RAG_EMBEDDING_PROVIDER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingProviderKind {
    Mock,
    /// OpenAI or any OpenAI-compatible `/v1/embeddings` server (cloud, Ollama, LM Studio).
    OpenAi,
    /// Native Ollama embeddings (`POST /api/embeddings` or batch `/api/embed`).
    /// Base URL may be `http://127.0.0.1:11434` or `.../v1` (the `/v1` suffix is stripped for native calls;
    /// when the URL ends with `/v1`, the OpenAI-compatible path is used instead via an `OpenAi`-style client).
    Ollama,
}

impl FromStr for EmbeddingProviderKind {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mock" => Ok(Self::Mock),
            "openai" | "openai_compat" => Ok(Self::OpenAi),
            "ollama" => Ok(Self::Ollama),
            other => Err(AppError::config(format!(
                "invalid RAG_EMBEDDING_PROVIDER '{other}': expected 'mock', 'openai', \
                 'openai_compat', or 'ollama'"
            ))),
        }
    }
}

impl EmbeddingProviderKind {
    /// Wire / manifest name for this provider.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::OpenAi => "openai",
            Self::Ollama => "ollama",
        }
    }
}

/// Source-code splitting policy selected via `RAG_CODE_CHUNKING`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodeChunkingMode {
    /// Split rust/python/js/ts by top-level items (functions, classes, impl
    /// blocks) with `symbol` / `symbol_kind` chunk metadata; oversized items
    /// window-split inside their own span. Other languages fall back to
    /// window splitting.
    #[default]
    Structural,
    /// Legacy boundary-preferring char windows for every source file.
    Window,
}

impl FromStr for CodeChunkingMode {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "structural" | "items" => Ok(Self::Structural),
            "window" | "legacy" => Ok(Self::Window),
            other => Err(AppError::config(format!(
                "invalid RAG_CODE_CHUNKING '{other}': expected 'structural' or 'window'"
            ))),
        }
    }
}

/// Runtime configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    pub db_path: PathBuf,
    pub embedding_provider: EmbeddingProviderKind,
    pub embedding_base_url: String,
    pub embedding_api_key: String,
    pub embedding_model: String,
    pub embedding_dims: usize,
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    /// Source-code splitting policy (`RAG_CODE_CHUNKING`: `structural` default
    /// | `window`). `window` restores the pre-v17 boundary-preferring splitter.
    pub code_chunking: CodeChunkingMode,
    pub default_top_k: usize,
    /// Path allowlist for `ingest_file` (and later directory/watch). Empty = no roots allowed.
    pub ingest_roots: Vec<PathBuf>,
    /// Default token budget when packing search hits into a context window.
    pub max_context_tokens: usize,
    /// Max chunks retained per document when applying diversity collapse.
    pub max_chunks_per_doc: usize,
    /// DuckDB FTS stemmer name (`porter`, language names, or `none` for CJK/code).
    pub fts_stemmer: String,
    /// Default `search` mode when the tool omits `mode`.
    pub default_search_mode: SearchMode,
    /// Chat provider preset (`RAG_LLM_PROVIDER`: ollama|openai|codex|claude|kimi|deepseek|custom).
    pub llm_provider: crate::llm::LlmProviderKind,
    /// API base URL (provider default unless `RAG_LLM_BASE_URL` set).
    pub llm_base_url: String,
    /// Chat model name (provider default unless `RAG_LLM_MODEL` set).
    pub llm_model: String,
    /// API key (`RAG_LLM_API_KEY` or provider-specific env; see docs/LLM_PROVIDERS.md).
    pub llm_api_key: String,
    /// When false, compile_source / chat tools refuse with a clear error.
    pub llm_enabled: bool,
    /// HTTP timeout for chat/completions (`RAG_LLM_TIMEOUT_SECS`, default **120**).
    pub llm_timeout_secs: u64,
    /// Max completion tokens per chat request (`RAG_LLM_MAX_TOKENS`, default **4096**).
    pub llm_max_tokens: usize,
    /// Max documents touched per maintenance run (`RAG_MAINT_MAX_DOCS`, default **50**).
    pub maint_max_docs: usize,
    /// Cosine similarity θ for near-duplicate detection
    /// (`RAG_MAINT_NEAR_DUP_THRESHOLD`, default **0.92**). Values in (0, 1].
    pub maint_near_dup_threshold: f64,
    /// Public MCP tool surface (`RAG_TOOLS`: `spine` default | `full`).
    pub tool_surface: crate::mcp::ToolSurface,
    /// Optional HTTP gateway (`RAG_HTTP_BIND`, e.g. `127.0.0.1:7432`).
    /// Empty = disabled. Same process / DuckDB: graph UI routes + streamable MCP at `/mcp`.
    pub http_bind: Option<String>,
    /// When true, wiki page **updates** require `if_match_revision` / etag (CAS).
    /// Env: `RAG_WIKI_REQUIRE_IF_MATCH` (default **false**). Creates may still omit it.
    /// Omit-if-match = last-write-wins when this flag is false.
    pub wiki_require_if_match: bool,
    /// Rerank backend (`RAG_RERANK_PROVIDER`: off|mock|openai_compat|ollama, default **off**).
    pub rerank_provider: crate::rerank::RerankProviderKind,
    /// Rerank API base URL (`RAG_RERANK_BASE_URL`).
    pub rerank_base_url: String,
    /// Rerank API key (`RAG_RERANK_API_KEY`; optional for local endpoints).
    pub rerank_api_key: String,
    /// Rerank model name (`RAG_RERANK_MODEL`).
    pub rerank_model: String,
    /// Candidate pool reranked when `rerank=true` (`RAG_RERANK_TOP_N`, default **50**).
    pub rerank_top_n: usize,
    /// HTTP timeout for rerank requests (`RAG_RERANK_TIMEOUT_SECS`, default **30**).
    pub rerank_timeout_secs: u64,
    /// Rewrite variants requested when `rewrite=true` (`RAG_QUERY_REWRITE_VARIANTS`, default **3**, cap 8).
    pub query_rewrite_variants: usize,
    /// Optional chat provider override for query rewriting (`RAG_QUERY_REWRITE_PROVIDER`; default reuses `RAG_LLM_PROVIDER`).
    pub query_rewrite_provider: Option<crate::llm::LlmProviderKind>,
    /// Optional model override for query rewriting (`RAG_QUERY_REWRITE_MODEL`; default reuses `RAG_LLM_MODEL`).
    pub query_rewrite_model: Option<String>,
    /// Bounded in-process rewrite cache size (`RAG_QUERY_REWRITE_CACHE_SIZE`, default **256**, 0 disables).
    pub query_rewrite_cache_size: usize,
    /// Optional chat provider override for HyDE (`RAG_HYDE_PROVIDER`; default reuses `RAG_LLM_PROVIDER`).
    pub hyde_provider: Option<crate::llm::LlmProviderKind>,
    /// Optional model override for HyDE (`RAG_HYDE_MODEL`; default reuses `RAG_LLM_MODEL`).
    pub hyde_model: Option<String>,
    /// Bounded in-process HyDE cache size (`RAG_HYDE_CACHE_SIZE`, default **256**, 0 disables).
    pub hyde_cache_size: usize,
    /// Host allowlist for `ingest_url` (`RAG_INGEST_URL_ALLOW`, comma-separated
    /// host patterns; `*.example.com` matches subdomains). Empty = `ingest_url`
    /// is disabled and refuses every URL with a clear error.
    pub ingest_url_allow: Vec<String>,
    /// HTTP timeout for `ingest_url` fetches (`RAG_INGEST_URL_TIMEOUT_SECS`, default **30**).
    pub ingest_url_timeout_secs: u64,
    /// Max response bytes accepted by `ingest_url` (`RAG_INGEST_URL_MAX_BYTES`,
    /// default **10 MiB**). Responses without a bounded body are refused.
    pub ingest_url_max_bytes: usize,
    /// Max documents re-embedded per migration batch (`RAG_REEMBED_BATCH`,
    /// default **100**). Migration batches are resumable; repeat until
    /// `reembed_status` reports zero pending documents.
    pub reembed_batch: usize,
    /// FTS dirty-generation gap that triggers an automatic reindex in the
    /// maintenance loop (`RAG_FTS_STALE_GENERATION_THRESHOLD`, default **64**;
    /// **0** disables auto-reindex). Surfaced in `/v1/status`.
    pub fts_stale_generation_threshold: u64,
    /// Maintenance-loop interval for FTS staleness checks
    /// (`RAG_FTS_MAINT_INTERVAL_SECS`, default **300**).
    pub fts_maint_interval_secs: u64,
}

impl Default for Config {
    /// Defaults matching empty-env `from_env` with the mock embedding provider.
    fn default() -> Self {
        use crate::llm::LlmProviderKind;
        let llm_provider = LlmProviderKind::Ollama;
        Self {
            db_path: PathBuf::from("./rag.duckdb"),
            embedding_provider: EmbeddingProviderKind::Mock,
            embedding_base_url: "https://api.openai.com/v1".to_string(),
            embedding_api_key: String::new(),
            embedding_model: "text-embedding-3-small".to_string(),
            embedding_dims: 1536,
            chunk_size: 800,
            chunk_overlap: 120,
            code_chunking: CodeChunkingMode::Structural,
            default_top_k: 5,
            ingest_roots: Vec::new(),
            max_context_tokens: 4096,
            max_chunks_per_doc: 3,
            fts_stemmer: "porter".to_string(),
            default_search_mode: SearchMode::Vec,
            llm_provider,
            llm_base_url: llm_provider.default_base_url().to_string(),
            llm_model: llm_provider.default_model().to_string(),
            llm_api_key: String::new(),
            llm_enabled: true,
            llm_timeout_secs: 120,
            llm_max_tokens: 4096,
            maint_max_docs: 50,
            maint_near_dup_threshold: 0.92,
            tool_surface: crate::mcp::ToolSurface::Spine,
            http_bind: None,
            wiki_require_if_match: false,
            rerank_provider: crate::rerank::RerankProviderKind::Off,
            rerank_base_url: String::new(),
            rerank_api_key: String::new(),
            rerank_model: String::new(),
            rerank_top_n: 50,
            rerank_timeout_secs: 30,
            query_rewrite_variants: crate::query_transform::DEFAULT_REWRITE_VARIANTS,
            query_rewrite_provider: None,
            query_rewrite_model: None,
            query_rewrite_cache_size: crate::query_transform::DEFAULT_CACHE_SIZE,
            hyde_provider: None,
            hyde_model: None,
            hyde_cache_size: crate::query_transform::DEFAULT_CACHE_SIZE,
            ingest_url_allow: Vec::new(),
            ingest_url_timeout_secs: 30,
            ingest_url_max_bytes: 10 * 1024 * 1024,
            reembed_batch: 100,
            fts_stale_generation_threshold: 64,
            fts_maint_interval_secs: 300,
        }
    }
}

impl Config {
    /// Shared baseline for unit and integration tests.
    ///
    /// Mock-friendly defaults: full MCP tool surface, `wiki_require_if_match = false`,
    /// local Ollama LLM placeholder key. Override with struct-update syntax, e.g.
    /// `Config { db_path, embedding_dims: 32, llm_enabled: false, ..Config::for_tests() }`.
    pub fn for_tests() -> Self {
        Self {
            llm_api_key: "ollama".to_string(),
            tool_surface: crate::mcp::ToolSurface::Full,
            wiki_require_if_match: false,
            ..Self::default()
        }
    }

    /// Load configuration from process environment.
    ///
    /// Defaults match SPEC (+ P0 env additions). Validates provider/API key, dims,
    /// chunk overlap, token/chunk caps, FTS stemmer, and search mode.
    pub fn from_env() -> Result<Self, AppError> {
        let db_path = env::var("RAG_DB_PATH")
            .unwrap_or_else(|_| "./rag.duckdb".to_string())
            .into();

        let provider_raw =
            env::var("RAG_EMBEDDING_PROVIDER").unwrap_or_else(|_| "mock".to_string());
        let embedding_provider = EmbeddingProviderKind::from_str(&provider_raw)?;

        let embedding_base_url =
            env::var("RAG_EMBEDDING_BASE_URL").unwrap_or_else(|_| match embedding_provider {
                EmbeddingProviderKind::Ollama => "http://127.0.0.1:11434".to_string(),
                _ => "https://api.openai.com/v1".to_string(),
            });

        let embedding_api_key = env::var("RAG_EMBEDDING_API_KEY").unwrap_or_else(|_| String::new());

        let embedding_model =
            env::var("RAG_EMBEDDING_MODEL").unwrap_or_else(|_| match embedding_provider {
                EmbeddingProviderKind::Ollama => "nomic-embed-text".to_string(),
                _ => "text-embedding-3-small".to_string(),
            });

        let dims_default = match embedding_provider {
            EmbeddingProviderKind::Ollama => "768",
            _ => "1536",
        };
        let embedding_dims = parse_usize("RAG_EMBEDDING_DIMS", dims_default)?;
        let chunk_size = parse_usize("RAG_CHUNK_SIZE", "800")?;
        let chunk_overlap = parse_usize("RAG_CHUNK_OVERLAP", "120")?;
        let code_chunking = env::var("RAG_CODE_CHUNKING")
            .unwrap_or_else(|_| "structural".to_string())
            .parse::<CodeChunkingMode>()?;
        let default_top_k = parse_usize("RAG_DEFAULT_TOP_K", "5")?;

        let ingest_roots = crate::util::ingest_roots_from_env();

        let max_context_tokens = parse_usize("RAG_MAX_CONTEXT_TOKENS", "4096")?;
        let max_chunks_per_doc = parse_usize("RAG_MAX_CHUNKS_PER_DOC", "3")?;

        let fts_stemmer = env::var("RAG_FTS_STEMMER").unwrap_or_else(|_| "porter".to_string());

        let mode_raw = env::var("RAG_DEFAULT_SEARCH_MODE").unwrap_or_else(|_| "vec".to_string());
        let default_search_mode = parse_search_mode(&mode_raw)?;

        // LLM providers: ollama | openai | codex | claude | kimi | deepseek | custom
        //   RAG_LLM_PROVIDER, RAG_LLM_BASE_URL, RAG_LLM_MODEL, RAG_LLM_API_KEY
        //   Fallbacks: OPENAI_API_KEY, ANTHROPIC_API_KEY, DEEPSEEK_API_KEY, MOONSHOT_API_KEY
        //   RAG_LLM_ENABLED, RAG_LLM_TIMEOUT_SECS, RAG_LLM_MAX_TOKENS
        //   RAG_MAINT_MAX_DOCS, RAG_MAINT_NEAR_DUP_THRESHOLD
        use crate::llm::{resolve_api_key, LlmProviderKind};
        let llm_provider = env::var("RAG_LLM_PROVIDER")
            .unwrap_or_else(|_| "ollama".to_string())
            .parse::<LlmProviderKind>()?;
        let llm_base_url = env::var("RAG_LLM_BASE_URL")
            .unwrap_or_else(|_| llm_provider.default_base_url().to_string());
        let llm_model =
            env::var("RAG_LLM_MODEL").unwrap_or_else(|_| llm_provider.default_model().to_string());
        let llm_api_key_raw = env::var("RAG_LLM_API_KEY").unwrap_or_default();
        let llm_api_key = resolve_api_key(llm_provider, &llm_api_key_raw);
        let llm_enabled = parse_bool_env("RAG_LLM_ENABLED", true)?;
        let llm_timeout_secs = parse_u64("RAG_LLM_TIMEOUT_SECS", "120")?;
        let llm_max_tokens = parse_usize("RAG_LLM_MAX_TOKENS", "4096")?;
        let maint_max_docs = parse_usize("RAG_MAINT_MAX_DOCS", "50")?;
        let maint_near_dup_threshold = parse_f64("RAG_MAINT_NEAR_DUP_THRESHOLD", "0.92")?;
        let tool_surface = env::var("RAG_TOOLS")
            .unwrap_or_else(|_| "spine".to_string())
            .parse::<crate::mcp::ToolSurface>()?;
        let http_bind = env::var("RAG_HTTP_BIND")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(ref b) = http_bind {
            crate::http_api::parse_bind(b)?;
        }
        let wiki_require_if_match = parse_bool_env("RAG_WIKI_REQUIRE_IF_MATCH", false)?;

        // Rerank stage (opt-in): RAG_RERANK_PROVIDER, RAG_RERANK_MODEL,
        // RAG_RERANK_BASE_URL, RAG_RERANK_API_KEY, RAG_RERANK_TOP_N,
        // RAG_RERANK_TIMEOUT_SECS. Default off = zero behavior change.
        let rerank_provider = env::var("RAG_RERANK_PROVIDER")
            .unwrap_or_else(|_| "off".to_string())
            .parse::<crate::rerank::RerankProviderKind>()?;
        let rerank_base_url =
            env::var("RAG_RERANK_BASE_URL").unwrap_or_else(|_| match rerank_provider {
                crate::rerank::RerankProviderKind::Ollama => "http://127.0.0.1:11434".to_string(),
                _ => String::new(),
            });
        let rerank_api_key = env::var("RAG_RERANK_API_KEY").unwrap_or_else(|_| String::new());
        let rerank_model = env::var("RAG_RERANK_MODEL").unwrap_or_else(|_| String::new());
        let rerank_top_n = parse_usize("RAG_RERANK_TOP_N", "50")?;
        let rerank_timeout_secs = parse_u64("RAG_RERANK_TIMEOUT_SECS", "30")?;

        // Server-side query transforms (opt-in per request): rewrite + HyDE.
        // Both reuse the active RAG_LLM_* chat provider unless overridden.
        let query_rewrite_variants = parse_usize("RAG_QUERY_REWRITE_VARIANTS", "3")?;
        let query_rewrite_provider = parse_optional_llm_provider("RAG_QUERY_REWRITE_PROVIDER")?;
        let query_rewrite_model = env::var("RAG_QUERY_REWRITE_MODEL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let query_rewrite_cache_size = parse_usize("RAG_QUERY_REWRITE_CACHE_SIZE", "256")?;
        let hyde_provider = parse_optional_llm_provider("RAG_HYDE_PROVIDER")?;
        let hyde_model = env::var("RAG_HYDE_MODEL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let hyde_cache_size = parse_usize("RAG_HYDE_CACHE_SIZE", "256")?;

        // Web ingestion (ingest_url): disabled unless RAG_INGEST_URL_ALLOW lists
        // host patterns; bounded by timeout + max response bytes.
        let ingest_url_allow = env::var("RAG_INGEST_URL_ALLOW")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_string)
            .collect();
        let ingest_url_timeout_secs = parse_u64("RAG_INGEST_URL_TIMEOUT_SECS", "30")?;
        let ingest_url_max_bytes = parse_usize("RAG_INGEST_URL_MAX_BYTES", "10485760")?;

        // Resumable reembed migration batch bound (documents per run).
        let reembed_batch = parse_usize("RAG_REEMBED_BATCH", "100")?;
        let fts_stale_generation_threshold = parse_u64("RAG_FTS_STALE_GENERATION_THRESHOLD", "64")?;
        let fts_maint_interval_secs = parse_u64("RAG_FTS_MAINT_INTERVAL_SECS", "300")?;

        // Local Ollama / LM Studio accept dummy keys; fill empty for ollama or local openai-compat.
        let embedding_api_key = if embedding_api_key.trim().is_empty()
            && (embedding_provider == EmbeddingProviderKind::Ollama
                || (embedding_provider == EmbeddingProviderKind::OpenAi
                    && is_local_openai_base(&embedding_base_url)))
        {
            "ollama".to_string()
        } else {
            embedding_api_key
        };

        let config = Self {
            db_path,
            embedding_provider,
            embedding_base_url,
            embedding_api_key,
            embedding_model,
            embedding_dims,
            chunk_size,
            chunk_overlap,
            code_chunking,
            default_top_k,
            ingest_roots,
            max_context_tokens,
            max_chunks_per_doc,
            fts_stemmer,
            default_search_mode,
            llm_provider,
            llm_base_url,
            llm_model,
            llm_api_key,
            llm_enabled,
            llm_timeout_secs,
            llm_max_tokens,
            maint_max_docs,
            maint_near_dup_threshold,
            tool_surface,
            http_bind,
            wiki_require_if_match,
            rerank_provider,
            rerank_base_url,
            rerank_api_key,
            rerank_model,
            rerank_top_n,
            rerank_timeout_secs,
            query_rewrite_variants,
            query_rewrite_provider,
            query_rewrite_model,
            query_rewrite_cache_size,
            hyde_provider,
            hyde_model,
            hyde_cache_size,
            ingest_url_allow,
            ingest_url_timeout_secs,
            ingest_url_max_bytes,
            reembed_batch,
            fts_stale_generation_threshold,
            fts_maint_interval_secs,
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), AppError> {
        if self.embedding_dims == 0 {
            return Err(AppError::config(
                "RAG_EMBEDDING_DIMS must be greater than 0",
            ));
        }

        if self.chunk_overlap >= self.chunk_size {
            return Err(AppError::config(format!(
                "RAG_CHUNK_OVERLAP ({}) must be less than RAG_CHUNK_SIZE ({})",
                self.chunk_overlap, self.chunk_size
            )));
        }

        if self.default_top_k == 0 {
            return Err(AppError::config("RAG_DEFAULT_TOP_K must be greater than 0"));
        }

        if self.max_context_tokens == 0 {
            return Err(AppError::config(
                "RAG_MAX_CONTEXT_TOKENS must be greater than 0",
            ));
        }

        if self.max_chunks_per_doc == 0 {
            return Err(AppError::config(
                "RAG_MAX_CHUNKS_PER_DOC must be greater than 0",
            ));
        }

        let stemmer = self.fts_stemmer.trim();
        if stemmer.is_empty() {
            return Err(AppError::config(
                "RAG_FTS_STEMMER must be non-empty (use a language name or 'none')",
            ));
        }

        if self.embedding_provider == EmbeddingProviderKind::OpenAi
            && self.embedding_api_key.trim().is_empty()
            && !is_local_openai_base(&self.embedding_base_url)
        {
            return Err(AppError::config(
                "RAG_EMBEDDING_API_KEY is required when RAG_EMBEDDING_PROVIDER=openai \
                 (except local Ollama/LM Studio base URLs)",
            ));
        }

        if self.embedding_provider == EmbeddingProviderKind::Ollama {
            if self.embedding_base_url.trim().is_empty() {
                return Err(AppError::config(
                    "RAG_EMBEDDING_BASE_URL must be non-empty when \
                     RAG_EMBEDDING_PROVIDER=ollama",
                ));
            }
            if self.embedding_model.trim().is_empty() {
                return Err(AppError::config(
                    "RAG_EMBEDDING_MODEL must be non-empty when \
                     RAG_EMBEDDING_PROVIDER=ollama",
                ));
            }
        }

        if self.llm_enabled {
            if self.llm_base_url.trim().is_empty() {
                return Err(AppError::config(
                    "RAG_LLM_BASE_URL must be non-empty when RAG_LLM_ENABLED=true",
                ));
            }
            if self.llm_model.trim().is_empty() {
                return Err(AppError::config(
                    "RAG_LLM_MODEL must be non-empty when RAG_LLM_ENABLED=true",
                ));
            }
            if !self.llm_provider.allows_empty_api_key() && self.llm_api_key.trim().is_empty() {
                return Err(AppError::config(format!(
                    "RAG_LLM_API_KEY (or provider key env) required when RAG_LLM_PROVIDER={}",
                    self.llm_provider.as_str()
                )));
            }
        }

        if self.llm_timeout_secs == 0 {
            return Err(AppError::config(
                "RAG_LLM_TIMEOUT_SECS must be greater than 0",
            ));
        }

        if self.llm_max_tokens == 0 {
            return Err(AppError::config(
                "RAG_LLM_MAX_TOKENS must be greater than 0",
            ));
        }

        if self.maint_max_docs == 0 {
            return Err(AppError::config(
                "RAG_MAINT_MAX_DOCS must be greater than 0",
            ));
        }

        if !(self.maint_near_dup_threshold > 0.0 && self.maint_near_dup_threshold <= 1.0) {
            return Err(AppError::config(format!(
                "RAG_MAINT_NEAR_DUP_THRESHOLD ({}) must be in (0, 1]",
                self.maint_near_dup_threshold
            )));
        }

        for root in &self.ingest_roots {
            if root.as_os_str().is_empty() {
                return Err(AppError::config(
                    "RAG_INGEST_ROOTS contains an empty path entry",
                ));
            }
        }

        for pattern in &self.ingest_url_allow {
            let pattern = pattern.trim();
            if pattern.is_empty()
                || pattern.contains('/')
                || pattern.chars().any(char::is_whitespace)
            {
                return Err(AppError::config(format!(
                    "RAG_INGEST_URL_ALLOW entry '{pattern}' must be a bare host pattern (no scheme, path, or whitespace)"
                )));
            }
        }
        if self.ingest_url_timeout_secs == 0 {
            return Err(AppError::config(
                "RAG_INGEST_URL_TIMEOUT_SECS must be greater than 0",
            ));
        }
        if self.ingest_url_max_bytes == 0 {
            return Err(AppError::config(
                "RAG_INGEST_URL_MAX_BYTES must be greater than 0",
            ));
        }
        if self.reembed_batch == 0 {
            return Err(AppError::config("RAG_REEMBED_BATCH must be greater than 0"));
        }

        use crate::rerank::RerankProviderKind;
        if self.rerank_provider != RerankProviderKind::Off {
            if self.rerank_top_n == 0 || self.rerank_top_n > crate::db::search::MAX_TOP_K {
                return Err(AppError::config(format!(
                    "RAG_RERANK_TOP_N ({}) must be in 1..={}",
                    self.rerank_top_n,
                    crate::db::search::MAX_TOP_K
                )));
            }
            if self.rerank_timeout_secs == 0 {
                return Err(AppError::config(
                    "RAG_RERANK_TIMEOUT_SECS must be greater than 0",
                ));
            }
            match self.rerank_provider {
                RerankProviderKind::Mock => {}
                RerankProviderKind::OpenAiCompat | RerankProviderKind::Ollama => {
                    if self.rerank_base_url.trim().is_empty() {
                        return Err(AppError::config(format!(
                            "RAG_RERANK_BASE_URL must be non-empty when RAG_RERANK_PROVIDER={}",
                            self.rerank_provider.as_str()
                        )));
                    }
                    if self.rerank_model.trim().is_empty() {
                        return Err(AppError::config(format!(
                            "RAG_RERANK_MODEL must be non-empty when RAG_RERANK_PROVIDER={}",
                            self.rerank_provider.as_str()
                        )));
                    }
                }
                RerankProviderKind::Off => unreachable!(),
            }
        }

        if self.query_rewrite_variants == 0
            || self.query_rewrite_variants > crate::query_transform::MAX_REWRITE_VARIANTS
        {
            return Err(AppError::config(format!(
                "RAG_QUERY_REWRITE_VARIANTS ({}) must be in 1..={}",
                self.query_rewrite_variants,
                crate::query_transform::MAX_REWRITE_VARIANTS
            )));
        }

        Ok(())
    }
}

/// Parse an optional LLM provider override env var (unset/empty → `None`).
fn parse_optional_llm_provider(var: &str) -> Result<Option<crate::llm::LlmProviderKind>, AppError> {
    match env::var(var) {
        Err(_) => Ok(None),
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                trimmed.parse::<crate::llm::LlmProviderKind>().map(Some)
            }
        }
    }
}

fn is_local_openai_base(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    u.contains("127.0.0.1")
        || u.contains("localhost")
        || u.contains("0.0.0.0")
        || u.contains(":11434")
        || u.contains(":1234")
}

/// Parse env-style truthy/falsey tokens: `1`/`true`/`yes`/`on` → true,
/// `0`/`false`/`no`/`off` → false. Trim + ASCII case-insensitive.
/// Unrecognized values return `None` (callers choose error vs default vs deny).
///
/// Single owner for `RAG_WIKI_REQUIRE_IF_MATCH`, `RAG_LLM_ENABLED`,
/// `RAG_HTTP_ALLOW_REMOTE`, and similar flags.
pub fn parse_env_truthy(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Read a bool env var. Unset → `default`. Known tokens → value. Other → error.
fn parse_bool_env(var: &str, default: bool) -> Result<bool, AppError> {
    match env::var(var) {
        Err(_) => Ok(default),
        Ok(raw) => parse_env_truthy(&raw).ok_or_else(|| {
            AppError::config(format!(
                "invalid {var} value '{}': expected true/false",
                raw.trim()
            ))
        }),
    }
}

fn parse_usize(var: &str, default: &str) -> Result<usize, AppError> {
    let raw = env::var(var).unwrap_or_else(|_| default.to_string());
    raw.trim().parse::<usize>().map_err(|_| {
        AppError::config(format!(
            "invalid {var} value '{raw}': expected a non-negative integer"
        ))
    })
}

fn parse_u64(var: &str, default: &str) -> Result<u64, AppError> {
    let raw = env::var(var).unwrap_or_else(|_| default.to_string());
    raw.trim().parse::<u64>().map_err(|_| {
        AppError::config(format!(
            "invalid {var} value '{raw}': expected a non-negative integer"
        ))
    })
}

fn parse_f64(var: &str, default: &str) -> Result<f64, AppError> {
    let raw = env::var(var).unwrap_or_else(|_| default.to_string());
    raw.trim().parse::<f64>().map_err(|_| {
        AppError::config(format!(
            "invalid {var} value '{raw}': expected a floating-point number"
        ))
    })
}

/// Parse `RAG_DEFAULT_SEARCH_MODE`: `vec` | `lex` | `hybrid` (case-insensitive).
fn parse_search_mode(raw: &str) -> Result<SearchMode, AppError> {
    SearchMode::parse(raw)
        .map_err(|e| AppError::config(format!("invalid RAG_DEFAULT_SEARCH_MODE: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn provider_from_str() {
        assert_eq!(
            EmbeddingProviderKind::from_str("mock").unwrap(),
            EmbeddingProviderKind::Mock
        );
        assert_eq!(
            EmbeddingProviderKind::from_str("OpenAI").unwrap(),
            EmbeddingProviderKind::OpenAi
        );
        assert_eq!(
            EmbeddingProviderKind::from_str("openai_compat").unwrap(),
            EmbeddingProviderKind::OpenAi
        );
        assert_eq!(
            EmbeddingProviderKind::from_str("ollama").unwrap(),
            EmbeddingProviderKind::Ollama
        );
        assert_eq!(EmbeddingProviderKind::Ollama.as_str(), "ollama");
        assert!(EmbeddingProviderKind::from_str("foo").is_err());
    }

    #[test]
    fn search_mode_parse() {
        assert_eq!(parse_search_mode("vec").unwrap(), SearchMode::Vec);
        assert_eq!(parse_search_mode("LEX").unwrap(), SearchMode::Lex);
        assert_eq!(parse_search_mode(" Hybrid ").unwrap(), SearchMode::Hybrid);
        assert!(parse_search_mode("bm25").is_err());
    }

    #[test]
    fn parse_env_truthy_tokens() {
        for t in ["1", "true", "TRUE", "yes", "on", " Yes ", "On"] {
            assert_eq!(parse_env_truthy(t), Some(true), "truthy {t:?}");
        }
        for t in ["0", "false", "FALSE", "no", "off", " No ", "Off"] {
            assert_eq!(parse_env_truthy(t), Some(false), "falsey {t:?}");
        }
        for t in ["", "maybe", "2", "enabled"] {
            assert_eq!(parse_env_truthy(t), None, "unknown {t:?}");
        }
    }

    #[test]
    fn ingest_roots_comma_split_via_util() {
        let roots = crate::util::parse_ingest_roots(OsStr::new("/vault, ./notes ,/tmp/wiki"));
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/vault"),
                PathBuf::from("./notes"),
                PathBuf::from("/tmp/wiki"),
            ]
        );
        assert!(crate::util::parse_ingest_roots(OsStr::new("")).is_empty());
    }

    #[test]
    fn validate_rejects_zero_caps() {
        let mut cfg = sample_config();
        cfg.max_context_tokens = 0;
        assert!(cfg.validate().is_err());

        let mut cfg = sample_config();
        cfg.max_chunks_per_doc = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_llm_and_maint_bounds() {
        let mut cfg = sample_config();
        cfg.llm_timeout_secs = 0;
        assert!(cfg.validate().is_err());

        let mut cfg = sample_config();
        cfg.llm_max_tokens = 0;
        assert!(cfg.validate().is_err());

        let mut cfg = sample_config();
        cfg.maint_max_docs = 0;
        assert!(cfg.validate().is_err());

        let mut cfg = sample_config();
        cfg.maint_near_dup_threshold = 0.0;
        assert!(cfg.validate().is_err());

        let mut cfg = sample_config();
        cfg.maint_near_dup_threshold = 1.5;
        assert!(cfg.validate().is_err());

        let mut cfg = sample_config();
        cfg.maint_near_dup_threshold = 1.0;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_stemmer() {
        let mut cfg = sample_config();
        cfg.fts_stemmer = "   ".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_accepts_none_stemmer_and_roots() {
        let mut cfg = sample_config();
        cfg.fts_stemmer = "none".to_string();
        cfg.ingest_roots = vec![PathBuf::from("/allowed")];
        cfg.default_search_mode = SearchMode::Hybrid;
        assert!(cfg.validate().is_ok());
    }

    fn sample_config() -> Config {
        Config::for_tests()
    }

    #[test]
    fn for_tests_baseline_validates_and_disables_wiki_cas() {
        let cfg = Config::for_tests();
        assert!(cfg.validate().is_ok());
        assert!(!cfg.wiki_require_if_match);
        assert_eq!(cfg.tool_surface, crate::mcp::ToolSurface::Full);
        assert_eq!(cfg.embedding_provider, EmbeddingProviderKind::Mock);
    }

    #[test]
    fn default_matches_empty_env_shape() {
        let cfg = Config::default();
        assert_eq!(cfg.embedding_provider, EmbeddingProviderKind::Mock);
        assert_eq!(cfg.embedding_dims, 1536);
        assert_eq!(cfg.tool_surface, crate::mcp::ToolSurface::Spine);
        assert!(!cfg.wiki_require_if_match);
        assert!(cfg.validate().is_ok());
    }
}
