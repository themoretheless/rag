//! Vector, lexical, and hybrid (RRF) search over stored chunks.
//!
//! - **vec**: cosine similarity over embeddings (Rust-side full scan).
//! - **lex**: DuckDB FTS BM25 when the extension is available; otherwise simple
//!   term-frequency ranking in Rust (same public API).
//! - **hybrid**: reciprocal rank fusion of lex + vec lists.
//!
//! Post-processing: `min_score` filter, document diversity collapse
//! (`max_chunks_per_document`), and citation snippets on each hit.

use std::collections::HashMap;

use tracing::{debug, warn};

use crate::embeddings::cosine_similarity;
use crate::error::{AppError, Result};
use crate::models::{Chunk, SearchContextChunk, SearchHit, SearchMode};

use super::fts::{self, LexFilters};
use super::store::Store;

/// Default RRF rank constant (Cormack / common IR practice).
pub const DEFAULT_RRF_K: f32 = 60.0;

/// Default max snippet length in Unicode characters.
pub const DEFAULT_SNIPPET_CHARS: usize = 200;

/// Extra candidates pulled from each list before fusion / collapse.
const CANDIDATE_MULTIPLIER: usize = 5;
const CANDIDATE_FLOOR: usize = 50;

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
    /// Optional token budget for packing hit content (~4 chars/token). Applied after rank/diversity.
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
            max_context_tokens: None,
            context_expansion: None,
            neighbor_chunks: 1,
            rrf_k: DEFAULT_RRF_K,
            snippet_max_chars: DEFAULT_SNIPPET_CHARS,
            fts_stemmer: "porter".into(),
        }
    }
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

    let pool = candidate_pool_size(query.top_k);
    let filters = scope_filters(query);

    // Exhaustive dispatch only: new SearchMode variants must add a run_* arm here
    // (and update SearchMode::as_str / parse / needs_*). Do not use `_`.
    let hits = match query.mode {
        SearchMode::Vec => run_vec(store, query, pool, &filters)?,
        SearchMode::Lex => run_lex(store, query, pool, &filters)?,
        SearchMode::Hybrid => match run_hybrid(store, query, pool, &filters)? {
            HybridOutcome::Fused(h) => h,
            HybridOutcome::VecOnly(h) => return finalize_hits(store, h, query),
        },
    };

    finalize_hits(store, hits, query)
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
    let mut v = search_vec(store, emb, pool, query.document_id.as_deref())?;
    v = apply_scope_filters(store, v, filters)?;
    Ok(v)
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
) -> Result<HybridOutcome> {
    let emb = query
        .query_embedding
        .as_deref()
        .ok_or_else(|| AppError::embeddings("hybrid search requires query_embedding"))?;
    let text = query
        .query_text
        .as_deref()
        .ok_or_else(|| AppError::fts("hybrid search requires query_text"))?;
    let mut vec_hits = search_vec(store, emb, pool, query.document_id.as_deref())?;
    vec_hits = apply_scope_filters(store, vec_hits, filters)?;
    let lex_hits = match fts::search_bm25_with_stemmer(
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
    Ok(HybridOutcome::Fused(fuse_rrf(
        &vec_hits,
        &lex_hits,
        query.rrf_k,
    )))
}

/// Post-process ranked hits: snippets, min_score, diversity, top_k, token pack.
fn finalize_hits(store: &Store, mut hits: Vec<SearchHit>, query: &SearchQuery) -> Result<Vec<SearchHit>> {
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
            hits = mmr_select(hits, cap, candidate_pool_size(query.top_k));
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

    attach_context(store, &mut hits, query.context_expansion, query.neighbor_chunks)?;

    if let Some(budget) = query.max_context_tokens {
        if budget > 0 {
            let packed = crate::search_pack::pack_hits(&hits, budget);
            hits = packed.hits;
        }
    }

    Ok(hits)
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
                chunks.iter().filter(|c| {
                    (c.chunk_index - current.chunk_index).abs() <= radius
                }).collect()
            }
            ContextExpansion::ParentSection => chunks.iter().filter(|c| {
                let (path, _) = chunk_section_metadata(c);
                heading_path.is_some() && path == heading_path
            }).collect(),
        };
        let selected = if selected.is_empty() { vec![current] } else { selected };
        hit.context = Some(selected.iter().map(|chunk| {
            let (heading_path, section) = chunk_section_metadata(chunk);
            SearchContextChunk {
                chunk_id: chunk.id.clone(),
                chunk_index: chunk.chunk_index,
                content: chunk.content.clone(),
                heading_path,
                section,
            }
        }).collect());
        hit.content = selected.iter().map(|c| c.content.as_str()).collect::<Vec<_>>().join("\n");
    }
    Ok(())
}

