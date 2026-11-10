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
use std::sync::Arc;
use std::time::{Duration, Instant};

use duckdb::params_from_iter;
use tracing::{debug, warn};

use crate::error::{AppError, Result};
use crate::models::{SearchContextChunk, SearchExplanation, SearchHit, SearchMode};

#[cfg(test)]
use crate::embeddings::cosine_similarity;

use super::fts::{self, LexFilters};
use super::store::{ChunkBody, Store};

/// Default RRF rank constant (Cormack / common IR practice).
pub const DEFAULT_RRF_K: f32 = 60.0;

/// Default max snippet length in Unicode characters.
pub const DEFAULT_SNIPPET_CHARS: usize = 200;

/// Extra candidates pulled from each list before fusion / collapse.
const CANDIDATE_MULTIPLIER: usize = 5;
const CANDIDATE_FLOOR: usize = 50;
/// Hard per-source bound for adaptive diversity refill.
const MAX_DIVERSITY_CANDIDATES: usize = 4096;
pub use crate::config::limits::MAX_TOP_K;
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
    /// Stored parent/child hierarchy: a hit on a leaf chunk returns its parent
    /// chunk plus sibling leaves (the full section / code item), deduped when
    /// several hits share one parent. Chunks with no stored parent keep their
    /// own body.
    Parent,
}

