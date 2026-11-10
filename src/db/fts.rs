//! DuckDB FTS / BM25 index management for `chunks.content`.
//!
//! # Preferred path: DuckDB `fts` extension
//!
//! 1. `INSTALL fts;` then `LOAD fts;` (INSTALL is best-effort; LOAD is required)
//! 2. Tear down the reserved derived catalog in dependency order, then create without overwrite.
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

use super::schema::upsert_meta;
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
/// Set when chunk ids may have been reused with different text (recovery
/// import keeps bundle ids): the next refresh must rebuild, not patch by id.
const META_FORCE_REBUILD: &str = "fts_force_rebuild";
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
    /// The one scope predicate shared by the lexical and vector retrieval legs.
    ///
    /// Values are trimmed and blank values ignored, so both legs of a hybrid
    /// query always see the same scope. `document_id_column` names the column
    /// that carries the document id in the caller's join (`c.document_id` or
    /// `d.id`); every other column is qualified with the `d` documents alias.
    pub(crate) fn scope_where_clause(&self, document_id_column: &str) -> (String, Vec<String>) {
        let mut parts = Vec::new();
        let mut bind = Vec::new();
        for (column, value) in [
            (document_id_column, self.document_id.as_deref()),
            ("d.wing", self.wing.as_deref()),
            ("d.room", self.room.as_deref()),
            ("d.layer", self.layer.as_deref()),
            ("d.kind", self.kind.as_deref()),
            ("d.uri", self.uri.as_deref()),
            ("d.source_file", self.source_file.as_deref()),
        ] {
            if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
                parts.push(format!("{column} = ?"));
                bind.push(value.to_string());
            }
        }
        if !self.include_archived {
            parts.push(super::sql::active_document_sql("d"));
        }
        let where_sql = if parts.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", parts.join(" AND "))
        };
        (where_sql, bind)
    }

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
///
/// The change is logged as touching unknown documents, so delta readers of
/// [`changed_documents_since`] fall back to a full listing. Writers that know
/// which documents' chunks they change use [`mark_chunks_changed_for`].
pub fn mark_fts_dirty(conn: &Connection) -> Result<u64> {
    let next = advance_chunks_generation(conn)?;
    conn.execute(
        "INSERT INTO chunk_change_log (generation, document_id) VALUES (?, NULL)",
        duckdb::params![next as i64],
    )?;
    Ok(next)
}

/// [`mark_fts_dirty`] for a write that changes only the chunks of
/// `document_ids`: logs them so the next vector refresh re-reads just those
/// documents' chunks.
pub fn mark_chunks_changed_for<S: AsRef<str>>(
    conn: &Connection,
    document_ids: &[S],
) -> Result<u64> {
    let next = advance_chunks_generation(conn)?;
    log_changed_documents(conn, next, document_ids)?;
    Ok(next)
}

/// Generations kept in `chunk_change_log`; older rows are pruned and a
/// reader that needs them falls back to a full listing.
const CHANGE_LOG_WINDOW: u64 = 4096;

fn advance_chunks_generation(conn: &Connection) -> Result<u64> {
    let current = chunks_generation(conn)?;
    let next = current
        .checked_add(1)
        .ok_or_else(|| AppError::fts("FTS chunks generation overflow"))?;
    upsert_meta(conn, META_CHUNKS_GENERATION, &next.to_string())?;
    if next % 256 == 0 && next > CHANGE_LOG_WINDOW {
        conn.execute(
            "DELETE FROM chunk_change_log WHERE generation <= ?",
            duckdb::params![(next - CHANGE_LOG_WINDOW) as i64],
        )?;
    }
    Ok(next)
}

fn log_changed_documents<S: AsRef<str>>(
    conn: &Connection,
    generation: u64,
    document_ids: &[S],
) -> Result<()> {
    if document_ids.is_empty() {
        // Nothing named: record "unknown" so readers do a full listing.
        conn.execute(
            "INSERT INTO chunk_change_log (generation, document_id) VALUES (?, NULL)",
            duckdb::params![generation as i64],
        )?;
        return Ok(());
    }
    let mut stmt = conn
        .prepare_cached("INSERT INTO chunk_change_log (generation, document_id) VALUES (?, ?)")?;
    for id in document_ids {
        stmt.execute(duckdb::params![generation as i64, id.as_ref()])?;
    }
    Ok(())
}

