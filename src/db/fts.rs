//! DuckDB FTS / BM25 index management for `chunks.content`.
//!
//! # Preferred path: DuckDB `fts` extension
//!
//! 1. `INSTALL fts;` then `LOAD fts;` (INSTALL is best-effort; LOAD is required)
//! 2. `PRAGMA create_fts_index('chunks', 'id', 'content', stemmer := …, overwrite := 1)`
//! 3. Rank with `fts_main_chunks.match_bm25(id, query)`
//!
//! The FTS inverted index does **not** update when indexed chunk rows change.
//! Text/row mutations call [`mark_fts_dirty`], and [`refresh_fts_if_stale`] owns
//! the one serialized generation refresh. Embedding-only updates instead
//! advance the chunk and clean index generations together: this invalidates
//! vector snapshots without rebuilding unchanged BM25 text. Guarded corpus-wide
//! workflows (source sync, doctor repair, duplicate cleanup, delete-by-source,
//! recovery import, and maintenance apply/refresh/compress) eagerly finalize a
//! stale index before normal terminal success. Ordinary single-document writes
//! and interrupted or failed workflows retain the next lexical search as their
//! single-flight consistency fallback. Clean, warm searches do not execute FTS DDL.
//! [`reindex`] remains the explicit force-rebuild API.
//!
//! # Fallback: term-frequency in Rust
//!
//! When `INSTALL`/`LOAD` of the `fts` extension fails (offline CI, locked-down
//! hosts, extension repo unavailable), the same public API degrades to a simple
//! length-normalized term-frequency scorer over live `chunks` rows. Lex search
//! still works; scores are not true Okapi BM25. [`FtsBackend::TermFrequency`]
//! and `meta.fts_backend` surface this honestly for `doctor` / `status`.

use std::collections::HashMap;

use duckdb::{params, params_from_iter, Connection};

use super::store::Store;
use crate::error::{AppError, Result};
use crate::models::SearchHit;

/// DuckDB schema created by `PRAGMA create_fts_index` on table `chunks`.
pub const FTS_SCHEMA: &str = "fts_main_chunks";

/// Indexed base table.
pub const FTS_TABLE: &str = "chunks";

const META_BACKEND: &str = "fts_backend";
const META_STEMMER: &str = "fts_stemmer";
const META_CHUNKS_GENERATION: &str = "fts_chunks_generation";
const META_INDEX_GENERATION: &str = "fts_index_generation";
const META_REBUILD_COUNT: &str = "fts_rebuild_count";
const DEFAULT_STEMMER: &str = "porter";
const SNIPPET_CHARS: usize = 240;

/// Which lexical backend is active after [`ensure_fts`] / [`reindex`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtsBackend {
    /// DuckDB `fts` extension + `match_bm25`.
    DuckDbBm25,
    /// Rust term-frequency fallback (extension unavailable).
    TermFrequency,
}

impl FtsBackend {
    /// Stable wire / meta value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DuckDbBm25 => "duckdb_bm25",
            Self::TermFrequency => "term_frequency",
        }
    }

    fn from_meta(s: &str) -> Option<Self> {
        match s.trim() {
            "duckdb_bm25" | "duckdb" | "fts" | "bm25" => Some(Self::DuckDbBm25),
            "term_frequency" | "tf" | "fallback" => Some(Self::TermFrequency),
            _ => None,
        }
    }
}

/// Snapshot of FTS configuration after ensure/reindex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtsState {
    pub backend: FtsBackend,
    pub stemmer: String,
}

/// Generation snapshot for the derived lexical index.
///
/// `chunks_generation` advances before a mutation can change `chunks`.
/// `index_generation` advances only after a successful BM25 rebuild (or after
/// selecting the live-row term-frequency fallback), except that an
/// embedding-only transaction may advance both because indexed text is
/// unchanged. A mismatch is therefore a crash-safe dirty marker: false
/// positives can cause one extra rebuild, while a completed text mutation
/// cannot be hidden behind a clean generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtsGenerationState {
    pub chunks_generation: u64,
    pub index_generation: Option<u64>,
    pub dirty: bool,
    /// Successful refresh/reindex attempts. Primarily useful for diagnostics
    /// and for proving that clean searches stay on the read-only fast path.
    pub rebuild_count: u64,
}

/// Optional equality filters for lexical search (joined via `documents`).
///
/// Empty / `None` fields are ignored. Used by hybrid / lex search tools.
/// When `include_archived` is false (default), rows with `status` of
/// `archived` or `tombstone` are excluded.
#[derive(Debug, Clone, Default)]
pub struct LexFilters {
    pub document_id: Option<String>,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub layer: Option<String>,
    pub kind: Option<String>,
    pub uri: Option<String>,
    pub source_file: Option<String>,
    /// When false (default), exclude archived / tombstone documents.
    pub include_archived: bool,
}

