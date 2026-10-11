//! Vector, lexical, and hybrid (RRF) search over stored chunks.
//!
//! - **vec**: cosine similarity over embeddings (Rust-side full scan).
//! - **lex**: DuckDB FTS BM25 when the extension is available; otherwise simple
//!   term-frequency ranking in Rust (same public API).
//! - **hybrid**: reciprocal rank fusion of lex + vec lists.
//!
//! Post-processing: `min_score` filter, document diversity collapse
//! (`max_chunks_per_document`), and citation snippets on each hit.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use duckdb::params_from_iter;
use tracing::{debug, warn};

use crate::error::{AppError, Result};
use crate::models::{Chunk, SearchContextChunk, SearchExplanation, SearchHit, SearchMode};

#[cfg(test)]
use crate::embeddings::cosine_similarity;

use super::fts::{self, LexFilters};
use super::store::Store;

/// Default RRF rank constant (Cormack / common IR practice).
pub const DEFAULT_RRF_K: f32 = 60.0;

/// Default max snippet length in Unicode characters.
pub const DEFAULT_SNIPPET_CHARS: usize = 200;

/// Extra candidates pulled from each list before fusion / collapse.
const CANDIDATE_MULTIPLIER: usize = 5;
const CANDIDATE_FLOOR: usize = 50;
/// Hard per-source bound for adaptive diversity refill.
const MAX_DIVERSITY_CANDIDATES: usize = 4096;
pub const MAX_TOP_K: usize = 100;
pub const MAX_QUERY_CHARS: usize = 4096;

/// Result diversity strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiversityMode {
    /// Maximal Marginal Relevance (relevance vs. token-set overlap with selected).
    Mmr,
    /// Keep at most N chunks per document (highest rank first).
    CollapseByDocument,
}

/// Optional source-aware expansion applied after ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextExpansion {
    Neighbors,
    ParentSection,
}

impl ContextExpansion {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "neighbors" | "neighboring_chunks" => Ok(Self::Neighbors),
            "parent_section" | "section" => Ok(Self::ParentSection),
            other => Err(AppError::config(format!(
                "invalid context_expansion '{other}': expected 'neighbors' or 'parent_section'"
            ))),
        }
    }
}

impl DiversityMode {
    /// Parse tool wire name: `mmr` | `collapse_by_document`.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "mmr" => Ok(Self::Mmr),
            "collapse_by_document" | "collapse" => Ok(Self::CollapseByDocument),
            other => Err(AppError::config(format!(
                "invalid diversity '{other}': expected 'mmr' or 'collapse_by_document'"
            ))),
        }
    }
}

/// Parameters for multi-mode search.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub mode: SearchMode,
    pub top_k: usize,
    /// Optional free-text query (required for `lex` / `hybrid`; used for snippets).
    pub query_text: Option<String>,
    /// Dense query embedding (required for `vec` / `hybrid`).
    pub query_embedding: Option<Vec<f32>>,
    pub document_id: Option<String>,
    /// Filter documents by wing (MemPalace-style scope).
    pub wing: Option<String>,
    /// Filter documents by room.
    pub room: Option<String>,
    /// Filter documents by layer (`raw`, `wiki`, …).
    pub layer: Option<String>,
    /// Filter documents by provenance `source_file`.
    pub source_file: Option<String>,
    /// When false (default), exclude documents with status `archived` / `tombstone`.
    pub include_archived: bool,
    /// Drop hits whose primary `score` is strictly below this threshold.
    pub min_score: Option<f32>,
    /// Diversity strategy (`mmr` or `collapse_by_document`).
    pub diversity: Option<DiversityMode>,
    /// Cap retained hits per `document_id`. `None` = no cap (unless diversity needs a default).
    pub max_chunks_per_document: Option<usize>,
    /// Optional freshness boost half-life. `None` keeps ranking unchanged.
    pub recency_half_life_days: Option<f64>,
    /// Complete citation-block budget (~4 chars/token), including expanded text and headers.
    pub max_context_tokens: Option<usize>,
    /// Opt-in source context expansion; absent preserves the ranked chunk body.
    pub context_expansion: Option<ContextExpansion>,
    /// Chunks on each side for `neighbors` (default 1).
    pub neighbor_chunks: usize,
    /// RRF constant `k` in `1 / (k + rank)`. Default [`DEFAULT_RRF_K`].
    pub rrf_k: f32,
    /// Max characters for [`SearchHit::snippet`]. `0` disables snippets.
    pub snippet_max_chars: usize,
    /// DuckDB FTS stemmer (`porter`, language name, or `none`). Default `porter`.
    pub fts_stemmer: String,
    /// Injectable clock for deterministic freshness tests.
    pub now: Option<chrono::DateTime<chrono::Utc>>,
    /// Synchronous search budget checked between retrieval stages.
    pub timeout_ms: Option<u64>,
    /// Query embedding time measured by the caller (reported in `explanation.embed_ms`).
    pub embed_ms: Option<f64>,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            mode: SearchMode::Vec,
            top_k: 5,
            query_text: None,
            query_embedding: None,
            document_id: None,
            wing: None,
            room: None,
            layer: None,
            source_file: None,
            include_archived: false,
            min_score: None,
            diversity: None,
            max_chunks_per_document: None,
            recency_half_life_days: None,
            max_context_tokens: None,
            context_expansion: None,
            neighbor_chunks: 1,
            rrf_k: DEFAULT_RRF_K,
            snippet_max_chars: DEFAULT_SNIPPET_CHARS,
            fts_stemmer: "porter".into(),
            now: None,
            timeout_ms: Some(5_000),
            embed_ms: None,
        }
    }
}

/// Per-stage wall-clock timings collected while running one search mode.
#[derive(Debug, Clone, Copy, Default)]
struct StageTimings {
    vec_ms: Option<f64>,
    lex_ms: Option<f64>,
}