/// Documents whose chunks changed after generation `since` up to `until`,
/// when every generation in that range was logged with known documents.
/// `None` when any is unknown or missing (pruned, legacy, or a writer that
/// used plain [`mark_fts_dirty`]): the caller must do a full listing.
pub(crate) fn changed_documents_since(
    conn: &Connection,
    since: u64,
    until: u64,
) -> Result<Option<std::collections::HashSet<String>>> {
    if until <= since {
        return Ok(Some(std::collections::HashSet::new()));
    }
    let mut stmt = conn.prepare_cached(
        "SELECT generation, document_id FROM chunk_change_log \
         WHERE generation > ? AND generation <= ?",
    )?;
    let mut rows = stmt.query(duckdb::params![since as i64, until as i64])?;
    let mut generations = std::collections::HashSet::new();
    let mut documents = std::collections::HashSet::new();
    while let Some(row) = rows.next()? {
        let generation: i64 = row.get(0)?;
        let Some(document_id) = row.get::<_, Option<String>>(1)? else {
            return Ok(None);
        };
        generations.insert(generation);
        documents.insert(document_id);
    }
    if generations.len() as u64 != until - since {
        return Ok(None);
    }
    Ok(Some(documents))
}

/// Mark the index dirty and require the next refresh to be a full rebuild.
///
/// For writers that may re-insert existing chunk ids with different text
/// (recovery import), where the incremental id diff would miss the change.
pub(crate) fn mark_fts_needs_rebuild(conn: &Connection) -> Result<u64> {
    upsert_meta(conn, META_FORCE_REBUILD, "1")?;
    mark_fts_dirty(conn)
}

/// Advance the chunk generation after changing only stored embeddings.
///
/// Vector snapshots key off the chunk generation, so it must advance even
/// though the indexed text rows are unchanged. When the lexical index was
/// clean before the mutation, advancing its generation in the same transaction
/// preserves that clean state without rebuilding BM25. An already-dirty or
/// unconfigured index stays dirty. Callers must invoke this on the same
/// transaction that owns the embedding updates so data and both generations
/// commit or roll back together.
///
/// `document_ids` names the documents whose vectors changed, for the vector
/// refresh's change log (`None` logs "unknown", forcing a full listing).
pub(crate) fn mark_embeddings_changed_for<S: AsRef<str>>(
    conn: &Connection,
    document_ids: Option<&[S]>,
) -> Result<u64> {
    let fts_was_clean = !fts_generation_state(conn)?.dirty;
    let next = match document_ids {
        Some(ids) => mark_chunks_changed_for(conn, ids)?,
        None => mark_fts_dirty(conn)?,
    };
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

/// Generation gap between source chunks and the derived FTS index.
///
/// `None` when no completed index generation exists yet (never refreshed).
pub fn fts_generation_gap(state: &FtsGenerationState) -> Option<u64> {
    state
        .index_generation
        .map(|index| state.chunks_generation.saturating_sub(index))
}

/// Staleness policy for the maintenance loop: a dirty FTS whose generation
/// gap reaches `threshold` should be reindexed. `threshold == 0` disables
/// auto-reindex; a dirty index with no completed generation always qualifies.
pub fn fts_stale_needs_reindex(state: &FtsGenerationState, threshold: u64) -> bool {
    if threshold == 0 || !state.dirty {
        return false;
    }
    match fts_generation_gap(state) {
        Some(gap) => gap >= threshold,
        None => true,
    }
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
        if let Some(state) = current
            .as_ref()
            .filter(|state| state.backend == FtsBackend::TermFrequency)
        {
            // TF scores live chunks directly. A chunk mutation only needs to
            // advance the logical generation; retry extension installation
            // only on an explicit reindex or configuration change.
            publish_refreshed_state(conn, state, generations.chunks_generation)?;
            return Ok(state.clone());
        }
        let force_rebuild = read_meta(conn, META_FORCE_REBUILD)?.is_some_and(|flag| flag == "1");
        if let Some(state) = current
            .filter(|state| state.backend == FtsBackend::DuckDbBm25)
            .filter(|_| !force_rebuild)
        {
            // A write burst usually touches a few documents: patch the BM25
            // tables for the changed chunks instead of re-tokenizing the corpus.
            match incremental_fts_update(conn, &stemmer) {
                Ok(true) => {
                    tracing::debug!(
                        chunks_generation = generations.chunks_generation,
                        "FTS index updated incrementally"
                    );
                    publish_refreshed_state(conn, &state, generations.chunks_generation)?;
                    return Ok(state);
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(error = %error, "incremental FTS update failed; rebuilding");
                }
            }
        }
    }

    rebuild_fts(conn, stemmer, generations.chunks_generation)
}