impl LexFilters {
    /// Convenience: filter to a single document id.
    pub fn document_id(id: impl Into<String>) -> Self {
        Self {
            document_id: Some(id.into()),
            ..Default::default()
        }
    }
}

/// Ensure the lexical search path is ready on `chunks`.
///
/// Tries DuckDB FTS with stemmer `"porter"`. On extension failure, records the
/// term-frequency fallback and returns successfully so callers never hard-fail
/// solely because FTS could not be installed. Once initialized, this is a
/// no-op while the recorded generations and requested configuration are clean.
pub fn ensure_fts(conn: &Connection) -> Result<FtsState> {
    ensure_fts_with_stemmer(conn, DEFAULT_STEMMER)
}

/// Like [`ensure_fts`], but uses `stemmer` (e.g. from `RAG_FTS_STEMMER`).
///
/// Pass `"none"` to disable stemming (better for CJK / code). When stemmer is
/// `"none"`, stopwords are also set to `"none"`.
pub fn ensure_fts_with_stemmer(conn: &Connection, stemmer: &str) -> Result<FtsState> {
    refresh_fts_if_stale_with_stemmer(conn, stemmer)
}

/// Mark the derived FTS index stale before mutating `chunks`.
///
/// Store mutations are serialized by [`Store`]'s connection mutex, so the
/// read/increment/write sequence is single-writer. Call this before the first
/// statement that may change chunks: a failed mutation can leave a harmless
/// dirty marker, whereas marking afterwards could expose stale results if the
/// marker write itself failed.
pub fn mark_fts_dirty(conn: &Connection) -> Result<u64> {
    let current = chunks_generation(conn)?;
    let next = current
        .checked_add(1)
        .ok_or_else(|| AppError::fts("FTS chunks generation overflow"))?;
    upsert_meta(conn, META_CHUNKS_GENERATION, &next.to_string())?;
    Ok(next)
}

/// Advance the chunk generation after changing only `embedding_json` values.
///
/// Vector snapshots key off the chunk generation, so it must advance even
/// though the indexed text rows are unchanged. When the lexical index was
/// clean before the mutation, advancing its generation in the same transaction
/// preserves that clean state without rebuilding BM25. An already-dirty or
/// unconfigured index stays dirty. Callers must invoke this on the same
/// transaction that owns the embedding updates so data and both generations
/// commit or roll back together.
pub(crate) fn mark_embeddings_changed(conn: &Connection) -> Result<u64> {
    let fts_was_clean = !fts_generation_state(conn)?.dirty;
    let next = mark_fts_dirty(conn)?;
    if fts_was_clean {
        upsert_meta(conn, META_INDEX_GENERATION, &next.to_string())?;
    }
    Ok(next)
}

/// Read only the source chunk generation (one `meta` lookup, no FTS probe).
///
/// This is the hot-path invalidation token for caches derived from `chunks`.
/// Missing legacy/corrupt metadata reads as generation zero; the next mutation
/// advances it through [`mark_fts_dirty`].
pub fn chunks_generation(conn: &Connection) -> Result<u64> {
    Ok(read_u64_meta(conn, META_CHUNKS_GENERATION)?.unwrap_or(0))
}

/// Read the current dirty/generation state without refreshing the index.
pub fn fts_generation_state(conn: &Connection) -> Result<FtsGenerationState> {
    let recorded_chunks_generation = read_u64_meta(conn, META_CHUNKS_GENERATION)?;
    let chunks_generation = recorded_chunks_generation.unwrap_or(0);
    let index_generation = read_u64_meta(conn, META_INDEX_GENERATION)?;
    let rebuild_count = read_u64_meta(conn, META_REBUILD_COUNT)?.unwrap_or(0);
    let configured = fts_status(conn)?;
    let physical_index_missing = configured
        .as_ref()
        .is_some_and(|state| state.backend == FtsBackend::DuckDbBm25 && !fts_index_present(conn));
    let dirty = recorded_chunks_generation.is_none()
        || configured.is_none()
        || index_generation != Some(chunks_generation)
        || physical_index_missing;

    Ok(FtsGenerationState {
        chunks_generation,
        index_generation,
        dirty,
        rebuild_count,
    })
}

/// Refresh the derived index once when chunk generations are stale.
///
/// The stored stemmer is reused, defaulting to `porter`. Search calls this
/// while holding the store connection mutex, which makes concurrent stale
/// searches a singleflight: the first rebuilds and advances the generation;
/// waiters observe a clean generation and skip DDL.
pub fn refresh_fts_if_stale(conn: &Connection) -> Result<FtsState> {
    let stemmer = read_meta(conn, META_STEMMER)?.unwrap_or_else(|| DEFAULT_STEMMER.to_string());
    refresh_fts_if_stale_with_stemmer(conn, &stemmer)
}