/// Stamp 1-based candidate ranks (`rank_vec` / `rank_lex`) before fusion.
fn stamp_ranks(hits: &mut [SearchHit], lexical: bool) {
    for (index, hit) in hits.iter_mut().enumerate() {
        let rank = Some((index + 1) as u32);
        if lexical {
            hit.rank_lex = rank;
        } else {
            hit.rank_vec = rank;
        }
    }
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

/// Rank chunks by cosine similarity to `query_embedding` and return the top hits.
///
/// Backward-compatible vector-only entry point used by MCP tools and integration
/// tests. Prefer [`search`] for lex/hybrid, min_score, and diversity.
pub fn search_chunks(
    store: &Store,
    query_embedding: &[f32],
    top_k: usize,
    document_id: Option<&str>,
) -> Result<Vec<SearchHit>> {
    search(
        store,
        &SearchQuery {
            mode: SearchMode::Vec,
            top_k,
            query_embedding: Some(query_embedding.to_vec()),
            document_id: document_id.map(str::to_string),
            ..SearchQuery::default()
        },
    )
}

/// Multi-mode search: `vec` | `lex` | `hybrid` with optional filters, min_score,
/// diversity, token pack, and citation fields (`score_*`, `snippet`, offsets).
pub fn search(store: &Store, query: &SearchQuery) -> Result<Vec<SearchHit>> {
    if query.top_k == 0 {
        return Ok(Vec::new());
    }

    let corpus_guard = acquire_corpus_search_guard(store, query.mode)?;
    search_with_corpus_guard(store, query, &corpus_guard)
}

/// Acquire the corpus coordination guard required by every retrieval mode.
///
/// The returned read guard permits concurrent searches and prevents an
/// exclusive corpus mutation from starting until the caller finishes. An
/// active mutation produces a retryable [`AppError::Busy`] before embedding,
/// FTS, or vector work.
pub(crate) struct CorpusSearchGuard {
    _guard: tokio::sync::OwnedRwLockReadGuard<()>,
}

pub(crate) fn acquire_corpus_search_guard(
    store: &Store,
    _mode: SearchMode,
) -> Result<CorpusSearchGuard> {
    let guard = store.try_corpus_idle_guard().ok_or_else(|| {
        AppError::busy("exclusive corpus mutation is active; retry search after it completes")
    })?;
    Ok(CorpusSearchGuard { _guard: guard })
}

/// Run search while the caller keeps its corpus read guard alive.
///
/// Transport orchestration uses this after acquiring the guard before an async
/// embedding request. Direct synchronous callers should use [`search`].
pub(crate) fn search_with_corpus_guard(
    store: &Store,
    query: &SearchQuery,
    _corpus_guard: &CorpusSearchGuard,
) -> Result<Vec<SearchHit>> {
    validate_query(query)?;
    // Source sync advances the global chunk generation once per committed
    // document. Starting exact lexical retrieval in the middle of that stream
    // would rebuild BM25 (and hybrid's vector input) only to invalidate it on
    // the next commit. The guard fast-fails before this function and preserves
    // normal read-your-writes once the writer releases.
    let started = Instant::now();
    let mut pool = candidate_pool_size(query.top_k);
    let filters = scope_filters(query);

    // Exhaustive dispatch only: new SearchMode variants must add a run_* arm here
    // (and update SearchMode::as_str / parse / needs_*). Do not use `_`.
    let mut timings = StageTimings::default();
    let mut refill_rounds = 0;
    let mut reached_refill_cap = false;
    let hits = loop {
        enforce_timeout(started, query.timeout_ms, "candidate refill")?;
        let round_started = Instant::now();
        let mut round_timings = StageTimings::default();
        let hits = match query.mode {
            SearchMode::Vec => {
                let mut hits = run_vec(store, query, pool, &filters)?;
                round_timings.vec_ms = Some(elapsed_ms(round_started));
                stamp_ranks(&mut hits, false);
                hits
            }
            SearchMode::Lex => {
                let mut hits = run_lex(store, query, pool, &filters)?;
                round_timings.lex_ms = Some(elapsed_ms(round_started));
                stamp_ranks(&mut hits, true);
                hits
            }
            SearchMode::Hybrid => {
                match run_hybrid(store, query, pool, &filters, &mut round_timings)? {
                    HybridOutcome::Fused(h) => h,
                    HybridOutcome::VecOnly(h) => h,
                }
            }
        };
        if let Some(ms) = round_timings.vec_ms {
            *timings.vec_ms.get_or_insert(0.0) += ms;
        }
        if let Some(ms) = round_timings.lex_ms {
            *timings.lex_ms.get_or_insert(0.0) += ms;
        }
        enforce_timeout(started, query.timeout_ms, "candidate retrieval")?;
        // The hybrid union is at least as large as either component list; a
        // union smaller than `pool` therefore means both sources are exhausted.
        let short = diversity_capacity(&hits, query).is_some_and(|count| count < query.top_k);
        if !short || hits.len() < pool || query.document_id.is_some() {
            break hits;
        }
        if pool >= MAX_DIVERSITY_CANDIDATES {
            reached_refill_cap = true;
            break hits;
        }
        pool = (pool * 2).min(MAX_DIVERSITY_CANDIDATES);
        refill_rounds += 1;
    };

    enforce_timeout(started, query.timeout_ms, "retrieval")?;
    let retrieval_ms = elapsed_ms(started);
    let post_started = Instant::now();
    let mut hits = finalize_hits(store, hits, query)?;
    enforce_timeout(started, query.timeout_ms, "post-processing")?;
    let postprocess_ms = elapsed_ms(post_started);
    let total_ms = elapsed_ms(started);
    let rrf_k = matches!(query.mode, SearchMode::Hybrid).then_some(query.rrf_k);
    for hit in &mut hits {
        let explanation = hit
            .explanation
            .get_or_insert_with(SearchExplanation::default);
        if refill_rounds > 0 {
            explanation
                .reasons
                .push("diversity_candidate_refill".into());
        }
        if reached_refill_cap {
            explanation
                .reasons
                .push("diversity_candidate_cap_reached".into());
        }
        explanation.retrieval_ms = retrieval_ms;
        explanation.postprocess_ms = postprocess_ms;
        explanation.total_ms = total_ms;
        explanation.embed_ms = query.embed_ms;
        explanation.vec_ms = timings.vec_ms;
        explanation.lex_ms = timings.lex_ms;
        explanation.rrf_k = rrf_k;
    }
    Ok(hits)
}

fn validate_query(query: &SearchQuery) -> Result<()> {
    if query.top_k > MAX_TOP_K {
        return Err(AppError::config(format!(
            "top_k {} exceeds hard cap {MAX_TOP_K}",
            query.top_k
        )));
    }
    if let Some(text) = query.query_text.as_deref() {
        if text.trim().is_empty() {
            return Err(AppError::config("search query must not be empty"));
        }
        let chars = text.chars().count();
        if chars > MAX_QUERY_CHARS {
            return Err(AppError::config(format!(
                "search query has {chars} characters; maximum is {MAX_QUERY_CHARS}"
            )));
        }
    }
    Ok(())
}

fn enforce_timeout(started: Instant, timeout_ms: Option<u64>, stage: &str) -> Result<()> {
    if timeout_ms.is_some_and(|ms| started.elapsed() > Duration::from_millis(ms.max(1))) {
        return Err(AppError::config(format!(
            "search timeout budget exceeded after {stage}"
        )));
    }
    Ok(())
}

/// Dense vector path (requires `query_embedding`).
fn run_vec(
    store: &Store,
    query: &SearchQuery,
    pool: usize,
    filters: &LexFilters,
) -> Result<Vec<SearchHit>> {
    // Vec-only path stays usable when hybrid/lex infrastructure is incomplete.
    let emb = query
        .query_embedding
        .as_deref()
        .ok_or_else(|| AppError::embeddings("vec search requires query_embedding"))?;
    search_vec(store, emb, pool, filters)
}

/// Lexical / FTS path (requires `query_text`).
fn run_lex(
    store: &Store,
    query: &SearchQuery,
    pool: usize,
    filters: &LexFilters,
) -> Result<Vec<SearchHit>> {
    let text = query
        .query_text
        .as_deref()
        .ok_or_else(|| AppError::fts("lex search requires query_text"))?;
    // Prefer fts module (DuckDB BM25 or TF fallback) with scope filters.
    match fts::search_bm25_with_stemmer(
        store,
        text,
        pool,
        filters,
        Some(query.fts_stemmer.as_str()),
    ) {
        Ok(h) => {
            debug!(count = h.len(), "lex search via db::fts");
            Ok(h)
        }
        Err(e) => {
            warn!(error = %e, "fts::search_bm25 failed; using in-module lex fallback");
            search_lex(store, text, pool, filters, &query.fts_stemmer)
        }
    }
}

/// Hybrid path result: full RRF fusion, or vec-only when lex is unavailable.
enum HybridOutcome {
    Fused(Vec<SearchHit>),
    /// Lex failed; caller finalizes vec hits early (same as previous early-return).
    VecOnly(Vec<SearchHit>),
}

/// Reciprocal rank fusion of vec + lex (requires embedding and query text).
fn run_hybrid(
    store: &Store,
    query: &SearchQuery,
    pool: usize,
    filters: &LexFilters,
    timings: &mut StageTimings,
) -> Result<HybridOutcome> {
    let emb = query
        .query_embedding
        .as_deref()
        .ok_or_else(|| AppError::embeddings("hybrid search requires query_embedding"))?;
    let text = query
        .query_text
        .as_deref()
        .ok_or_else(|| AppError::fts("hybrid search requires query_text"))?;
    let vec_started = Instant::now();
    // Scope before selecting the vector top-k. Filtering a global top-k after
    // ranking can drop every in-project candidate when another project has
    // enough stronger matches; `search_vec` also owns the generation-aware
    // normalized snapshot cache.
    let mut vec_hits = search_vec(store, emb, pool, filters)?;
    stamp_ranks(&mut vec_hits, false);
    timings.vec_ms = Some(elapsed_ms(vec_started));
    let lex_started = Instant::now();
    let mut lex_hits = match fts::search_bm25_with_stemmer(
        store,
        text,
        pool,
        filters,
        Some(query.fts_stemmer.as_str()),
    ) {
        Ok(h) => h,
        Err(e) => {
            warn!(error = %e, "fts::search_bm25 failed in hybrid; lex fallback");
            match search_lex(store, text, pool, filters, &query.fts_stemmer) {
                Ok(h) => h,
                Err(e2) => {
                    // Hybrid incomplete: keep vec results so search still returns hits.
                    warn!(
                        error = %e2,
                        "hybrid lex fallback failed; returning vec-only results"
                    );
                    return Ok(HybridOutcome::VecOnly(vec_hits));
                }
            }
        }
    };
    stamp_ranks(&mut lex_hits, true);
    timings.lex_ms = Some(elapsed_ms(lex_started));
    Ok(HybridOutcome::Fused(fuse_rrf(
        &vec_hits,
        &lex_hits,
        query.rrf_k,
    )))
}

/// Post-process ranked hits: snippets, min_score, diversity, top_k, token pack.
fn finalize_hits(
    store: &Store,
    mut hits: Vec<SearchHit>,
    query: &SearchQuery,
) -> Result<Vec<SearchHit>> {
    // Attach snippets (modes that already set score_* leave those intact).
    let qtext = query.query_text.as_deref();
    for hit in &mut hits {
        if query.snippet_max_chars > 0 && hit.snippet.is_none() {
            hit.snippet = make_snippet(&hit.content, qtext, query.snippet_max_chars);
        }
    }

    if let Some(min) = query.min_score {
        hits = apply_min_score(hits, min);
    }

    if let Some(half_life_days) = query
        .recency_half_life_days
        .filter(|days| days.is_finite() && *days > 0.0)
    {
        let now = query.now.unwrap_or_else(chrono::Utc::now);
        for hit in &mut hits {
            if let Some(document) = store.get_document(&hit.document_id)? {
                let age_days = now
                    .signed_duration_since(document.updated_at)
                    .num_seconds()
                    .max(0) as f64
                    / 86_400.0;
                let freshness = 2f64.powf(-age_days / half_life_days) as f32;
                hit.score *= 1.0 + freshness;
            }
        }
        hits.sort_by(stable_score_order);
    }

    apply_field_boosts(store, &mut hits, query.query_text.as_deref())?;
    hits.sort_by(stable_score_order);

    let mut seen = HashSet::new();
    let mut duplicate_ids = HashSet::new();
    hits.retain(|hit| {
        if seen.insert(hit.chunk_id.clone()) {
            true
        } else {
            duplicate_ids.insert(hit.chunk_id.clone());
            false
        }
    });
    for hit in &mut hits {
        if duplicate_ids.contains(&hit.chunk_id) {
            hit.explanation
                .get_or_insert_with(SearchExplanation::default)
                .deduplication = Some("duplicate_chunk_id_removed".into());
        }
    }

    let max_per_doc = query.max_chunks_per_document.unwrap_or(usize::MAX);
    match query.diversity {
        Some(DiversityMode::CollapseByDocument) => {
            if max_per_doc > 0 && max_per_doc != usize::MAX {
                hits = collapse_by_document(hits, max_per_doc);
            } else if max_per_doc == usize::MAX {
                // Explicit collapse without cap still collapses to 1 per doc by convention.
                hits = collapse_by_document(hits, 1);
            }
        }
        Some(DiversityMode::Mmr) => {
            let cap = if max_per_doc == usize::MAX {
                3
            } else {
                max_per_doc.max(1)
            };
            // Only the returned prefix matters; refill can make the pool much
            // larger, so avoid selecting a discarded MMR suffix.
            hits = mmr_select(hits, cap, query.top_k);
        }
        None => {
            if let Some(max_per) = query.max_chunks_per_document {
                if max_per > 0 {
                    hits = collapse_by_document(hits, max_per);
                }
            }
        }
    }

    hits.truncate(query.top_k);

    attach_context(
        store,
        &mut hits,
        query.context_expansion,
        query.neighbor_chunks,
    )?;

    if let Some(budget) = query.max_context_tokens {
        let packed = crate::search_pack::pack_hits(&hits, budget);
        hits = packed.hits;
    }

    Ok(hits)
}

fn stable_score_order(a: &SearchHit, b: &SearchHit) -> std::cmp::Ordering {
    b.score
        .total_cmp(&a.score)
        .then_with(|| a.document_id.cmp(&b.document_id))
        .then_with(|| a.chunk_index.cmp(&b.chunk_index))
        .then_with(|| a.chunk_id.cmp(&b.chunk_id))
}

fn apply_field_boosts(store: &Store, hits: &mut [SearchHit], query: Option<&str>) -> Result<()> {
    let Some(raw) = query.map(str::trim).filter(|q| !q.is_empty()) else {
        return Ok(());
    };
    let needle = raw.to_lowercase();
    for hit in hits {
        let mut reasons = Vec::new();
        if let Some(document) = store.get_document(&hit.document_id)? {
            if document.title.trim().eq_ignore_ascii_case(raw) {
                hit.score *= 1.35;
                reasons.push("exact_title_boost".into());
            }
            let uri = document.uri.to_lowercase();
            if tokenize(&needle)
                .into_iter()
                .all(|term| uri.contains(&term))
            {
                hit.score *= 1.15;
                reasons.push("uri_match_boost".into());
            }
            if document.pinned {
                hit.score *= 1.20;
                reasons.push("pinned_document_boost".into());
            }
            if (document.boost - 1.0).abs() > f64::EPSILON {
                hit.score *= document.boost.max(0.0) as f32;
                reasons.push("document_boost".into());
            }
        }
        if hit
            .heading_path
            .as_ref()
            .is_some_and(|path| path.iter().any(|h| h.to_lowercase().contains(&needle)))
        {
            hit.score *= 1.15;
            reasons.push("heading_match_boost".into());
        }
        let explanation = hit
            .explanation
            .get_or_insert_with(SearchExplanation::default);
        explanation.reasons.extend(reasons);
    }
    Ok(())
}

/// Fuse any number of independently ranked query result lists with RRF.
pub fn fuse_rrf_many(lists: &[Vec<SearchHit>], rrf_k: f32, top_k: usize) -> Vec<SearchHit> {
    let k = if rrf_k > 0.0 { rrf_k } else { DEFAULT_RRF_K };
    let mut fused = HashMap::new();
    for list in lists {
        accumulate_rrf_list(&mut fused, list, k);
    }
    let mut out: Vec<_> = fused
        .into_values()
        .map(|(rrf, mut hit)| {
            hit.score = rrf;
            hit.score_rrf = Some(rrf);
            hit.explanation
                .get_or_insert_with(SearchExplanation::default)
                .reasons
                .push("multi_query_rrf".into());
            hit
        })
        .collect();
    out.sort_by(stable_score_order);
    out.truncate(top_k.min(MAX_TOP_K));
    out
}

pub fn attach_context(
    store: &Store,
    hits: &mut [SearchHit],
    expansion: Option<ContextExpansion>,
    neighbor_chunks: usize,
) -> Result<()> {
    for hit in hits {
        let chunks = store.list_chunks_for_document(&hit.document_id)?;
        let Some(current) = chunks.iter().find(|c| c.id == hit.chunk_id) else {
            continue;
        };
        let (heading_path, section) = chunk_section_metadata(current);
        hit.heading_path = heading_path.clone();
        hit.section = section.clone();

        let Some(expansion) = expansion else { continue };
        let selected: Vec<&Chunk> = match expansion {
            ContextExpansion::Neighbors => {
                let radius = neighbor_chunks.max(1) as i32;
                chunks
                    .iter()
                    .filter(|c| (c.chunk_index - current.chunk_index).abs() <= radius)
                    .collect()
            }
            ContextExpansion::ParentSection => chunks
                .iter()
                .filter(|c| {
                    let (path, _) = chunk_section_metadata(c);
                    heading_path.is_some() && path == heading_path
                })
                .collect(),
        };
        let selected = if selected.is_empty() {
            vec![current]
        } else {
            selected
        };
        hit.context = Some(
            selected
                .iter()
                .map(|chunk| {
                    let (heading_path, section) = chunk_section_metadata(chunk);
                    SearchContextChunk {
                        chunk_id: chunk.id.clone(),
                        chunk_index: chunk.chunk_index,
                        content: chunk.content.clone(),
                        heading_path,
                        section,
                    }
                })
                .collect(),
        );
        hit.content = selected
            .iter()
            .map(|c| c.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
    }
    Ok(())
}

fn chunk_section_metadata(chunk: &Chunk) -> (Option<Vec<String>>, Option<String>) {
    section_metadata(&chunk.metadata_json)
}

fn section_metadata(metadata_json: &str) -> (Option<Vec<String>>, Option<String>) {
    let value = serde_json::from_str::<serde_json::Value>(metadata_json).ok();
    let heading_path = value
        .as_ref()
        .and_then(|v| v.get("heading_path"))
        .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
        .filter(|v| !v.is_empty());
    let section = value
        .as_ref()
        .and_then(|v| v.get("section"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|v| !v.is_empty());
    (heading_path, section)
}

fn scope_filters(query: &SearchQuery) -> super::fts::LexFilters {
    super::fts::LexFilters {
        document_id: query.document_id.clone(),
        wing: query.wing.clone(),
        room: query.room.clone(),
        layer: query.layer.clone(),
        source_file: query.source_file.clone(),
        include_archived: query.include_archived,
        ..Default::default()
    }
}

/// Greedy MMR: λ * score − (1−λ) * max Jaccard similarity to selected hits.
fn mmr_select(hits: Vec<SearchHit>, max_per_doc: usize, limit: usize) -> Vec<SearchHit> {
    use std::collections::HashSet;

    if hits.is_empty() || limit == 0 {
        return Vec::new();
    }
    let lambda = 0.7f32;
    let token_sets: Vec<HashSet<String>> = hits
        .iter()
        .map(|h| tokenize(&h.content).into_iter().collect())
        .collect();

    let mut selected: Vec<usize> = Vec::new();
    let mut remaining: HashSet<usize> = (0..hits.len()).collect();
    let mut per_doc: HashMap<String, usize> = HashMap::new();

    while selected.len() < limit && !remaining.is_empty() {
        let mut best_i: Option<usize> = None;
        let mut best_score = f32::NEG_INFINITY;

        for &i in &remaining {
            let count = per_doc.get(&hits[i].document_id).copied().unwrap_or(0);
            if count >= max_per_doc {
                continue;
            }
            let rel = hits[i].score;
            let max_sim = selected
                .iter()
                .map(|&j| jaccard(&token_sets[i], &token_sets[j]))
                .fold(0.0f32, f32::max);
            let mmr = lambda * rel - (1.0 - lambda) * max_sim;
            if mmr > best_score {
                best_score = mmr;
                best_i = Some(i);
            }
        }

        match best_i {
            Some(i) => {
                remaining.remove(&i);
                *per_doc.entry(hits[i].document_id.clone()).or_insert(0) += 1;
                selected.push(i);
            }
            None => break,
        }
    }

    selected.into_iter().map(|i| hits[i].clone()).collect()
}

fn jaccard(a: &std::collections::HashSet<String>, b: &std::collections::HashSet<String>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Number of unique candidates that survive a document cap, before prompt packing.
/// This cheap check deliberately avoids running boosts/MMR/expansion repeatedly.
fn diversity_capacity(hits: &[SearchHit], query: &SearchQuery) -> Option<usize> {
    let cap = match query.diversity {
        Some(DiversityMode::Mmr) => query
            .max_chunks_per_document
            .filter(|cap| *cap != usize::MAX)
            .unwrap_or(3)
            .max(1),
        Some(DiversityMode::CollapseByDocument) => match query.max_chunks_per_document {
            None | Some(usize::MAX) => 1,
            Some(0) => return None,
            Some(cap) => cap,
        },
        None => query.max_chunks_per_document.filter(|cap| *cap > 0)?,
    };
    let mut ids = HashSet::new();
    let mut per_doc = HashMap::new();
    for hit in hits {
        if query.min_score.is_some_and(|min| hit.score < min) || !ids.insert(&hit.chunk_id) {
            continue;
        }
        let count = per_doc.entry(&hit.document_id).or_insert(0usize);
        *count = (*count + 1).min(cap);
    }
    Some(per_doc.values().sum())
}

fn candidate_pool_size(top_k: usize) -> usize {
    top_k
        .saturating_mul(CANDIDATE_MULTIPLIER)
        .max(CANDIDATE_FLOOR)
}

// ---------------------------------------------------------------------------
// Vector search
// ---------------------------------------------------------------------------

static VECTOR_SNAPSHOTS: OnceLock<Mutex<HashMap<PathBuf, Arc<VectorSnapshot>>>> = OnceLock::new();

#[derive(Debug)]
struct VectorSnapshot {
    generation: u64,
    embeddings: Vec<f32>,
    chunks: Vec<VectorSnapshotChunk>,
}

#[derive(Debug)]
struct VectorSnapshotChunk {
    id: String,
    document_id: String,
    chunk_index: i32,
    char_start: i32,
    char_end: i32,
    metadata_json: String,
    embedding_start: usize,
    embedding_len: usize,
}

type VectorDocumentLookup = HashMap<String, (String, String)>;

fn search_vec(
    store: &Store,
    query_embedding: &[f32],
    top_k: usize,
    filters: &LexFilters,
) -> Result<Vec<SearchHit>> {
    if top_k == 0 || query_embedding.is_empty() {
        return Ok(Vec::new());
    }

    let Some(normalized_query) = normalized_vector(query_embedding) else {
        return Ok(Vec::new());
    };

    let (snapshot, documents) = vector_search_inputs(store, filters)?;
    let mut heap: BinaryHeap<Reverse<RankedVectorCandidate>> =
        BinaryHeap::with_capacity(top_k.saturating_add(1));

    for (ordinal, chunk) in snapshot.chunks.iter().enumerate() {
        if !documents.contains_key(&chunk.document_id)
            || chunk.embedding_len != normalized_query.len()
        {
            continue;
        }
        let end = chunk.embedding_start.saturating_add(chunk.embedding_len);
        let Some(embedding) = snapshot.embeddings.get(chunk.embedding_start..end) else {
            continue;
        };
        let score = normalized_query
            .iter()
            .zip(embedding)
            .map(|(left, right)| left * right)
            .sum::<f32>()
            .clamp(-1.0, 1.0);
        let candidate = RankedVectorCandidate {
            score,
            ordinal,
            snapshot_index: ordinal,
        };
        if heap.len() < top_k {
            heap.push(Reverse(candidate));
        } else if heap.peek().is_some_and(|lowest| candidate > lowest.0) {
            heap.pop();
            heap.push(Reverse(candidate));
        }
    }
    let mut ranked = heap
        .into_iter()
        .map(|candidate| candidate.0)
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.cmp(left));
    hydrate_vector_hits(store, &snapshot, &documents, ranked)
}

#[derive(Debug)]
struct RankedVectorCandidate {
    score: f32,
    ordinal: usize,
    snapshot_index: usize,
}

impl PartialEq for RankedVectorCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.score.total_cmp(&other.score) == Ordering::Equal && self.ordinal == other.ordinal
    }
}

impl Eq for RankedVectorCandidate {}

impl PartialOrd for RankedVectorCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedVectorCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            // Stable SQL order wins score ties.
            .then_with(|| other.ordinal.cmp(&self.ordinal))
    }
}

