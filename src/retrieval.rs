//! Transport-independent retrieval use cases shared by MCP and HTTP adapters.

use serde::Serialize;

use crate::db::search::{search, DiversityMode, SearchQuery, MAX_TOP_K};
use crate::db::Store;
use crate::embeddings::l2_normalize;
use crate::error::AppError;
use crate::models::{Chunk, Document, SearchHit, SearchMode};

pub const MAX_MULTI_GET_DOCUMENTS: usize = 100;
pub const MAX_NEIGHBOR_RADIUS: u32 = 20;

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
