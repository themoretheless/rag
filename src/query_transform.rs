//! Server-side query transformation: LLM query rewriting and HyDE.
//!
//! Both stages are **disabled by default** and only run when the caller passes
//! `rewrite=true` / `hyde=true` on `search`. They reuse the chat LLM provider
//! layer (`RAG_LLM_*`, overridable per feature via `RAG_QUERY_REWRITE_*` /
//! `RAG_HYDE_*`).
//!
//! - **Rewrite**: the LLM proposes query variants; the caller runs one search
//!   per variant plus the original and fuses the lists with RRF (the same
//!   machinery as `multi_query_search`).
//! - **HyDE**: the LLM writes a short hypothetical answer passage; its
//!   embedding becomes an additional dense query fused with the lexical/
//!   vector ranking of the original query.
//!
//! Failure semantics mirror the rerank stage: any LLM error is logged and the
//! search falls back to the untransformed query — a request never fails
//! because a transform did. LLM results are memoized in a bounded in-process
//! cache keyed by (kind, query, provider, model).

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use async_trait::async_trait;

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::llm::{ChatClient, ChatMessage};

/// Default number of rewrite variants (`RAG_QUERY_REWRITE_VARIANTS`).
pub const DEFAULT_REWRITE_VARIANTS: usize = 3;
/// Hard cap on rewrite variants fused per search.
pub const MAX_REWRITE_VARIANTS: usize = 8;
/// Default bounded-cache capacity per transform kind.
pub const DEFAULT_CACHE_SIZE: usize = 256;
/// Character cap for the hypothetical passage sent to the embedder.
const MAX_HYDE_CHARS: usize = 2_000;
/// Completion budget for rewrite / HyDE calls (short outputs).
const REWRITE_MAX_TOKENS: u32 = 256;
const HYDE_MAX_TOKENS: u32 = 512;

/// Minimal chat capability the transform stages need (stubbed in tests).
#[async_trait]
pub trait TransformLlm: Send + Sync {
    async fn complete(&self, system: &str, user: &str, max_tokens: u32) -> Result<String>;
    /// Cache-key identity: `provider/model`.
    fn cache_identity(&self) -> String;
}

#[async_trait]
impl TransformLlm for ChatClient {
    async fn complete(&self, system: &str, user: &str, max_tokens: u32) -> Result<String> {
        let messages = [
            ChatMessage {
                role: "system".into(),
                content: system.into(),
            },
            ChatMessage {
                role: "user".into(),
                content: user.into(),
            },
        ];
        self.complete_with_max_tokens(&messages, max_tokens).await
    }

    fn cache_identity(&self) -> String {
        format!("{}/{}", self.provider().as_str(), self.model())
    }
}

/// Which transform stage an LLM client is being built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformKind {
    Rewrite,
    Hyde,
}

impl TransformKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rewrite => "query_rewrite",
            Self::Hyde => "hyde",
        }
    }
}

/// Build the chat client for a transform stage, honoring per-feature provider /
/// model overrides and falling back to the active `RAG_LLM_*` chat provider.
///
/// Returns `Ok(None)` when `RAG_LLM_ENABLED=false` (transforms then fall back
/// to plain search).
pub fn build_transform_llm(config: &Config, kind: TransformKind) -> Result<Option<ChatClient>> {
    if !config.llm_enabled {
        return Ok(None);
    }
    let (provider_override, model_override) = match kind {
        TransformKind::Rewrite => (config.query_rewrite_provider, &config.query_rewrite_model),
        TransformKind::Hyde => (config.hyde_provider, &config.hyde_model),
    };
    let provider = provider_override.unwrap_or(config.llm_provider);
    // With an override provider, fall back to that provider's preset base URL /
    // key envs unless the main llm config already targets the same provider.
    let (base_url, api_key) = if provider == config.llm_provider {
        (
            config.llm_base_url.clone(),
            config.llm_api_key.expose().to_string(),
        )
    } else {
        (
            provider.default_base_url().to_string(),
            crate::llm::resolve_api_key(provider, ""),
        )
    };
    let model = model_override
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| config.llm_model.clone());
    let max_tokens = u32::try_from(config.llm_max_tokens)
        .unwrap_or(u32::MAX)
        .max(1);
    ChatClient::with_provider(
        provider,
        base_url,
        api_key,
        model,
        config.llm_timeout_secs,
        max_tokens,
    )
    .map(Some)
}

/// Bounded FIFO memoization for transform outputs.
///
/// One process-local cache per kind; capacity comes from
/// `RAG_QUERY_REWRITE_CACHE_SIZE` / `RAG_HYDE_CACHE_SIZE` (default 256, 0
/// disables caching). Keys include the provider/model identity so a config
/// change cannot serve stale variants.
struct BoundedCache {
    map: HashMap<String, Vec<String>>,
    order: VecDeque<String>,
    capacity: usize,
}

impl BoundedCache {
    fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    fn get(&self, key: &str) -> Option<Vec<String>> {
        self.map.get(key).cloned()
    }