/// Like [`refresh_fts_if_stale`], with an explicit requested stemmer.
pub fn refresh_fts_if_stale_with_stemmer(conn: &Connection, stemmer: &str) -> Result<FtsState> {
    refresh_fts(conn, stemmer, RefreshMode::IfStale)
}

/// Force-rebuild the FTS index after ingest/delete (read-your-writes).
///
/// For [`FtsBackend::TermFrequency`] this only refreshes meta (scoring always
/// reads live rows). Uses the stemmer last stored in `meta`, defaulting to
/// `"porter"`.
pub fn reindex(conn: &Connection) -> Result<FtsState> {
    let stemmer = read_meta(conn, META_STEMMER)?.unwrap_or_else(|| DEFAULT_STEMMER.to_string());
    refresh_fts(conn, &stemmer, RefreshMode::Force)
}

/// Rebuild with an explicit stemmer (updates `meta.fts_stemmer`).
pub fn reindex_with_stemmer(conn: &Connection, stemmer: &str) -> Result<FtsState> {
    refresh_fts(conn, stemmer, RefreshMode::Force)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshMode {
    IfStale,
    Force,
}

fn refresh_fts(conn: &Connection, stemmer: &str, mode: RefreshMode) -> Result<FtsState> {
    let stemmer = sanitize_stemmer(stemmer);
    let current = fts_status(conn)?;
    let generations = fts_generation_state(conn)?;
    let configuration_changed = current
        .as_ref()
        .is_none_or(|state| state.stemmer != stemmer);

    if mode == RefreshMode::IfStale && !configuration_changed {
        if !generations.dirty {
            return current.ok_or_else(|| {
                AppError::fts("FTS generation is clean but backend metadata is missing")
            });
        }
        if let Some(state) = current.filter(|state| state.backend == FtsBackend::TermFrequency) {
            // TF scores live chunks directly. A chunk mutation only needs to
            // advance the logical generation; retry extension installation
            // only on an explicit reindex or configuration change.
            publish_refreshed_state(conn, &state, generations.chunks_generation)?;
            return Ok(state);
        }
    }

    rebuild_fts(conn, stemmer, generations.chunks_generation)
}

fn rebuild_fts(conn: &Connection, stemmer: String, chunks_generation: u64) -> Result<FtsState> {
    let state = match try_duckdb_fts_index(conn, &stemmer) {
        Ok(()) => {
            let state = FtsState {
                backend: FtsBackend::DuckDbBm25,
                stemmer,
            };
            tracing::info!(
                backend = state.backend.as_str(),
                stemmer = %state.stemmer,
                chunks_generation,
                "FTS index refreshed on chunks (DuckDB BM25)"
            );
            state
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                stemmer = %stemmer,
                chunks_generation,
                "DuckDB FTS extension unavailable; lex search uses term-frequency fallback"
            );
            FtsState {
                backend: FtsBackend::TermFrequency,
                stemmer,
            }
        }
    };

    publish_refreshed_state(conn, &state, chunks_generation)?;
    Ok(state)
}

fn publish_refreshed_state(
    conn: &Connection,
    state: &FtsState,
    chunks_generation: u64,
) -> Result<()> {
    // Publish the generation only after the derived search backend is ready.
    // If the process fails between these writes, the missing/mismatched
    // generation remains dirty and the next refresh safely retries.
    persist_state(conn, state)?;
    upsert_meta(conn, META_CHUNKS_GENERATION, &chunks_generation.to_string())?;
    upsert_meta(conn, META_INDEX_GENERATION, &chunks_generation.to_string())?;
    increment_rebuild_count(conn)?;
    Ok(())
}

/// Load last-known FTS state from `meta`, if [`ensure_fts`] has run.
pub fn fts_status(conn: &Connection) -> Result<Option<FtsState>> {
    let backend = match read_meta(conn, META_BACKEND)? {
        Some(b) => FtsBackend::from_meta(&b),
        None => None,
    };
    let Some(backend) = backend else {
        return Ok(None);
    };
    let stemmer = read_meta(conn, META_STEMMER)?.unwrap_or_else(|| DEFAULT_STEMMER.to_string());
    Ok(Some(FtsState { backend, stemmer }))
}

/// True when the DuckDB FTS index for `chunks` is present.
///
/// Checks schema `fts_main_chunks`, then tables, then a direct relation probe.
/// Useful for `doctor` / `status`. False under term-frequency fallback.
pub fn fts_index_present(conn: &Connection) -> bool {
    if conn
        .query_row(
            r#"
            SELECT COUNT(*) FROM information_schema.schemata
            WHERE schema_name = ?
            "#,
            params![FTS_SCHEMA],
            |row| row.get::<_, i64>(0),
        )
        .map(|n: i64| n > 0)
        .unwrap_or(false)
    {
        return true;
    }
    if conn
        .query_row(
            r#"
            SELECT COUNT(*) FROM information_schema.tables
            WHERE lower(table_name) = 'fts_main_chunks'
               OR lower(table_schema) = 'fts_main_chunks'
            "#,
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|n: i64| n > 0)
        .unwrap_or(false)
    {
        return true;
    }
    conn.prepare("SELECT 1 FROM fts_main_chunks LIMIT 0")
        .is_ok()
}