/// Patch the DuckDB BM25 index (`fts_main_chunks.{docs,terms,dict,stats}`) for
/// chunks added or removed since it was built, using the extension's own
/// `tokenize` macro, stopword table and stemmer, so scores match a rebuild.
///
/// Chunk text is immutable (writes replace chunks under new ids), so the id
/// diff between `chunks` and `docs` is the complete change set. Returns
/// `Ok(false)` without changing anything when a rebuild is the better plan:
/// no physical index, the extension cannot load, an outer transaction is open,
/// or at least half of the indexed documents changed.
fn incremental_fts_update(conn: &Connection, stemmer: &str) -> Result<bool> {
    if !fts_index_present(conn) || !load_fts_extension(conn) {
        return Ok(false);
    }
    let (indexed, added, removed): (i64, i64, i64) = conn.query_row(
        r#"
        SELECT
          (SELECT COUNT(*) FROM fts_main_chunks.docs),
          (SELECT COUNT(*) FROM chunks c
            WHERE NOT EXISTS (SELECT 1 FROM fts_main_chunks.docs d WHERE d.name = c.id)),
          (SELECT COUNT(*) FROM fts_main_chunks.docs d
            WHERE NOT EXISTS (SELECT 1 FROM chunks c WHERE c.id = d.name))
        "#,
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if added == 0 && removed == 0 {
        // Only vectors changed: the lexical index is already current.
        return Ok(true);
    }
    if (added + removed) * 2 >= indexed.max(1) {
        return Ok(false);
    }
    // `stemmer` is sanitized to ASCII letters/underscore by the caller.
    let stem = if stemmer == "none" {
        "w".to_string()
    } else {
        format!("stem(w, '{stemmer}')")
    };
    if conn.execute_batch("BEGIN TRANSACTION").is_err() {
        return Ok(false);
    }
    let patch = format!(
        r#"
        CREATE OR REPLACE TEMP TABLE fts_delta_removed AS
          SELECT d.docid FROM fts_main_chunks.docs d
          WHERE NOT EXISTS (SELECT 1 FROM chunks c WHERE c.id = d.name);
        UPDATE fts_main_chunks.dict SET df = dict.df - gone.n
          FROM (SELECT t.termid, COUNT(DISTINCT t.docid) AS n
                FROM fts_main_chunks.terms t
                JOIN fts_delta_removed r ON r.docid = t.docid
                GROUP BY t.termid) gone
          WHERE dict.termid = gone.termid;
        DELETE FROM fts_main_chunks.terms
          WHERE docid IN (SELECT docid FROM fts_delta_removed);
        DELETE FROM fts_main_chunks.docs
          WHERE docid IN (SELECT docid FROM fts_delta_removed);
        DELETE FROM fts_main_chunks.dict WHERE df <= 0;

        CREATE OR REPLACE TEMP TABLE fts_delta_added AS
          SELECT c.id AS name, c.content,
                 (SELECT COALESCE(MAX(docid), -1) FROM fts_main_chunks.docs)
                   + row_number() OVER (ORDER BY c.id) AS docid
          FROM chunks c
          WHERE NOT EXISTS (SELECT 1 FROM fts_main_chunks.docs d WHERE d.name = c.id);
        CREATE OR REPLACE TEMP TABLE fts_delta_terms AS
          SELECT docid, {stem} AS term
          FROM (SELECT docid, unnest(fts_main_chunks.tokenize(content)) AS w
                FROM fts_delta_added)
          WHERE w NOT IN (SELECT sw FROM fts_main_chunks.stopwords) AND w <> '';
        INSERT INTO fts_main_chunks.dict
          SELECT (SELECT COALESCE(MAX(termid), -1) FROM fts_main_chunks.dict)
                   + row_number() OVER (ORDER BY term),
                 term, 0
          FROM (SELECT DISTINCT term FROM fts_delta_terms
                WHERE term NOT IN (SELECT term FROM fts_main_chunks.dict));
        UPDATE fts_main_chunks.dict SET df = dict.df + fresh.n
          FROM (SELECT term, COUNT(DISTINCT docid) AS n
                FROM fts_delta_terms GROUP BY term) fresh
          WHERE dict.term = fresh.term;
        INSERT INTO fts_main_chunks.terms
          SELECT t.docid, 0, d.termid
          FROM fts_delta_terms t JOIN fts_main_chunks.dict d ON d.term = t.term;
        INSERT INTO fts_main_chunks.docs
          SELECT a.docid, a.name, COUNT(t.term)
          FROM fts_delta_added a LEFT JOIN fts_delta_terms t ON t.docid = a.docid
          GROUP BY a.docid, a.name;
        UPDATE fts_main_chunks.stats
          SET num_docs = (SELECT COUNT(*) FROM fts_main_chunks.docs),
              avgdl = (SELECT AVG(len) FROM fts_main_chunks.docs);
        DROP TABLE fts_delta_removed;
        DROP TABLE fts_delta_added;
        DROP TABLE fts_delta_terms;
        "#
    );
    match conn.execute_batch(&patch) {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(true)
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(AppError::fts(format!("incremental FTS update: {error}")))
        }
    }
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
    #[cfg(test)]
    test_fts_crashpoint(conn, "published");
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
    if read_meta(conn, META_FORCE_REBUILD)?.as_deref() == Some("1") {
        // Only a full rebuild may clear the flag; the incremental path is
        // never taken while it is set.
        upsert_meta(conn, META_FORCE_REBUILD, "0")?;
    }
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

#[cfg(test)]
static FTS_CRASH_ARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
fn test_fts_crashpoint(conn: &Connection, phase: &str) {
    if !FTS_CRASH_ARMED.load(std::sync::atomic::Ordering::SeqCst)
        || std::env::var("RAG_TEST_FTS_CRASH_PHASE").ok().as_deref() != Some(phase)
    {
        return;
    }
    let path = std::path::PathBuf::from(std::env::var_os("RAG_TEST_FTS_CRASH_DB").unwrap());
    let state = fts_generation_state(conn).unwrap();
    std::fs::write(path.with_extension("ready.tmp"),serde_json::to_string(&serde_json::json!({"phase":phase,"extension_available":true,"chunks_generation":state.chunks_generation,"dirty":state.dirty})).unwrap()).unwrap();
    std::fs::rename(
        path.with_extension("ready.tmp"),
        path.with_extension("ready.json"),
    )
    .unwrap();
    loop {
        std::thread::park();
    }
}

/// Remove only the reserved derived FTS catalog, leaf objects before its schema.
/// DuckDB's extension overwrite path can log DROP SCHEMA ahead of its dependent
/// table drops; that WAL cannot replay when the old schema was checkpointed.
fn drop_derived_fts_catalog(conn: &Connection) -> Result<()> {
    let mut drops = Vec::new();
    {
        let mut stmt=conn.prepare("SELECT DISTINCT function_name,function_type FROM duckdb_functions() WHERE schema_name=? ORDER BY function_name")?;
        let rows = stmt.query_map(params![FTS_SCHEMA], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (name, kind) = row?;
            if kind != "macro" {
                return Err(AppError::fts(
                    "unsupported object in reserved FTS schema; refusing partial teardown",
                ));
            }
            drops.push(format!(
                "DROP MACRO {}.{}",
                quoted_identifier(FTS_SCHEMA),
                quoted_identifier(&name)
            ));
        }
        let mut stmt=conn.prepare("SELECT table_name,table_type FROM information_schema.tables WHERE table_schema=? ORDER BY CASE WHEN table_type='VIEW' THEN 0 ELSE 1 END,table_name")?;
        let rows = stmt.query_map(params![FTS_SCHEMA], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (name, kind) = row?;
            let object = match kind.as_str() {
                "VIEW" => "VIEW",
                "BASE TABLE" => "TABLE",
                _ => return Err(AppError::fts("unsupported relation in reserved FTS schema")),
            };
            drops.push(format!(
                "DROP {object} {}.{}",
                quoted_identifier(FTS_SCHEMA),
                quoted_identifier(&name)
            ));
        }
    }
    // A killed explicit force-rebuild must not leave a partial but logically
    // clean index. Invalidate only the derived generation, not chunk/vector data.
    conn.execute(
        "DELETE FROM meta WHERE key=?",
        params![META_INDEX_GENERATION],
    )?;
    #[cfg(test)]
    test_fts_crashpoint(conn, "dirty");
    while !drops.is_empty() {
        let before = drops.len();
        let mut remaining = Vec::new();
        let mut last_error = None;
        for ddl in drops {
            if let Err(error) = conn.execute_batch(&ddl) {
                last_error = Some(error);
                remaining.push(ddl);
            } else {
                #[cfg(test)]
                test_fts_crashpoint(conn, "leaf");
            }
        }
        if remaining.len() == before {
            return Err(AppError::fts(format!(
                "FTS leaf teardown could not resolve dependencies: {}",
                last_error.unwrap()
            )));
        }
        drops = remaining;
    }

    #[cfg(test)]
    test_fts_crashpoint(conn, "leaves");
    // No CASCADE: replay must see an already empty schema, and unexpected
    // dependencies must produce an explicit fallback instead of being discarded.
    conn.execute_batch(&format!(
        "DROP SCHEMA IF EXISTS {}",
        quoted_identifier(FTS_SCHEMA)
    ))?;
    #[cfg(test)]
    test_fts_crashpoint(conn, "schema");
    Ok(())
}
fn quoted_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn try_duckdb_fts_index(conn: &Connection, stemmer: &str) -> Result<()> {
    // INSTALL may fail when already installed, offline, or extension repo is
    // unreachable. LOAD is what enables the PRAGMA macros.
    if !load_fts_extension(conn) {
        return Err(AppError::fts(
            "LOAD fts failed (INSTALL/LOAD unavailable on this connection)",
        ));
    }

    drop_derived_fts_catalog(conn)?;
    // The old catalog is empty/removed in replay-safe order. Never ask the
    // extension to cascade-drop a checkpointed schema through overwrite=1.
    let stopwords = if stemmer == "none" { "none" } else { "english" };
    let pragma = format!(
        "PRAGMA create_fts_index(\
            'chunks', 'id', 'content', \
            stemmer = '{stemmer}', \
            stopwords = '{stopwords}', \
            overwrite = 0\
        );"
    );
    conn.execute_batch(&pragma)
        .map_err(|e| AppError::fts(format!("create_fts_index on chunks failed: {e}")))?;
    #[cfg(test)]
    test_fts_crashpoint(conn, "built");
    Ok(())
}

fn search_duckdb_bm25(
    conn: &Connection,
    query: &str,
    top_k: usize,
    filters: &LexFilters,
) -> Result<Vec<SearchHit>> {
    let (where_sql, mut bind) = filters.scope_where_clause("c.document_id");
    // query first for match_bm25, then filter binds. `top_k` is inlined (usize).
    let mut all_binds: Vec<String> = Vec::with_capacity(bind.len() + 1);
    all_binds.push(query.to_string());
    all_binds.append(&mut bind);

    // Score chunk ids only, keep the top k, then read content and titles for
    // those rows: chunk bodies are never scanned for the rest of the corpus.
    let sql = format!(
        r#"
        WITH ranked AS (
          SELECT c.id AS chunk_id, fts_main_chunks.match_bm25(c.id, ?) AS score
          FROM chunks c
          INNER JOIN documents d ON d.id = c.document_id
          {where_sql}
        ),
        top AS (
          SELECT chunk_id, score FROM ranked
          WHERE score IS NOT NULL
          ORDER BY score DESC
          LIMIT {top_k}
        )
        SELECT c.id, c.document_id, d.title, d.uri, c.chunk_index, c.content,
               c.char_start, c.char_end, top.score
        FROM top
        INNER JOIN chunks c ON c.id = top.chunk_id
        INNER JOIN documents d ON d.id = c.document_id
        ORDER BY top.score DESC
        "#
    );

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::fts(format!("prepare match_bm25: {e}")))?;

    let mut rows = stmt
        .query(params_from_iter(all_binds.iter()))
        .map_err(|e| AppError::fts(format!("query match_bm25: {e}")))?;

    let mut hits = Vec::new();
    crate::db::rows::visit_rows(&mut rows, |row| {
        let score: f64 = row.get(8)?;
        let content: String = row.get(5)?;
        hits.push(lexical_hit(row, content, score as f32)?);
        Ok(())
    })?;
    Ok(hits)
}