fn chunk_section_metadata(chunk: &Chunk) -> (Option<Vec<String>>, Option<String>) {
    let value = serde_json::from_str::<serde_json::Value>(&chunk.metadata_json).ok();
    let heading_path = value.as_ref()
        .and_then(|v| v.get("heading_path"))
        .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
        .filter(|v| !v.is_empty());
    let section = value.as_ref()
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

fn has_equality_scope(filters: &super::fts::LexFilters) -> bool {
    filters.wing.as_ref().map(|s| !s.is_empty()).unwrap_or(false)
        || filters.room.as_ref().map(|s| !s.is_empty()).unwrap_or(false)
        || filters.layer.as_ref().map(|s| !s.is_empty()).unwrap_or(false)
        || filters.kind.as_ref().map(|s| !s.is_empty()).unwrap_or(false)
        || filters.source_file
            .as_ref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
}

/// Filter vector hits by document wing/room/layer/source_file and archive status.
///
/// Lex path applies the same predicates via SQL joins; vec path filters post-hoc
/// so mode=`vec` keeps working when hybrid/lex is incomplete.
fn apply_scope_filters(
    store: &Store,
    hits: Vec<SearchHit>,
    filters: &super::fts::LexFilters,
) -> Result<Vec<SearchHit>> {
    // Always filter when equality scopes are set or archived rows must be excluded.
    if !has_equality_scope(filters) && filters.include_archived {
        return Ok(hits);
    }

    use crate::models::DocumentFilter;

    let docs = store.list_documents_filtered(&DocumentFilter {
        wing: filters.wing.clone().filter(|s| !s.is_empty()),
        room: filters.room.clone().filter(|s| !s.is_empty()),
        layer: filters.layer.clone().filter(|s| !s.is_empty()),
        kind: filters.kind.clone().filter(|s| !s.is_empty()),
        source_file: filters.source_file.clone().filter(|s| !s.is_empty()),
        include_archived: Some(filters.include_archived),
        ..Default::default()
    })?;

    let allowed: std::collections::HashSet<String> = docs.into_iter().map(|d| d.id).collect();
    Ok(hits
        .into_iter()
        .filter(|h| allowed.contains(&h.document_id))
        .collect())
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

fn candidate_pool_size(top_k: usize) -> usize {
    top_k.saturating_mul(CANDIDATE_MULTIPLIER).max(CANDIDATE_FLOOR)
}

// ---------------------------------------------------------------------------
// Vector search
// ---------------------------------------------------------------------------

fn search_vec(
    store: &Store,
    query_embedding: &[f32],
    top_k: usize,
    document_id: Option<&str>,
) -> Result<Vec<SearchHit>> {
    if top_k == 0 {
        return Ok(Vec::new());
    }

    let chunks = load_chunks(store, document_id)?;
    if chunks.is_empty() {
        return Ok(Vec::new());
    }

    let mut scored: Vec<(f32, usize)> = chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| (cosine_similarity(query_embedding, &chunk.embedding), i))
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
            score_vec: Some(score),
            score_lex: None,
            score_rrf: None,
            snippet: None,
            char_start: Some(chunk.char_start),
            char_end: Some(chunk.char_end),
            heading_path: chunk_section_metadata(chunk).0,
            section: chunk_section_metadata(chunk).1,
            context: None,
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

    let hits = fts::search_bm25_with_stemmer(
        store,
        query_text,
        top_k,
        filters,
        Some(fts_stemmer),
    )?;
    debug!(count = hits.len(), "lex search via db::fts (BM25 or TF fallback)");
    Ok(hits)
}

/// Allow only safe stemmer identifiers (unit tests).
#[cfg(test)]
fn sanitize_stemmer(raw: &str) -> String {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return "porter".into();
    }
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        s
    } else {
        "porter".into()
    }
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

