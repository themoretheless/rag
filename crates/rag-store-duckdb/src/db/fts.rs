//! Okapi BM25 + Porter stemmer lexical index management for `chunks.content` over `lin`.
//!
//! Tracks `chunks_generation`, `index_generation`, `rebuild_count`, `fts_backend`,
//! and `fts_stemmer` in `meta` plus per-document deltas in `chunk_change_log`
//! so vector snapshots and lexical searches preserve read-your-writes, singleflight
//! refresh, and incremental invalidation.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use super::lin_engine::{
    LinReader, LinWriter, RecBuilder, cell_i64, cell_nonempty_string, cell_opt_string, cell_str,
    cell_string, composite_id_2,
};
use super::schema::{delete_meta, get_meta, upsert_meta};
use super::store::Store;
use crate::error::{AppError, Result};
use crate::models::SearchHit;

/// Legacy schema identifier retained for public API compatibility.
pub const FTS_SCHEMA: &str = "fts_main_chunks";

/// Indexed base collection.
pub const FTS_TABLE: &str = "chunks";

const META_BACKEND: &str = "fts_backend";
const META_STEMMER: &str = "fts_stemmer";
const META_CHUNKS_GENERATION: &str = "fts_chunks_generation";
const META_INDEX_GENERATION: &str = "fts_index_generation";
const META_REBUILD_COUNT: &str = "fts_rebuild_count";
const META_FORCE_REBUILD: &str = "fts_force_rebuild";
const DEFAULT_STEMMER: &str = "porter";
const SNIPPET_CHARS: usize = 240;

const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

const ENGLISH_STOPWORDS: &[&str] = &[
    "a",
    "about",
    "above",
    "after",
    "again",
    "against",
    "all",
    "am",
    "an",
    "and",
    "any",
    "are",
    "as",
    "at",
    "be",
    "because",
    "been",
    "before",
    "being",
    "below",
    "between",
    "both",
    "but",
    "by",
    "can",
    "cannot",
    "could",
    "did",
    "do",
    "does",
    "doing",
    "down",
    "during",
    "each",
    "few",
    "for",
    "from",
    "further",
    "had",
    "has",
    "have",
    "having",
    "he",
    "her",
    "here",
    "hers",
    "herself",
    "him",
    "himself",
    "his",
    "how",
    "i",
    "if",
    "in",
    "into",
    "is",
    "it",
    "its",
    "itself",
    "just",
    "me",
    "more",
    "most",
    "must",
    "my",
    "myself",
    "no",
    "nor",
    "not",
    "now",
    "of",
    "off",
    "on",
    "once",
    "only",
    "or",
    "other",
    "ought",
    "our",
    "ours",
    "ourselves",
    "out",
    "over",
    "own",
    "same",
    "she",
    "should",
    "so",
    "some",
    "such",
    "than",
    "that",
    "the",
    "their",
    "theirs",
    "them",
    "themselves",
    "then",
    "there",
    "these",
    "they",
    "this",
    "those",
    "through",
    "to",
    "too",
    "under",
    "until",
    "up",
    "very",
    "was",
    "we",
    "were",
    "what",
    "when",
    "where",
    "which",
    "while",
    "who",
    "whom",
    "why",
    "will",
    "with",
    "without",
    "would",
    "you",
    "your",
    "yours",
    "yourself",
    "yourselves",
];

/// Which lexical backend is active after [`ensure_fts`] / [`reindex`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtsBackend {
    /// Okapi BM25 lexical index (`lin_bm25`; `DuckDbBm25` name retained for public API compat).
    DuckDbBm25,
    /// Rust term-frequency fallback.
    TermFrequency,
}

impl FtsBackend {
    /// Stable wire / meta value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DuckDbBm25 => "lin_bm25",
            Self::TermFrequency => "term_frequency",
        }
    }

    fn from_meta(s: &str) -> Option<Self> {
        match s.trim() {
            "lin_bm25" | "duckdb_bm25" | "duckdb" | "lin" | "fts" | "bm25" => {
                Some(Self::DuckDbBm25)
            }
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtsGenerationState {
    pub chunks_generation: u64,
    pub index_generation: Option<u64>,
    pub dirty: bool,
    pub rebuild_count: u64,
}

/// Optional equality filters for lexical search (joined via `documents`).
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

const ACTIVE_DOCUMENT_SQL: &str = "COALESCE(d.status, 'active') NOT IN ('archived', 'tombstone')";

impl LexFilters {
    /// Shared scope filter descriptor.
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
            parts.push(ACTIVE_DOCUMENT_SQL.into());
        }
        let where_sql = if parts.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", parts.join(" AND "))
        };
        (where_sql, bind)
    }

    /// Evaluate whether a `documents` `lin::Row` satisfies `self`.
    pub(crate) fn matches_document_row(&self, doc_row: &lin::Row) -> bool {
        if let Some(want_id) = self
            .document_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && cell_str(doc_row, "id") != Some(want_id)
        {
            return false;
        }
        if let Some(want_wing) = self
            .wing
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && cell_str(doc_row, "wing") != Some(want_wing)
        {
            return false;
        }
        if let Some(want_room) = self
            .room
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && cell_str(doc_row, "room") != Some(want_room)
        {
            return false;
        }
        if let Some(want_layer) = self
            .layer
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && cell_str(doc_row, "layer") != Some(want_layer)
        {
            return false;
        }
        if let Some(want_kind) = self
            .kind
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && cell_str(doc_row, "kind") != Some(want_kind)
        {
            return false;
        }
        if let Some(want_uri) = self.uri.as_deref().map(str::trim).filter(|s| !s.is_empty())
            && cell_str(doc_row, "uri") != Some(want_uri)
        {
            return false;
        }
        if let Some(want_src) = self
            .source_file
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && cell_str(doc_row, "source_file") != Some(want_src)
        {
            return false;
        }
        if !self.include_archived {
            let status = cell_nonempty_string(doc_row, "status").unwrap_or_else(|| "active".into());
            if status == "archived" || status == "tombstone" {
                return false;
            }
        }
        true
    }

    /// True when any scope column other than default active-status exclusion is set.
    pub(crate) fn has_explicit_scope(&self) -> bool {
        [
            self.document_id.as_deref(),
            self.wing.as_deref(),
            self.room.as_deref(),
            self.layer.as_deref(),
            self.kind.as_deref(),
            self.uri.as_deref(),
            self.source_file.as_deref(),
        ]
        .into_iter()
        .any(|v| v.map(str::trim).is_some_and(|s| !s.is_empty()))
    }

    /// Convenience: filter to a single document id.
    pub fn document_id(id: impl Into<String>) -> Self {
        Self {
            document_id: Some(id.into()),
            ..Default::default()
        }
    }
}

/// Ensure the lexical search path is ready on `chunks` (`"porter"` default).
pub fn ensure_fts(w: &mut impl LinWriter) -> Result<FtsState> {
    ensure_fts_with_stemmer(w, DEFAULT_STEMMER)
}

/// Like [`ensure_fts`], using `stemmer` (e.g. `"porter"` or `"none"`).
pub fn ensure_fts_with_stemmer(w: &mut impl LinWriter, stemmer: &str) -> Result<FtsState> {
    refresh_fts_if_stale_with_stemmer(w, stemmer)
}

/// Mark the derived FTS index stale before mutating `chunks`.
pub fn mark_fts_dirty(w: &mut impl LinWriter) -> Result<u64> {
    let next = advance_chunks_generation(w)?;
    w.insert_one(
        "chunk_change_log",
        RecBuilder::new()
            .str("id", format!("gen-{next}"))
            .i64("generation", next as i64)
            .opt_str("document_id", None)
            .build(),
    )?;
    Ok(next)
}

/// [`mark_fts_dirty`] for a write that changes only the chunks of `document_ids`.
pub fn mark_chunks_changed_for<S: AsRef<str>>(
    w: &mut impl LinWriter,
    document_ids: &[S],
) -> Result<u64> {
    let next = advance_chunks_generation(w)?;
    log_changed_documents(w, next, document_ids)?;
    Ok(next)
}