/// One lexical hit from the shared projection
/// `(chunk id, document id, title, uri, chunk index, content, char_start, char_end)`
/// used by both the BM25 and the term-frequency paths.
fn lexical_hit(row: &duckdb::Row<'_>, content: String, score: f32) -> Result<SearchHit> {
    let snippet = make_snippet(&content, SNIPPET_CHARS);
    Ok(SearchHit {
        chunk_id: row.get(0)?,
        document_id: row.get(1)?,
        document_title: row.get(2)?,
        document_uri: row.get(3)?,
        chunk_index: row.get(4)?,
        content,
        score,
        score_lex: Some(score),
        snippet: Some(snippet),
        char_start: Some(row.get(6)?),
        char_end: Some(row.get(7)?),
        ..Default::default()
    })
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

    let (where_sql, bind) = filters.scope_where_clause("c.document_id");
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

    // Bounded top-k: a hit (and its snippet) is built only for rows that
    // currently rank inside the window, not for every matching chunk.
    let mut best: std::collections::BinaryHeap<std::cmp::Reverse<TfCandidate>> =
        std::collections::BinaryHeap::with_capacity(top_k.saturating_add(1));
    let mut ordinal = 0usize;
    crate::db::rows::try_visit_rows(&mut rows, |row| {
        ordinal += 1;
        if top_k == 0 {
            return Ok(std::ops::ControlFlow::Break(()));
        }
        let content: String = row.get(5)?;
        let score = term_frequency_score(&query_terms, &content);
        if score <= 0.0 {
            return Ok(std::ops::ControlFlow::Continue(()));
        }
        let rank = (score, ordinal);
        if best.len() >= top_k && best.peek().is_some_and(|worst| !worst.0.ranks_below(rank)) {
            return Ok(std::ops::ControlFlow::Continue(()));
        }
        best.push(std::cmp::Reverse(TfCandidate {
            score,
            ordinal,
            hit: lexical_hit(row, content, score)?,
        }));
        if best.len() > top_k {
            best.pop();
        }
        Ok(std::ops::ControlFlow::Continue(()))
    })?;

    let mut ranked: Vec<TfCandidate> = best.into_iter().map(|entry| entry.0).collect();
    ranked.sort_by(|a, b| b.cmp(a));
    Ok(ranked.into_iter().map(|candidate| candidate.hit).collect())
}