/// Best-effort: extension loadable **and** index present; this probe lives next
/// to index management so callers do not need schema internals.
pub fn probe_ready(conn: &Connection) -> bool {
    if !load_fts_extension(conn) {
        return false;
    }
    fts_index_present(conn)
}

/// Try to LOAD (and INSTALL if needed) the DuckDB `fts` extension.
pub fn load_fts_extension(conn: &Connection) -> bool {
    if conn.execute_batch("LOAD fts;").is_ok() {
        return true;
    }
    if conn.execute_batch("INSTALL fts;\nLOAD fts;").is_ok() {
        return true;
    }
    if conn.execute_batch("INSTALL 'fts';\nLOAD 'fts';").is_ok() {
        return true;
    }
    false
}

/// Lexical search over chunks: BM25 when the extension is active, else TF.
///
/// Ensures a backend is configured and refreshes it only when a chunk mutation
/// advanced the stored generation. The store mutex serializes this check and
/// refresh, preserving read-your-writes without rebuilding on warm searches.
///
/// Returns hits sorted by lexical score descending, capped at `top_k`.
/// Each hit sets `score` and `score_lex` to the lexical score; `snippet` is a
/// short content prefix.
pub fn search_bm25(
    store: &Store,
    query: &str,
    top_k: usize,
    filters: &LexFilters,
) -> Result<Vec<SearchHit>> {
    search_bm25_with_stemmer(store, query, top_k, filters, None)
}

/// Like [`search_bm25`], but uses `stemmer` when (re)building the DuckDB index
/// (e.g. `RAG_FTS_STEMMER`). `None` keeps the last meta stemmer / default.
pub fn search_bm25_with_stemmer(
    store: &Store,
    query: &str,
    top_k: usize,
    filters: &LexFilters,
    stemmer: Option<&str>,
) -> Result<Vec<SearchHit>> {
    if top_k == 0 || query.trim().is_empty() {
        return Ok(Vec::new());
    }

    let conn = store.lock()?;
    let preferred_stemmer = stemmer
        .map(sanitize_stemmer)
        .or_else(|| read_meta(&conn, META_STEMMER).ok().flatten())
        .unwrap_or_else(|| DEFAULT_STEMMER.to_string());

    // The Store connection mutex covers the dirty check plus optional rebuild.
    // Concurrent stale searches therefore collapse into one refresh.
    let state = refresh_fts_if_stale_with_stemmer(&conn, &preferred_stemmer)?;

    match state.backend {
        FtsBackend::DuckDbBm25 => match search_duckdb_bm25(&conn, query, top_k, filters) {
            Ok(hits) => Ok(hits),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "match_bm25 failed; falling back to term-frequency for this query"
                );
                search_tf(&conn, query, top_k, filters)
            }
        },
        FtsBackend::TermFrequency => search_tf(&conn, query, top_k, filters),
    }
}

// ---------------------------------------------------------------------------
// DuckDB FTS path
// ---------------------------------------------------------------------------

fn try_duckdb_fts_index(conn: &Connection, stemmer: &str) -> Result<()> {
    // INSTALL may fail when already installed, offline, or extension repo is
    // unreachable. LOAD is what enables the PRAGMA macros.
    if !load_fts_extension(conn) {
        return Err(AppError::fts(
            "LOAD fts failed (INSTALL/LOAD unavailable on this connection)",
        ));
    }

    // `overwrite = 1` makes ensure/reindex idempotent. Index id = chunks.id,
    // text field = content only (portable; title/heading can be added later).
    let stopwords = if stemmer == "none" { "none" } else { "english" };
    let pragma = format!(
        "PRAGMA create_fts_index(\
            'chunks', 'id', 'content', \
            stemmer = '{stemmer}', \
            stopwords = '{stopwords}', \
            overwrite = 1\
        );"
    );
    conn.execute_batch(&pragma)
        .map_err(|e| AppError::fts(format!("create_fts_index on chunks failed: {e}")))?;
    Ok(())
}