fn vector_search_inputs(
    store: &Store,
    filters: &LexFilters,
) -> Result<(Arc<VectorSnapshot>, VectorDocumentLookup)> {
    let conn = store.lock()?;
    let generation = fts::chunks_generation(&conn)?;
    let snapshot = if has_selective_vector_scope(filters) {
        // Exact project/document searches should pay only for their matching
        // chunks. Do not populate or consult the database-wide cache: unrelated
        // source-sync commits advance the global generation continuously.
        Arc::new(build_vector_snapshot_with_filters(
            &conn,
            generation,
            Some(filters),
        )?)
    } else {
        let key = store.path().to_path_buf();
        let cached = vector_snapshots()
            .lock()
            .map_err(|error| AppError::db(format!("vector snapshot lock poisoned: {error}")))?
            .get(&key)
            .filter(|snapshot| snapshot.generation == generation)
            .cloned();
        match cached {
            Some(snapshot) => snapshot,
            None => {
                let snapshot = Arc::new(build_vector_snapshot(&conn, generation)?);
                vector_snapshots()
                    .lock()
                    .map_err(|error| {
                        AppError::db(format!("vector snapshot lock poisoned: {error}"))
                    })?
                    .insert(key, snapshot.clone());
                snapshot
            }
        }
    };
    let documents = load_vector_documents(&conn, filters)?;
    Ok((snapshot, documents))
}

