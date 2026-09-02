//! Pure document indexing pipeline: split, annotate, embed, and build chunks.
//!
//! This module deliberately has no database dependency. Callers finish all
//! potentially slow/fallible preparation here, then hand the resulting chunks
//! to their persistence boundary.

use uuid::Uuid;

use crate::chunking::{code_chunks, from_config, markdown_section_metadata, Chunker};
use crate::config::Config;
use crate::embeddings::EmbeddingProvider;
use crate::error::{AppError, Result};
use crate::models::{Chunk, Document};

/// Chunk contents and metadata prepared before the fallible embedding call.
///
/// Keeping this storage-free representation lets source synchronization group
/// several small documents into one provider request without duplicating the
/// chunking and annotation policy used by ordinary ingest.
pub(crate) struct PreparedDocumentChunks {
    document_id: String,
    pieces: Vec<PreparedChunk>,
}

struct PreparedChunk {
    content: String,
    char_start: i32,
    char_end: i32,
    metadata_json: String,
}

impl PreparedDocumentChunks {
    pub(crate) fn len(&self) -> usize {
        self.pieces.len()
    }

    fn into_chunks(self, embeddings: &mut impl Iterator<Item = Vec<f32>>) -> Vec<Chunk> {
        let Self {
            document_id,
            pieces,
        } = self;
        pieces
            .into_iter()
            .enumerate()
            .map(|(index, piece)| Chunk {
                id: Uuid::new_v4().to_string(),
                document_id: document_id.clone(),
                chunk_index: index as i32,
                content: piece.content,
                embedding: embeddings
                    .next()
                    .expect("embedding count was validated before chunk assembly"),
                char_start: piece.char_start,
                char_end: piece.char_end,
                metadata_json: piece.metadata_json,
            })
            .collect()
    }
}

/// Builds indexable chunks for a document without performing database writes.
pub struct DocumentIndexer<'a> {
    embedder: &'a dyn EmbeddingProvider,
    chunk_size: usize,
    chunk_overlap: usize,
}

impl<'a> DocumentIndexer<'a> {
    pub fn new(embedder: &'a dyn EmbeddingProvider, config: &Config) -> Self {
        Self {
            embedder,
            chunk_size: config.chunk_size,
            chunk_overlap: config.chunk_overlap,
        }
    }

    /// Normalize chunking policy, add section metadata, embed, and build rows.
    pub async fn build_chunks(&self, document: &Document) -> Result<Vec<Chunk>> {
        self.build_chunks_with_profile(document, ChunkingProfile::Auto)
            .await
    }

    /// Build fixed text chunks even when metadata contains a code language.
    ///
    /// Wiki and content-update paths used this policy before the shared indexer.
    pub async fn build_plain_chunks(&self, document: &Document) -> Result<Vec<Chunk>> {
        self.build_chunks_with_profile(document, ChunkingProfile::Plain)
            .await
    }

    /// Prepare one document synchronously for a later shared embedding call.
    pub(crate) fn prepare_chunks(&self, document: &Document) -> PreparedDocumentChunks {
        self.prepare_chunks_with_profile(document, ChunkingProfile::Auto)
    }

    /// Embed several prepared documents in one provider call and map vectors
    /// back to their original document/chunk order.
    pub(crate) async fn embed_prepared_batch(
        &self,
        prepared: Vec<PreparedDocumentChunks>,
    ) -> Result<Vec<Vec<Chunk>>> {
        let expected = prepared
            .iter()
            .map(PreparedDocumentChunks::len)
            .sum::<usize>();
        if expected == 0 {
            return Ok(prepared.into_iter().map(|_| Vec::new()).collect());
        }

        let texts = prepared
            .iter()
            .flat_map(|document| document.pieces.iter())
            .map(|piece| piece.content.clone())
            .collect::<Vec<_>>();
        let embeddings = self.embedder.embed(&texts).await?;
        ensure_vector_count(embeddings.len(), expected)?;

        let mut embeddings = embeddings.into_iter();
        Ok(prepared
            .into_iter()
            .map(|document| document.into_chunks(&mut embeddings))
            .collect())
    }

    async fn build_chunks_with_profile(
        &self,
        document: &Document,
        profile: ChunkingProfile,
    ) -> Result<Vec<Chunk>> {
        let prepared = self.prepare_chunks_with_profile(document, profile);
        Ok(self
            .embed_prepared_batch(vec![prepared])
            .await?
            .pop()
            .expect("one prepared document yields one chunk group"))
    }

    /// Refresh vectors for existing chunks while preserving ids and metadata.
    pub async fn reembed_chunks(&self, chunks: &mut [Chunk]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let texts = chunks
            .iter()
            .map(|chunk| chunk.content.clone())
            .collect::<Vec<_>>();
        let embeddings = self.embedder.embed(&texts).await?;
        ensure_vector_count(embeddings.len(), chunks.len())?;
        for (chunk, embedding) in chunks.iter_mut().zip(embeddings) {
            chunk.embedding = embedding;
        }
        Ok(())
    }

    fn normalized_pieces(
        &self,
        document: &Document,
        profile: ChunkingProfile,
    ) -> Vec<(String, i32, i32)> {
        if profile == ChunkingProfile::Auto && uses_code_chunking(document) {
            code_chunks(&document.content, self.chunk_size, self.chunk_overlap)
        } else {
            let chunker = from_config(self.chunk_size, self.chunk_overlap);
            Chunker::chunk(&chunker, &document.content)
        }
    }

    fn prepare_chunks_with_profile(
        &self,
        document: &Document,
        profile: ChunkingProfile,
    ) -> PreparedDocumentChunks {
        let pieces = self.normalized_pieces(document, profile);
        let section_metadata = markdown_section_metadata(&document.content, &pieces);
        PreparedDocumentChunks {
            document_id: document.id.clone(),
            pieces: pieces
                .into_iter()
                .zip(section_metadata)
                .map(
                    |((content, char_start, char_end), metadata_json)| PreparedChunk {
                        content,
                        char_start,
                        char_end,
                        metadata_json,
                    },
                )
                .collect(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChunkingProfile {
    Auto,
    Plain,
}

fn uses_code_chunking(document: &Document) -> bool {
    serde_json::from_str::<serde_json::Value>(&document.metadata_json)
        .ok()
        .is_some_and(|value| value.get("language").is_some())
}

fn ensure_vector_count(actual: usize, expected: usize) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(AppError::embeddings(format!(
            "embedder returned {actual} vectors for {expected} chunks"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbedder;

    #[tokio::test]
    async fn builds_section_metadata_without_storage() {
        let config = Config {
            embedding_dims: 8,
            chunk_size: 24,
            chunk_overlap: 4,
            ..Config::for_tests()
        };
        let embedder = MockEmbedder::new(8);
        let document = Document {
            id: "doc".into(),
            content: "# Heading\nfirst paragraph\n\n## Detail\nsecond paragraph".into(),
            metadata_json: "{}".into(),
            ..Default::default()
        };

        let chunks = DocumentIndexer::new(&embedder, &config)
            .build_chunks(&document)
            .await
            .expect("chunks");

        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|chunk| chunk.document_id == "doc"));
        assert!(chunks
            .iter()
            .any(|chunk| chunk.metadata_json.contains("Heading")));
        assert!(chunks.iter().all(|chunk| chunk.embedding.len() == 8));
    }
}