    fn insert(&mut self, key: String, value: Vec<String>) {
        if self.capacity == 0 {
            return;
        }
        if let Some(existing) = self.map.get_mut(&key) {
            *existing = value;
            return;
        }
        while self.order.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            } else {
                break;
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, value);
    }
}

fn cache_for(kind: TransformKind, capacity: usize) -> &'static Mutex<BoundedCache> {
    static REWRITE: OnceLock<Mutex<BoundedCache>> = OnceLock::new();
    static HYDE: OnceLock<Mutex<BoundedCache>> = OnceLock::new();
    let slot = match kind {
        TransformKind::Rewrite => &REWRITE,
        TransformKind::Hyde => &HYDE,
    };
    slot.get_or_init(|| Mutex::new(BoundedCache::new(capacity)))
}

/// Test hook: drop all cached transform outputs.
#[cfg(test)]
pub fn clear_caches_for_tests() {
    for slot in [TransformKind::Rewrite, TransformKind::Hyde] {
        let cache = cache_for(slot, DEFAULT_CACHE_SIZE);
        let mut guard = cache.lock().expect("cache lock");
        guard.map.clear();
        guard.order.clear();
    }
}

fn cache_capacity(config: &Config, kind: TransformKind) -> usize {
    match kind {
        TransformKind::Rewrite => config.query_rewrite_cache_size,
        TransformKind::Hyde => config.hyde_cache_size,
    }
}

fn cache_key(kind: TransformKind, identity: &str, query: &str) -> String {
    format!("{}|{}|{}", kind.as_str(), identity, query.trim())
}

/// Ask the LLM for query variants (cached). Returns up to
/// `RAG_QUERY_REWRITE_VARIANTS` rewrites, excluding the original query.
pub async fn rewrite_queries(
    llm: &dyn TransformLlm,
    config: &Config,
    query: &str,
) -> Result<Vec<String>> {
    let kind = TransformKind::Rewrite;
    let key = cache_key(kind, &llm.cache_identity(), query);
    let cache = cache_for(kind, cache_capacity(config, kind));
    if let Some(hit) = cache.lock().expect("cache lock").get(&key) {
        return Ok(hit);
    }
    let variants_goal = config.query_rewrite_variants.clamp(1, MAX_REWRITE_VARIANTS);
    let system = "You rewrite search queries for a retrieval system. \
                  Reply with ONLY the rewritten queries, one per line: no numbering, \
                  no bullets, no explanations.";
    let user = format!(
        "Generate {variants_goal} alternative phrasings of this search query. \
         Keep the original language and intent; vary vocabulary and specificity.\n\n\
         Query: {query}"
    );
    let raw = llm.complete(system, &user, REWRITE_MAX_TOKENS).await?;
    let variants = parse_variants(&raw, query, variants_goal);
    if variants.is_empty() {
        return Err(AppError::llm(format!(
            "query rewrite returned no usable variants: {}",
            crate::llm::truncate(&raw, 200)
        )));
    }
    cache
        .lock()
        .expect("cache lock")
        .insert(key, variants.clone());
    Ok(variants)
}

/// Ask the LLM for a short hypothetical answer passage (cached).
pub async fn hyde_passage(llm: &dyn TransformLlm, config: &Config, query: &str) -> Result<String> {
    let kind = TransformKind::Hyde;
    let key = cache_key(kind, &llm.cache_identity(), query);
    let cache = cache_for(kind, cache_capacity(config, kind));
    if let Some(hit) = cache.lock().expect("cache lock").get(&key) {
        return hit
            .into_iter()
            .next()
            .ok_or_else(|| AppError::llm("hyde cache entry was empty".to_string()));
    }
    let system = "You write short hypothetical knowledge-base passages. \
                  Given a search query, write ONE concise paragraph that could \
                  plausibly appear in a document answering it. No preamble, no \
                  markdown fences.";
    let user = format!("Query: {query}");
    let raw = llm.complete(system, &user, HYDE_MAX_TOKENS).await?;
    let passage: String = raw.trim().chars().take(MAX_HYDE_CHARS).collect();
    if passage.trim().is_empty() {
        return Err(AppError::llm("hyde returned an empty passage".to_string()));
    }
    cache
        .lock()
        .expect("cache lock")
        .insert(key, vec![passage.clone()]);
    Ok(passage)
}