const CHANGE_LOG_WINDOW: u64 = 4096;

fn advance_chunks_generation(w: &mut impl LinWriter) -> Result<u64> {
    let current = chunks_generation(w)?;
    let next = current
        .checked_add(1)
        .ok_or_else(|| AppError::fts("FTS chunks generation overflow"))?;
    upsert_meta(w, META_CHUNKS_GENERATION, &next.to_string())?;
    if next.is_multiple_of(256) && next > CHANGE_LOG_WINDOW {
        let cutoff = (next - CHANGE_LOG_WINDOW) as i64;
        w.delete_where(
            "chunk_change_log",
            lin::query::pred::le("generation", cutoff),
        )?;
    }
    Ok(next)
}

fn log_changed_documents<S: AsRef<str>>(
    w: &mut impl LinWriter,
    generation: u64,
    document_ids: &[S],
) -> Result<()> {
    if document_ids.is_empty() {
        w.insert_one(
            "chunk_change_log",
            RecBuilder::new()
                .str("id", format!("gen-{generation}"))
                .i64("generation", generation as i64)
                .opt_str("document_id", None)
                .build(),
        )?;
        return Ok(());
    }
    let mut records = Vec::with_capacity(document_ids.len());
    for (idx, id) in document_ids.iter().enumerate() {
        let row_id = composite_id_2(&generation.to_string(), &format!("{idx}:{}", id.as_ref()));
        records.push(
            RecBuilder::new()
                .str("id", row_id)
                .i64("generation", generation as i64)
                .str("document_id", id.as_ref())
                .build(),
        );
    }
    w.insert_records("chunk_change_log", records)?;
    Ok(())
}

/// Documents whose chunks changed after generation `since` up to `until`.
pub(crate) fn changed_documents_since(
    r: &impl LinReader,
    since: u64,
    until: u64,
) -> Result<Option<HashSet<String>>> {
    if until <= since {
        return Ok(Some(HashSet::new()));
    }
    if !r.has_collection("chunk_change_log") {
        return Ok(None);
    }
    let pred = lin::query::pred::and(
        lin::query::pred::gt("generation", since as i64),
        lin::query::pred::le("generation", until as i64),
    );
    let rows = r.query_filter("chunk_change_log", Some(pred), None)?;
    let mut generations = HashSet::new();
    let mut documents = HashSet::new();
    for row in &rows {
        let generation = cell_i64(row, "generation", 0);
        let Some(doc_id) = cell_opt_string(row, "document_id").filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        generations.insert(generation);
        documents.insert(doc_id);
    }
    if generations.len() as u64 != until - since {
        return Ok(None);
    }
    Ok(Some(documents))
}

/// Mark the index dirty and require the next refresh to be a full rebuild.
pub(crate) fn mark_fts_needs_rebuild(w: &mut impl LinWriter) -> Result<u64> {
    upsert_meta(w, META_FORCE_REBUILD, "1")?;
    mark_fts_dirty(w)
}

/// Advance the chunk generation after changing only chunk embeddings.
pub(crate) fn mark_embeddings_changed_for<S: AsRef<str>>(
    w: &mut impl LinWriter,
    document_ids: Option<&[S]>,
) -> Result<u64> {
    let fts_was_clean = !fts_generation_state(w)?.dirty;
    let next = match document_ids {
        Some(ids) => mark_chunks_changed_for(w, ids)?,
        None => mark_fts_dirty(w)?,
    };
    if fts_was_clean {
        upsert_meta(w, META_INDEX_GENERATION, &next.to_string())?;
    }
    Ok(next)
}

/// Read only the source chunk generation.
pub fn chunks_generation(r: &impl LinReader) -> Result<u64> {
    Ok(read_u64_meta(r, META_CHUNKS_GENERATION)?.unwrap_or(0))
}

/// Generation gap between source chunks and the derived FTS index.
pub fn fts_generation_gap(state: &FtsGenerationState) -> Option<u64> {
    state
        .index_generation
        .map(|index| state.chunks_generation.saturating_sub(index))
}

/// Staleness policy for the maintenance loop.
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
pub fn fts_generation_state(r: &impl LinReader) -> Result<FtsGenerationState> {
    let recorded_chunks_generation = read_u64_meta(r, META_CHUNKS_GENERATION)?;
    let chunks_generation = recorded_chunks_generation.unwrap_or(0);
    let index_generation = read_u64_meta(r, META_INDEX_GENERATION)?;
    let rebuild_count = read_u64_meta(r, META_REBUILD_COUNT)?.unwrap_or(0);
    let configured = fts_status(r)?;
    let physical_index_missing = configured
        .as_ref()
        .is_some_and(|state| state.backend == FtsBackend::DuckDbBm25 && !fts_index_present(r));
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
pub fn refresh_fts_if_stale(w: &mut impl LinWriter) -> Result<FtsState> {
    let stemmer = get_meta(w, META_STEMMER)?.unwrap_or_else(|| DEFAULT_STEMMER.to_string());
    refresh_fts_if_stale_with_stemmer(w, &stemmer)
}

/// Like [`refresh_fts_if_stale`], with an explicit requested stemmer.
pub fn refresh_fts_if_stale_with_stemmer(
    w: &mut impl LinWriter,
    stemmer: &str,
) -> Result<FtsState> {
    refresh_fts(w, stemmer, RefreshMode::IfStale)
}

/// Force-rebuild the FTS index.
pub fn reindex(w: &mut impl LinWriter) -> Result<FtsState> {
    let stemmer = get_meta(w, META_STEMMER)?.unwrap_or_else(|| DEFAULT_STEMMER.to_string());
    refresh_fts(w, &stemmer, RefreshMode::Force)
}

/// Rebuild with an explicit stemmer (updates `meta.fts_stemmer`).
pub fn reindex_with_stemmer(w: &mut impl LinWriter, stemmer: &str) -> Result<FtsState> {
    refresh_fts(w, stemmer, RefreshMode::Force)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshMode {
    IfStale,
    Force,
}

fn refresh_fts(w: &mut impl LinWriter, stemmer: &str, mode: RefreshMode) -> Result<FtsState> {
    let stemmer = sanitize_stemmer(stemmer);
    let current = fts_status(w)?;
    let generations = fts_generation_state(w)?;
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
            publish_refreshed_state(w, state, generations.chunks_generation)?;
            return Ok(state.clone());
        }
    }

    if mode == RefreshMode::Force {
        let _ = delete_meta(w, META_INDEX_GENERATION);
    }

    let state = FtsState {
        backend: FtsBackend::DuckDbBm25,
        stemmer,
    };
    publish_refreshed_state(w, &state, generations.chunks_generation)?;
    Ok(state)
}

pub(super) fn publish_refreshed_state(
    w: &mut impl LinWriter,
    state: &FtsState,
    chunks_generation: u64,
) -> Result<()> {
    persist_state(w, state)?;
    if get_meta(w, META_FORCE_REBUILD)?.as_deref() == Some("1") {
        upsert_meta(w, META_FORCE_REBUILD, "0")?;
    }
    upsert_meta(w, META_CHUNKS_GENERATION, &chunks_generation.to_string())?;
    upsert_meta(w, META_INDEX_GENERATION, &chunks_generation.to_string())?;
    increment_rebuild_count(w)?;
    Ok(())
}

/// Load last-known FTS state from `meta`, if [`ensure_fts`] has run.
pub fn fts_status(r: &impl LinReader) -> Result<Option<FtsState>> {
    let backend = match get_meta(r, META_BACKEND)? {
        Some(b) => FtsBackend::from_meta(&b),
        None => None,
    };
    let Some(backend) = backend else {
        return Ok(None);
    };
    let stemmer = get_meta(r, META_STEMMER)?.unwrap_or_else(|| DEFAULT_STEMMER.to_string());
    Ok(Some(FtsState { backend, stemmer }))
}