fn has_selective_vector_scope(filters: &LexFilters) -> bool {
    [
        filters.document_id.as_deref(),
        filters.wing.as_deref(),
        filters.room.as_deref(),
        filters.uri.as_deref(),
        filters.source_file.as_deref(),
    ]
    .into_iter()
    .any(|value| value.is_some_and(|value| !value.trim().is_empty()))
}

fn vector_snapshots() -> &'static Mutex<HashMap<PathBuf, Arc<VectorSnapshot>>> {
    VECTOR_SNAPSHOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn build_vector_snapshot(conn: &duckdb::Connection, generation: u64) -> Result<VectorSnapshot> {
    build_vector_snapshot_with_filters(conn, generation, None)
}

fn build_vector_snapshot_with_filters(
    conn: &duckdb::Connection,
    generation: u64,
    filters: Option<&LexFilters>,
) -> Result<VectorSnapshot> {
    let (join_sql, where_sql, bind) = match filters {
        Some(filters) => {
            let (where_sql, bind) = filters.scope_where_clause("d.id");
            (
                "INNER JOIN documents d ON d.id = c.document_id",
                where_sql,
                bind,
            )
        }
        None => ("", String::new(), Vec::new()),
    };
    let sql = format!(
        r#"
        SELECT c.id, c.document_id, c.chunk_index, c.embedding_json,
               c.char_start, c.char_end, c.metadata_json
        FROM chunks c
        {join_sql}
        {where_sql}
        ORDER BY c.document_id ASC, c.chunk_index ASC, c.id ASC
        "#
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(bind.iter()))?;
    let mut embeddings = Vec::new();
    let mut chunks = Vec::new();
    while let Some(row) = rows.next()? {
        let embedding_json: String = row.get(3)?;
        let raw: Vec<f32> = serde_json::from_str(&embedding_json)?;
        let embedding = normalized_vector(&raw).unwrap_or_else(|| vec![0.0; raw.len()]);
        let embedding_start = embeddings.len();
        let embedding_len = embedding.len();
        embeddings.extend(embedding);
        chunks.push(VectorSnapshotChunk {
            id: row.get(0)?,
            document_id: row.get(1)?,
            chunk_index: row.get(2)?,
            char_start: row.get(4)?,
            char_end: row.get(5)?,
            metadata_json: row
                .get::<_, Option<String>>(6)?
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "{}".into()),
            embedding_start,
            embedding_len,
        });
    }
    Ok(VectorSnapshot {
        generation,
        embeddings,
        chunks,
    })
}