fn accumulate_rrf_list(
    fused: &mut HashMap<String, (f32, SearchHit)>,
    hits: &[SearchHit],
    k: f32,
) {
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
        snippet.insert_str(0, "…");
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

fn load_chunks(store: &Store, document_id: Option<&str>) -> Result<Vec<Chunk>> {
    match document_id {
        Some(doc_id) => store.list_chunks_for_document(doc_id),
        None => store.all_chunks_with_embeddings(),
    }
}

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
mod tests {
    use super::*;
    use crate::models::Chunk;
    use chrono::Utc;
    use crate::models::Document;

    fn sample_hit(chunk_id: &str, doc_id: &str, score: f32) -> SearchHit {
        SearchHit {
            chunk_id: chunk_id.into(),
            document_id: doc_id.into(),
            document_title: format!("title-{doc_id}"),
            document_uri: format!("uri://{doc_id}"),
            chunk_index: 0,
            content: format!("content of {chunk_id}"),
            score,
            score_vec: Some(score),
            score_lex: None,
            score_rrf: None,
            snippet: None,
            char_start: Some(0),
            char_end: Some(10),
            heading_path: None,
            section: None,
            context: None,
        }
    }

    fn hit_from_chunk(chunk: &Chunk, score: f32, title: &str, uri: &str) -> SearchHit {
        SearchHit {
            chunk_id: chunk.id.clone(),
            document_id: chunk.document_id.clone(),
            document_title: title.to_string(),
            document_uri: uri.to_string(),
            chunk_index: chunk.chunk_index,
            content: chunk.content.clone(),
            score,
            score_vec: Some(score),
            char_start: Some(chunk.char_start),
            char_end: Some(chunk.char_end),
            ..Default::default()
        }
    }

    #[test]
    fn ranking_order_matches_cosine() {
        let q = [1.0f32, 0.0, 0.0];
        let a = [1.0f32, 0.0, 0.0];
        let b = [0.0f32, 1.0, 0.0];
        let c = [-1.0f32, 0.0, 0.0];

        let mut scores = vec![
            (cosine_similarity(&q, &b), "b"),
            (cosine_similarity(&q, &c), "c"),
            (cosine_similarity(&q, &a), "a"),
        ];
        scores.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap_or(std::cmp::Ordering::Equal));

        assert_eq!(scores[0].1, "a");
        assert_eq!(scores[1].1, "b");
        assert_eq!(scores[2].1, "c");
        assert!((scores[0].0 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn search_hit_fields_populated() {
        let chunk = Chunk {
            id: "c1".into(),
            document_id: "d1".into(),
            chunk_index: 2,
            content: "hello".into(),
            embedding: vec![1.0, 0.0],
            char_start: 0,
            char_end: 5,
            metadata_json: "{}".into(),
        };
        let hit = hit_from_chunk(&chunk, 0.9, "Title", "uri://x");
        assert_eq!(hit.chunk_id, "c1");
        assert_eq!(hit.document_title, "Title");
        assert_eq!(hit.chunk_index, 2);
        assert!((hit.score - 0.9).abs() < 1e-6);
    }

    #[test]
    fn rrf_prefers_items_high_in_both_lists() {
        // a ranks 1 in both lists → highest RRF
        // b ranks 1 vec / 3 lex
        // c ranks 2 vec / 2 lex
        let vec_list = vec![
            sample_hit("a", "d1", 0.9),
            sample_hit("c", "d2", 0.8),
            sample_hit("b", "d3", 0.7),
        ];
        let mut lex_a = sample_hit("a", "d1", 5.0);
        lex_a.score_vec = None;
        lex_a.score_lex = Some(5.0);
        let mut lex_c = sample_hit("c", "d2", 4.0);
        lex_c.score_vec = None;
        lex_c.score_lex = Some(4.0);
        let mut lex_b = sample_hit("b", "d3", 3.0);
        lex_b.score_vec = None;
        lex_b.score_lex = Some(3.0);
        let lex_list = vec![lex_a, lex_c, lex_b];

        let fused = fuse_rrf(&vec_list, &lex_list, 60.0);
        assert_eq!(fused[0].chunk_id, "a");
        assert!(fused[0].score_rrf.is_some());
        assert!(fused[0].score_vec.is_some());
        assert!(fused[0].score_lex.is_some());
        // RRF(a) = 1/(60+1) + 1/(60+1)
        let expected_a = 2.0 / 61.0;
        assert!((fused[0].score_rrf.unwrap() - expected_a).abs() < 1e-6);
        assert!((fused[0].score - expected_a).abs() < 1e-6);
    }

    #[test]
    fn rrf_includes_items_present_in_only_one_list() {
        let vec_list = vec![sample_hit("only_vec", "d1", 0.99)];
        let mut lex = sample_hit("only_lex", "d2", 10.0);
        lex.score_vec = None;
        lex.score_lex = Some(10.0);
        let lex_list = vec![lex];

        let fused = fuse_rrf(&vec_list, &lex_list, 60.0);
        assert_eq!(fused.len(), 2);
        let ids: Vec<&str> = fused.iter().map(|h| h.chunk_id.as_str()).collect();
        assert!(ids.contains(&"only_vec"));
        assert!(ids.contains(&"only_lex"));
    }

    #[test]
    fn collapse_limits_per_document() {
        let hits = vec![
            sample_hit("c1", "d1", 1.0),
            sample_hit("c2", "d1", 0.9),
            sample_hit("c3", "d1", 0.8),
            sample_hit("c4", "d2", 0.7),
            sample_hit("c5", "d2", 0.6),
        ];
        let out = collapse_by_document(hits, 2);
        assert_eq!(out.len(), 4);
        assert_eq!(out.iter().filter(|h| h.document_id == "d1").count(), 2);
        assert_eq!(out.iter().filter(|h| h.document_id == "d2").count(), 2);
        // Order preserved for kept items
        assert_eq!(out[0].chunk_id, "c1");
        assert_eq!(out[1].chunk_id, "c2");
        assert_eq!(out[2].chunk_id, "c4");
    }

    #[test]
    fn min_score_filters() {
        let hits = vec![
            sample_hit("a", "d1", 0.9),
            sample_hit("b", "d1", 0.4),
            sample_hit("c", "d1", 0.1),
        ];
        let out = apply_min_score(hits, 0.5);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].chunk_id, "a");
    }

    #[test]
    fn snippet_around_query_term() {
        let content = "alpha beta gamma delta epsilon zeta eta theta";
        let snip = make_snippet(content, Some("gamma"), 20).expect("snippet");
        assert!(snip.to_lowercase().contains("gamma"));
        // Should not be longer than max_chars + ellipsis markers.
        assert!(snip.chars().count() <= 20 + 2);
    }

    #[test]
    fn snippet_prefix_without_query() {
        let content = "hello world this is a long piece of text for testing";
        let snip = make_snippet(content, None, 11).expect("snippet");
        assert!(snip.starts_with("hello world") || snip.starts_with("hello"));
    }

    #[test]
    fn tokenize_and_tf_score() {
        let terms = tokenize("Hello, WORLD! hello");
        assert_eq!(terms, vec!["hello", "world", "hello"]);
        let q = tokenize("hello world");
        let s = tf_score(&q, "hello hello world foo");
        assert!((s - 3.0).abs() < 1e-6); // hello x2 + world x1
    }

    #[test]
    fn sanitize_stemmer_rejects_injection() {
        assert_eq!(sanitize_stemmer("porter"), "porter");
        assert_eq!(sanitize_stemmer("none"), "none");
        assert_eq!(sanitize_stemmer("  German "), "german");
        assert_eq!(sanitize_stemmer("x'; DROP TABLE"), "porter");
    }

    #[test]
    fn search_lex_tf_and_hybrid_with_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("search_test.duckdb");
        let store = Store::open(&path).expect("open");

        let now = Utc::now();
        let doc1 = Document {
            id: "d1".into(),
            uri: "uri://d1".into(),
            title: "Cats".into(),
            content: "cats and kittens play".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        let doc2 = Document {
            id: "d2".into(),
            uri: "uri://d2".into(),
            title: "Dogs".into(),
            content: "dogs and puppies run".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        store.upsert_document(&doc1).unwrap();
        store.upsert_document(&doc2).unwrap();

        let chunks = vec![
            Chunk {
                id: "c1".into(),
                document_id: "d1".into(),
                chunk_index: 0,
                content: "unique xylophone cats meow loudly".into(),
                embedding: vec![1.0, 0.0, 0.0],
                char_start: 0,
                char_end: 32,
                metadata_json: "{}".into(),
            },
            Chunk {
                id: "c2".into(),
                document_id: "d1".into(),
                chunk_index: 1,
                content: "more cats again cats cats".into(),
                embedding: vec![0.9, 0.1, 0.0],
                char_start: 33,
                char_end: 58,
                metadata_json: "{}".into(),
            },
            Chunk {
                id: "c3".into(),
                document_id: "d2".into(),
                chunk_index: 0,
                content: "dogs bark at the moon".into(),
                embedding: vec![0.0, 1.0, 0.0],
                char_start: 0,
                char_end: 21,
                metadata_json: "{}".into(),
            },
        ];
        store.insert_chunks(&chunks).unwrap();

        // Pure TF path (bypass FTS by calling search_lex_tf).
        let lex = search_lex_tf(&store, "cats xylophone", 10, None).unwrap();
        assert!(!lex.is_empty());
        assert!(lex[0].score_lex.is_some());
        // c1 has both cats + xylophone; c2 has many cats only
        assert!(lex.iter().any(|h| h.chunk_id == "c1"));

        // Vec mode via public search.
        let vec_hits = search(
            &store,
            &SearchQuery {
                mode: SearchMode::Vec,
                top_k: 5,
                query_embedding: Some(vec![1.0, 0.0, 0.0]),
                ..SearchQuery::default()
            },
        )
        .unwrap();
        assert!(!vec_hits.is_empty());
        assert!(vec_hits[0].score_vec.is_some());
        assert_eq!(vec_hits[0].chunk_id, "c1");

        // Hybrid: needs both embedding and text.
        let hybrid = search(
            &store,
            &SearchQuery {
                mode: SearchMode::Hybrid,
                top_k: 5,
                query_text: Some("cats xylophone".into()),
                query_embedding: Some(vec![1.0, 0.0, 0.0]),
                max_chunks_per_document: Some(1),
                min_score: None,
                ..SearchQuery::default()
            },
        )
        .unwrap();
        assert!(!hybrid.is_empty());
        assert!(hybrid[0].score_rrf.is_some());
        // Diversity: at most one hit per document
        let mut seen = std::collections::HashSet::new();
        for h in &hybrid {
            assert!(seen.insert(h.document_id.clone()));
            assert!(h.snippet.is_some());
        }

        // min_score filters high threshold on vec scores (~1.0 max)
        let filtered = search(
            &store,
            &SearchQuery {
                mode: SearchMode::Vec,
                top_k: 5,
                query_embedding: Some(vec![1.0, 0.0, 0.0]),
                min_score: Some(0.99),
                ..SearchQuery::default()
            },
        )
        .unwrap();
        for h in &filtered {
            assert!(h.score >= 0.99);
        }

        // Backward-compatible search_chunks still works.
        let legacy = search_chunks(&store, &[1.0, 0.0, 0.0], 3, None).unwrap();
        assert!(!legacy.is_empty());
        assert!(legacy[0].score_vec.is_some());
    }

    #[test]
    fn search_vec_respects_wing_room_source_and_archived() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("search_scope.duckdb");
        let store = Store::open(&path).expect("open");
        let now = Utc::now();

        let active = Document {
            id: "active".into(),
            uri: "uri://active".into(),
            title: "Active".into(),
            content: "scoped active body".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            wing: Some("research".into()),
            room: Some("rag".into()),
            source_file: Some("/vault/active.md".into()),
            status: "active".into(),
            ..Default::default()
        };
        let other_wing = Document {
            id: "other".into(),
            uri: "uri://other".into(),
            title: "Other".into(),
            content: "other wing body".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            wing: Some("ops".into()),
            room: Some("runbooks".into()),
            source_file: Some("/vault/other.md".into()),
            status: "active".into(),
            ..Default::default()
        };
        let archived = Document {
            id: "arch".into(),
            uri: "uri://arch".into(),
            title: "Archived".into(),
            content: "archived body".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            wing: Some("research".into()),
            room: Some("rag".into()),
            source_file: Some("/vault/arch.md".into()),
            status: "archived".into(),
            ..Default::default()
        };
        store.upsert_document(&active).unwrap();
        store.upsert_document(&other_wing).unwrap();
        store.upsert_document(&archived).unwrap();

        let emb = vec![1.0f32, 0.0, 0.0];
        store
            .insert_chunks(&[
                Chunk {
                    id: "c-active".into(),
                    document_id: "active".into(),
                    chunk_index: 0,
                    content: "scoped active body".into(),
                    embedding: emb.clone(),
                    char_start: 0,
                    char_end: 18,
                    metadata_json: "{}".into(),
                },
                Chunk {
                    id: "c-other".into(),
                    document_id: "other".into(),
                    chunk_index: 0,
                    content: "other wing body".into(),
                    embedding: emb.clone(),
                    char_start: 0,
                    char_end: 15,
                    metadata_json: "{}".into(),
                },
                Chunk {
                    id: "c-arch".into(),
                    document_id: "arch".into(),
                    chunk_index: 0,
                    content: "archived body".into(),
                    embedding: emb.clone(),
                    char_start: 0,
                    char_end: 13,
                    metadata_json: "{}".into(),
                },
            ])
            .unwrap();

        // Default: exclude archived; no wing filter → active + other.
        let open = search(
            &store,
            &SearchQuery {
                mode: SearchMode::Vec,
                top_k: 10,
                query_embedding: Some(emb.clone()),
                ..SearchQuery::default()
            },
        )
        .unwrap();
        let ids: Vec<_> = open.iter().map(|h| h.document_id.as_str()).collect();
        assert!(ids.contains(&"active"));
        assert!(ids.contains(&"other"));
        assert!(!ids.contains(&"arch"));

        // Wing filter.
        let wing = search(
            &store,
            &SearchQuery {
                mode: SearchMode::Vec,
                top_k: 10,
                query_embedding: Some(emb.clone()),
                wing: Some("research".into()),
                ..SearchQuery::default()
            },
        )
        .unwrap();
        assert_eq!(wing.len(), 1);
        assert_eq!(wing[0].document_id, "active");

        // source_file filter.
        let by_src = search(
            &store,
            &SearchQuery {
                mode: SearchMode::Vec,
                top_k: 10,
                query_embedding: Some(emb.clone()),
                source_file: Some("/vault/other.md".into()),
                ..SearchQuery::default()
            },
        )
        .unwrap();
        assert_eq!(by_src.len(), 1);
        assert_eq!(by_src[0].document_id, "other");

        // include_archived + wing.
        let with_arch = search(
            &store,
            &SearchQuery {
                mode: SearchMode::Vec,
                top_k: 10,
                query_embedding: Some(emb),
                wing: Some("research".into()),
                room: Some("rag".into()),
                include_archived: true,
                ..SearchQuery::default()
            },
        )
        .unwrap();
        let arch_ids: std::collections::HashSet<_> =
            with_arch.iter().map(|h| h.document_id.as_str()).collect();
        assert!(arch_ids.contains("active"));
        assert!(arch_ids.contains("arch"));
        assert!(!arch_ids.contains("other"));
    }
}
