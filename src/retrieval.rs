//! Transport-independent retrieval use cases shared by MCP and HTTP adapters.

use serde::Serialize;

use crate::db::search::{
    search, ContextExpansion, DiversityMode, SearchQuery, DEFAULT_RRF_K, MAX_QUERY_CHARS,
    MAX_TOP_K,
};
use crate::db::Store;
use crate::embeddings::{l2_normalize, EmbeddingProvider};
use crate::error::AppError;
use crate::models::{Chunk, Document, SearchHit, SearchMode};

pub const MAX_MULTI_GET_DOCUMENTS: usize = 100;
pub const MAX_NEIGHBOR_RADIUS: u32 = 20;

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
    command: SearchCommand,
) -> Result<Vec<SearchHit>, AppError> {
    let query = prepare_search(embedder, command).await?;
    search(store, &query)
}

pub async fn prepare_search(
    embedder: &dyn EmbeddingProvider,
    command: SearchCommand,
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
    let mode = match nonempty(command.mode) {
        Some(raw) => SearchMode::parse(&raw).map_err(AppError::config)?,
        None => command.default_mode,
    };
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
    if command
        .rrf_k
        .is_some_and(|k| !k.is_finite() || k <= 0.0)
    {
        return Err(AppError::config("rrf_k must be finite and greater than zero"));
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
    pub chunks: Option<Vec<Chunk>>,
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
) -> Result<DocumentWithChunks, AppError> {
    let document_id = document_id.trim();
    let document = store
        .get_document(document_id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;
    let chunks = include_chunks
        .then(|| store.list_chunks_for_document(&document.id))
        .transpose()?;
    Ok(DocumentWithChunks { document, chunks })
}

pub fn multi_get(
    store: &Store,
    document_ids: Vec<String>,
    include_chunks: bool,
) -> Result<MultiGetResult, AppError> {
    if document_ids.is_empty() || document_ids.len() > MAX_MULTI_GET_DOCUMENTS {
        return Err(AppError::config(format!(
            "document_ids must contain 1..={MAX_MULTI_GET_DOCUMENTS} ids"
        )));
    }

    let mut documents = Vec::with_capacity(document_ids.len());
    let mut missing = Vec::new();
    for requested_id in document_ids {
        let document_id = requested_id.trim();
        let Some(document) = store.get_document(document_id)? else {
            missing.push(requested_id);
            continue;
        };
        let chunks = include_chunks
            .then(|| store.list_chunks_for_document(&document.id))
            .transpose()?;
        documents.push(DocumentWithChunks { document, chunks });
    }
    Ok(MultiGetResult { documents, missing })
}

pub fn expand_chunks(
    store: &Store,
    document_id: &str,
    chunk_index: i32,
    radius: u32,
) -> Result<Vec<Chunk>, AppError> {
    let radius = radius.min(MAX_NEIGHBOR_RADIUS) as i32;
    let start = chunk_index.saturating_sub(radius);
    let end = chunk_index.saturating_add(radius);
    let chunks = store
        .list_chunks_for_document(document_id.trim())?
        .into_iter()
        .filter(|chunk| chunk.chunk_index >= start && chunk.chunk_index <= end)
        .collect::<Vec<_>>();
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
    query: SimilarDocumentsQuery,
) -> Result<Vec<SearchHit>, AppError> {
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
    let mut hits = search(
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
    )?;
    hits.retain(|hit| hit.document_id != document_id);
    hits.truncate(top_k);
    Ok(hits)
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|item| !item.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbedder;

    fn command(query: &str) -> SearchCommand {
        SearchCommand {
            query: query.into(), mode: Some("lex".into()), default_mode: SearchMode::Hybrid,
            top_k: None, default_top_k: 7, document_id: Some(" ".into()), wing: Some(" rag ".into()),
            room: None, layer: None, source_file: None, include_archived: false, min_score: None,
            diversity: None, group_by: Some("document".into()), recency_half_life_days: None,
            max_context_tokens: Some(500), max_chunks_per_document: Some(2),
            context_expansion: Some("neighbors".into()), neighbor_chunks: Some(3), timeout_ms: None,
            fts_stemmer: "porter".into(), rrf_k: None,
        }
    }

    #[tokio::test]
    async fn search_command_preserves_adapter_defaults_and_normalizes_filters() {
        let query = prepare_search(&MockEmbedder::new(8), command("  architecture  ")).await.unwrap();
        assert_eq!(query.mode, SearchMode::Lex);
        assert_eq!(query.top_k, 7);
        assert_eq!(query.query_text.as_deref(), Some("architecture"));
        assert_eq!(query.wing.as_deref(), Some(" rag "));
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
    }
}