fn load_vector_documents(
    conn: &duckdb::Connection,
    filters: &LexFilters,
) -> Result<VectorDocumentLookup> {
    let (where_sql, bind) = filters.scope_where_clause("d.id");
    let sql = format!("SELECT d.id, d.title, d.uri FROM documents d {where_sql}");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(bind.iter()))?;
    let mut documents = HashMap::new();
    while let Some(row) = rows.next()? {
        documents.insert(row.get(0)?, (row.get(1)?, row.get(2)?));
    }
    Ok(documents)
}

fn normalized_vector(vector: &[f32]) -> Option<Vec<f32>> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    (norm > 0.0).then(|| vector.iter().map(|value| value / norm).collect())
}

fn hydrate_vector_hits(
    store: &Store,
    snapshot: &VectorSnapshot,
    documents: &VectorDocumentLookup,
    ranked: Vec<RankedVectorCandidate>,
) -> Result<Vec<SearchHit>> {
    let conn = store.lock()?;
    let mut stmt = conn.prepare("SELECT content FROM chunks WHERE id = ?")?;
    let mut hits = Vec::with_capacity(ranked.len());
    for candidate in ranked {
        let chunk = &snapshot.chunks[candidate.snapshot_index];
        let Some((title, uri)) = documents.get(&chunk.document_id) else {
            continue;
        };
        let content = stmt.query_row([chunk.id.as_str()], |row| row.get(0))?;
        let (heading_path, section) = section_metadata(&chunk.metadata_json);
        hits.push(SearchHit {
            chunk_id: chunk.id.clone(),
            document_id: chunk.document_id.clone(),
            document_title: title.clone(),
            document_uri: uri.clone(),
            chunk_index: chunk.chunk_index,
            content,
            score: candidate.score,
            score_vec: Some(candidate.score),
            score_lex: None,
            score_rrf: None,
            snippet: None,
            char_start: Some(chunk.char_start),
            char_end: Some(chunk.char_end),
            heading_path,
            section,
            context: None,
            explanation: None,
            rank_vec: None,
            rank_lex: None,
        });
    }
    Ok(hits)
}