/// True when the BM25 lexical index is configured and active on `chunks`.
pub fn fts_index_present(r: &impl LinReader) -> bool {
    if !r.has_collection("chunks") {
        return false;
    }
    matches!(
        fts_status(r).ok().flatten(),
        Some(FtsState {
            backend: FtsBackend::DuckDbBm25,
            ..
        })
    )
}

/// Best-effort probe that the FTS index is ready.
pub fn probe_ready(r: &impl LinReader) -> bool {
    fts_index_present(r)
}

/// Compatibility shim: `lin` has built-in lexical indexing without external dynamic extensions.
pub fn load_fts_extension(_r: &impl LinReader) -> bool {
    true
}

// ============================================================================
// Process-local + Zero-Heap Out-of-Core Mmap Okapi BM25 Inverted Index
// ============================================================================

struct Bm25Posting {
    doc_idx: u32,
    tf: u32,
}

struct Bm25DocMeta {
    chunk_id: String,
    doc_len: u32,
}

/// Pure in-RAM BM25 index used ONLY for `:memory:` / non-DiskV2 stores.
struct CachedBm25Index {
    generation: u64,
    stemmer: String,
    docs: Vec<Bm25DocMeta>,
    postings: HashMap<String, Vec<Bm25Posting>>,
    avgdl: f64,
}

/// Zero-heap `mmap` BM25 handle for `DiskEngineV2` stores (`0 MiB` heap at 500+ GiB scale!).
struct CachedMmapBm25 {
    stemmer: String,
    index: lin::disk::MmapBm25Index,
}

static BM25_CACHE: std::sync::LazyLock<RwLock<HashMap<usize, Arc<CachedBm25Index>>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

static BM25_MMAP_CACHE: std::sync::LazyLock<RwLock<HashMap<usize, Arc<CachedMmapBm25>>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

static BM25_MMAP_BUILD_LOCKS: std::sync::LazyLock<[std::sync::Mutex<()>; 16]> =
    std::sync::LazyLock::new(Default::default);

type Bm25ScopeKey = (usize, u64, u64, String);
static BM25_SCOPE_BITSET_CACHE: std::sync::LazyLock<RwLock<HashMap<Bm25ScopeKey, Arc<Vec<u64>>>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// Evict any process-local cached BM25 index for `store` (used in cold-benchmark resets).
pub fn clear_bm25_ram_cache_for_store(store: &Store) {
    let key = store_cache_key(store);
    if let Ok(mut cache) = BM25_CACHE.write() {
        cache.remove(&key);
    }
    if let Ok(mut mcache) = BM25_MMAP_CACHE.write() {
        mcache.remove(&key);
    }
    if let Ok(mut scache) = BM25_SCOPE_BITSET_CACHE.write() {
        scache.retain(|(k, _, _, _), _| *k != key);
    }
}

fn bm25_v3_mmap_path(db_dir: &std::path::Path, stemmer: &str) -> std::path::PathBuf {
    let safe_stemmer: String = stemmer
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    db_dir.join(format!("bm25_{safe_stemmer}.v3.mmap"))
}

fn legacy_bm25_v2_bin_path(db_dir: &std::path::Path, stemmer: &str) -> std::path::PathBuf {
    let safe_stemmer: String = stemmer
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    db_dir.join(format!("bm25_{safe_stemmer}.v2.bin"))
}

fn build_mmap_bm25_on_disk(
    r: &impl LinReader,
    db_dir: &std::path::Path,
    generation: u64,
    stemmer: &str,
) -> Result<lin::disk::MmapBm25Index> {
    let mut builder = lin::disk::MmapBm25Builder::new(generation);
    r.scan_each_projected("chunks", &["id", "content"], |row| {
        let Some(chunk_id) = cell_str(row, "id") else {
            return Ok(true);
        };
        let content = cell_str(row, "content").unwrap_or("");
        let terms = stem_and_filter_tokens(content, stemmer);
        builder.add_document(chunk_id, &terms);
        Ok(true)
    })?;
    let dst = bm25_v3_mmap_path(db_dir, stemmer);
    let idx = builder.finish_to_file(&dst)?;
    // Clean up obsolete heap-deserialized v2 file if present so disk space is reclaimed!
    let old_v2 = legacy_bm25_v2_bin_path(db_dir, stemmer);
    if old_v2.exists() {
        let _ = std::fs::remove_file(old_v2);
    }
    Ok(idx)
}

impl CachedBm25Index {
    fn build(r: &impl LinReader, generation: u64, stemmer: &str) -> Result<Self> {
        let mut docs: Vec<Bm25DocMeta> = Vec::new();
        let mut postings: HashMap<String, Vec<Bm25Posting>> = HashMap::new();
        let mut total_tokens = 0u64;

        r.scan_each_projected("chunks", &["id", "content"], |row| {
            let Some(chunk_id) = cell_str(row, "id") else {
                return Ok(true);
            };
            let content = cell_str(row, "content").unwrap_or("");
            let terms = stem_and_filter_tokens(content, stemmer);
            let doc_idx = docs.len() as u32;
            let doc_len = terms.len() as u32;
            total_tokens += u64::from(doc_len);
            docs.push(Bm25DocMeta {
                chunk_id: chunk_id.to_string(),
                doc_len,
            });

            if !terms.is_empty() {
                let mut tf_map: HashMap<String, u32> = HashMap::new();
                for t in terms {
                    *tf_map.entry(t).or_insert(0) += 1;
                }
                for (term, tf) in tf_map {
                    postings
                        .entry(term)
                        .or_default()
                        .push(Bm25Posting { doc_idx, tf });
                }
            }
            Ok(true)
        })?;

        let avgdl = if docs.is_empty() {
            1.0
        } else {
            (total_tokens as f64) / (docs.len() as f64)
        }
        .max(1e-9);

        Ok(Self {
            generation,
            stemmer: stemmer.to_string(),
            docs,
            postings,
            avgdl,
        })
    }

    /// Score all chunks matching `query_terms`, returning `(chunk_id, bm25_score)`.
    fn score_query(&self, query: &str) -> Vec<(&str, f32)> {
        if self.docs.is_empty() {
            return Vec::new();
        }
        let mut q_terms = stem_and_filter_tokens(query, &self.stemmer);
        q_terms.sort();
        q_terms.dedup();
        if q_terms.is_empty() {
            return Vec::new();
        }

        let n_docs = self.docs.len() as f64;
        let mut scores: HashMap<u32, f64> = HashMap::new();

        for term in &q_terms {
            let Some(plist) = self.postings.get(term.as_str()) else {
                continue;
            };
            let df = plist.len() as f64;
            let idf = (1.0 + (n_docs - df + 0.5) / (df + 0.5)).ln();
            if idf <= 0.0 {
                continue;
            }
            for post in plist {
                let dl = f64::from(self.docs[post.doc_idx as usize].doc_len);
                let tf = f64::from(post.tf);
                let denom = tf + BM25_K1 * (1.0 - BM25_B + BM25_B * (dl / self.avgdl));
                let contrib = idf * (tf * (BM25_K1 + 1.0)) / denom;
                *scores.entry(post.doc_idx).or_insert(0.0) += contrib;
            }
        }

        scores
            .into_iter()
            .filter(|(_, s)| *s > 0.0)
            .map(|(doc_idx, s)| (self.docs[doc_idx as usize].chunk_id.as_str(), s as f32))
            .collect()
    }
}

fn store_cache_key(store: &Store) -> usize {
    store.backend_key()
}