impl ContextExpansion {
    /// Parse a context expansion name (`neighbors` or `parent_section`, with aliases).
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "neighbors" | "neighboring_chunks" => Ok(Self::Neighbors),
            "parent_section" | "section" => Ok(Self::ParentSection),
            "parent" | "parent_chunk" | "parent_chunks" => Ok(Self::Parent),
            other => Err(AppError::config(format!(
                "invalid context_expansion '{other}': expected 'neighbors', 'parent_section', or 'parent'"
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
    let guard = store.corpus_lane().try_corpus_idle_guard().ok_or_else(|| {
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
        return Err(AppError::timeout(format!(
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

/// Post-process ranked hits: min_score, boosts, diversity, top_k, snippets, token pack.
fn finalize_hits(
    store: &Store,
    mut hits: Vec<SearchHit>,
    query: &SearchQuery,
) -> Result<Vec<SearchHit>> {
    if let Some(min) = query.min_score {
        hits = apply_min_score(hits, min);
    }

    let recency = query
        .recency_half_life_days
        .filter(|days| days.is_finite() && *days > 0.0);
    let boost_query = query
        .query_text
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty());
    // One lightweight batch read serves both recency and field boosts; this
    // used to be two full `get_document` loads per hit.
    let ranking = if recency.is_some() || boost_query.is_some() {
        ranking_fields(store, &hits)?
    } else {
        HashMap::new()
    };

    if let Some(half_life_days) = recency {
        let now = query.now.unwrap_or_else(chrono::Utc::now);
        for hit in &mut hits {
            if let Some(document) = ranking.get(&hit.document_id) {
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

    apply_field_boosts(&ranking, &mut hits, boost_query);
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

    // Snippets only for the returned hits; the refill pool can be thousands.
    let qtext = query.query_text.as_deref();
    for hit in &mut hits {
        if query.snippet_max_chars > 0 && hit.snippet.is_none() {
            hit.snippet = make_snippet(&hit.content, qtext, query.snippet_max_chars);
        }
    }

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

/// Document fields used for ranking, without the document body.
struct RankingFields {
    title: String,
    uri: String,
    pinned: bool,
    boost: f64,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// Ranking fields for every distinct document among `hits`, in one query.
fn ranking_fields(store: &Store, hits: &[SearchHit]) -> Result<HashMap<String, RankingFields>> {
    let ids: Vec<&str> = hits
        .iter()
        .map(|hit| hit.document_id.as_str())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = crate::db::sql::in_placeholders(ids.len());
    let conn = store.lock()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT id, title, uri, COALESCE(pinned, false), COALESCE(boost, 1.0), \
         CAST(updated_at AS VARCHAR) FROM documents WHERE id IN ({placeholders})"
    ))?;
    let rows = crate::db::rows::query_vec(&mut stmt, params_from_iter(ids.iter()), |row| {
        let updated_raw: String = row.get(5)?;
        let updated_at = crate::util::parse_db_timestamp(&updated_raw).ok_or_else(|| {
            AppError::db(format!("invalid timestamp value: {}", updated_raw.trim()))
        })?;
        Ok((
            row.get::<_, String>(0)?,
            RankingFields {
                title: row.get(1)?,
                uri: row.get(2)?,
                pinned: row.get(3)?,
                boost: row.get(4)?,
                updated_at,
            },
        ))
    })?;
    Ok(rows.into_iter().collect())
}

fn apply_field_boosts(
    ranking: &HashMap<String, RankingFields>,
    hits: &mut [SearchHit],
    query: Option<&str>,
) {
    let Some(raw) = query else {
        return;
    };
    let needle = raw.to_lowercase();
    let terms = fts::tokenize(&needle);
    // Per-document checks repeat for every hit of that document: memoize.
    let mut uri_matches: HashMap<&str, bool> = HashMap::new();
    for hit in hits {
        let mut reasons = Vec::new();
        if let Some(document) = ranking.get(&hit.document_id) {
            if document.title.trim().eq_ignore_ascii_case(raw) {
                hit.score *= 1.35;
                reasons.push("exact_title_boost".into());
            }
            let uri_match = *uri_matches.entry(document.uri.as_str()).or_insert_with(|| {
                let uri = document.uri.to_lowercase();
                terms.iter().all(|term| uri.contains(term.as_str()))
            });
            if uri_match {
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

/// Attach section labels and, when requested, neighbor or parent-section context to hits.
pub fn attach_context(
    store: &Store,
    hits: &mut [SearchHit],
    expansion: Option<ContextExpansion>,
    neighbor_chunks: usize,
) -> Result<()> {
    // Parent ids already expanded by an earlier hit (dedup shared parents).
    let mut expanded_parents: std::collections::HashSet<String> = std::collections::HashSet::new();
    let Some(expansion) = expansion else {
        // Section labels only: one metadata read for all hits, no chunk bodies.
        let ids: Vec<&str> = hits.iter().map(|hit| hit.chunk_id.as_str()).collect();
        let metadata = store.chunks().chunk_metadata_by_ids(&ids)?;
        for hit in hits {
            if let Some(json) = metadata.get(&hit.chunk_id) {
                (hit.heading_path, hit.section) = section_metadata(json);
            }
        }
        return Ok(());
    };

    // Parent sections can span a whole document, so those load every chunk
    // body once per document; neighbor windows read only `index ± radius`.
    // Bodies never include embeddings, and section metadata is parsed once.
    type Sectioned = (ChunkBody, Option<Vec<String>>, Option<String>);
    let sectioned = |bodies: Vec<ChunkBody>| -> Vec<Sectioned> {
        bodies
            .into_iter()
            .map(|chunk| {
                let (path, section) = section_metadata(&chunk.metadata_json);
                (chunk, path, section)
            })
            .collect()
    };
    let radius = neighbor_chunks.max(1) as i32;
    let mut chunks_by_document: HashMap<String, Vec<Sectioned>> = HashMap::new();
    let mut window: Vec<Sectioned>;
    for hit in hits {
        let chunks: &Vec<Sectioned> = match expansion {
            ContextExpansion::ParentSection | ContextExpansion::Parent => {
                if !chunks_by_document.contains_key(&hit.document_id) {
                    let loaded = sectioned(
                        store
                            .chunks()
                            .list_chunk_bodies_for_document(&hit.document_id)?,
                    );
                    chunks_by_document.insert(hit.document_id.clone(), loaded);
                }
                &chunks_by_document[&hit.document_id]
            }
            ContextExpansion::Neighbors => {
                window = sectioned(store.chunks().list_chunk_bodies_in_range(
                    &hit.document_id,
                    hit.chunk_index.saturating_sub(radius),
                    hit.chunk_index.saturating_add(radius),
                )?);
                &window
            }
        };
        let Some((current, heading_path, section)) =
            chunks.iter().find(|(c, _, _)| c.id == hit.chunk_id)
        else {
            continue;
        };
        hit.heading_path = heading_path.clone();
        hit.section = section.clone();

        let selected: Vec<&Sectioned> = match expansion {
            ContextExpansion::Neighbors => chunks
                .iter()
                .filter(|(c, _, _)| (c.chunk_index - current.chunk_index).abs() <= radius)
                .collect(),
            ContextExpansion::ParentSection => chunks
                .iter()
                .filter(|(_, path, _)| heading_path.is_some() && path == heading_path)
                .collect(),
            ContextExpansion::Parent => {
                match chunk_parent_id(current) {
                    Some(parent_id) if expanded_parents.insert(parent_id.clone()) => {
                        let family: Vec<&Sectioned> = chunks
                            .iter()
                            .filter(|(c, _, _)| {
                                c.id == parent_id
                                    || chunk_parent_id(c).as_deref() == Some(parent_id.as_str())
                            })
                            .collect();
                        // A dangling parent id (stale link, parent in another
                        // document) falls back to the hit's own body below.
                        family
                    }
                    // Parent already returned by an earlier hit, or no stored
                    // parent (pre-v17 rows, single-chunk sections): keep self.
                    _ => Vec::new(),
                }
            }
        };
        let selected: Vec<&Sectioned> = if selected.is_empty() {
            chunks
                .iter()
                .filter(|(c, _, _)| c.id == current.id)
                .collect()
        } else {
            selected
        };
        hit.context = Some(
            selected
                .iter()
                .map(|(chunk, heading_path, section)| SearchContextChunk {
                    chunk_id: chunk.id.to_string(),
                    chunk_index: chunk.chunk_index,
                    content: chunk.content.clone(),
                    heading_path: heading_path.clone(),
                    section: section.clone(),
                })
                .collect(),
        );
        hit.content = selected
            .iter()
            .map(|(c, _, _)| c.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
    }
    Ok(())
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
        .map(|h| fts::tokenize(&h.content).into_iter().collect())
        .collect();

    // `max_sim[i]` is hit i's highest similarity to anything selected so far;
    // each round folds in only the newest pick: O(limit * pool), not
    // O(limit^2 * pool). Candidates are scanned in rank order so score ties
    // resolve deterministically to the better-ranked hit.
    let mut max_sim = vec![0.0f32; hits.len()];
    let mut available = vec![true; hits.len()];
    let mut selected: Vec<usize> = Vec::new();
    let mut per_doc: HashMap<&str, usize> = HashMap::new();

    while selected.len() < limit {
        let mut best: Option<(usize, f32)> = None;
        for i in (0..hits.len()).filter(|&i| available[i]) {
            let count = per_doc
                .get(hits[i].document_id.as_str())
                .copied()
                .unwrap_or(0);
            if count >= max_per_doc {
                continue;
            }
            let mmr = lambda * hits[i].score - (1.0 - lambda) * max_sim[i];
            if best.is_none_or(|(_, best_score)| mmr > best_score) {
                best = Some((i, mmr));
            }
        }
        let Some((pick, _)) = best else { break };
        available[pick] = false;
        *per_doc.entry(hits[pick].document_id.as_str()).or_insert(0) += 1;
        selected.push(pick);
        for i in (0..hits.len()).filter(|&i| available[i]) {
            max_sim[i] = max_sim[i].max(jaccard(&token_sets[i], &token_sets[pick]));
        }
    }

    let mut slots: Vec<Option<SearchHit>> = hits.into_iter().map(Some).collect();
    selected
        .into_iter()
        .filter_map(|i| slots[i].take())
        .collect()
}

fn jaccard(a: &std::collections::HashSet<String>, b: &std::collections::HashSet<String>) -> f32 {
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let inter = small.iter().filter(|token| large.contains(*token)).count();
    let union = a.len() + b.len() - inter;
    if union == 0 {
        0.0
    } else {
        inter as f32 / union as f32
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

/// Normalized vector snapshots of one database, owned by its [`Store`] (and
/// shared by its clones). Rows are read under the connection mutex; parsing,
/// normalizing and interning run after it is released, so writers are not
/// blocked by a snapshot build.
#[derive(Default)]
pub(crate) struct VectorCache {
    /// Database-wide snapshot of the latest generation built.
    global: Option<Arc<VectorSnapshot>>,
    /// The last scoped search's snapshot.
    scoped: Option<ScopedVectorSnapshot>,
}

impl std::fmt::Debug for VectorCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorCache")
            .field("global", &self.global.as_ref().map(|s| s.generation))
            .field(
                "scoped",
                &self.scoped.as_ref().map(|s| s.snapshot.generation),
            )
            .finish()
    }
}

#[derive(Debug)]
pub(crate) struct VectorSnapshot {
    generation: u64,
    chunks: Vec<VectorSnapshotChunk>,
    /// Interned document ids, shared with the previous snapshot while no new
    /// document appears.
    documents: Arc<DocumentTable>,
}

/// Interned document ids: `VectorSnapshotChunk::document` indexes `ids`.
/// Ids of deleted documents may linger; no chunk points at them.
#[derive(Debug, Clone, Default)]
struct DocumentTable {
    ids: Vec<String>,
    index: HashMap<String, u32>,
}

impl DocumentTable {
    fn intern(&mut self, id: &str) -> Result<u32> {
        if let Some(index) = self.index.get(id) {
            return Ok(*index);
        }
        let index = u32::try_from(self.ids.len())
            .map_err(|_| AppError::db("vector snapshot exceeds u32 documents"))?;
        self.index.insert(id.to_owned(), index);
        self.ids.push(id.to_owned());
        Ok(index)
    }
}

#[derive(Debug, Clone)]
struct VectorSnapshotChunk {
    id: Arc<str>,
    document: u32,
    chunk_index: i32,
    /// `chunks.embedding_digest`: lets a refresh reuse this parsed vector
    /// when the stored embedding is unchanged.
    embedding_hash: i64,
    /// Normalized vector, shared by reference across snapshot refreshes so an
    /// unchanged chunk is never copied.
    vector: Arc<[f32]>,
}

impl VectorSnapshot {
    fn document_id(&self, chunk: &VectorSnapshotChunk) -> &str {
        &self.documents.ids[chunk.document as usize]
    }
}

/// A stored embedding read under the connection lock: either reused from the
/// previous snapshot or still to be normalized.
enum RawVector {
    Reuse(Arc<[f32]>),
    Parse(Vec<f32>),
}

/// One chunk row read under the lock, before parsing and interning.
struct RawChunk {
    id: String,
    document_id: String,
    chunk_index: i32,
    embedding_hash: i64,
    vector: RawVector,
}

/// Rows of a snapshot read under the lock; [`Self::finish`] runs without it.
struct RawSnapshot {
    generation: u64,
    /// In snapshot order (document, chunk index, id).
    rows: Vec<RawChunk>,
    /// Change-log refresh: the previous snapshot, and per interned document
    /// whether `rows` replaces its chunks. Other documents keep theirs.
    base: Option<(Arc<VectorSnapshot>, Vec<bool>)>,
}

impl RawSnapshot {
    fn finish(self) -> Result<VectorSnapshot> {
        let Self {
            generation,
            rows,
            base,
        } = self;
        // Reuse the previous document table unless a new document appears.
        let mut documents = match &base {
            Some((previous, _))
                if rows
                    .iter()
                    .all(|row| previous.documents.index.contains_key(&row.document_id)) =>
            {
                previous.documents.clone()
            }
            Some((previous, _)) => Arc::new((*previous.documents).clone()),
            None => Arc::new(DocumentTable::default()),
        };
        let mut fresh = Vec::with_capacity(rows.len());
        {
            let table = Arc::make_mut(&mut documents);
            for row in rows {
                let vector = match row.vector {
                    RawVector::Reuse(vector) => vector,
                    RawVector::Parse(raw) => normalize_stored(&raw),
                };
                fresh.push(VectorSnapshotChunk {
                    id: row.id.into(),
                    document: table.intern(&row.document_id)?,
                    chunk_index: row.chunk_index,
                    embedding_hash: row.embedding_hash,
                    vector,
                });
            }
        }
        let chunks = match base {
            None => fresh,
            Some((previous, changed)) => {
                // Merge two sorted runs: the kept chunks and the fresh rows.
                let key = |chunk: &VectorSnapshotChunk| {
                    (
                        documents.ids[chunk.document as usize].as_str(),
                        chunk.chunk_index,
                    )
                };
                let mut kept = previous
                    .chunks
                    .iter()
                    .filter(|chunk| !changed[chunk.document as usize])
                    .peekable();
                let mut fresh = fresh.into_iter().peekable();
                let mut merged = Vec::with_capacity(previous.chunks.len() + fresh.len());
                loop {
                    let take_kept = match (kept.peek(), fresh.peek()) {
                        (Some(old), Some(new)) => (key(old), &*old.id) <= (key(new), &*new.id),
                        (Some(_), None) => true,
                        (None, Some(_)) => false,
                        (None, None) => break,
                    };
                    if take_kept {
                        merged.extend(kept.next().cloned());
                    } else {
                        merged.extend(fresh.next());
                    }
                }
                merged
            }
        };
        Ok(VectorSnapshot {
            generation,
            chunks,
            documents,
        })
    }
}

fn normalize_stored(raw: &[f32]) -> Arc<[f32]> {
    normalized_vector(raw)
        .unwrap_or_else(|| vec![0.0; raw.len()])
        .into()
}

/// Documents a vector search may return. Titles and uris are read only for
/// the hits that are returned.
enum VectorDocumentScope {
    /// Every document except these. Used when the scope is only low-cardinality
    /// filters (status, layer, kind) that match most of the corpus, so the
    /// complement is the small side (typically the archived and tombstoned
    /// documents); listing the matching side would read most of the corpus on
    /// every query.
    Excluding(HashSet<String>),
    /// Exactly these documents: a selective filter applies, or the scope
    /// matches at most half the corpus.
    Only(HashSet<String>),
}

impl VectorDocumentScope {
    /// Membership per interned document of `snapshot`.
    fn mask(&self, snapshot: &VectorSnapshot) -> Vec<bool> {
        match self {
            Self::Excluding(ids) => snapshot
                .documents
                .ids
                .iter()
                .map(|id| !ids.contains(id))
                .collect(),
            Self::Only(ids) => {
                let mut mask = vec![false; snapshot.documents.ids.len()];
                for id in ids {
                    if let Some(index) = snapshot.documents.index.get(id) {
                        mask[*index as usize] = true;
                    }
                }
                mask
            }
        }
    }

    /// Add the documents of chunks without a document row to an exclusion
    /// set (no-op for `Only`, which lists existing documents).
    fn exclude_orphan_documents(&mut self, store: &Store) -> Result<()> {
        if let Self::Excluding(ids) = self {
            let conn = store.lock()?;
            let mut stmt = conn.prepare(
                "SELECT DISTINCT c.document_id FROM chunks c \
                 WHERE NOT EXISTS (SELECT 1 FROM documents d WHERE d.id = c.document_id)",
            )?;
            crate::db::rows::for_each_row(&mut stmt, [], |row| {
                ids.insert(row.get(0)?);
                Ok(())
            })?;
        }
        Ok(())
    }

    fn only_ids(&self) -> Option<&HashSet<String>> {
        match self {
            Self::Excluding(_) => None,
            Self::Only(ids) => Some(ids),
        }
    }
}

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

    let (snapshot, mut documents) = vector_search_inputs(store, filters)?;
    let ranked = rank_vector_candidates(&snapshot, &documents, &normalized_query, top_k);
    let (hits, missing_document) = hydrate_vector_hits(store, &snapshot, ranked)?;
    if !missing_document || documents.only_ids().is_some() {
        return Ok(hits);
    }
    // The database-wide snapshot keeps chunks whose document row is missing
    // (orphans). An exclusion set admits them; they hold ranking slots and are
    // dropped at hydration. Exclude them and rank again so `top_k` real hits
    // remain.
    documents.exclude_orphan_documents(store)?;
    let ranked = rank_vector_candidates(&snapshot, &documents, &normalized_query, top_k);
    Ok(hydrate_vector_hits(store, &snapshot, ranked)?.0)
}

fn rank_vector_candidates(
    snapshot: &VectorSnapshot,
    documents: &VectorDocumentScope,
    normalized_query: &[f32],
    top_k: usize,
) -> Vec<RankedVectorCandidate> {
    let mut heap: BinaryHeap<Reverse<RankedVectorCandidate>> =
        BinaryHeap::with_capacity(top_k.saturating_add(1));
    let allowed = documents.mask(snapshot);

    for (ordinal, chunk) in snapshot.chunks.iter().enumerate() {
        if !allowed[chunk.document as usize] || chunk.vector.len() != normalized_query.len() {
            continue;
        }
        let score = normalized_query
            .iter()
            .zip(chunk.vector.iter())
            .map(|(left, right)| left * right)
            .sum::<f32>()
            .clamp(-1.0, 1.0);
        let candidate = RankedVectorCandidate { score, ordinal };
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
    ranked
}

#[derive(Debug)]
struct RankedVectorCandidate {
    score: f32,
    /// Index into the snapshot's chunks (stable SQL order).
    ordinal: usize,
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

/// What the locked phase of [`vector_search_inputs`] decided.
enum SnapshotPlan {
    Cached(Arc<VectorSnapshot>),
    /// Rows read; finish outside the lock, then cache as database-wide.
    Global(RawSnapshot),
    /// Rows read; finish outside the lock, then cache as the scoped entry.
    Scoped {
        raw: RawSnapshot,
        scope: (String, Vec<String>),
    },
}

fn lock_cache(store: &Store) -> Result<std::sync::MutexGuard<'_, VectorCache>> {
    store
        .vector_cache()
        .lock()
        .map_err(|error| AppError::db(format!("vector snapshot lock poisoned: {error}")))
}

fn vector_search_inputs(
    store: &Store,
    filters: &LexFilters,
) -> Result<(Arc<VectorSnapshot>, VectorDocumentScope)> {
    let (plan, documents) = {
        let conn = store.lock()?;
        let generation = fts::chunks_generation(&conn)?;
        // Read live: metadata-only writes (placement, archive, restore) change
        // the scope membership without advancing the chunk generation.
        let documents = load_vector_documents(&conn, filters)?;
        // Lock order: connection, then cache; the cache lock is never held
        // while waiting for the connection.
        let (global, scoped) = {
            let cache = lock_cache(store)?;
            (
                cache.global.clone(),
                cache.scoped.as_ref().map(|entry| {
                    (
                        entry.scope.clone(),
                        entry.documents.clone(),
                        entry.snapshot.clone(),
                    )
                }),
            )
        };
        // A database-wide snapshot already built for this generation serves
        // any scope: ranking skips chunks whose document is outside the scope.
        let plan = match global.as_ref().filter(|s| s.generation == generation) {
            Some(snapshot) => SnapshotPlan::Cached(snapshot.clone()),
            None if has_selective_vector_scope(filters) => {
                // Exact project/document searches should pay only for their
                // matching chunks. Do not populate the database-wide cache:
                // unrelated source-sync commits advance the global generation
                // continuously. The last scope is cached instead, so candidate
                // refill rounds and multi-query fan-out reuse it. It is reused
                // only while every document now in scope was in scope when it
                // was built; a document moved into the scope forces a rebuild.
                let scope = filters.scope_where_clause("d.id");
                let reusable = scoped
                    .as_ref()
                    .filter(|(entry_scope, entry_docs, snapshot)| {
                        *entry_scope == scope
                            && snapshot.generation == generation
                            && documents
                                .only_ids()
                                .is_some_and(|ids| ids.iter().all(|id| entry_docs.contains(id)))
                    });
                match reusable {
                    Some((_, _, snapshot)) => SnapshotPlan::Cached(snapshot.clone()),
                    None => {
                        // Reuse parsed vectors (by chunk id + digest) from the
                        // last snapshot of this scope, else from the global
                        // snapshot, else from another scope; parse only new or
                        // changed chunks.
                        let previous = match (scoped, global) {
                            (Some((entry_scope, _, same)), _) if entry_scope == scope => Some(same),
                            (_, Some(global)) => Some(global),
                            (Some((_, _, other)), None) => Some(other),
                            (None, None) => None,
                        };
                        let raw = match previous {
                            Some(previous) => {
                                read_refresh(&conn, &previous, generation, Some(filters))?
                            }
                            None => read_full(&conn, generation, Some(filters))?,
                        };
                        SnapshotPlan::Scoped { raw, scope }
                    }
                }
            }
            None => {
                // Refresh from the previous generation's snapshot when there
                // is one; parse the whole corpus only on the first build.
                let raw = match global {
                    Some(previous) => read_refresh(&conn, &previous, generation, None)?,
                    None => read_full(&conn, generation, None)?,
                };
                SnapshotPlan::Global(raw)
            }
        };
        (plan, documents)
    };
    // The connection is released: parse, normalize and intern without it.
    let snapshot = match plan {
        SnapshotPlan::Cached(snapshot) => snapshot,
        SnapshotPlan::Global(raw) => {
            let snapshot = Arc::new(raw.finish()?);
            let mut cache = lock_cache(store)?;
            // A concurrent search may have cached a newer generation.
            if cache
                .global
                .as_ref()
                .is_none_or(|current| current.generation <= snapshot.generation)
            {
                cache.global = Some(snapshot.clone());
            }
            snapshot
        }
        SnapshotPlan::Scoped { raw, scope } => {
            let snapshot = Arc::new(raw.finish()?);
            lock_cache(store)?.scoped = Some(ScopedVectorSnapshot {
                scope,
                documents: documents.only_ids().cloned().unwrap_or_default(),
                snapshot: snapshot.clone(),
            });
            snapshot
        }
    };
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

/// The last scoped search's snapshot, with the scope it was built for.
struct ScopedVectorSnapshot {
    /// Scope SQL clause plus binds.
    scope: (String, Vec<String>),
    /// Documents in scope when the snapshot was built.
    documents: HashSet<String>,
    snapshot: Arc<VectorSnapshot>,
}

/// `JOIN` and `WHERE` for a scoped listing; none for the database-wide one.
fn snapshot_scope_sql(filters: Option<&LexFilters>) -> (&'static str, String, Vec<String>) {
    match filters {
        Some(filters) => {
            let (where_sql, bind) = filters.scope_where_clause("d.id");
            (
                "INNER JOIN documents d ON d.id = c.document_id",
                where_sql,
                bind,
            )
        }
        // No join: the database-wide snapshot must stay a function of the
        // chunk generation alone, and creating a document row does not bump
        // it. Orphan chunks are filtered at query time instead.
        None => ("", String::new(), Vec::new()),
    }
}

/// Read every chunk (of the scope) with its stored embedding.
fn read_full(
    conn: &duckdb::Connection,
    generation: u64,
    filters: Option<&LexFilters>,
) -> Result<RawSnapshot> {
    let (join_sql, where_sql, bind) = snapshot_scope_sql(filters);
    let sql = format!(
        r#"
        SELECT c.id, c.document_id, c.chunk_index, c.embedding,
               COALESCE(c.embedding_digest, CAST(hash(c.embedding) >> 1 AS BIGINT))
        FROM chunks c
        {join_sql}
        {where_sql}
        ORDER BY c.document_id ASC, c.chunk_index ASC, c.id ASC
        "#
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = crate::db::rows::query_vec(&mut stmt, params_from_iter(bind.iter()), |row| {
        Ok(RawChunk {
            id: row.get(0)?,
            document_id: row.get(1)?,
            chunk_index: row.get(2)?,
            vector: RawVector::Parse(crate::db::rows::embedding(row, 3)?),
            embedding_hash: row.get(4)?,
        })
    })?;
    Ok(RawSnapshot {
        generation,
        rows,
        base: None,
    })
}

/// Read what a snapshot at `generation` needs beyond `previous`.
///
/// Lists every chunk's identity, order and stored `embedding_digest` (the
/// embedding is never read for unchanged rows), reuses the parsed vector of
/// each chunk whose id and digest match `previous`, and reads only new or
/// changed rows. A typical ingest touches one document, so this replaces a
/// full-corpus parse with a cheap listing plus that document's chunks. When
/// most rows changed a full read is cheaper and is used instead.
fn read_refresh(
    conn: &duckdb::Connection,
    previous: &Arc<VectorSnapshot>,
    generation: u64,
    filters: Option<&LexFilters>,
) -> Result<RawSnapshot> {
    // Database-wide snapshot: when the change log names every document whose
    // chunks changed since `previous`, re-read only those documents.
    if filters.is_none() {
        if let Some(documents) =
            fts::changed_documents_since(conn, previous.generation, generation)?
        {
            return read_changed_documents(conn, previous, generation, &documents);
        }
    }
    let (join_sql, where_sql, bind) = snapshot_scope_sql(filters);
    let mut stmt = conn.prepare(&format!(
        r#"
        SELECT c.id, c.document_id, c.chunk_index,
               COALESCE(c.embedding_digest, CAST(hash(c.embedding) >> 1 AS BIGINT))
        FROM chunks c
        {join_sql}
        {where_sql}
        ORDER BY c.document_id ASC, c.chunk_index ASC, c.id ASC
        "#
    ))?;
    let listed = crate::db::rows::query_vec(&mut stmt, params_from_iter(bind.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i32>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    drop(stmt);

    let reusable: HashMap<&str, &VectorSnapshotChunk> = previous
        .chunks
        .iter()
        .map(|chunk| (&*chunk.id, chunk))
        .collect();
    let changed: Vec<&str> = listed
        .iter()
        .filter(|(id, _, _, hash)| {
            reusable
                .get(id.as_str())
                .is_none_or(|old| old.embedding_hash != *hash)
        })
        .map(|(id, _, _, _)| id.as_str())
        .collect();
    if changed.len() * 2 > listed.len() {
        return read_full(conn, generation, filters);
    }
    let mut stored = read_embeddings(conn, &changed)?;
    let mut rows = Vec::with_capacity(listed.len());
    for (id, document_id, chunk_index, embedding_hash) in listed {
        let vector = match stored.remove(&id) {
            Some(raw) => RawVector::Parse(raw),
            // Unchanged: share the previous snapshot's vector, no copy.
            None => match reusable.get(id.as_str()) {
                Some(old) if old.embedding_hash == embedding_hash => {
                    RawVector::Reuse(old.vector.clone())
                }
                // Deleted between the listing and the read: skip it.
                _ => continue,
            },
        };
        rows.push(RawChunk {
            id,
            document_id,
            chunk_index,
            embedding_hash,
            vector,
        });
    }
    Ok(RawSnapshot {
        generation,
        rows,
        base: None,
    })
}

/// Stored embeddings of `ids`, in `IN` batches.
fn read_embeddings<S: AsRef<str>>(
    conn: &duckdb::Connection,
    ids: &[S],
) -> Result<HashMap<String, Vec<f32>>> {
    const READ_BATCH: usize = 512;
    let mut stored: HashMap<String, Vec<f32>> = HashMap::with_capacity(ids.len());
    for batch in ids.chunks(READ_BATCH) {
        let placeholders = crate::db::sql::in_placeholders(batch.len());
        let mut stmt = conn.prepare(&format!(
            "SELECT id, embedding FROM chunks WHERE id IN ({placeholders})"
        ))?;
        let binds: Vec<&str> = batch.iter().map(AsRef::as_ref).collect();
        crate::db::rows::for_each_row(&mut stmt, params_from_iter(binds.iter()), |row| {
            stored.insert(row.get(0)?, crate::db::rows::embedding(row, 1)?);
            Ok(())
        })?;
    }
    Ok(stored)
}

/// Database-wide refresh from the change log: keep every chunk of documents
/// not in `documents`, and re-read (reusing vectors by id + digest) only the
/// chunks of `documents`. Cost follows the change, not the corpus.
fn read_changed_documents(
    conn: &duckdb::Connection,
    previous: &Arc<VectorSnapshot>,
    generation: u64,
    documents: &HashSet<String>,
) -> Result<RawSnapshot> {
    const DOCUMENT_BATCH: usize = 512;
    let ids: Vec<&str> = documents.iter().map(String::as_str).collect();
    let changed_mask: Vec<bool> = previous
        .documents
        .ids
        .iter()
        .map(|id| documents.contains(id))
        .collect();
    let reusable: HashMap<&str, &VectorSnapshotChunk> = previous
        .chunks
        .iter()
        .filter(|chunk| changed_mask[chunk.document as usize])
        .map(|chunk| (&*chunk.id, chunk))
        .collect();
    let mut listed: Vec<RawChunk> = Vec::new();
    for batch in ids.chunks(DOCUMENT_BATCH) {
        let placeholders = crate::db::sql::in_placeholders(batch.len());
        let mut stmt = conn.prepare(&format!(
            "SELECT id, document_id, chunk_index, \
                    COALESCE(embedding_digest, CAST(hash(embedding) >> 1 AS BIGINT)) \
             FROM chunks WHERE document_id IN ({placeholders})"
        ))?;
        crate::db::rows::for_each_row(&mut stmt, params_from_iter(batch.iter()), |row| {
            let id: String = row.get(0)?;
            let embedding_hash: i64 = row.get(3)?;
            let vector = match reusable
                .get(id.as_str())
                .filter(|old| old.embedding_hash == embedding_hash)
            {
                Some(old) => RawVector::Reuse(old.vector.clone()),
                None => RawVector::Parse(Vec::new()),
            };
            listed.push(RawChunk {
                id,
                document_id: row.get(1)?,
                chunk_index: row.get(2)?,
                embedding_hash,
                vector,
            });
            Ok(())
        })?;
    }
    let changed: Vec<&str> = listed
        .iter()
        .filter(|row| matches!(row.vector, RawVector::Parse(_)))
        .map(|row| row.id.as_str())
        .collect();
    let mut stored = read_embeddings(conn, &changed)?;
    let mut rows = Vec::with_capacity(listed.len());
    for mut row in listed {
        if matches!(row.vector, RawVector::Parse(_)) {
            match stored.remove(&row.id) {
                Some(raw) => row.vector = RawVector::Parse(raw),
                // Deleted between the listing and the read: skip it.
                None => continue,
            }
        }
        rows.push(row);
    }
    // Same order as a full listing (document, index, id).
    rows.sort_by(|a, b| {
        a.document_id
            .cmp(&b.document_id)
            .then(a.chunk_index.cmp(&b.chunk_index))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(RawSnapshot {
        generation,
        rows,
        base: Some((previous.clone(), changed_mask)),
    })
}

fn load_vector_documents(
    conn: &duckdb::Connection,
    filters: &LexFilters,
) -> Result<VectorDocumentScope> {
    let (where_sql, bind) = filters.scope_where_clause("d.id");
    let read_ids = |sql: &str| -> Result<HashSet<String>> {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(params_from_iter(bind.iter()))?;
        let mut ids = HashSet::new();
        crate::db::rows::visit_rows(&mut rows, |row| {
            ids.insert(row.get(0)?);
            Ok(())
        })?;
        Ok(ids)
    };
    if has_selective_vector_scope(filters) {
        return Ok(VectorDocumentScope::Only(read_ids(&format!(
            "SELECT d.id FROM documents d {where_sql}"
        ))?));
    }
    let Some(predicate) = where_sql.strip_prefix("WHERE ") else {
        return Ok(VectorDocumentScope::Excluding(HashSet::new()));
    };
    // Only status/layer/kind filters remain: list whichever side is smaller.
    // A NULL column fails the predicate, so it counts as outside the scope.
    let predicate = format!("COALESCE({predicate}, false)");
    let (matching, total): (i64, i64) = conn.query_row(
        &format!("SELECT count(*) FILTER (WHERE {predicate}), count(*) FROM documents d"),
        params_from_iter(bind.iter()),
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if matching * 2 <= total {
        return Ok(VectorDocumentScope::Only(read_ids(&format!(
            "SELECT d.id FROM documents d WHERE {predicate}"
        ))?));
    }
    Ok(VectorDocumentScope::Excluding(read_ids(&format!(
        "SELECT d.id FROM documents d WHERE NOT {predicate}"
    ))?))
}

fn normalized_vector(vector: &[f32]) -> Option<Vec<f32>> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    (norm > 0.0).then(|| vector.iter().map(|value| value / norm).collect())
}

fn hydrate_vector_hits(
    store: &Store,
    snapshot: &VectorSnapshot,
    ranked: Vec<RankedVectorCandidate>,
) -> Result<(Vec<SearchHit>, bool)> {
    // Candidate pools reach thousands during refill: read rows in a few
    // `IN (...)` batches instead of one point query per candidate. Content,
    // spans and section metadata are read only for the ranked hits.
    const CONTENT_BATCH: usize = 512;
    struct HitRow {
        content: String,
        char_start: i32,
        char_end: i32,
        metadata_json: Option<String>,
    }
    let ids: Vec<&str> = ranked
        .iter()
        .map(|candidate| &*snapshot.chunks[candidate.ordinal].id)
        .collect();
    let document_ids: Vec<&str> = ranked
        .iter()
        .map(|candidate| snapshot.document_id(&snapshot.chunks[candidate.ordinal]))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut rows: HashMap<String, HitRow> = HashMap::with_capacity(ids.len());
    let mut documents: HashMap<String, (String, String)> =
        HashMap::with_capacity(document_ids.len());
    {
        let conn = store.lock()?;
        for batch in ids.chunks(CONTENT_BATCH) {
            let placeholders = crate::db::sql::in_placeholders(batch.len());
            let mut stmt = conn.prepare(&format!(
                "SELECT id, content, char_start, char_end, metadata_json \
                 FROM chunks WHERE id IN ({placeholders})"
            ))?;
            crate::db::rows::for_each_row(&mut stmt, params_from_iter(batch.iter()), |row| {
                rows.insert(
                    row.get(0)?,
                    HitRow {
                        content: row.get(1)?,
                        char_start: row.get(2)?,
                        char_end: row.get(3)?,
                        metadata_json: row.get(4)?,
                    },
                );
                Ok(())
            })?;
        }
        for batch in document_ids.chunks(CONTENT_BATCH) {
            let placeholders = crate::db::sql::in_placeholders(batch.len());
            let mut stmt = conn.prepare(&format!(
                "SELECT id, title, uri FROM documents WHERE id IN ({placeholders})"
            ))?;
            crate::db::rows::for_each_row(&mut stmt, params_from_iter(batch.iter()), |row| {
                documents.insert(row.get(0)?, (row.get(1)?, row.get(2)?));
                Ok(())
            })?;
        }
    }
    let mut hits = Vec::with_capacity(ranked.len());
    let mut missing_document = false;
    for candidate in ranked {
        let chunk = &snapshot.chunks[candidate.ordinal];
        let document_id = snapshot.document_id(chunk);
        let Some((title, uri)) = documents.get(document_id) else {
            missing_document = true;
            continue;
        };
        // A chunk replaced after the snapshot was taken has no row left.
        let Some(row) = rows.remove(&*chunk.id) else {
            continue;
        };
        let (heading_path, section) = section_metadata(
            row.metadata_json
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or("{}"),
        );
        hits.push(SearchHit {
            chunk_id: chunk.id.to_string(),
            document_id: document_id.to_owned(),
            document_title: title.clone(),
            document_uri: uri.clone(),
            chunk_index: chunk.chunk_index,
            content: row.content,
            score: candidate.score,
            score_vec: Some(candidate.score),
            score_lex: None,
            score_rrf: None,
            score_rerank: None,
            snippet: None,
            char_start: Some(row.char_start),
            char_end: Some(row.char_end),
            heading_path,
            section,
            context: None,
            explanation: None,
            rank_vec: None,
            rank_lex: None,
        });
    }
    Ok((hits, missing_document))
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

    let mut anchor = 0usize;
    if let Some(q) = query {
        // Lowercasing can change byte lengths (`İ` → `i̇`), so offsets found in
        // the lowered text are mapped back through each char's lowered start.
        let mut lower_content = String::with_capacity(content.len());
        let mut lower_starts = Vec::new();
        for ch in content.chars() {
            lower_starts.push(lower_content.len());
            lower_content.extend(ch.to_lowercase());
        }
        for term in fts::tokenize(q) {
            if term.is_empty() {
                continue;
            }
            if let Some(byte_pos) = lower_content.find(&term) {
                anchor = lower_starts
                    .partition_point(|&start| start <= byte_pos)
                    .saturating_sub(1);
                break;
            }
        }
    }

    // Center a window of max_chars around the anchor when possible.
    let total = content.chars().count();
    let half = max_chars / 2;
    let start = anchor.saturating_sub(half);
    let end = (start + max_chars).min(total);
    let start = end.saturating_sub(max_chars);

    let byte_at = |char_index: usize| {
        content
            .char_indices()
            .nth(char_index)
            .map_or(content.len(), |(byte, _)| byte)
    };
    let mut snippet = String::with_capacity(max_chars * 4 + 6);
    if start > 0 {
        snippet.push('…');
    }
    snippet.push_str(&content[byte_at(start)..byte_at(end)]);
    if end < total {
        snippet.push('…');
    }
    if snippet.is_empty() {
        None
    } else {
        Some(snippet)
    }
}

#[cfg(test)]
mod tests;

fn chunk_parent_id(chunk: &ChunkBody) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(&chunk.metadata_json)
        .ok()
        .and_then(|v| v.get("parent_chunk_id")?.as_str().map(str::to_string))
        .filter(|v| !v.is_empty())
}