// ---------------------------------------------------------------------------
// Lexical search (delegates to db::fts: DuckDB BM25 or TF fallback)
// ---------------------------------------------------------------------------

fn search_lex(
    store: &Store,
    query_text: &str,
    top_k: usize,
    filters: &LexFilters,
    fts_stemmer: &str,
) -> Result<Vec<SearchHit>> {
    if top_k == 0 || query_text.trim().is_empty() {
        return Ok(Vec::new());
    }

    let hits = fts::search_bm25_with_stemmer(store, query_text, top_k, filters, Some(fts_stemmer))?;
    debug!(
        count = hits.len(),
        "lex search via db::fts (BM25 or TF fallback)"
    );
    Ok(hits)
}

/// Simple term-frequency lexical ranking (direct TF path for unit tests).
#[cfg(test)]
fn search_lex_tf(
    store: &Store,
    query_text: &str,
    top_k: usize,
    document_id: Option<&str>,
) -> Result<Vec<SearchHit>> {
    let chunks = load_chunks(store, document_id)?;
    if chunks.is_empty() {
        return Ok(Vec::new());
    }

    let query_terms = tokenize(query_text);
    if query_terms.is_empty() {
        return Ok(Vec::new());
    }

    let mut scored: Vec<(f32, usize)> = chunks
        .iter()
        .enumerate()
        .filter_map(|(i, chunk)| {
            let s = tf_score(&query_terms, &chunk.content);
            if s > 0.0 {
                Some((s, i))
            } else {
                None
            }
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);

    let mut doc_cache: HashMap<String, (String, String)> = HashMap::new();
    let mut hits = Vec::with_capacity(scored.len());
    for (score, idx) in scored {
        let chunk = &chunks[idx];
        let (title, uri) = resolve_doc(store, &chunk.document_id, &mut doc_cache)?;
        hits.push(SearchHit {
            chunk_id: chunk.id.clone(),
            document_id: chunk.document_id.clone(),
            document_title: title,
            document_uri: uri,
            chunk_index: chunk.chunk_index,
            content: chunk.content.clone(),
            score,
            score_vec: None,
            score_lex: Some(score),
            score_rrf: None,
            snippet: None,
            char_start: Some(chunk.char_start),
            char_end: Some(chunk.char_end),
            heading_path: chunk_section_metadata(chunk).0,
            section: chunk_section_metadata(chunk).1,
            context: None,
            explanation: None,
            rank_vec: None,
            rank_lex: None,
        });
    }
    Ok(hits)
}

/// Term-frequency score: sum of occurrences of each query term in `content`.
#[cfg(test)]
fn tf_score(query_terms: &[String], content: &str) -> f32 {
    fts::term_frequency_score(query_terms, content)
}

/// Lowercase alphanumeric tokens (simple, language-agnostic).
fn tokenize(text: &str) -> Vec<String> {
    fts::tokenize(text)
}

// ---------------------------------------------------------------------------
// RRF fusion
// ---------------------------------------------------------------------------

/// Reciprocal rank fusion of two pre-ranked hit lists.
///
/// `score_rrf(d) = Σ 1 / (k + rank_i(d))` over lists that contain `d`.
/// Primary `score` becomes the fused RRF value; `score_vec` / `score_lex` are
/// preserved from the source lists when present.
pub fn fuse_rrf(vec_hits: &[SearchHit], lex_hits: &[SearchHit], rrf_k: f32) -> Vec<SearchHit> {
    let k = if rrf_k > 0.0 { rrf_k } else { DEFAULT_RRF_K };

    // chunk_id -> accumulated RRF + best partial hit
    let mut fused: HashMap<String, (f32, SearchHit)> = HashMap::new();

    accumulate_rrf_list(&mut fused, vec_hits, k);
    accumulate_rrf_list(&mut fused, lex_hits, k);

    let mut out: Vec<SearchHit> = fused
        .into_values()
        .map(|(rrf, mut hit)| {
            hit.score_rrf = Some(rrf);
            hit.score = rrf;
            hit
        })
        .collect();

    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.chunk_id.cmp(&b.chunk_id))
    });
    out
}

