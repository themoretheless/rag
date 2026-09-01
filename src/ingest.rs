//! Application service for document ingestion and embedding refresh.

use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::chunking::{code_chunks, from_config, markdown_section_metadata, Chunker};
use crate::config::Config;
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::graph::rebuild_document_graph;
use crate::models::{Chunk, Document, DocumentMetaUpdate, IngestResult, OpsLogEntry};
use crate::util::content_hash;

#[derive(Debug, Clone)]
pub struct IngestCommand {
    pub text: String,
    pub title: Option<String>,
    pub uri: Option<String>,
    pub metadata_json: Option<String>,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub source_file: Option<String>,
    pub layer: String,
    pub kind: String,
    pub immutable: bool,
}

#[derive(Debug, Serialize)]
pub struct ReembedDocumentResult {
    pub document_id: String,
    pub chunk_count: usize,
    pub dims: i32,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct UpdateDocumentCommand {
    pub document_id: String,
    pub update: DocumentMetaUpdate,
}

#[derive(Debug, Serialize)]
pub struct UpdateDocumentResult {
    pub document_id: String,
    pub uri: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    pub status: String,
    pub pinned: bool,
    pub boost: f64,
    pub metadata_json: String,
    pub layer: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    pub content_changed: bool,
    pub reembedded: bool,
    pub chunk_count: usize,
    pub updated_at: String,
}

pub struct IngestService<'a> {
    store: &'a Store,
    embedder: &'a Arc<dyn EmbeddingProvider>,
    config: &'a Config,
}

impl<'a> IngestService<'a> {
    pub fn new(
        store: &'a Store,
        embedder: &'a Arc<dyn EmbeddingProvider>,
        config: &'a Config,
    ) -> Self {
        Self {
            store,
            embedder,
            config,
        }
    }

    pub async fn ingest(&self, command: IngestCommand) -> Result<IngestResult, AppError> {
        self.ensure_vector_compatibility()?;
        let command = NormalizedIngest::try_from(command)?;
        let new_hash = content_hash(&command.text);
        let now = Utc::now();

        let (document_id, created_at, operation) = if let Some(existing) =
            self.store.find_by_uri(&command.uri)?
        {
            if command.immutable {
                let existing_hash = existing
                    .content_hash
                    .clone()
                    .unwrap_or_else(|| content_hash(&existing.content));
                if existing_hash == new_hash {
                    let etag = existing.etag();
                    let chunk_count = self.store.list_chunks_for_document(&existing.id)?.len();
                    let node_id = self
                        .store
                        .find_node_by_document_id(&existing.id)?
                        .map(|node| node.id)
                        .unwrap_or_default();
                    return Ok(IngestResult {
                        document_id: existing.id,
                        chunk_count,
                        node_id,
                        edge_count: 0,
                        content_hash: existing_hash,
                        op: "unchanged".into(),
                        revision: existing.revision,
                        etag,
                    });
                }
                return Err(AppError::conflict(format!(
                        "raw source uri '{}' is immutable; content differs from registered source (document_id={})",
                        command.uri, existing.id
                    )));
            }
            (existing.id, existing.created_at, "updated")
        } else {
            (Uuid::new_v4().to_string(), now, "inserted")
        };

        let document = Document {
            id: document_id.clone(),
            uri: command.uri,
            title: command.title,
            content: command.text,
            metadata_json: command.metadata_json,
            created_at,
            updated_at: now,
            wing: command.wing,
            room: command.room,
            source_file: command.source_file,
            layer: command.layer,
            kind: command.kind,
            content_hash: Some(new_hash.clone()),
            ..Default::default()
        };
        let chunks = self.build_chunks(&document).await?;
        let chunk_count = chunks.len();
        if operation == "updated" {
            self.store.delete_chunks_for_document(&document_id)?;
        }
        let revision = self.store.upsert_document_cas(&document, None)?;
        if !chunks.is_empty() {
            self.store.insert_chunks(&chunks)?;
        }
        let (node_id, edge_count) = rebuild_document_graph(self.store, &document)?;

        Ok(IngestResult {
            document_id,
            chunk_count,
            node_id,
            edge_count,
            content_hash: new_hash,
            op: operation.into(),
            revision,
            etag: crate::models::format_document_etag(revision),
        })
    }

