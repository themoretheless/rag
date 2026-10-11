//! Transport-independent retrieval use cases shared by MCP and HTTP adapters.

use serde::Serialize;

use crate::config::Config;
use crate::db::search::{
    acquire_corpus_search_guard, attach_context, fuse_rrf_many, search_with_corpus_guard,
    ContextExpansion, DiversityMode, SearchQuery, DEFAULT_RRF_K, MAX_QUERY_CHARS, MAX_TOP_K,
};
use crate::db::Store;
use crate::embeddings::{l2_normalize, EmbeddingProvider};
use crate::error::AppError;
use crate::models::{Document, SearchHit, SearchMode};
use crate::search_pack::{pack_hits, PackedContext};
use crate::util::{non_blank, trimmed_non_blank};

pub const MAX_MULTI_GET_DOCUMENTS: usize = 100;
pub const MAX_NEIGHBOR_RADIUS: u32 = 20;
/// Default number of text-only chunks returned by an explicit `include_chunks` read.
pub const DEFAULT_INCLUDED_CHUNKS: usize = 100;
/// Hard per-document cap for text-only chunks returned by document reads.
pub const MAX_INCLUDED_CHUNKS: usize = 500;
/// Hard aggregate chunk-content budget for one document or multi-get response.
pub const MAX_INCLUDED_CHUNK_CONTENT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SearchCommand {
    pub query: String,
    pub mode: Option<String>,
    pub default_mode: SearchMode,
    pub top_k: Option<usize>,
    pub default_top_k: usize,
    pub document_id: Option<String>,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub layer: Option<String>,
    pub source_file: Option<String>,
    pub include_archived: bool,
    pub min_score: Option<f32>,
    pub diversity: Option<String>,
    pub group_by: Option<String>,
    pub recency_half_life_days: Option<f64>,
    pub max_context_tokens: Option<usize>,
    pub max_chunks_per_document: Option<usize>,
    pub context_expansion: Option<String>,
    pub neighbor_chunks: Option<usize>,
    pub timeout_ms: Option<u64>,
    pub fts_stemmer: String,
    /// RRF constant for hybrid fusion; `None` keeps [`crate::db::search::DEFAULT_RRF_K`].
    pub rrf_k: Option<f32>,
}

/// Transport-neutral search input: what a caller sent, before defaults.
///
/// [`SearchRequest::into_command`] is the single place that drops blank
/// strings and applies the configured defaults, so MCP and HTTP resolve the
/// same request to the same [`SearchCommand`].
#[derive(Debug, Clone, Default)]
pub struct SearchRequest {
    pub query: String,
    pub mode: Option<String>,
    pub top_k: Option<usize>,
    pub document_id: Option<String>,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub layer: Option<String>,
    pub source_file: Option<String>,
    pub include_archived: bool,
    pub min_score: Option<f32>,
    pub diversity: Option<String>,
    pub group_by: Option<String>,
    pub recency_half_life_days: Option<f64>,
    pub max_context_tokens: Option<usize>,
    pub max_chunks_per_document: Option<usize>,
    pub context_expansion: Option<String>,
    pub neighbor_chunks: Option<usize>,
    pub timeout_ms: Option<u64>,
    pub rrf_k: Option<f32>,
}

/// Search deadline when the caller does not set one.
pub const DEFAULT_SEARCH_TIMEOUT_MS: u64 = 5_000;