fn accumulate_rrf_list(fused: &mut HashMap<String, (f32, SearchHit)>, hits: &[SearchHit], k: f32) {
    for (rank0, hit) in hits.iter().enumerate() {
        let rank = (rank0 + 1) as f32;
        let contrib = 1.0 / (k + rank);
        fused
            .entry(hit.chunk_id.clone())
            .and_modify(|(rrf, existing)| {
                *rrf += contrib;
                merge_score_fields(existing, hit);
            })
            .or_insert_with(|| {
                let mut base = hit.clone();
                // Clear rrf until final write; primary will be set after full fusion.
                base.score_rrf = None;
                (contrib, base)
            });
    }
}

fn merge_score_fields(dst: &mut SearchHit, src: &SearchHit) {
    if dst.score_vec.is_none() {
        dst.score_vec = src.score_vec;
    }
    if dst.score_lex.is_none() {
        dst.score_lex = src.score_lex;
    }
    if dst.rank_vec.is_none() {
        dst.rank_vec = src.rank_vec;
    }
    if dst.rank_lex.is_none() {
        dst.rank_lex = src.rank_lex;
    }
    // Prefer longer content / filled offsets if somehow missing.
    if dst.char_start.is_none() {
        dst.char_start = src.char_start;
    }
    if dst.char_end.is_none() {
        dst.char_end = src.char_end;
    }
    if dst.document_title.is_empty() && !src.document_title.is_empty() {
        dst.document_title = src.document_title.clone();
    }
    if dst.document_uri.is_empty() && !src.document_uri.is_empty() {
        dst.document_uri = src.document_uri.clone();
    }
}