fn search_duckdb_bm25(
    conn: &Connection,
    query: &str,
    top_k: usize,
    filters: &LexFilters,
) -> Result<Vec<SearchHit>> {
    let (where_sql, mut bind) = filter_where_clause(filters);
    // query first for match_bm25, then filter binds. `top_k` is inlined (usize).
    let mut all_binds: Vec<String> = Vec::with_capacity(bind.len() + 1);
    all_binds.push(query.to_string());
    all_binds.append(&mut bind);

    let sql = format!(
        r#"
        SELECT
          chunk_id,
          document_id,
          document_title,
          document_uri,
          chunk_index,
          content,
          char_start,
          char_end,
          score
        FROM (
          SELECT
            c.id AS chunk_id,
            c.document_id AS document_id,
            d.title AS document_title,
            d.uri AS document_uri,
            c.chunk_index AS chunk_index,
            c.content AS content,
            c.char_start AS char_start,
            c.char_end AS char_end,
            fts_main_chunks.match_bm25(c.id, ?) AS score
          FROM chunks c
          INNER JOIN documents d ON d.id = c.document_id
          {where_sql}
        ) ranked
        WHERE score IS NOT NULL
        ORDER BY score DESC
        LIMIT {top_k}
        "#
    );

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::fts(format!("prepare match_bm25: {e}")))?;

    let mut rows = stmt
        .query(params_from_iter(all_binds.iter()))
        .map_err(|e| AppError::fts(format!("query match_bm25: {e}")))?;

    let mut hits = Vec::new();
    while let Some(row) = rows.next()? {
        let content: String = row.get(5)?;
        let score: f64 = row.get(8)?;
        let score = score as f32;
        let char_start: i32 = row.get(6)?;
        let char_end: i32 = row.get(7)?;
        hits.push(SearchHit {
            chunk_id: row.get(0)?,
            document_id: row.get(1)?,
            document_title: row.get(2)?,
            document_uri: row.get(3)?,
            chunk_index: row.get(4)?,
            content: content.clone(),
            score,
            score_lex: Some(score),
            snippet: Some(make_snippet(&content, SNIPPET_CHARS)),
            char_start: Some(char_start),
            char_end: Some(char_end),
            ..Default::default()
        });
    }
    Ok(hits)
}

// ---------------------------------------------------------------------------
// Term-frequency fallback
// ---------------------------------------------------------------------------

fn search_tf(
    conn: &Connection,
    query: &str,
    top_k: usize,
    filters: &LexFilters,
) -> Result<Vec<SearchHit>> {
    let query_terms = tokenize(query);
    if query_terms.is_empty() {
        return Ok(Vec::new());
    }

    let (where_sql, bind) = filter_where_clause(filters);
    let sql = format!(
        r#"
        SELECT
          c.id,
          c.document_id,
          d.title,
          d.uri,
          c.chunk_index,
          c.content,
          c.char_start,
          c.char_end
        FROM chunks c
        INNER JOIN documents d ON d.id = c.document_id
        {where_sql}
        "#
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = if bind.is_empty() {
        stmt.query([])?
    } else {
        stmt.query(params_from_iter(bind.iter()))?
    };

    let mut scored: Vec<(f32, SearchHit)> = Vec::new();
    while let Some(row) = rows.next()? {
        let content: String = row.get(5)?;
        let score = term_frequency_score(&query_terms, &content);
        if score <= 0.0 {
            continue;
        }
        let char_start: i32 = row.get(6)?;
        let char_end: i32 = row.get(7)?;
        let hit = SearchHit {
            chunk_id: row.get(0)?,
            document_id: row.get(1)?,
            document_title: row.get(2)?,
            document_uri: row.get(3)?,
            chunk_index: row.get(4)?,
            content: content.clone(),
            score,
            score_lex: Some(score),
            snippet: Some(make_snippet(&content, SNIPPET_CHARS)),
            char_start: Some(char_start),
            char_end: Some(char_end),
            ..Default::default()
        };
        scored.push((score, hit));
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);
    Ok(scored.into_iter().map(|(_, h)| h).collect())
}

/// Term-frequency score: sum of occurrences of each query term in `content`.
///
/// Not BM25 (no IDF, no k1/b). Used as the lex fallback when DuckDB FTS cannot
/// load. Ranking is still useful for keyword queries on small corpora.
pub fn term_frequency_score(query_terms: &[String], content: &str) -> f32 {
    if query_terms.is_empty() {
        return 0.0;
    }
    let doc_terms = tokenize(content);
    if doc_terms.is_empty() {
        return 0.0;
    }

    let mut tf: HashMap<&str, u32> = HashMap::new();
    for t in &doc_terms {
        *tf.entry(t.as_str()).or_insert(0) += 1;
    }

    let mut raw = 0.0f32;
    for q in query_terms {
        if let Some(&c) = tf.get(q.as_str()) {
            raw += c as f32;
        }
    }
    raw
}

/// Lowercase alphanumeric tokens (Unicode-aware letter/digit).
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            cur.extend(ch.to_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ---------------------------------------------------------------------------
// Filters / meta / helpers
// ---------------------------------------------------------------------------

fn filter_where_clause(filters: &LexFilters) -> (String, Vec<String>) {
    let mut parts: Vec<String> = Vec::new();
    let mut bind: Vec<String> = Vec::new();

    push_eq(&mut parts, &mut bind, "c.document_id", &filters.document_id);
    push_eq(&mut parts, &mut bind, "d.wing", &filters.wing);
    push_eq(&mut parts, &mut bind, "d.room", &filters.room);
    push_eq(&mut parts, &mut bind, "d.layer", &filters.layer);
    push_eq(&mut parts, &mut bind, "d.kind", &filters.kind);
    push_eq(&mut parts, &mut bind, "d.uri", &filters.uri);
    push_eq(&mut parts, &mut bind, "d.source_file", &filters.source_file);

    if !filters.include_archived {
        parts.push("COALESCE(d.status, 'active') NOT IN ('archived', 'tombstone')".into());
    }

    let where_sql = if parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", parts.join(" AND "))
    };
    (where_sql, bind)
}

fn push_eq(parts: &mut Vec<String>, bind: &mut Vec<String>, column: &str, value: &Option<String>) {
    if let Some(v) = value {
        if !v.is_empty() {
            parts.push(format!("{column} = ?"));
            bind.push(v.clone());
        }
    }
}

fn persist_state(conn: &Connection, state: &FtsState) -> Result<()> {
    upsert_meta(conn, META_BACKEND, state.backend.as_str())?;
    upsert_meta(conn, META_STEMMER, &state.stemmer)?;
    Ok(())
}

fn upsert_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        r#"
        INSERT OR REPLACE INTO meta (key, value)
        VALUES (?, ?)
        "#,
        params![key, value],
    )?;
    Ok(())
}