fn get_or_open_mmap_bm25(
    store: &Store,
    r: &impl LinReader,
    eng: &lin::disk::DiskEngine,
    generation: u64,
    stemmer: &str,
) -> Result<Option<Arc<CachedMmapBm25>>> {
    let key = store_cache_key(store);
    if let Ok(cache) = BM25_MMAP_CACHE.read()
        && let Some(existing) = cache.get(&key)
        && existing.index.generation == generation
        && existing.stemmer == stemmer
    {
        return Ok(Some(Arc::clone(existing)));
    }

    let v3_path = bm25_v3_mmap_path(eng.dir(), stemmer);
    if let Some(opened) = lin::disk::MmapBm25Index::open_if_valid(&v3_path, generation) {
        let handle = Arc::new(CachedMmapBm25 {
            stemmer: stemmer.to_string(),
            index: opened,
        });
        if let Ok(mut cache) = BM25_MMAP_CACHE.write() {
            if cache.len() > 64 {
                cache.clear();
            }
            cache.insert(key, Arc::clone(&handle));
        }
        return Ok(Some(handle));
    }

    if eng.is_read_only() {
        return Ok(None);
    }

    // Singleflight per-store build so concurrent readers never build the same `.v3.mmap` twice!
    let _build_guard = BM25_MMAP_BUILD_LOCKS[(key >> 4) & 15]
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Ok(cache) = BM25_MMAP_CACHE.read()
        && let Some(existing) = cache.get(&key)
        && existing.index.generation == generation
        && existing.stemmer == stemmer
    {
        return Ok(Some(Arc::clone(existing)));
    }
    let idx = if let Some(opened) = lin::disk::MmapBm25Index::open_if_valid(&v3_path, generation) {
        opened
    } else {
        build_mmap_bm25_on_disk(r, eng.dir(), generation, stemmer)?
    };
    let handle = Arc::new(CachedMmapBm25 {
        stemmer: stemmer.to_string(),
        index: idx,
    });
    if let Ok(mut cache) = BM25_MMAP_CACHE.write() {
        if cache.len() > 64 {
            cache.clear();
        }
        cache.insert(key, Arc::clone(&handle));
    }
    Ok(Some(handle))
}

fn get_or_build_ram_bm25_index(
    store: &Store,
    r: &impl LinReader,
    generation: u64,
    stemmer: &str,
) -> Result<Arc<CachedBm25Index>> {
    let key = store_cache_key(store);
    if let Ok(cache) = BM25_CACHE.read()
        && let Some(existing) = cache.get(&key)
        && existing.generation == generation
        && existing.stemmer == stemmer
    {
        return Ok(Arc::clone(existing));
    }

    let built = Arc::new(CachedBm25Index::build(r, generation, stemmer)?);
    if let Ok(mut cache) = BM25_CACHE.write() {
        if cache.len() > 64 {
            cache.clear();
        }
        cache.insert(key, Arc::clone(&built));
    }
    Ok(built)
}

/// Persist the zero-heap `mmap` Okapi BM25 inverted index on disk (`<db_dir>/bm25_<stemmer>.v3.mmap`) if backed by `DiskEngineV2`.
pub fn persist_bm25_disk_cache(store: &Store, stemmer: &str) -> Result<bool> {
    let r = store.read()?;
    let Some(eng) = r.disk_v2() else {
        return Ok(false);
    };
    if eng.is_read_only() {
        return Ok(false);
    }
    let clean_stemmer = sanitize_stemmer(stemmer);
    let generation = chunks_generation(&r)?;
    let v3_path = bm25_v3_mmap_path(eng.dir(), &clean_stemmer);
    let idx = if let Some(valid) = lin::disk::MmapBm25Index::open_if_valid(&v3_path, generation) {
        let old_v2 = legacy_bm25_v2_bin_path(eng.dir(), &clean_stemmer);
        if old_v2.exists() {
            let _ = std::fs::remove_file(old_v2);
        }
        valid
    } else {
        build_mmap_bm25_on_disk(&r, eng.dir(), generation, &clean_stemmer)?
    };
    let handle = Arc::new(CachedMmapBm25 {
        stemmer: clean_stemmer,
        index: idx,
    });
    if let Ok(mut cache) = BM25_MMAP_CACHE.write() {
        cache.insert(store_cache_key(store), handle);
    }
    Ok(true)
}

/// Evict any process-local BM25 handle for `store` so the next lexical query reopens from `mmap` or rebuilds.
pub fn evict_bm25_ram_cache(store: &Store) {
    clear_bm25_ram_cache_for_store(store);
}

/// Lexical search over chunks: Okapi BM25 when active, else TF fallback.
pub fn search_bm25(
    store: &Store,
    query: &str,
    top_k: usize,
    filters: &LexFilters,
) -> Result<Vec<SearchHit>> {
    search_bm25_with_stemmer(store, query, top_k, filters, None)
}

/// Like [`search_bm25`], using `stemmer` when refreshing a stale index.
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

    // Fast concurrent read check first! If the index is already clean and stemmer matches,
    // we never even acquire the write lock!
    let (state, generation) = {
        let r = store.read()?;
        let preferred = stemmer
            .map(sanitize_stemmer)
            .or_else(|| get_meta(&r, META_STEMMER).ok().flatten())
            .unwrap_or_else(|| DEFAULT_STEMMER.to_string());
        let gen_state = fts_generation_state(&r)?;
        let cur_status = fts_status(&r)?;
        if !gen_state.dirty
            && let Some(ref st) = cur_status
            && st.stemmer == preferred
        {
            (st.clone(), gen_state.chunks_generation)
        } else {
            drop(r);
            // Upgrade to singleflight write lock only when dirty or uninitialized
            match store.write() {
                Ok(mut w) => {
                    let st = refresh_fts_if_stale_with_stemmer(&mut w, &preferred)?;
                    let g = chunks_generation(&w)?;
                    (st, g)
                }
                Err(_) => {
                    // Read-only store (`open_read_only`): score directly without mutating meta!
                    let r2 = store.read()?;
                    let g = chunks_generation(&r2)?;
                    let st = cur_status.unwrap_or(FtsState {
                        backend: FtsBackend::DuckDbBm25,
                        stemmer: preferred,
                    });
                    (st, g)
                }
            }
        }
    };

    let r = store.read()?;
    match state.backend {
        FtsBackend::DuckDbBm25 => {
            search_lin_bm25(store, &r, query, top_k, filters, generation, &state.stemmer)
        }
        FtsBackend::TermFrequency => search_tf(&r, query, top_k, filters),
    }
}

const DOC_BM25_META_COLS: &[&str] = &[
    "id",
    "title",
    "uri",
    "status",
    "wing",
    "room",
    "layer",
    "kind",
    "source_file",
];
const DOC_BM25_FILTER_COLS: &[&str] = &[
    "id",
    "status",
    "wing",
    "room",
    "layer",
    "kind",
    "uri",
    "source_file",
];
const CHUNK_BM25_HYDRATE_COLS: &[&str] = &[
    "id",
    "document_id",
    "chunk_index",
    "content",
    "char_start",
    "char_end",
];

fn lex_scope_cache_signature(filters: &LexFilters) -> String {
    fn trim_opt(v: Option<&str>) -> &str {
        v.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("")
    }
    format!(
        "d={}\x1fw={}\x1fr={}\x1fl={}\x1fk={}\x1fu={}\x1fs={}\x1fa={}",
        trim_opt(filters.document_id.as_deref()),
        trim_opt(filters.wing.as_deref()),
        trim_opt(filters.room.as_deref()),
        trim_opt(filters.layer.as_deref()),
        trim_opt(filters.kind.as_deref()),
        trim_opt(filters.uri.as_deref()),
        trim_opt(filters.source_file.as_deref()),
        u8::from(filters.include_archived),
    )
}

