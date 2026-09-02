//! Transport-independent retrieval use cases shared by MCP and HTTP adapters.

use duckdb::params;
use serde::Serialize;

use crate::config::Config;
use crate::db::search::{
    acquire_corpus_search_guard, search_with_corpus_guard, ContextExpansion, DiversityMode,
    SearchQuery, DEFAULT_RRF_K, MAX_QUERY_CHARS, MAX_TOP_K,
};
use crate::db::Store;
use crate::embeddings::{l2_normalize, EmbeddingProvider};
use crate::error::AppError;
use crate::models::{Document, SearchHit, SearchMode};

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

pub async fn execute_search(
    store: &Store,
    embedder: &dyn EmbeddingProvider,
    config: &Config,
    command: SearchCommand,
) -> Result<Vec<SearchHit>, AppError> {
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
    let query = prepare_search_with_mode(embedder, command, mode).await?;
    search_with_corpus_guard(store, &query, &corpus_guard)
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
        return Err(AppError::config("search query must not be empty"));
    }
    if text.chars().count() > MAX_QUERY_CHARS {
        return Err(AppError::config(format!(
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
                return Err(AppError::config(format!(
                    "invalid group_by '{other}': expected document or none"
                )))
            }
        }
    }
    if command
        .recency_half_life_days
        .is_some_and(|days| !days.is_finite() || days <= 0.0)
    {
        return Err(AppError::config(
            "recency_half_life_days must be finite and greater than zero",
        ));
    }
    let context_expansion = command
        .context_expansion
        .as_deref()
        .map(ContextExpansion::parse)
        .transpose()?;
    if command.rrf_k.is_some_and(|k| !k.is_finite() || k <= 0.0) {
        return Err(AppError::config(
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
        document_id: nonempty(command.document_id),
        wing: nonempty(command.wing),
        room: nonempty(command.room),
        layer: nonempty(command.layer),
        source_file: nonempty(command.source_file),
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

/// Public document-read chunk projection. Embedding vectors are deliberately
/// excluded from transport responses.
#[derive(Debug, Serialize)]
pub struct ChunkText {
    pub id: String,
    pub chunk_index: i32,
    pub content: String,
    pub char_start: i32,
    pub char_end: i32,
}

struct ChunkTextPage {
    items: Vec<ChunkText>,
    total: u64,
    content_bytes: usize,
}

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
        return Err(AppError::config(format!(
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

/// Read a bounded text-only page under one connection lock. The SQL byte
/// preflight runs before any chunk content is materialized and excludes stored
/// embedding vectors from both memory and the response model.
fn list_chunk_text_page(
    store: &Store,
    document_id: &str,
    limit: usize,
    max_content_bytes: usize,
) -> Result<ChunkTextPage, AppError> {
    let conn = store.lock()?;
    let total: i64 = conn.query_row(
        "SELECT COUNT(*)::BIGINT FROM chunks WHERE document_id = ?",
        [document_id],
        |row| row.get(0),
    )?;
    let limit = limit.clamp(1, MAX_INCLUDED_CHUNKS);
    let content_bytes: i64 = conn.query_row(
        r#"
        SELECT COALESCE(SUM(octet_length(encode(content))), 0)::BIGINT
        FROM (
            SELECT content
            FROM chunks
            WHERE document_id = ?
            ORDER BY chunk_index, id
            LIMIT ?
        ) selected
        "#,
        params![document_id, i64::try_from(limit).unwrap_or(i64::MAX)],
        |row| row.get(0),
    )?;
    let content_bytes = usize::try_from(content_bytes.max(0)).unwrap_or(usize::MAX);
    if content_bytes > max_content_bytes {
        return Err(AppError::config(format!(
            "included chunk text needs {content_bytes} bytes, exceeding the {max_content_bytes}-byte response budget; lower chunk_limit or set include_chunks=false"
        )));
    }

    let mut statement = conn.prepare(
        r#"
        SELECT id, chunk_index, content, char_start, char_end
        FROM chunks
        WHERE document_id = ?
        ORDER BY chunk_index, id
        LIMIT ?
        "#,
    )?;
    let mut rows = statement.query(params![
        document_id,
        i64::try_from(limit).unwrap_or(i64::MAX)
    ])?;
    let mut items = Vec::with_capacity(limit.min(total.max(0) as usize));
    while let Some(row) = rows.next()? {
        items.push(ChunkText {
            id: row.get(0)?,
            chunk_index: row.get(1)?,
            content: row.get(2)?,
            char_start: row.get(3)?,
            char_end: row.get(4)?,
        });
    }
    Ok(ChunkTextPage {
        items,
        total: total.max(0) as u64,
        content_bytes,
    })
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
    let conn = store.lock()?;
    let content_bytes: i64 = conn.query_row(
        r#"
        SELECT COALESCE(SUM(octet_length(encode(content))), 0)::BIGINT
        FROM (
            SELECT content
            FROM chunks
            WHERE document_id = ? AND chunk_index BETWEEN ? AND ?
            ORDER BY chunk_index, id
            LIMIT ?
        ) selected
        "#,
        params![
            document_id.trim(),
            start,
            end,
            i64::try_from(limit).unwrap_or(i64::MAX)
        ],
        |row| row.get(0),
    )?;
    if usize::try_from(content_bytes.max(0)).unwrap_or(usize::MAX)
        > MAX_INCLUDED_CHUNK_CONTENT_BYTES
    {
        return Err(AppError::config(format!(
            "expanded chunk text exceeds the {MAX_INCLUDED_CHUNK_CONTENT_BYTES}-byte response budget; lower radius"
        )));
    }
    let mut statement = conn.prepare(
        r#"
        SELECT id, chunk_index, content, char_start, char_end
        FROM chunks
        WHERE document_id = ? AND chunk_index BETWEEN ? AND ?
        ORDER BY chunk_index, id
        LIMIT ?
        "#,
    )?;
    let mut rows = statement.query(params![
        document_id.trim(),
        start,
        end,
        i64::try_from(limit).unwrap_or(i64::MAX)
    ])?;
    let mut chunks = Vec::with_capacity(limit);
    while let Some(row) = rows.next()? {
        chunks.push(ChunkText {
            id: row.get(0)?,
            chunk_index: row.get(1)?,
            content: row.get(2)?,
            char_start: row.get(3)?,
            char_end: row.get(4)?,
        });
    }
    if chunks.is_empty() {
        return Err(AppError::not_found(format!(
            "no chunks around document_id={} chunk_index={chunk_index}",
            document_id.trim()
        )));
    }
    Ok(chunks)
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
        return Err(AppError::config(
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
            wing: nonempty(query.wing),
            room: nonempty(query.room),
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

fn nonempty(value: Option<String>) -> Option<String> {
    value.and_then(|item| {
        let trimmed = item.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