/// TF fallback candidate: higher score first, then earlier row (stable order).
struct TfCandidate {
    score: f32,
    ordinal: usize,
    hit: SearchHit,
}

impl TfCandidate {
    /// Whether this candidate ranks below a new `(score, ordinal)`.
    fn ranks_below(&self, (score, ordinal): (f32, usize)) -> bool {
        self.score
            .total_cmp(&score)
            .then_with(|| ordinal.cmp(&self.ordinal))
            .is_lt()
    }
}

impl PartialEq for TfCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for TfCandidate {}

impl PartialOrd for TfCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TfCandidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.ordinal.cmp(&self.ordinal))
    }
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

fn persist_state(conn: &Connection, state: &FtsState) -> Result<()> {
    upsert_meta(conn, META_BACKEND, state.backend.as_str())?;
    upsert_meta(conn, META_STEMMER, &state.stemmer)?;
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
pub(super) fn sanitize_stemmer(raw: &str) -> String {
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

/// Test-only fault injection for derived-index finalization.
///
/// A workflow names its finalization site; a test selects that one site by
/// putting [`test_fault::fail`] or [`test_fault::panic`] into the configured
/// stemmer. Other sites in the same flow keep the real refresh path.
#[cfg(test)]
pub(crate) mod test_fault {
    use crate::error::{AppError, Result};

    const FAIL_PREFIX: &str = "__test_fail_finalize_fts__:";
    const PANIC_PREFIX: &str = "__test_panic_finalize_fts__:";

    /// Stemmer value that makes `site`'s finalization return an FTS error.
    pub(crate) fn fail(site: &str) -> String {
        format!("{FAIL_PREFIX}{site}")
    }

    /// Stemmer value that makes `site`'s finalization panic.
    pub(crate) fn panic(site: &str) -> String {
        format!("{PANIC_PREFIX}{site}")
    }

    /// Called at the top of each finalization site.
    pub(crate) fn check(site: &str, stemmer: &str) -> Result<()> {
        if stemmer.strip_prefix(PANIC_PREFIX) == Some(site) {
            panic!("injected {site} FTS finalization panic");
        }
        if stemmer.strip_prefix(FAIL_PREFIX) == Some(site) {
            return Err(AppError::fts(format!(
                "injected {site} FTS finalization failure"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