fn collect_matching_doc_ids_for_lex(
    r: &impl LinReader,
    filters: &LexFilters,
) -> Result<HashSet<String>> {
    fn trim_opt(v: Option<&str>) -> Option<&str> {
        v.map(str::trim).filter(|s| !s.is_empty())
    }
    let want_id = trim_opt(filters.document_id.as_deref());
    let want_wing = trim_opt(filters.wing.as_deref());
    let want_room = trim_opt(filters.room.as_deref());
    let want_layer = trim_opt(filters.layer.as_deref());
    let want_kind = trim_opt(filters.kind.as_deref());
    let want_uri = trim_opt(filters.uri.as_deref());
    let want_src = trim_opt(filters.source_file.as_deref());

    let mut out = HashSet::new();
    if let Some(id) = want_id {
        if let Some(doc_row) = r.get_projected_by_id("documents", id, DOC_BM25_FILTER_COLS)?
            && filters.matches_document_row(&doc_row)
        {
            out.insert(id.to_string());
        }
        return Ok(out);
    }

    let eq_preds: [(&str, Option<&str>); 6] = [
        ("uri", want_uri),
        ("source_file", want_src),
        ("room", want_room),
        ("kind", want_kind),
        ("layer", want_layer),
        ("wing", want_wing),
    ];
    let active_count = eq_preds.iter().filter(|(_, v)| v.is_some()).count();

    // Zero-`Row`, zero-Zstd fast path when filtering by a single indexed column!
    if active_count == 1
        && let Some(&(field, Some(val))) = eq_preds.iter().find(|(_, v)| v.is_some())
    {
        let include_archived = filters.include_archived;
        r.for_each_by_eq_str3(
            "documents",
            field,
            val,
            "id",
            "status",
            None,
            |id_opt, st_opt, _| {
                let Some(id) = id_opt else {
                    return Ok(true);
                };
                if !include_archived {
                    let st = st_opt.filter(|s| !s.is_empty()).unwrap_or("active");
                    if st == "archived" || st == "tombstone" {
                        return Ok(true);
                    }
                }
                out.insert(id.to_string());
                Ok(true)
            },
        )?;
        return Ok(out);
    }

    if let Some(&(field, Some(val))) = eq_preds.iter().find(|(_, v)| v.is_some()) {
        r.for_each_by_eq_projected("documents", field, val, Some(DOC_BM25_FILTER_COLS), |row| {
            if filters.matches_document_row(row)
                && let Some(id) = cell_str(row, "id")
            {
                out.insert(id.to_string());
            }
            Ok(true)
        })?;
        return Ok(out);
    }

    r.scan_each_projected("documents", DOC_BM25_FILTER_COLS, |row| {
        if filters.matches_document_row(row)
            && let Some(id) = cell_str(row, "id")
        {
            out.insert(id.to_string());
        }
        Ok(true)
    })?;
    Ok(out)
}

fn get_or_build_bm25_scope_bitset(
    store: &Store,
    r: &impl LinReader,
    eng: &lin::disk::DiskEngine,
    bm25: &lin::disk::MmapBm25Index,
    bm25_gen: u64,
    filters: &LexFilters,
) -> Result<Option<Arc<Vec<u64>>>> {
    let key = (
        store_cache_key(store),
        eng.manifest.generation,
        bm25_gen,
        lex_scope_cache_signature(filters),
    );
    if let Ok(cache) = BM25_SCOPE_BITSET_CACHE.read()
        && let Some(existing) = cache.get(&key)
    {
        return Ok(Some(Arc::clone(existing)));
    }
    let allowed_docs = collect_matching_doc_ids_for_lex(r, filters)?;
    if allowed_docs.is_empty() {
        let empty = Arc::new(Vec::new());
        if let Ok(mut cache) = BM25_SCOPE_BITSET_CACHE.write() {
            if cache.len() >= 32 {
                cache.clear();
            }
            cache.insert(key, Arc::clone(&empty));
        }
        return Ok(Some(empty));
    }
    let Some(bits) = eng.build_bm25_allow_bitset("chunks", "document_id", bm25, |did| {
        allowed_docs.contains(did)
    })?
    else {
        return Ok(None);
    };
    let arc_bits = Arc::new(bits);
    if let Ok(mut cache) = BM25_SCOPE_BITSET_CACHE.write() {
        if cache.len() >= 32 {
            cache.clear();
        }
        cache.insert(key, Arc::clone(&arc_bits));
    }
    Ok(Some(arc_bits))
}

fn hydrate_bm25_hits(
    r: &impl LinReader,
    raw_scored: Vec<(&str, f32)>,
    top_k: usize,
    filters: &LexFilters,
) -> Result<Vec<SearchHit>> {
    if raw_scored.is_empty() || top_k == 0 {
        return Ok(Vec::new());
    }
    let mut hits = Vec::with_capacity(top_k.min(raw_scored.len()));
    let mut doc_filter_cache: HashMap<String, Option<(String, String)>> = HashMap::new();
    let scoped = filters.has_explicit_scope();

    for (chunk_id, score) in raw_scored {
        if scoped && r.disk_v2().is_some() {
            let Some(cid_row) = r.get_projected_by_id("chunks", chunk_id, &["document_id"])? else {
                continue;
            };
            let doc_id = cell_string(&cid_row, "document_id");
            let doc_info = if let Some(cached) = doc_filter_cache.get(&doc_id) {
                cached.clone()
            } else {
                let info =
                    match r.get_projected_by_id("documents", &doc_id, DOC_BM25_FILTER_COLS)? {
                        Some(f_row) if filters.matches_document_row(&f_row) => r
                            .get_projected_by_id("documents", &doc_id, &["title", "uri"])?
                            .map(|m_row| {
                                (cell_string(&m_row, "title"), cell_string(&m_row, "uri"))
                            }),
                        _ => None,
                    };
                doc_filter_cache.insert(doc_id.clone(), info.clone());
                info
            };
            let Some((doc_title, doc_uri)) = doc_info else {
                continue;
            };
            let Some(chunk_row) =
                r.get_projected_by_id("chunks", chunk_id, CHUNK_BM25_HYDRATE_COLS)?
            else {
                continue;
            };
            let content = cell_string(&chunk_row, "content");
            let snippet = make_snippet(&content, SNIPPET_CHARS);
            hits.push(SearchHit {
                chunk_id: chunk_id.to_string(),
                document_id: doc_id,
                document_title: doc_title,
                document_uri: doc_uri,
                chunk_index: cell_i64(&chunk_row, "chunk_index", 0) as i32,
                content,
                score,
                score_lex: Some(score),
                snippet: Some(snippet),
                char_start: Some(cell_i64(&chunk_row, "char_start", 0) as i32),
                char_end: Some(cell_i64(&chunk_row, "char_end", 0) as i32),
                ..Default::default()
            });
            if hits.len() >= top_k {
                break;
            }
            continue;
        }

        let Some(chunk_row) = r.get_projected_by_id("chunks", chunk_id, CHUNK_BM25_HYDRATE_COLS)?
        else {
            continue;
        };
        let doc_id = cell_string(&chunk_row, "document_id");
        let doc_info = if let Some(cached) = doc_filter_cache.get(&doc_id) {
            cached.clone()
        } else {
            let info = match r.get_projected_by_id("documents", &doc_id, DOC_BM25_META_COLS)? {
                Some(doc_row) if filters.matches_document_row(&doc_row) => {
                    Some((cell_string(&doc_row, "title"), cell_string(&doc_row, "uri")))
                }
                _ => None,
            };
            doc_filter_cache.insert(doc_id.clone(), info.clone());
            info
        };

        let Some((doc_title, doc_uri)) = doc_info else {
            continue;
        };

        let content = cell_string(&chunk_row, "content");
        let snippet = make_snippet(&content, SNIPPET_CHARS);
        hits.push(SearchHit {
            chunk_id: chunk_id.to_string(),
            document_id: doc_id,
            document_title: doc_title,
            document_uri: doc_uri,
            chunk_index: cell_i64(&chunk_row, "chunk_index", 0) as i32,
            content,
            score,
            score_lex: Some(score),
            snippet: Some(snippet),
            char_start: Some(cell_i64(&chunk_row, "char_start", 0) as i32),
            char_end: Some(cell_i64(&chunk_row, "char_end", 0) as i32),
            ..Default::default()
        });
        if hits.len() >= top_k {
            break;
        }
    }

    Ok(hits)
}