    pub async fn reembed_document(
        &self,
        document_id: &str,
    ) -> Result<ReembedDocumentResult, AppError> {
        let document_id = document_id.trim();
        if document_id.is_empty() {
            return Err(AppError::config("document_id must be non-empty"));
        }
        let document = self
            .store
            .get_document(document_id)?
            .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;
        let mut chunks = self.store.list_chunks_for_document(&document.id)?;
        let chunk_count = chunks.len();
        if !chunks.is_empty() {
            let texts = chunks
                .iter()
                .map(|chunk| chunk.content.clone())
                .collect::<Vec<_>>();
            let embeddings = self.embedder.embed(&texts).await?;
            ensure_vector_count(embeddings.len(), chunks.len())?;
            for (chunk, embedding) in chunks.iter_mut().zip(embeddings) {
                chunk.embedding = embedding;
            }
            self.store
                .replace_chunks_for_document(&document.id, &chunks)?;
        }
        let manifest = self
            .store
            .write_embedding_manifest_from_config(self.config)?;
        Ok(ReembedDocumentResult {
            document_id: document.id,
            chunk_count,
            dims: manifest.dims,
            provider: manifest.provider,
            model: manifest.model,
        })
    }

    pub async fn update_document(
        &self,
        command: UpdateDocumentCommand,
    ) -> Result<UpdateDocumentResult, AppError> {
        let document_id = command.document_id.trim();
        if document_id.is_empty() {
            return Err(AppError::config("document_id must be non-empty"));
        }
        let applied = self
            .store
            .update_document_meta(document_id, &command.update)?
            .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;
        let mut reembedded = false;
        let mut chunk_count = self
            .store
            .list_chunks_for_document(&applied.document.id)?
            .len();

        if applied.content_changed {
            self.ensure_vector_compatibility()?;
            let chunks = self.build_plain_chunks(&applied.document).await?;
            chunk_count = chunks.len();
            self.store
                .replace_chunks_for_document(&applied.document.id, &chunks)?;
            rebuild_document_graph(self.store, &applied.document)?;
            reembedded = true;
        } else if applied.title_changed {
            if let Some(mut node) = self.store.find_node_by_document_id(&applied.document.id)? {
                node.label = applied.document.title.clone();
                self.store.upsert_graph_node(&node)?;
            }
        }

        let document = applied.document;
        let _ = self.store.append_ops_log(&OpsLogEntry {
            id: String::new(),
            seq: 0,
            ts: Utc::now(),
            op: "update_document_meta".into(),
            prefix: Some("META".into()),
            message: format!("updated document meta for {}", document.id),
            entity_id: Some(document.id.clone()),
            entity_kind: Some("document".into()),
            payload_json: serde_json::json!({
                "content_changed": applied.content_changed,
                "title_changed": applied.title_changed,
                "reembedded": reembedded,
                "wing": document.wing,
                "room": document.room,
                "status": document.status,
                "pinned": document.pinned,
                "boost": document.boost,
            })
            .to_string(),
            agent_name: None,
        });
        Ok(UpdateDocumentResult {
            document_id: document.id,
            uri: document.uri,
            title: document.title,
            wing: document.wing,
            room: document.room,
            status: document.status,
            pinned: document.pinned,
            boost: document.boost,
            metadata_json: document.metadata_json,
            layer: document.layer,
            kind: document.kind,
            source_file: document.source_file,
            content_changed: applied.content_changed,
            reembedded,
            chunk_count,
            updated_at: document.updated_at.to_rfc3339(),
        })
    }

    fn ensure_vector_compatibility(&self) -> Result<(), AppError> {
        self.store.ensure_embedding_manifest(self.config)?;
        self.store
            .require_embedding_dims_match(self.config.embedding_dims)
    }

