//! Defaults and caps that `Config` parses against (domain constants).
//!
//! The owning modules re-export them under their historical paths.

/// Default cap on documents one maintenance run touches (`RAG_MAINT_MAX_DOCS`).
pub const DEFAULT_MAX_DOCS: usize = 50;

/// Default number of rewrite variants (`RAG_QUERY_REWRITE_VARIANTS`).
pub const DEFAULT_REWRITE_VARIANTS: usize = 3;
/// Hard cap on rewrite variants fused per search.
pub const MAX_REWRITE_VARIANTS: usize = 8;
/// Default bounded-cache capacity per transform kind.
pub const DEFAULT_CACHE_SIZE: usize = 256;

/// Default call-log ring size (`RAG_CALL_LOG_CAPACITY`).
pub const DEFAULT_CAPACITY: usize = 2_000;

/// Hard cap on `top_k` for one search.
pub const MAX_TOP_K: usize = 100;