fn search_lin_bm25(
    store: &Store,
    r: &impl LinReader,
    query: &str,
    top_k: usize,
    filters: &LexFilters,
    generation: u64,
    stemmer: &str,
) -> Result<Vec<SearchHit>> {
    if top_k == 0 {
        return Ok(Vec::new());
    }
    if let Some(eng) = r.disk_v2()
        && let Some(mmap_handle) = get_or_open_mmap_bm25(store, r, eng, generation, stemmer)?
    {
        let mut q_terms = stem_and_filter_tokens(query, stemmer);
        q_terms.sort();
        q_terms.dedup();
        if q_terms.is_empty() {
            return Ok(Vec::new());
        }

        // Ultra-fast pre-filtered scoped BM25: `97 KiB` `doc_idx` bitset inside posting varint loop!
        if filters.has_explicit_scope()
            && let Some(scope_bits) = get_or_build_bm25_scope_bitset(
                store,
                r,
                eng,
                &mmap_handle.index,
                generation,
                filters,
            )?
        {
            if scope_bits.is_empty() {
                return Ok(Vec::new());
            }
            let scored_idxs = mmap_handle.index.score_terms_filtered_top_k(
                &q_terms,
                BM25_K1,
                BM25_B,
                Some(top_k),
                Some(scope_bits.as_slice()),
            );
            if scored_idxs.is_empty() {
                return Ok(Vec::new());
            }
            let raw_scored: Vec<(&str, f32)> = scored_idxs
                .into_iter()
                .filter_map(|(doc_idx, score)| {
                    mmap_handle.index.doc_id(doc_idx).map(|id| (id, score))
                })
                .collect();
            return hydrate_bm25_hits(r, raw_scored, top_k, filters);
        }

        // Progressive O(N) `select_nth` windowing for global queries
        let initial_window = if filters.has_explicit_scope() {
            (top_k.saturating_mul(24)).max(512)
        } else {
            (top_k.saturating_mul(2)).max(24)
        };
        let scored_idxs =
            mmap_handle
                .index
                .score_terms_top_k(&q_terms, BM25_K1, BM25_B, Some(initial_window));
        if scored_idxs.is_empty() {
            return Ok(Vec::new());
        }
        let exhausted_window = scored_idxs.len() >= initial_window;
        let raw_scored: Vec<(&str, f32)> = scored_idxs
            .into_iter()
            .filter_map(|(doc_idx, score)| mmap_handle.index.doc_id(doc_idx).map(|id| (id, score)))
            .collect();
        let hits = hydrate_bm25_hits(r, raw_scored, top_k, filters)?;
        if hits.len() >= top_k || !exhausted_window {
            return Ok(hits);
        }

        // Rare unaligned fallback: rank full candidate list
        let mut full_idxs = mmap_handle
            .index
            .score_terms_top_k(&q_terms, BM25_K1, BM25_B, None);
        full_idxs.sort_unstable_by(|a, b| {
            b.1.total_cmp(&a.1).then_with(|| {
                match (mmap_handle.index.doc_id(a.0), mmap_handle.index.doc_id(b.0)) {
                    (Some(sa), Some(sb)) => sa.cmp(sb),
                    _ => a.0.cmp(&b.0),
                }
            })
        });
        let full_raw: Vec<(&str, f32)> = full_idxs
            .into_iter()
            .filter_map(|(doc_idx, score)| mmap_handle.index.doc_id(doc_idx).map(|id| (id, score)))
            .collect();
        return hydrate_bm25_hits(r, full_raw, top_k, filters);
    }

    let index = get_or_build_ram_bm25_index(store, r, generation, stemmer)?;
    let mut raw_scored = index.score_query(query);
    if raw_scored.is_empty() {
        return Ok(Vec::new());
    }
    raw_scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    hydrate_bm25_hits(r, raw_scored, top_k, filters)
}

// ============================================================================
// Term-frequency fallback
// ============================================================================

fn search_tf(
    r: &impl LinReader,
    query: &str,
    top_k: usize,
    filters: &LexFilters,
) -> Result<Vec<SearchHit>> {
    let query_terms = tokenize(query);
    if query_terms.is_empty() || top_k == 0 {
        return Ok(Vec::new());
    }

    let mut allowed_docs: HashMap<String, (String, String)> = HashMap::new();
    r.scan_each_projected(
        "documents",
        &[
            "id",
            "title",
            "uri",
            "wing",
            "room",
            "layer",
            "kind",
            "source_file",
            "status",
        ],
        |doc_row| {
            if filters.matches_document_row(doc_row)
                && let Some(id) = cell_str(doc_row, "id")
            {
                allowed_docs.insert(
                    id.to_string(),
                    (cell_string(doc_row, "title"), cell_string(doc_row, "uri")),
                );
            }
            Ok(true)
        },
    )?;

    if allowed_docs.is_empty() {
        return Ok(Vec::new());
    }

    let mut best: std::collections::BinaryHeap<std::cmp::Reverse<TfCandidate>> =
        std::collections::BinaryHeap::with_capacity(top_k.saturating_add(1));
    let mut ordinal = 0usize;

    r.scan_each_projected(
        "chunks",
        &[
            "id",
            "document_id",
            "chunk_index",
            "content",
            "char_start",
            "char_end",
        ],
        |chunk_row| {
            let Some(doc_id) = cell_str(chunk_row, "document_id") else {
                return Ok(true);
            };
            let Some((doc_title, doc_uri)) = allowed_docs.get(doc_id) else {
                return Ok(true);
            };
            ordinal += 1;
            let content = cell_str(chunk_row, "content").unwrap_or("");
            let score = term_frequency_score(&query_terms, content);
            if score <= 0.0 {
                return Ok(true);
            }
            let rank = (score, ordinal);
            if best.len() >= top_k && best.peek().is_some_and(|worst| !worst.0.ranks_below(rank)) {
                return Ok(true);
            }
            let content_owned = content.to_string();
            let snippet = make_snippet(&content_owned, SNIPPET_CHARS);
            let hit = SearchHit {
                chunk_id: cell_string(chunk_row, "id"),
                document_id: doc_id.to_string(),
                document_title: doc_title.clone(),
                document_uri: doc_uri.clone(),
                chunk_index: cell_i64(chunk_row, "chunk_index", 0) as i32,
                content: content_owned,
                score,
                score_lex: Some(score),
                snippet: Some(snippet),
                char_start: Some(cell_i64(chunk_row, "char_start", 0) as i32),
                char_end: Some(cell_i64(chunk_row, "char_end", 0) as i32),
                ..Default::default()
            };
            best.push(std::cmp::Reverse(TfCandidate {
                score,
                ordinal,
                hit,
            }));
            if best.len() > top_k {
                best.pop();
            }
            Ok(true)
        },
    )?;

    let mut ranked: Vec<TfCandidate> = best.into_iter().map(|entry| entry.0).collect();
    ranked.sort_by(|a, b| b.cmp(a));
    Ok(ranked.into_iter().map(|c| c.hit).collect())
}

struct TfCandidate {
    score: f32,
    ordinal: usize,
    hit: SearchHit,
}