impl SearchRequest {
    pub fn into_command(self, config: &Config) -> SearchCommand {
        SearchCommand {
            query: self.query,
            mode: non_blank(self.mode),
            default_mode: config.default_search_mode,
            top_k: self.top_k,
            default_top_k: config.default_top_k,
            document_id: non_blank(self.document_id),
            wing: non_blank(self.wing),
            room: non_blank(self.room),
            layer: non_blank(self.layer),
            source_file: non_blank(self.source_file),
            include_archived: self.include_archived,
            min_score: self.min_score,
            diversity: non_blank(self.diversity),
            group_by: non_blank(self.group_by),
            recency_half_life_days: self.recency_half_life_days,
            max_context_tokens: self.max_context_tokens.or(Some(config.max_context_tokens)),
            max_chunks_per_document: self
                .max_chunks_per_document
                .or(Some(config.max_chunks_per_doc)),
            context_expansion: non_blank(self.context_expansion),
            neighbor_chunks: self.neighbor_chunks,
            timeout_ms: self.timeout_ms.or(Some(DEFAULT_SEARCH_TIMEOUT_MS)),
            fts_stemmer: config.fts_stemmer.clone(),
            rrf_k: self.rrf_k,
        }
    }
}

pub async fn execute_search(
    store: &Store,
    embedder: &dyn EmbeddingProvider,
    config: &Config,
    command: SearchCommand,
) -> Result<Vec<SearchHit>, AppError> {
    let started = std::time::Instant::now();
    let budget = std::time::Duration::from_millis(command.timeout_ms.unwrap_or(5_000).max(1));
    let mode = resolve_search_mode(command.mode.as_deref(), command.default_mode)?;
    // Acquire before the potentially slow async embedding request. The read
    // guard both fast-fails an already-running source sync and prevents a new
    // sync from starting between the identity check, embedding, and exact
    // retrieval.
    let corpus_guard = acquire_corpus_search_guard(store, mode)?;
    if mode.needs_embedding() {
        store.ensure_embedding_manifest(config)?;
        store.require_embedding_manifest_match(config)?;
    }
    let mut query = tokio::time::timeout(budget, prepare_search_with_mode(embedder, command, mode))
        .await
        .map_err(|_| AppError::invalid("search timeout budget exceeded during embedding"))??;
    let remaining = budget
        .checked_sub(started.elapsed())
        .filter(|v| !v.is_zero())
        .ok_or_else(|| AppError::invalid("search timeout budget exceeded before retrieval"))?;
    query.timeout_ms = Some(remaining.as_millis().max(1).min(u64::MAX as u128) as u64);
    let mut hits = search_with_corpus_guard(store, &query, &corpus_guard)?;
    if started.elapsed() > budget {
        return Err(AppError::invalid(
            "search timeout budget exceeded after retrieval",
        ));
    }
    for hit in &mut hits {
        if let Some(explanation) = &mut hit.explanation {
            explanation.total_ms = started.elapsed().as_secs_f64() * 1_000.0;
        }
    }
    Ok(hits)
}

/// Upper bound on queries fused by one `multi_query_search`.
pub const MAX_MULTI_QUERIES: usize = 16;

/// Original query plus caller-supplied rewrites, fused with RRF.
#[derive(Debug, Clone, Default)]
pub struct MultiQuerySearchCommand {
    pub queries: Vec<String>,
    pub top_k: Option<usize>,
    pub mode: Option<String>,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub layer: Option<String>,
    pub source_file: Option<String>,
    pub timeout_ms: Option<u64>,
}