/// Parse one-variant-per-line model output: strip numbering / bullets / quotes,
/// drop empties, duplicates, and echoes of the original query.
fn parse_variants(raw: &str, original: &str, limit: usize) -> Vec<String> {
    let original_norm = original.trim().to_ascii_lowercase();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut text = line.trim();
        // Strip common list prefixes: "1.", "1)", "-", "*", "•".
        if let Some((prefix, rest)) = text.split_once(['.', ')']) {
            if !prefix.is_empty() && prefix.trim().chars().all(|c| c.is_ascii_digit()) {
                text = rest.trim();
            }
        }
        text = text
            .strip_prefix('-')
            .or_else(|| text.strip_prefix('*'))
            .or_else(|| text.strip_prefix('•'))
            .unwrap_or(text)
            .trim();
        let text = text.trim_matches(|c| c == '"' || c == '\'');
        if text.is_empty() {
            continue;
        }
        let norm = text.to_ascii_lowercase();
        if norm == original_norm || !seen.insert(norm) {
            continue;
        }
        out.push(text.to_string());
        if out.len() >= limit {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmProviderKind;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct StubLlm {
        reply: String,
        calls: AtomicUsize,
    }

    impl StubLlm {
        fn new(reply: &str) -> Self {
            Self {
                reply: reply.into(),
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl TransformLlm for StubLlm {
        async fn complete(&self, _: &str, _: &str, _: u32) -> Result<String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.reply.clone())
        }
        fn cache_identity(&self) -> String {
            "stub/test".into()
        }
    }

    struct FailingLlm;

    #[async_trait]
    impl TransformLlm for FailingLlm {
        async fn complete(&self, _: &str, _: &str, _: u32) -> Result<String> {
            Err(AppError::llm("stub llm down"))
        }
        fn cache_identity(&self) -> String {
            "stub/failing".into()
        }
    }

    fn test_config() -> Config {
        Config::for_tests()
    }

    #[test]
    fn parse_variants_strips_numbering_bullets_and_dupes() {
        let raw = "1. Alpha beta\n- alpha beta\n2) \"gamma delta\"\n* gamma delta\n\noriginal query\n• epsilon";
        let variants = parse_variants(raw, "original query", 5);
        assert_eq!(variants, vec!["Alpha beta", "gamma delta", "epsilon"]);
    }

    #[test]
    fn parse_variants_respects_limit() {
        let raw = "a\nb\nc\nd";
        assert_eq!(parse_variants(raw, "x", 2), vec!["a", "b"]);
    }

    #[tokio::test]
    async fn rewrite_caches_by_query_and_identity() {
        clear_caches_for_tests();
        let llm = StubLlm::new("v1\nv2\nv3");
        let config = test_config();
        let first = rewrite_queries(&llm, &config, "cache me").await.unwrap();
        let second = rewrite_queries(&llm, &config, "cache me").await.unwrap();
        assert_eq!(first, second);
        assert_eq!(llm.calls.load(Ordering::Relaxed), 1);
        let other = rewrite_queries(&llm, &config, "cache me too")
            .await
            .unwrap();
        assert_eq!(other.len(), 3);
        assert_eq!(llm.calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn rewrite_errors_when_llm_fails_or_returns_nothing_usable() {
        clear_caches_for_tests();
        let config = test_config();
        assert!(rewrite_queries(&FailingLlm, &config, "q").await.is_err());
        let empty = StubLlm::new("q\nq \n");
        assert!(rewrite_queries(&empty, &config, "q").await.is_err());
    }

    #[tokio::test]
    async fn hyde_trims_and_caches_passage() {
        clear_caches_for_tests();
        let long = "x".repeat(MAX_HYDE_CHARS * 2);
        let llm = StubLlm::new(&long);
        let config = test_config();
        let passage = hyde_passage(&llm, &config, "hyde q").await.unwrap();
        assert_eq!(passage.chars().count(), MAX_HYDE_CHARS);
        hyde_passage(&llm, &config, "hyde q").await.unwrap();
        assert_eq!(llm.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn hyde_rejects_empty_passage() {
        clear_caches_for_tests();
        let llm = StubLlm::new("   ");
        assert!(hyde_passage(&llm, &test_config(), "q2").await.is_err());
    }

    #[test]
    fn bounded_cache_evicts_oldest() {
        let mut cache = BoundedCache::new(2);
        cache.insert("a".into(), vec!["1".into()]);
        cache.insert("b".into(), vec!["2".into()]);
        cache.insert("c".into(), vec!["3".into()]);
        assert!(cache.get("a").is_none());
        assert_eq!(cache.get("c").unwrap(), vec!["3".to_string()]);
    }

    #[test]
    fn build_transform_llm_none_when_llm_disabled() {
        let config = Config {
            llm_enabled: false,
            ..Config::for_tests()
        };
        assert!(build_transform_llm(&config, TransformKind::Rewrite)
            .unwrap()
            .is_none());
    }

    #[test]
    fn build_transform_llm_uses_override_provider_and_model() {
        let config = Config {
            query_rewrite_provider: Some(LlmProviderKind::DeepSeek),
            query_rewrite_model: Some("deepseek-reasoner".into()),
            ..Config::for_tests()
        };
        let client = build_transform_llm(&config, TransformKind::Rewrite)
            .unwrap()
            .unwrap();
        assert_eq!(client.provider(), LlmProviderKind::DeepSeek);
        assert_eq!(client.model(), "deepseek-reasoner");
        assert_eq!(
            client.base_url(),
            LlmProviderKind::DeepSeek.default_base_url()
        );
    }
}