impl TfCandidate {
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

// ============================================================================
// Porter Stemmer & Stopword Filtering
// ============================================================================

fn stem_and_filter_tokens(text: &str, stemmer: &str) -> Vec<String> {
    let raw = tokenize(text);
    if stemmer == "none" {
        return raw;
    }
    raw.into_iter()
        .filter(|tok| ENGLISH_STOPWORDS.binary_search(&tok.as_str()).is_err())
        .map(|tok| {
            if tok.is_ascii() && tok.bytes().all(|b| b.is_ascii_lowercase()) {
                porter_stem(&tok)
            } else {
                tok
            }
        })
        .filter(|tok| !tok.is_empty())
        .collect()
}

fn is_consonant(bytes: &[u8], i: usize) -> bool {
    match bytes[i] {
        b'a' | b'e' | b'i' | b'o' | b'u' => false,
        b'y' => {
            if i == 0 {
                true
            } else {
                !is_consonant(bytes, i - 1)
            }
        }
        _ => true,
    }
}

fn porter_measure(stem: &str) -> usize {
    let b = stem.as_bytes();
    let n = b.len();
    let mut i = 0;
    while i < n && is_consonant(b, i) {
        i += 1;
    }
    let mut m = 0;
    while i < n {
        while i < n && !is_consonant(b, i) {
            i += 1;
        }
        if i >= n {
            break;
        }
        m += 1;
        while i < n && is_consonant(b, i) {
            i += 1;
        }
    }
    m
}

fn has_vowel(stem: &str) -> bool {
    let b = stem.as_bytes();
    (0..b.len()).any(|i| !is_consonant(b, i))
}

fn ends_double_consonant(stem: &str) -> bool {
    let b = stem.as_bytes();
    let n = b.len();
    n >= 2 && b[n - 1] == b[n - 2] && is_consonant(b, n - 1)
}

fn ends_cvc(stem: &str) -> bool {
    let b = stem.as_bytes();
    let n = b.len();
    if n < 3 {
        return false;
    }
    is_consonant(b, n - 1)
        && !is_consonant(b, n - 2)
        && is_consonant(b, n - 3)
        && !matches!(b[n - 1], b'w'..=b'y')
}

/// Classic Martin Porter (1980) English stemmer for ASCII lowercase words.
pub fn porter_stem(word: &str) -> String {
    if word.len() <= 2 || !word.is_ascii() {
        return word.to_string();
    }
    let mut s = word.to_string();

    // Step 1a
    if s.ends_with("sses") || s.ends_with("ies") {
        s.truncate(s.len() - 2);
    } else if !s.ends_with("ss") && s.ends_with('s') {
        s.pop();
    }

    // Step 1b
    let mut step1b_extra = false;
    if let Some(stem) = s.strip_suffix("eed") {
        if porter_measure(stem) > 0 {
            s.pop(); // -> "ee"
        }
    } else if let Some(stem) = s.strip_suffix("ed") {
        if has_vowel(stem) {
            s = stem.to_string();
            step1b_extra = true;
        }
    } else if let Some(stem) = s.strip_suffix("ing")
        && has_vowel(stem)
    {
        s = stem.to_string();
        step1b_extra = true;
    }

    if step1b_extra {
        if s.ends_with("at") || s.ends_with("bl") || s.ends_with("iz") {
            s.push('e');
        } else if ends_double_consonant(&s)
            && !matches!(s.as_bytes().last(), Some(b'l' | b's' | b'z'))
        {
            s.pop();
        } else if porter_measure(&s) == 1 && ends_cvc(&s) {
            s.push('e');
        }
    }

    // Step 1c
    if let Some(stem) = s.strip_suffix('y')
        && has_vowel(stem)
    {
        s.pop();
        s.push('i');
    }

    // Step 2
    const STEP2: &[(&str, &str)] = &[
        ("ational", "ate"),
        ("tional", "tion"),
        ("enci", "ence"),
        ("anci", "ance"),
        ("izer", "ize"),
        ("bli", "ble"),
        ("alli", "al"),
        ("entli", "ent"),
        ("eli", "e"),
        ("ousli", "ous"),
        ("ization", "ize"),
        ("ation", "ate"),
        ("ator", "ate"),
        ("alism", "al"),
        ("iveness", "ive"),
        ("fulness", "ful"),
        ("ousness", "ous"),
        ("aliti", "al"),
        ("iviti", "ive"),
        ("biliti", "ble"),
        ("logi", "log"),
    ];
    for &(suf, rep) in STEP2 {
        if let Some(stem) = s.strip_suffix(suf) {
            if porter_measure(stem) > 0 {
                s = format!("{stem}{rep}");
            }
            break;
        }
    }

    // Step 3
    const STEP3: &[(&str, &str)] = &[
        ("icate", "ic"),
        ("ative", ""),
        ("alize", "al"),
        ("iciti", "ic"),
        ("ical", "ic"),
        ("ful", ""),
        ("ness", ""),
    ];
    for &(suf, rep) in STEP3 {
        if let Some(stem) = s.strip_suffix(suf) {
            if porter_measure(stem) > 0 {
                s = format!("{stem}{rep}");
            }
            break;
        }
    }

    // Step 4
    const STEP4: &[&str] = &[
        "al", "ance", "ence", "er", "ic", "able", "ible", "ant", "ement", "ment", "ent", "ion",
        "ou", "ism", "ate", "iti", "ous", "ive", "ize",
    ];
    for &suf in STEP4 {
        if let Some(stem) = s.strip_suffix(suf) {
            if suf == "ion" {
                if stem.ends_with('s') || stem.ends_with('t') {
                    if porter_measure(stem) > 1 {
                        s = stem.to_string();
                    }
                    break;
                }
            } else {
                if porter_measure(stem) > 1 {
                    s = stem.to_string();
                }
                break;
            }
        }
    }

    // Step 5a
    if let Some(stem) = s.strip_suffix('e') {
        let m = porter_measure(stem);
        if m > 1 || (m == 1 && !ends_cvc(stem)) {
            s.pop();
        }
    }

    // Step 5b
    if s.ends_with("ll") && porter_measure(&s) > 1 {
        s.pop();
    }

    s
}

// ============================================================================
// Metadata Helpers & Test Fault Injection
// ============================================================================

fn persist_state(w: &mut impl LinWriter, state: &FtsState) -> Result<()> {
    upsert_meta(w, META_BACKEND, state.backend.as_str())?;
    upsert_meta(w, META_STEMMER, &state.stemmer)?;
    Ok(())
}

fn read_u64_meta(r: &impl LinReader, key: &str) -> Result<Option<u64>> {
    let Some(raw) = get_meta(r, key)? else {
        return Ok(None);
    };
    match raw.parse::<u64>() {
        Ok(value) => Ok(Some(value)),
        Err(e) => {
            tracing::warn!(meta_key = key, value = %raw, error = %e, "invalid FTS generation metadata");
            Ok(None)
        }
    }
}

fn increment_rebuild_count(w: &mut impl LinWriter) -> Result<u64> {
    let current = read_u64_meta(w, META_REBUILD_COUNT)?.unwrap_or(0);
    let next = current
        .checked_add(1)
        .ok_or_else(|| AppError::fts("FTS rebuild counter overflow"))?;
    upsert_meta(w, META_REBUILD_COUNT, &next.to_string())?;
    Ok(next)
}

/// Allow only ascii alpha / underscore stemmer names.
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
#[cfg(any(test, feature = "testkit"))]
#[doc(hidden)]
pub mod test_fault {
    use crate::error::{AppError, Result};

    const FAIL_PREFIX: &str = "__test_fail_finalize_fts__:";
    const PANIC_PREFIX: &str = "__test_panic_finalize_fts__:";

    pub fn fail(site: &str) -> String {
        format!("{FAIL_PREFIX}{site}")
    }

    pub fn panic(site: &str) -> String {
        format!("{PANIC_PREFIX}{site}")
    }