/// Run every query under one corpus read guard and fuse the ranked lists.
///
/// All query texts are embedded in a single provider call. No rewrite is
/// generated here; the caller supplies the variants.
pub async fn multi_query_search(
    store: &Store,
    embedder: &dyn EmbeddingProvider,
    config: &Config,
    command: MultiQuerySearchCommand,
) -> Result<Vec<SearchHit>, AppError> {
    if command.queries.is_empty() || command.queries.len() > MAX_MULTI_QUERIES {
        return Err(AppError::invalid(format!(
            "queries must contain 1..={MAX_MULTI_QUERIES} entries"
        )));
    }
    let top_k = command.top_k.unwrap_or(config.default_top_k);
    let mode = resolve_search_mode(command.mode.as_deref(), config.default_search_mode)?;
    let corpus_guard = acquire_corpus_search_guard(store, mode)?;
    let embeddings = if mode.needs_embedding() {
        store.ensure_embedding_manifest(config)?;
        store.require_embedding_manifest_match(config)?;
        let vectors = embedder.embed(&command.queries).await?;
        if vectors.len() != command.queries.len() {
            return Err(AppError::embeddings(format!(
                "embedder returned {} vectors for {} queries",
                vectors.len(),
                command.queries.len()
            )));
        }
        vectors.into_iter().map(Some).collect()
    } else {
        vec![None; command.queries.len()]
    };
    let wing = non_blank(command.wing);
    let room = non_blank(command.room);
    let layer = non_blank(command.layer);
    let source_file = non_blank(command.source_file);
    let mut lists = Vec::with_capacity(command.queries.len());
    for (text, embedding) in command.queries.into_iter().zip(embeddings) {
        lists.push(search_with_corpus_guard(
            store,
            &SearchQuery {
                mode,
                top_k,
                query_text: Some(text),
                query_embedding: embedding,
                wing: wing.clone(),
                room: room.clone(),
                layer: layer.clone(),
                source_file: source_file.clone(),
                timeout_ms: command.timeout_ms.or(Some(DEFAULT_SEARCH_TIMEOUT_MS)),
                fts_stemmer: config.fts_stemmer.clone(),
                ..SearchQuery::default()
            },
            &corpus_guard,
        )?);
    }
    Ok(fuse_rrf_many(&lists, DEFAULT_RRF_K, top_k))
}

pub async fn prepare_search(
    embedder: &dyn EmbeddingProvider,
    command: SearchCommand,
) -> Result<SearchQuery, AppError> {
    let mode = resolve_search_mode(command.mode.as_deref(), command.default_mode)?;
    prepare_search_with_mode(embedder, command, mode).await
}

pub(crate) fn resolve_search_mode(
    requested: Option<&str>,
    default_mode: SearchMode,
) -> Result<SearchMode, AppError> {
    match requested.map(str::trim).filter(|mode| !mode.is_empty()) {
        Some(raw) => SearchMode::parse(raw).map_err(AppError::config),
        None => Ok(default_mode),
    }
}

async fn prepare_search_with_mode(
    embedder: &dyn EmbeddingProvider,
    command: SearchCommand,
    mode: SearchMode,
) -> Result<SearchQuery, AppError> {
    let text = command.query.trim();
    if text.is_empty() {
        return Err(AppError::invalid("search query must not be empty"));
    }
    if text.chars().count() > MAX_QUERY_CHARS {
        return Err(AppError::invalid(format!(
            "query exceeds {MAX_QUERY_CHARS} characters"
        )));
    }
    let mut diversity = command
        .diversity
        .as_deref()
        .map(DiversityMode::parse)
        .transpose()?;
    if let Some(group_by) = command
        .group_by
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        match group_by.to_ascii_lowercase().as_str() {
            "document" | "document_id" => diversity = Some(DiversityMode::CollapseByDocument),
            "none" => {}
            other => {
                return Err(AppError::invalid(format!(
                    "invalid group_by '{other}': expected document or none"
                )))
            }
        }
    }
    if command
        .recency_half_life_days
        .is_some_and(|days| !days.is_finite() || days <= 0.0)
    {
        return Err(AppError::invalid(
            "recency_half_life_days must be finite and greater than zero",
        ));
    }
    let context_expansion = command
        .context_expansion
        .as_deref()
        .map(ContextExpansion::parse)
        .transpose()?;
    if command.rrf_k.is_some_and(|k| !k.is_finite() || k <= 0.0) {
        return Err(AppError::invalid(
            "rrf_k must be finite and greater than zero",
        ));
    }
    let embed_started = std::time::Instant::now();
    let query_embedding = if matches!(mode, SearchMode::Vec | SearchMode::Hybrid) {
        embedder
            .embed(&[text.to_owned()])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| AppError::embeddings("embedder returned no vector"))
            .map(Some)?
    } else {
        None
    };
    let embed_ms = query_embedding
        .as_ref()
        .map(|_| embed_started.elapsed().as_secs_f64() * 1_000.0);
    Ok(SearchQuery {
        mode,
        top_k: command.top_k.unwrap_or(command.default_top_k),
        query_text: Some(text.to_owned()),
        query_embedding,
        document_id: trimmed_non_blank(command.document_id),
        wing: trimmed_non_blank(command.wing),
        room: trimmed_non_blank(command.room),
        layer: trimmed_non_blank(command.layer),
        source_file: trimmed_non_blank(command.source_file),
        include_archived: command.include_archived,
        min_score: command.min_score,
        diversity,
        max_chunks_per_document: command.max_chunks_per_document,
        recency_half_life_days: command.recency_half_life_days,
        max_context_tokens: command.max_context_tokens,
        context_expansion,
        neighbor_chunks: command.neighbor_chunks.unwrap_or(1),
        timeout_ms: command.timeout_ms.or(Some(5_000)),
        fts_stemmer: command.fts_stemmer,
        rrf_k: command.rrf_k.unwrap_or(DEFAULT_RRF_K),
        embed_ms,
        ..SearchQuery::default()
    })
}