    async fn build_chunks(&self, document: &Document) -> Result<Vec<Chunk>, AppError> {
        let is_code = serde_json::from_str::<serde_json::Value>(&document.metadata_json)
            .ok()
            .and_then(|value| value.get("language").cloned())
            .is_some();
        let pieces = if is_code {
            code_chunks(
                &document.content,
                self.config.chunk_size,
                self.config.chunk_overlap,
            )
        } else {
            let chunker = from_config(self.config.chunk_size, self.config.chunk_overlap);
            Chunker::chunk(&chunker, &document.content)
        };
        if pieces.is_empty() {
            return Ok(Vec::new());
        }
        let section_metadata = markdown_section_metadata(&document.content, &pieces);
        let texts = pieces
            .iter()
            .map(|(content, _, _)| content.clone())
            .collect::<Vec<_>>();
        let embeddings = self.embedder.embed(&texts).await?;
        ensure_vector_count(embeddings.len(), pieces.len())?;
        Ok(pieces
            .into_iter()
            .zip(embeddings)
            .zip(section_metadata)
            .enumerate()
            .map(
                |(index, (((content, char_start, char_end), embedding), metadata_json))| Chunk {
                    id: Uuid::new_v4().to_string(),
                    document_id: document.id.clone(),
                    chunk_index: index as i32,
                    content,
                    embedding,
                    char_start,
                    char_end,
                    metadata_json,
                },
            )
            .collect())
    }

    async fn build_plain_chunks(&self, document: &Document) -> Result<Vec<Chunk>, AppError> {
        let chunker = from_config(self.config.chunk_size, self.config.chunk_overlap);
        let pieces = Chunker::chunk(&chunker, &document.content);
        if pieces.is_empty() {
            return Ok(Vec::new());
        }
        let section_metadata = markdown_section_metadata(&document.content, &pieces);
        let texts = pieces
            .iter()
            .map(|(content, _, _)| content.clone())
            .collect::<Vec<_>>();
        let embeddings = self.embedder.embed(&texts).await?;
        ensure_vector_count(embeddings.len(), pieces.len())?;
        Ok(pieces
            .into_iter()
            .zip(embeddings)
            .zip(section_metadata)
            .enumerate()
            .map(
                |(index, (((content, char_start, char_end), embedding), metadata_json))| Chunk {
                    id: Uuid::new_v4().to_string(),
                    document_id: document.id.clone(),
                    chunk_index: index as i32,
                    content,
                    embedding,
                    char_start,
                    char_end,
                    metadata_json,
                },
            )
            .collect())
    }
}

struct NormalizedIngest {
    text: String,
    title: String,
    uri: String,
    metadata_json: String,
    wing: Option<String>,
    room: Option<String>,
    source_file: Option<String>,
    layer: String,
    kind: String,
    immutable: bool,
}

impl TryFrom<IngestCommand> for NormalizedIngest {
    type Error = AppError;

    fn try_from(command: IngestCommand) -> Result<Self, Self::Error> {
        let metadata_json = nonempty(command.metadata_json).unwrap_or_else(|| "{}".into());
        serde_json::from_str::<serde_json::Value>(&metadata_json).map_err(|error| {
            AppError::config(format!("metadata_json is not valid JSON: {error}"))
        })?;
        let uri = nonempty(command.uri).unwrap_or_else(|| format!("text://{}", Uuid::new_v4()));
        let title = nonempty(command.title).unwrap_or_else(|| {
            uri.rsplit('/')
                .next()
                .filter(|value| !value.is_empty())
                .unwrap_or("untitled")
                .to_string()
        });
        Ok(Self {
            text: command.text,
            title,
            uri,
            metadata_json,
            wing: nonempty(command.wing),
            room: nonempty(command.room),
            source_file: nonempty(command.source_file),
            layer: normalized_or(command.layer, "raw"),
            kind: normalized_or(command.kind, "document"),
            immutable: command.immutable,
        })
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|item| !item.trim().is_empty())
}

fn normalized_or(value: String, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn ensure_vector_count(actual: usize, expected: usize) -> Result<(), AppError> {
    if actual == expected {
        Ok(())
    } else {
        Err(AppError::embeddings(format!(
            "embedder returned {actual} vectors for {expected} chunks"
        )))
    }
}