    pub fn check(site: &str, stemmer: &str) -> Result<()> {
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
mod tests {
    use super::*;
    use crate::models::{Chunk, Document};
    use chrono::Utc;
    use tempfile::TempDir;

    fn open_store() -> (TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fts.lin");
        let store = Store::open(&path).expect("open");
        (dir, store)
    }

    fn state(chunks: u64, index: Option<u64>, dirty: bool) -> FtsGenerationState {
        FtsGenerationState {
            chunks_generation: chunks,
            index_generation: index,
            dirty,
            rebuild_count: 0,
        }
    }

    #[test]
    fn lexical_and_vector_legs_share_one_trimmed_scope() {
        let filters = LexFilters {
            wing: Some(" rag ".into()),
            room: Some("   ".into()),
            document_id: Some(" doc-1".into()),
            ..LexFilters::default()
        };
        let (lexical_sql, lexical_bind) = filters.scope_where_clause("c.document_id");
        let (vector_sql, vector_bind) = filters.scope_where_clause("d.id");
        assert_eq!(lexical_bind, vec!["doc-1".to_string(), "rag".to_string()]);
        assert_eq!(lexical_bind, vector_bind);
        assert!(lexical_sql.contains("c.document_id = ?") && vector_sql.contains("d.id = ?"));
        assert!(
            !lexical_sql.contains("d.room"),
            "blank scope values are ignored"
        );
    }

    #[test]
    fn stale_threshold_policy() {
        assert!(!fts_stale_needs_reindex(&state(10, Some(10), false), 1));
        assert!(!fts_stale_needs_reindex(&state(100, Some(0), true), 0));
        assert!(!fts_stale_needs_reindex(&state(70, Some(10), true), 64));
        assert!(fts_stale_needs_reindex(&state(74, Some(10), true), 64));
        assert!(fts_stale_needs_reindex(&state(1, None, true), 64));
        assert_eq!(fts_generation_gap(&state(5, Some(9), false)), Some(0));
        assert_eq!(fts_generation_gap(&state(5, None, true)), None);
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
        store.fts_generation_state().unwrap()
    }

    #[test]
    fn tokenize_splits_and_lowercases() {
        assert_eq!(
            tokenize("Hello, WORLD! cat-nap 42"),
            vec!["hello", "world", "cat", "nap", "42"]
        );
    }

    #[test]
    fn porter_stemmer_conflates_english_inflections() {
        assert_eq!(porter_stem("cats"), porter_stem("cat"));
        assert_eq!(porter_stem("running"), porter_stem("run"));
        assert_eq!(porter_stem("runners"), porter_stem("runner"));
        assert_eq!(porter_stem("jumping"), porter_stem("jumps"));
    }

    #[test]
    fn term_frequency_ranks_relevant_higher() {
        let q = tokenize("cat mammal");
        let a = term_frequency_score(&q, "The domestic cat is a small carnivorous mammal.");
        let b = term_frequency_score(&q, "Dogs are domesticated descendants of the wolf.");
        assert!(a > b, "expected cat/mammal doc higher: {a} vs {b}");
        assert!((a - 2.0).abs() < 1e-6);
        assert_eq!(b, 0.0);
    }

    #[test]
    fn ensure_and_reindex_are_idempotent() {
        let store = Store::open_in_memory().unwrap();
        let mut conn = store.write().unwrap();
        let s1 = ensure_fts(&mut conn).expect("ensure");
        let s2 = reindex(&mut conn).expect("reindex");
        assert_eq!(s1.backend, s2.backend);
        assert_eq!(s1.stemmer, s2.stemmer);
        let status = fts_status(&conn).unwrap().expect("status");
        assert_eq!(status.backend, s1.backend);
        assert!(fts_index_present(&conn));
    }

    #[test]
    fn clean_warm_search_does_not_rebuild_fts() {
        let (_dir, store) = open_store();
        seed_docs(&store);
        {
            let mut conn = store.write().unwrap();
            ensure_fts(&mut conn).unwrap();
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
        assert_eq!(warm.rebuild_count, initialized.rebuild_count);
    }

    #[test]
    fn reopened_clean_index_searches_without_rebuild() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fts-reopen.lin");
        let before = {
            let store = Store::open(&path).unwrap();
            seed_docs(&store);
            {
                let mut conn = store.write().unwrap();
                ensure_fts(&mut conn).unwrap();
            }
            lifecycle(&store)
        };

        let reopened = Store::open(&path).unwrap();
        let hits = search_bm25(&reopened, "cat", 10, &LexFilters::default()).unwrap();
        assert!(!hits.is_empty());
        let after = lifecycle(&reopened);
        assert_eq!(after.rebuild_count, before.rebuild_count);
        assert!(!after.dirty);
    }

    #[test]
    fn dirty_term_frequency_backend_advances_without_fts_ddl_retry() {
        let store = Store::open_in_memory().unwrap();
        let mut conn = store.write().unwrap();
        let fallback = FtsState {
            backend: FtsBackend::TermFrequency,
            stemmer: DEFAULT_STEMMER.into(),
        };
        publish_refreshed_state(&mut conn, &fallback, 0).unwrap();
        mark_fts_dirty(&mut conn).unwrap();
        let before = fts_generation_state(&conn).unwrap();
        assert!(before.dirty);
        assert!(!fts_index_present(&conn));

        let refreshed = refresh_fts_if_stale(&mut conn).unwrap();
        assert_eq!(refreshed.backend, FtsBackend::TermFrequency);
        let after = fts_generation_state(&conn).unwrap();
        assert!(!after.dirty);
        assert_eq!(after.rebuild_count, before.rebuild_count + 1);
        assert!(!fts_index_present(&conn));
    }

    #[test]
    fn mutation_marks_dirty_and_controlled_refresh_restores_freshness() {
        let (_dir, store) = open_store();
        seed_docs(&store);
        {
            let mut conn = store.write().unwrap();
            ensure_fts(&mut conn).unwrap();
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
            let mut conn = store.write().unwrap();
            refresh_fts_if_stale(&mut conn).unwrap();
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
        assert_eq!(lifecycle(&store).rebuild_count, refreshed.rebuild_count);
    }

    #[test]
    fn concurrent_stale_searches_singleflight_one_refresh() {
        let (_dir, store) = open_store();
        seed_docs(&store);
        {
            let mut conn = store.write().unwrap();
            ensure_fts(&mut conn).unwrap();
            mark_fts_dirty(&mut conn).unwrap();
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
            let mut conn = store.write().unwrap();
            ensure_fts(&mut conn).unwrap();
            reindex(&mut conn).unwrap();
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
            let mut conn = store.write().unwrap();
            ensure_fts(&mut conn).unwrap();
            reindex(&mut conn).unwrap();
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
        assert!(
            search_bm25(&store, "  ", 5, &LexFilters::default())
                .unwrap()
                .is_empty()
        );
        assert!(
            search_bm25(&store, "cat", 0, &LexFilters::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn stemmer_none_ensure_succeeds() {
        let store = Store::open_in_memory().unwrap();
        let mut conn = store.write().unwrap();
        let state = ensure_fts_with_stemmer(&mut conn, "none").unwrap();
        assert_eq!(state.stemmer, "none");
    }

    #[test]
    fn disk_v2_mmap_bm25_replaces_legacy_v2_bin_and_vacuum_compacts_segments() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("rag.lin");
        let store = Store::open(&db_path).unwrap();
        seed_docs(&store);

        // Simulate a stale legacy v2 heap cache file on disk.
        let legacy_bin = db_path.join("bm25_porter.v2.bin");
        std::fs::write(&legacy_bin, b"stale-legacy-bm25-heap-blob").unwrap();
        assert!(legacy_bin.exists());

        assert!(persist_bm25_disk_cache(&store, "porter").unwrap());
        let mmap_path = db_path.join("bm25_porter.v3.mmap");
        assert!(
            mmap_path.exists(),
            "expected bm25_porter.v3.mmap to be written"
        );
        assert!(
            !legacy_bin.exists(),
            "expected stale bm25_porter.v2.bin to be deleted"
        );

        // Evict handle and verify cold mmap open + BM25 search returns exact matches.
        evict_bm25_ram_cache(&store);
        let hits = search_bm25(&store, "feline mammal", 5, &LexFilters::default()).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].chunk_id, "c1");

        // Delete a document, checkpoint, and verify vacuum_store compacts tombstones cleanly.
        assert!(store.delete_document("d2").unwrap());
        let vac = store.vacuum_store().unwrap();
        assert!(vac.bytes_after.unwrap_or(0) > 0);
        let stats = store.stats().unwrap();
        assert_eq!(stats.0, 2);
    }
}