#[derive(Debug, Serialize)]
pub struct DocumentWithChunks {
    pub document: Document,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Vec<ChunkText>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks_truncated: Option<bool>,
}

pub use crate::db::chunk_text::ChunkText;
use crate::db::chunk_text::{chunk_text_page, ChunkTextPage};

#[derive(Debug, Serialize)]
pub struct MultiGetResult {
    pub documents: Vec<DocumentWithChunks>,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SimilarDocumentsQuery {
    pub document_id: String,
    pub top_k: usize,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub fts_stemmer: String,
}

pub fn get_document(
    store: &Store,
    document_id: &str,
    include_chunks: bool,
    chunk_limit: Option<usize>,
) -> Result<DocumentWithChunks, AppError> {
    let document_id = document_id.trim();
    let document = store
        .get_document(document_id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;
    document_with_chunks(store, document, include_chunks, chunk_limit)
}

pub(crate) fn document_with_chunks(
    store: &Store,
    document: Document,
    include_chunks: bool,
    chunk_limit: Option<usize>,
) -> Result<DocumentWithChunks, AppError> {
    let page = include_chunks
        .then(|| {
            list_chunk_text_page(
                store,
                &document.id,
                included_chunk_limit(chunk_limit),
                MAX_INCLUDED_CHUNK_CONTENT_BYTES,
            )
        })
        .transpose()?;
    Ok(with_chunk_page(document, page))
}

pub fn multi_get(
    store: &Store,
    document_ids: Vec<String>,
    include_chunks: bool,
    chunk_limit: Option<usize>,
) -> Result<MultiGetResult, AppError> {
    if document_ids.is_empty() || document_ids.len() > MAX_MULTI_GET_DOCUMENTS {
        return Err(AppError::invalid(format!(
            "document_ids must contain 1..={MAX_MULTI_GET_DOCUMENTS} ids"
        )));
    }

    let mut documents = Vec::with_capacity(document_ids.len());
    let mut missing = Vec::new();
    let chunk_limit = included_chunk_limit(chunk_limit);
    let mut remaining_chunk_bytes = MAX_INCLUDED_CHUNK_CONTENT_BYTES;
    for requested_id in document_ids {
        let document_id = requested_id.trim();
        let Some(document) = store.get_document(document_id)? else {
            missing.push(requested_id);
            continue;
        };
        let page = include_chunks
            .then(|| list_chunk_text_page(store, &document.id, chunk_limit, remaining_chunk_bytes))
            .transpose()?;
        if let Some(page) = &page {
            remaining_chunk_bytes = remaining_chunk_bytes.saturating_sub(page.content_bytes);
        }
        documents.push(with_chunk_page(document, page));
    }
    Ok(MultiGetResult { documents, missing })
}

fn included_chunk_limit(requested: Option<usize>) -> usize {
    requested
        .unwrap_or(DEFAULT_INCLUDED_CHUNKS)
        .clamp(1, MAX_INCLUDED_CHUNKS)
}

fn with_chunk_page(document: Document, page: Option<ChunkTextPage>) -> DocumentWithChunks {
    match page {
        Some(page) => DocumentWithChunks {
            document,
            chunks_truncated: Some((page.items.len() as u64) < page.total),
            chunks_total: Some(page.total),
            chunks: Some(page.items),
        },
        None => DocumentWithChunks {
            document,
            chunks: None,
            chunks_total: None,
            chunks_truncated: None,
        },
    }
}

/// Read a bounded text-only page of one document's chunks.
fn list_chunk_text_page(
    store: &Store,
    document_id: &str,
    limit: usize,
    max_content_bytes: usize,
) -> Result<ChunkTextPage, AppError> {
    chunk_text_page(
        store,
        document_id,
        None,
        limit.clamp(1, MAX_INCLUDED_CHUNKS),
        max_content_bytes,
        |content_bytes| {
            AppError::invalid(format!(
                "included chunk text needs {content_bytes} bytes, exceeding the {max_content_bytes}-byte response budget; lower chunk_limit or set include_chunks=false"
            ))
        },
    )
}

pub fn expand_chunks(
    store: &Store,
    document_id: &str,
    chunk_index: i32,
    radius: u32,
) -> Result<Vec<ChunkText>, AppError> {
    let radius = radius.min(MAX_NEIGHBOR_RADIUS) as i32;
    let start = chunk_index.saturating_sub(radius);
    let end = chunk_index.saturating_add(radius);
    let limit = usize::try_from(radius)
        .unwrap_or(MAX_NEIGHBOR_RADIUS as usize)
        .saturating_mul(2)
        .saturating_add(1);
    let chunks = chunk_text_page(
        store,
        document_id.trim(),
        Some((start, end)),
        limit,
        MAX_INCLUDED_CHUNK_CONTENT_BYTES,
        |_| {
            AppError::invalid(format!(
                "expanded chunk text exceeds the {MAX_INCLUDED_CHUNK_CONTENT_BYTES}-byte response budget; lower radius"
            ))
        },
    )?
    .items;
    if chunks.is_empty() {
        return Err(AppError::not_found(format!(
            "no chunks around document_id={} chunk_index={chunk_index}",
            document_id.trim()
        )));
    }
    Ok(chunks)
}

/// `pack_context` use case shared by MCP and HTTP.
///
/// Owns the defaults (one neighbour chunk, `RAG_MAX_CONTEXT_TOKENS`) and the
/// blank-expansion rule so both transports pack identically.
#[derive(Debug, Clone, Default)]
pub struct PackContextCommand {
    pub hits: Vec<SearchHit>,
    pub max_tokens: Option<usize>,
    pub context_expansion: Option<String>,
    pub neighbor_chunks: Option<usize>,
}

/// Packed block plus the expanded hits it was cut from (trace provenance).
pub struct PackContextOutcome {
    pub expanded_hits: Vec<SearchHit>,
    pub packed: PackedContext,
}

/// Expand hits with neighbouring/parent context and pack them under budget.
/// Blocking: reads chunks from the store.
pub fn pack_context(
    store: &Store,
    config: &Config,
    command: PackContextCommand,
) -> Result<PackContextOutcome, AppError> {
    let expansion = non_blank(command.context_expansion)
        .as_deref()
        .map(ContextExpansion::parse)
        .transpose()?;
    let mut hits = command.hits;
    attach_context(
        store,
        &mut hits,
        expansion,
        command.neighbor_chunks.unwrap_or(1),
    )?;
    let packed = pack_hits(
        &hits,
        command.max_tokens.unwrap_or(config.max_context_tokens),
    );
    Ok(PackContextOutcome {
        expanded_hits: hits,
        packed,
    })
}

pub fn find_similar(
    store: &Store,
    config: &Config,
    query: SimilarDocumentsQuery,
) -> Result<Vec<SearchHit>, AppError> {
    let corpus_guard = acquire_corpus_search_guard(store, SearchMode::Vec)?;
    store.require_embedding_manifest_match(config)?;
    let document_id = query.document_id.trim();
    let document = store
        .get_document(document_id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;
    let chunks = store.list_chunks_for_document(document_id)?;
    let dims = chunks
        .first()
        .map(|chunk| chunk.embedding.len())
        .unwrap_or(0);
    if dims == 0 {
        return Err(AppError::invalid(
            "seed document has no chunk embeddings; run doctor_repair first",
        ));
    }

    let mut centroid = vec![0.0f32; dims];
    for chunk in chunks.iter().filter(|chunk| chunk.embedding.len() == dims) {
        for (dst, value) in centroid.iter_mut().zip(&chunk.embedding) {
            *dst += *value;
        }
    }
    l2_normalize(&mut centroid);

    let top_k = query.top_k.clamp(1, MAX_TOP_K.saturating_sub(1));
    let mut hits = search_with_corpus_guard(
        store,
        &SearchQuery {
            mode: SearchMode::Vec,
            top_k: top_k + 1,
            query_text: Some(document.title),
            query_embedding: Some(centroid),
            wing: trimmed_non_blank(query.wing),
            room: trimmed_non_blank(query.room),
            diversity: Some(DiversityMode::CollapseByDocument),
            max_chunks_per_document: Some(1),
            fts_stemmer: query.fts_stemmer,
            ..SearchQuery::default()
        },
        &corpus_guard,
    )?;
    hits.retain(|hit| hit.document_id != document_id);
    hits.truncate(top_k);
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_request_drops_blank_scope_and_applies_config_defaults() {
        let config = Config::for_tests();
        let command = SearchRequest {
            query: "q".into(),
            mode: Some("  ".into()),
            wing: Some("".into()),
            room: Some("src".into()),
            diversity: Some(" ".into()),
            ..SearchRequest::default()
        }
        .into_command(&config);
        assert_eq!(command.mode, None);
        assert_eq!(command.wing, None);
        assert_eq!(command.room.as_deref(), Some("src"));
        assert_eq!(command.diversity, None);
        assert_eq!(command.timeout_ms, Some(DEFAULT_SEARCH_TIMEOUT_MS));
        assert_eq!(command.max_context_tokens, Some(config.max_context_tokens));
        assert_eq!(command.max_chunks_per_document, Some(config.max_chunks_per_doc));
    }
    use crate::embeddings::MockEmbedder;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingEmbedder {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl EmbeddingProvider for CountingEmbedder {
        async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(vec![vec![1.0, 0.0]; texts.len()])
        }

        fn dimensions(&self) -> usize {
            2
        }
    }

    fn command(query: &str) -> SearchCommand {
        SearchCommand {
            query: query.into(),
            mode: Some("lex".into()),
            default_mode: SearchMode::Hybrid,
            top_k: None,
            default_top_k: 7,
            document_id: Some(" ".into()),
            wing: Some(" rag ".into()),
            room: None,
            layer: None,
            source_file: None,
            include_archived: false,
            min_score: None,
            diversity: None,
            group_by: Some("document".into()),
            recency_half_life_days: None,
            max_context_tokens: Some(500),
            max_chunks_per_document: Some(2),
            context_expansion: Some("neighbors".into()),
            neighbor_chunks: Some(3),
            timeout_ms: None,
            fts_stemmer: "porter".into(),
            rrf_k: None,
        }
    }

    #[tokio::test]
    async fn search_command_preserves_adapter_defaults_and_normalizes_filters() {
        let query = prepare_search(&MockEmbedder::new(8), command("  architecture  "))
            .await
            .unwrap();
        assert_eq!(query.mode, SearchMode::Lex);
        assert_eq!(query.top_k, 7);
        assert_eq!(query.query_text.as_deref(), Some("architecture"));
        assert_eq!(query.wing.as_deref(), Some("rag"));
        assert!(query.document_id.is_none());
        assert_eq!(query.diversity, Some(DiversityMode::CollapseByDocument));
        assert_eq!(query.context_expansion, Some(ContextExpansion::Neighbors));
        assert!(query.query_embedding.is_none());
    }

    #[tokio::test]
    async fn search_command_validates_before_embedding() {
        let embedder = MockEmbedder::new(8);
        assert!(prepare_search(&embedder, command("   ")).await.is_err());
        let mut invalid = command("valid");
        invalid.recency_half_life_days = Some(0.0);
        assert!(prepare_search(&embedder, invalid).await.is_err());
        let mut invalid = command("valid");
        invalid.group_by = Some("chunk".into());
        assert!(prepare_search(&embedder, invalid).await.is_err());
        let mut invalid = command("valid");
        invalid.rrf_k = Some(0.0);
        assert!(prepare_search(&embedder, invalid).await.is_err());
        let mut invalid = command("valid");
        invalid.rrf_k = Some(f32::NAN);
        assert!(prepare_search(&embedder, invalid).await.is_err());
    }

    #[tokio::test]
    async fn active_source_sync_rejects_hybrid_before_embedding_provider_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("pre-embed-busy.duckdb")).expect("open");
        let sync_guard = store.corpus_mutation_lane().write_owned().await;
        let embedder = CountingEmbedder {
            calls: AtomicUsize::new(0),
        };
        let config = Config {
            db_path: store.path().to_path_buf(),
            ..Config::for_tests()
        };
        let mut request = command("needle");
        request.mode = Some("hybrid".into());

        let error = execute_search(&store, &embedder, &config, request)
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::Busy(_)));
        assert_eq!(embedder.calls.load(Ordering::Relaxed), 0);
        drop(sync_guard);
    }

    #[tokio::test]
    async fn multi_query_search_embeds_all_rewrites_in_one_provider_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("multi-query.duckdb")).expect("open");
        let embedder = CountingEmbedder {
            calls: AtomicUsize::new(0),
        };
        let config = Config {
            db_path: store.path().to_path_buf(),
            embedding_dims: 2,
            ..Config::for_tests()
        };
        let hits = multi_query_search(
            &store,
            &embedder,
            &config,
            MultiQuerySearchCommand {
                queries: vec!["alpha".into(), "beta".into(), "gamma".into()],
                mode: Some("vec".into()),
                ..MultiQuerySearchCommand::default()
            },
        )
        .await
        .expect("empty corpus still answers");
        assert!(hits.is_empty());
        assert_eq!(embedder.calls.load(Ordering::Relaxed), 1);

        let too_many = MultiQuerySearchCommand {
            queries: vec!["q".into(); MAX_MULTI_QUERIES + 1],
            ..MultiQuerySearchCommand::default()
        };
        assert!(multi_query_search(&store, &embedder, &config, too_many)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn search_timeout_cancels_embedding_and_releases_the_corpus_lane() {
        struct NeverReturns;
        #[async_trait]
        impl EmbeddingProvider for NeverReturns {
            async fn embed(&self, _: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
                std::future::pending().await
            }
            fn dimensions(&self) -> usize {
                8
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("deadline.duckdb")).unwrap();
        let config = Config {
            embedding_dims: 8,
            ..Config::for_tests()
        };
        let mut request = command("needle");
        request.mode = Some("vec".into());
        request.timeout_ms = Some(20);
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            execute_search(&store, &NeverReturns, &config, request),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("timeout budget exceeded during embedding"));
        assert!(store.corpus_mutation_lane().try_write_owned().is_ok());
    }
}
