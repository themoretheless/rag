//! Response shapes shared by the document-reading tool clusters.

use serde::Serialize;

use crate::retrieval::{self, DocumentWithChunks};

#[derive(Debug, Serialize)]
pub(super) struct ChunkView {
    id: String,
    chunk_index: i32,
    content: String,
    char_start: i32,
    char_end: i32,
}

impl From<retrieval::ChunkText> for ChunkView {
    fn from(chunk: retrieval::ChunkText) -> Self {
        Self {
            id: chunk.id,
            chunk_index: chunk.chunk_index,
            content: chunk.content,
            char_start: chunk.char_start,
            char_end: chunk.char_end,
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct DocumentDetail {
    id: String,
    uri: String,
    title: String,
    content: String,
    metadata_json: String,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_download_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks: Option<Vec<ChunkView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks_truncated: Option<bool>,
}

impl From<DocumentWithChunks> for DocumentDetail {
    fn from(value: DocumentWithChunks) -> Self {
        let document = value.document;
        let source_download_path = document
            .source_file
            .as_ref()
            .map(|_| format!("/v1/source-file?document_id={}", document.id));
        Self {
            id: document.id,
            uri: document.uri,
            title: document.title,
            content: document.content,
            metadata_json: document.metadata_json,
            created_at: document.created_at.to_rfc3339(),
            updated_at: document.updated_at.to_rfc3339(),
            source_file: document.source_file,
            source_download_path,
            chunks_total: value.chunks_total,
            chunks_truncated: value.chunks_truncated,
            chunks: value
                .chunks
                .map(|chunks| chunks.into_iter().map(ChunkView::from).collect()),
        }
    }
}

/// Full raw-layer source for `get_source`.
#[derive(Debug, Serialize)]
pub(super) struct SourceDetail {
    id: String,
    uri: String,
    title: String,
    content: String,
    metadata_json: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    room: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_download_path: Option<String>,
    layer: String,
    kind: String,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks: Option<Vec<ChunkView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks_truncated: Option<bool>,
}

impl From<DocumentWithChunks> for SourceDetail {
    fn from(value: DocumentWithChunks) -> Self {
        let document = value.document;
        let source_download_path = document
            .source_file
            .as_ref()
            .map(|_| format!("/v1/source-file?document_id={}", document.id));
        Self {
            id: document.id,
            uri: document.uri,
            title: document.title,
            content: document.content,
            metadata_json: document.metadata_json,
            content_hash: document.content_hash,
            wing: document.wing,
            room: document.room,
            source_file: document.source_file,
            source_download_path,
            layer: document.layer,
            kind: document.kind,
            created_at: document.created_at.to_rfc3339(),
            updated_at: document.updated_at.to_rfc3339(),
            chunks_total: value.chunks_total,
            chunks_truncated: value.chunks_truncated,
            chunks: value
                .chunks
                .map(|chunks| chunks.into_iter().map(ChunkView::from).collect()),
        }
    }
}