fn read_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?")?;
    let mut rows = stmt.query(params![key])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

fn read_u64_meta(conn: &Connection, key: &str) -> Result<Option<u64>> {
    let Some(raw) = read_meta(conn, key)? else {
        return Ok(None);
    };
    match raw.parse::<u64>() {
        Ok(value) => Ok(Some(value)),
        Err(e) => {
            // Treat corrupt lifecycle metadata as absent. That is fail-safe for
            // generations (it forces refresh) and self-heals on the next write.
            tracing::warn!(meta_key = key, value = %raw, error = %e, "invalid FTS generation metadata");
            Ok(None)
        }
    }
}

fn increment_rebuild_count(conn: &Connection) -> Result<u64> {
    let current = read_u64_meta(conn, META_REBUILD_COUNT)?.unwrap_or(0);
    let next = current
        .checked_add(1)
        .ok_or_else(|| AppError::fts("FTS rebuild counter overflow"))?;
    upsert_meta(conn, META_REBUILD_COUNT, &next.to_string())?;
    Ok(next)
}

/// Allow only ascii alpha / underscore stemmer names (DuckDB language list + none).
fn sanitize_stemmer(raw: &str) -> String {
    let t = raw.trim().to_ascii_lowercase();
    if t.is_empty() {
        return DEFAULT_STEMMER.to_string();
    }
    if t.chars().all(|c| c.is_ascii_alphabetic() || c == '_') {
        t
    } else {
        tracing::warn!(stemmer = %raw, "invalid FTS stemmer; using porter");
        DEFAULT_STEMMER.to_string()
    }
}

