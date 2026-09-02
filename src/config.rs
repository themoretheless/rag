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

        Ok(())
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