// ---------------------------------------------------------------------------
// Diversity + min_score + snippets
// ---------------------------------------------------------------------------

/// Keep at most `max_per_doc` hits per `document_id`, preserving rank order.
pub fn collapse_by_document(hits: Vec<SearchHit>, max_per_doc: usize) -> Vec<SearchHit> {
    if max_per_doc == 0 {
        return Vec::new();
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::with_capacity(hits.len());
    for hit in hits {
        let n = counts.entry(hit.document_id.clone()).or_insert(0);
        if *n < max_per_doc {
            *n += 1;
            out.push(hit);
        }
    }
    out
}

/// Drop hits with primary `score` strictly below `min_score`.
pub fn apply_min_score(hits: Vec<SearchHit>, min_score: f32) -> Vec<SearchHit> {
    hits.into_iter().filter(|h| h.score >= min_score).collect()
}

/// Build a short excerpt around the first query-term match (or content prefix).
pub fn make_snippet(content: &str, query: Option<&str>, max_chars: usize) -> Option<String> {
    if content.is_empty() || max_chars == 0 {
        return None;
    }

    let chars: Vec<char> = content.chars().collect();
    if chars.is_empty() {
        return None;
    }

    let mut anchor = 0usize;
    if let Some(q) = query {
        let lower_content: String = content.to_lowercase();
        for term in tokenize(q) {
            if term.is_empty() {
                continue;
            }
            if let Some(byte_pos) = lower_content.find(&term) {
                // Map byte offset to char index.
                anchor = content[..byte_pos].chars().count();
                break;
            }
        }
    }

    // Center a window of max_chars around the anchor when possible.
    let half = max_chars / 2;
    let start = anchor.saturating_sub(half);
    let end = (start + max_chars).min(chars.len());
    let start = end.saturating_sub(max_chars);

    let mut snippet: String = chars[start..end].iter().collect();
    if start > 0 {
        snippet.insert(0, '…');
    }
    if end < chars.len() {
        snippet.push('…');
    }
    if snippet.is_empty() {
        None
    } else {
        Some(snippet)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
fn load_chunks(store: &Store, document_id: Option<&str>) -> Result<Vec<Chunk>> {
    match document_id {
        Some(doc_id) => store.list_chunks_for_document(doc_id),
        None => store.all_chunks_with_embeddings(),
    }
}

#[cfg(test)]
fn resolve_doc(
    store: &Store,
    document_id: &str,
    cache: &mut HashMap<String, (String, String)>,
) -> Result<(String, String)> {
    if let Some(entry) = cache.get(document_id) {
        return Ok(entry.clone());
    }
    let (title, uri) = match store.get_document(document_id)? {
        Some(doc) => (doc.title, doc.uri),
        None => (String::new(), String::new()),
    };
    cache.insert(document_id.to_string(), (title.clone(), uri.clone()));
    Ok((title, uri))
}

#[cfg(test)]
mod tests;