fn make_snippet(content: &str, max_chars: usize) -> String {
    let trimmed = content.trim();
    let count = trimmed.chars().count();
    if count <= max_chars {
        return trimmed.to_string();
    }
    let s: String = trimmed.chars().take(max_chars).collect();
    format!("{s}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use crate::models::{Chunk, Document};
    use chrono::Utc;
    use tempfile::TempDir;

    fn open_store() -> (TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fts.duckdb");
        let store = Store::open(&path).expect("open");
        (dir, store)
    }

    fn seed_docs(store: &Store) {
        let now = Utc::now();
        let docs = [
            (
                "d1",
                "file://cats.md",
                "Cats",
                "projects",
                "alpha",
                "raw",
                "document",
            ),
            (
                "d2",
                "file://dogs.md",
                "Dogs",
                "projects",
                "beta",
                "raw",
                "document",
            ),
            (
                "d3",
                "wiki://felines",
                "Felines",
                "archive",
                "misc",
                "wiki",
                "wiki",
            ),
        ];
        for (id, uri, title, wing, room, layer, kind) in docs {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: uri.into(),
                    title: title.into(),
                    content: title.into(),
                    metadata_json: "{}".into(),
                    created_at: now,
                    updated_at: now,
                    wing: Some(wing.into()),
                    room: Some(room.into()),
                    layer: layer.into(),
                    kind: kind.into(),
                    ..Default::default()
                })
                .unwrap();
        }

        let chunks = [
            (
                "c1",
                "d1",
                0,
                "The domestic cat is a small carnivorous mammal.",
            ),
            ("c2", "d1", 1, "Cats hunt mice and birds at night."),
            (
                "c3",
                "d2",
                0,
                "Dogs are domesticated descendants of the wolf.",
            ),
            (
                "c4",
                "d3",
                0,
                "Feline wiki: big cats and domestic cats overview.",
            ),
        ];
        let mut batch = Vec::new();
        for (id, doc, idx, content) in chunks {
            batch.push(Chunk {
                id: id.into(),
                document_id: doc.into(),
                chunk_index: idx,
                content: content.into(),
                embedding: vec![0.0; 4],
                char_start: 0,
                char_end: content.len() as i32,
                metadata_json: "{}".into(),
            });
        }
        store.insert_chunks(&batch).unwrap();
    }

    fn lifecycle(store: &Store) -> FtsGenerationState {
        let conn = store.lock().unwrap();
        fts_generation_state(&conn).unwrap()
    }

    #[test]
    fn tokenize_splits_and_lowercases() {
        assert_eq!(
            tokenize("Hello, WORLD! cat-nap 42"),
            vec!["hello", "world", "cat", "nap", "42"]
        );
    }

    #[test]
    fn term_frequency_ranks_relevant_higher() {
        let q = tokenize("cat mammal");
        let a = term_frequency_score(&q, "The domestic cat is a small carnivorous mammal.");
        let b = term_frequency_score(&q, "Dogs are domesticated descendants of the wolf.");
        assert!(a > b, "expected cat/mammal doc higher: {a} vs {b}");
        assert!((a - 2.0).abs() < 1e-6); // cat + mammal
        assert_eq!(b, 0.0);
    }

    #[test]
    fn ensure_and_reindex_are_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();
        // empty chunks table is fine for index create
        let s1 = ensure_fts(&conn).expect("ensure");
        let s2 = reindex(&conn).expect("reindex");
        assert_eq!(s1.backend, s2.backend);
        assert_eq!(s1.stemmer, s2.stemmer);
        let status = fts_status(&conn).unwrap().expect("status");
        assert_eq!(status.backend, s1.backend);
        match s1.backend {
            FtsBackend::DuckDbBm25 => assert!(fts_index_present(&conn)),
            FtsBackend::TermFrequency => {
                // schema may be absent under fallback
            }
        }
    }

    #[test]
    fn clean_warm_search_does_not_rebuild_fts() {
        let (_dir, store) = open_store();
        seed_docs(&store);
        {
            let conn = store.lock().unwrap();
            ensure_fts(&conn).unwrap();
        }
        let initialized = lifecycle(&store);
        assert!(!initialized.dirty);
        assert!(initialized.rebuild_count > 0);

        for _ in 0..2 {
            let hits = search_bm25(&store, "cat", 10, &LexFilters::default()).unwrap();
            assert!(!hits.is_empty());
        }

        let warm = lifecycle(&store);
        assert_eq!(warm.chunks_generation, initialized.chunks_generation);
        assert_eq!(warm.index_generation, initialized.index_generation);
        assert_eq!(
            warm.rebuild_count, initialized.rebuild_count,
            "a clean warm search must not execute create_fts_index"
        );
    }

    #[test]
    fn reopened_clean_index_searches_without_rebuild() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fts-reopen.duckdb");
        let (before, backend) = {
            let store = Store::open(&path).unwrap();
            seed_docs(&store);
            let backend = {
                let conn = store.lock().unwrap();
                ensure_fts(&conn).unwrap().backend
            };
            (lifecycle(&store), backend)
        };

        let reopened = Store::open(&path).unwrap();
        if backend == FtsBackend::DuckDbBm25 {
            let conn = reopened.lock().unwrap();
            let direct = search_duckdb_bm25(&conn, "cat", 10, &LexFilters::default()).unwrap();
            assert!(!direct.is_empty());
        }
        let hits = search_bm25(&reopened, "cat", 10, &LexFilters::default()).unwrap();
        assert!(!hits.is_empty());
        let after = lifecycle(&reopened);
        assert_eq!(after.rebuild_count, before.rebuild_count);
        assert!(!after.dirty);
    }

    #[test]
    fn dirty_term_frequency_backend_advances_without_fts_ddl_retry() {
        let conn = Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();
        let fallback = FtsState {
            backend: FtsBackend::TermFrequency,
            stemmer: DEFAULT_STEMMER.into(),
        };
        publish_refreshed_state(&conn, &fallback, 0).unwrap();
        mark_fts_dirty(&conn).unwrap();
        let before = fts_generation_state(&conn).unwrap();
        assert!(before.dirty);
        assert!(!fts_index_present(&conn));

        let refreshed = refresh_fts_if_stale(&conn).unwrap();
        assert_eq!(refreshed.backend, FtsBackend::TermFrequency);
        let after = fts_generation_state(&conn).unwrap();
        assert!(!after.dirty);
        assert_eq!(after.rebuild_count, before.rebuild_count + 1);
        assert!(
            !fts_index_present(&conn),
            "dirty TF refresh must not retry create_fts_index"
        );
    }

    #[test]
    fn mutation_marks_dirty_and_controlled_refresh_restores_freshness() {
        let (_dir, store) = open_store();
        seed_docs(&store);
        {
            let conn = store.lock().unwrap();
            ensure_fts(&conn).unwrap();
        }
        let before = lifecycle(&store);

        let content = "A quokka is a small marsupial native to Australia.";
        store
            .insert_chunks(&[Chunk {
                id: "c5".into(),
                document_id: "d1".into(),
                chunk_index: 2,
                content: content.into(),
                embedding: vec![0.0; 4],
                char_start: 0,
                char_end: content.len() as i32,
                metadata_json: "{}".into(),
            }])
            .unwrap();

        let dirty = lifecycle(&store);
        assert!(dirty.dirty);
        assert_eq!(dirty.chunks_generation, before.chunks_generation + 1);
        assert_eq!(dirty.index_generation, before.index_generation);

        {
            let conn = store.lock().unwrap();
            refresh_fts_if_stale(&conn).unwrap();
        }
        let refreshed = lifecycle(&store);
        assert!(!refreshed.dirty);
        assert_eq!(
            refreshed.index_generation,
            Some(refreshed.chunks_generation)
        );
        assert_eq!(refreshed.rebuild_count, before.rebuild_count + 1);

        let hits = search_bm25(&store, "quokka", 10, &LexFilters::default()).unwrap();
        assert!(hits.iter().any(|hit| hit.chunk_id == "c5"));
        assert_eq!(
            lifecycle(&store).rebuild_count,
            refreshed.rebuild_count,
            "search after controlled refresh must stay warm"
        );
    }

    #[test]
    fn concurrent_stale_searches_singleflight_one_refresh() {
        let (_dir, store) = open_store();
        seed_docs(&store);
        {
            let conn = store.lock().unwrap();
            ensure_fts(&conn).unwrap();
            mark_fts_dirty(&conn).unwrap();
        }
        let before = lifecycle(&store);
        assert!(before.dirty);

        let workers = (0..4)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    search_bm25(&store, "cat", 10, &LexFilters::default()).unwrap()
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            assert!(!worker.join().unwrap().is_empty());
        }

        let after = lifecycle(&store);
        assert!(!after.dirty);
        assert_eq!(after.rebuild_count, before.rebuild_count + 1);
    }

    #[test]
    fn search_bm25_finds_keyword_and_respects_top_k() {
        let (_dir, store) = open_store();
        seed_docs(&store);

        {
            let conn = store.lock().unwrap();
            ensure_fts(&conn).unwrap();
            reindex(&conn).unwrap();
        }

        let hits = search_bm25(&store, "cat mammal", 10, &LexFilters::default()).unwrap();
        assert!(!hits.is_empty(), "expected at least one lex hit");
        assert!(
            hits.iter()
                .any(|h| h.chunk_id == "c1" || h.content.to_lowercase().contains("cat")),
            "expected cat-related chunk, got {:?}",
            hits.iter().map(|h| &h.chunk_id).collect::<Vec<_>>()
        );
        for h in &hits {
            assert!(h.score_lex.is_some());
            assert!(h.score > 0.0);
            assert!(h.snippet.is_some());
        }

        let limited = search_bm25(&store, "cat", 1, &LexFilters::default()).unwrap();
        assert!(limited.len() <= 1);
    }

    #[test]
    fn search_bm25_filters_document_and_wing() {
        let (_dir, store) = open_store();
        seed_docs(&store);
        {
            let conn = store.lock().unwrap();
            ensure_fts(&conn).unwrap();
            reindex(&conn).unwrap();
        }

        let by_doc = search_bm25(&store, "cats", 10, &LexFilters::document_id("d1")).unwrap();
        assert!(!by_doc.is_empty());
        assert!(by_doc.iter().all(|h| h.document_id == "d1"));

        let by_wing = search_bm25(
            &store,
            "cats",
            10,
            &LexFilters {
                wing: Some("archive".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(by_wing.iter().all(|h| h.document_id == "d3"));
    }

    #[test]
    fn empty_query_or_top_k_zero_returns_empty() {
        let (_dir, store) = open_store();
        seed_docs(&store);
        assert!(search_bm25(&store, "  ", 5, &LexFilters::default())
            .unwrap()
            .is_empty());
        assert!(search_bm25(&store, "cat", 0, &LexFilters::default())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn stemmer_none_ensure_succeeds() {
        let conn = Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();
        let state = ensure_fts_with_stemmer(&conn, "none").unwrap();
        assert_eq!(state.stemmer, "none");
    }
}
