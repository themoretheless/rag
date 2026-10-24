//! Pure document indexing pipeline: split, annotate, embed, and build chunks.
//!
//! This module deliberately has no database dependency. Callers finish all
//! potentially slow/fallible preparation here, then hand the resulting chunks
//! to their persistence boundary.

use uuid::Uuid;

use crate::chunking::{
    code_chunks, from_config, is_structural_language, markdown_section_metadata, markdown_sections,
    structural_code_chunks, Chunker,
};
use crate::config::{CodeChunkingMode, Config};
use crate::embeddings::EmbeddingProvider;
use crate::error::{AppError, Result};
use crate::models::{Chunk, Document};

/// `embedder`, wrapped to reuse `document_id`'s stored vector for every chunk
/// text the document already has (exact: the embedding input is the chunk
/// text under one manifest). Rewriting a page then embeds only changed chunks.
pub(crate) fn reusing_document_vectors(
    store: &crate::db::Store,
    embedder: &std::sync::Arc<dyn EmbeddingProvider>,
    document_id: &str,
) -> crate::error::Result<std::sync::Arc<dyn EmbeddingProvider>> {
    let known = crate::util::blocking_section(|| store.chunk_vectors_by_content(&[document_id]))?;
    Ok(if known.is_empty() {
        embedder.clone()
    } else {
        std::sync::Arc::new(crate::embeddings::PrecomputedEmbedder::with_vectors(
            embedder.clone(),
            known,
        ))
    })
}

/// Chunk contents and metadata prepared before the fallible embedding call.
///
/// Keeping this storage-free representation lets source synchronization group
/// several small documents into one provider request without duplicating the
/// chunking and annotation policy used by ordinary ingest.
#[derive(Clone)]
pub(crate) struct PreparedDocumentChunks {
    document_id: String,
    pieces: Vec<PreparedChunk>,
}

#[derive(Clone)]
struct PreparedChunk {
    content: String,
    char_start: i32,
    char_end: i32,
    metadata_json: String,
    /// Index of the parent piece within the same document (Markdown section
    /// leader / first window of an oversized code item). Resolved to a chunk
    /// id during assembly, when ids exist.
    parent_index: Option<usize>,
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
        let parent_links: Vec<Option<usize>> =
            pieces.iter().map(|piece| piece.parent_index).collect();
        let mut chunks: Vec<Chunk> = pieces
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
            .collect();
        // Second pass: resolve parent piece indices to chunk ids and stamp
        // `parent_chunk_id` into chunk metadata (additive key).
        let ids: Vec<String> = chunks.iter().map(|chunk| chunk.id.clone()).collect();
        for (index, chunk) in chunks.iter_mut().enumerate() {
            let Some(parent_index) = parent_links[index] else {
                continue;
            };
            let Some(parent_id) = ids.get(parent_index) else {
                continue;
            };
            chunk.metadata_json = with_parent_link(&chunk.metadata_json, parent_id);
        }
        chunks
    }
}

/// Builds indexable chunks for a document without performing database writes.
pub struct DocumentIndexer<'a> {
    embedder: &'a dyn EmbeddingProvider,
    chunk_size: usize,
    chunk_overlap: usize,
    code_chunking: CodeChunkingMode,
}

impl<'a> DocumentIndexer<'a> {
    /// Build an indexer using the config's chunk size and overlap.
    pub fn new(embedder: &'a dyn EmbeddingProvider, config: &Config) -> Self {
        Self {
            embedder,
            chunk_size: config.chunk_size,
            chunk_overlap: config.chunk_overlap,
            code_chunking: config.code_chunking,
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

    /// The texts [`Self::build_plain_chunks`] would embed for `document`.
    pub(crate) fn plain_chunk_texts(&self, document: &Document) -> Vec<String> {
        self.prepare_chunks_with_profile(document, ChunkingProfile::Plain)
            .pieces
            .into_iter()
            .map(|piece| piece.content)
            .collect()
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

    fn prepare_chunks_with_profile(
        &self,
        document: &Document,
        profile: ChunkingProfile,
    ) -> PreparedDocumentChunks {
        let language = if profile == ChunkingProfile::Auto {
            document_language(document)
        } else {
            None
        };

        // Code path: structural item splitting for supported languages,
        // legacy window splitting otherwise. Symbol metadata replaces the
        // Markdown heading pass for structurally-split files only.
        if let Some(language) = language.as_deref() {
            if self.code_chunking == CodeChunkingMode::Structural
                && is_structural_language(language)
            {
                let structural = structural_code_chunks(
                    &document.content,
                    language,
                    self.chunk_size,
                    self.chunk_overlap,
                );
                return PreparedDocumentChunks {
                    document_id: document.id.clone(),
                    pieces: structural
                        .into_iter()
                        .enumerate()
                        .map(|(index, chunk)| PreparedChunk {
                            content: chunk.content,
                            char_start: chunk.char_start,
                            char_end: chunk.char_end,
                            metadata_json: symbol_metadata_json(
                                chunk.symbol.as_deref(),
                                chunk.symbol_kind,
                            ),
                            // Continuation windows of an oversized item link
                            // to the item's first window.
                            parent_index: (chunk.parts > 1 && chunk.part > 0)
                                .then_some(index - chunk.part),
                        })
                        .collect(),
                };
            }
            let pieces = code_chunks(&document.content, self.chunk_size, self.chunk_overlap);
            let section_metadata = markdown_section_metadata(&document.content, &pieces);
            return PreparedDocumentChunks {
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
                            parent_index: None,
                        },
                    )
                    .collect(),
            };
        }

        let chunker = from_config(self.chunk_size, self.chunk_overlap);
        let pieces = Chunker::chunk(&chunker, &document.content);
        let sections = markdown_sections(&document.content, &pieces);
        let parent_links = markdown_parent_links(&sections);
        PreparedDocumentChunks {
            document_id: document.id.clone(),
            pieces: pieces
                .into_iter()
                .zip(sections)
                .zip(parent_links)
                .map(
                    |(((content, char_start, char_end), (heading_path, section)), parent_index)| {
                        PreparedChunk {
                            content,
                            char_start,
                            char_end,
                            metadata_json: if heading_path.is_empty() {
                                "{}".into()
                            } else {
                                serde_json::json!({
                                    "heading_path": heading_path,
                                    "section": section,
                                })
                                .to_string()
                            },
                            parent_index,
                        }
                    },
                )
                .collect(),
        }
    }
}

/// Group consecutive Markdown pieces under an identical non-empty heading
/// path: every member after the first links to the section's leading chunk
/// (designated parent, "search small, return parent").
fn markdown_parent_links(sections: &[(Vec<String>, Option<String>)]) -> Vec<Option<usize>> {
    let mut links = Vec::with_capacity(sections.len());
    let mut current: Option<(Vec<String>, usize)> = None;
    for (index, (heading_path, section)) in sections.iter().enumerate() {
        if heading_path.is_empty() || section.is_none() {
            current = None;
            links.push(None);
            continue;
        }
        match &current {
            Some((path, leader)) if path == heading_path => {
                links.push(Some(*leader));
            }
            _ => {
                current = Some((heading_path.clone(), index));
                links.push(None);
            }
        }
    }
    links
}

/// Chunk metadata for a structurally-split code item (additive keys only).
fn symbol_metadata_json(symbol: Option<&str>, symbol_kind: Option<&str>) -> String {
    match (symbol, symbol_kind) {
        (Some(symbol), Some(kind)) => {
            serde_json::json!({"symbol": symbol, "symbol_kind": kind}).to_string()
        }
        (None, Some(kind)) => serde_json::json!({"symbol_kind": kind}).to_string(),
        _ => "{}".into(),
    }
}

/// Add `parent_chunk_id` to an existing chunk metadata object.
fn with_parent_link(metadata_json: &str, parent_id: &str) -> String {
    let mut value = serde_json::from_str::<serde_json::Value>(metadata_json)
        .ok()
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    value["parent_chunk_id"] = serde_json::Value::String(parent_id.to_string());
    value.to_string()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChunkingProfile {
    Auto,
    Plain,
}

fn document_language(document: &Document) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(&document.metadata_json)
        .ok()
        .and_then(|value| value.get("language")?.as_str().map(str::to_string))
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

    fn code_document(content: &str, language: &str) -> Document {
        Document {
            id: "code-doc".into(),
            content: content.into(),
            metadata_json: serde_json::json!({"language": language}).to_string(),
            ..Default::default()
        }
    }

    fn metadata(chunk: &Chunk) -> serde_json::Value {
        serde_json::from_str(&chunk.metadata_json).expect("chunk metadata JSON")
    }

    #[tokio::test]
    async fn structural_rust_chunks_carry_symbol_metadata() {
        let config = Config {
            embedding_dims: 8,
            chunk_size: 800,
            chunk_overlap: 120,
            ..Config::for_tests()
        };
        let embedder = MockEmbedder::new(8);
        let document = code_document(
            "fn alpha() -> i32 {\n    1\n}\n\nfn beta() {\n    loop {}\n}\n",
            "rust",
        );

        let chunks = DocumentIndexer::new(&embedder, &config)
            .build_chunks(&document)
            .await
            .expect("chunks");

        let alpha = chunks
            .iter()
            .find(|c| metadata(c)["symbol"] == "alpha")
            .expect("alpha chunk");
        assert_eq!(metadata(alpha)["symbol_kind"], "function");
        assert!(alpha.content.contains("fn alpha"));
        assert!(chunks.iter().any(|c| metadata(c)["symbol"] == "beta"));
        // No parent links: each function fits in one chunk.
        assert!(chunks
            .iter()
            .all(|c| metadata(c).get("parent_chunk_id").is_none()));
    }

    #[tokio::test]
    async fn oversized_code_item_parts_link_to_first_part() {
        let config = Config {
            embedding_dims: 8,
            chunk_size: 120,
            chunk_overlap: 10,
            ..Config::for_tests()
        };
        let embedder = MockEmbedder::new(8);
        let body = "    let x = 1;\n".repeat(60);
        let document = code_document(&format!("fn big() {{\n{body}}}\n"), "rust");

        let chunks = DocumentIndexer::new(&embedder, &config)
            .build_chunks(&document)
            .await
            .expect("chunks");
        let big: Vec<&Chunk> = chunks
            .iter()
            .filter(|c| metadata(c)["symbol"] == "big")
            .collect();
        assert!(big.len() > 1, "oversized fn splits: {}", big.len());
        let leader = big[0];
        assert!(metadata(leader).get("parent_chunk_id").is_none());
        for part in &big[1..] {
            assert_eq!(
                metadata(part)["parent_chunk_id"],
                serde_json::Value::String(leader.id.clone()),
                "continuation part links to first part"
            );
        }
    }

    #[tokio::test]
    async fn window_mode_preserves_legacy_code_chunking() {
        let config = Config {
            embedding_dims: 8,
            chunk_size: 800,
            chunk_overlap: 120,
            code_chunking: CodeChunkingMode::Window,
            ..Config::for_tests()
        };
        let embedder = MockEmbedder::new(8);
        let document = code_document("fn alpha() {}\nfn beta() {}\n", "rust");

        let chunks = DocumentIndexer::new(&embedder, &config)
            .build_chunks(&document)
            .await
            .expect("chunks");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].metadata_json, "{}");
    }

    #[tokio::test]
    async fn markdown_section_members_link_to_section_leader() {
        let config = Config {
            embedding_dims: 8,
            chunk_size: 24,
            chunk_overlap: 4,
            ..Config::for_tests()
        };
        let embedder = MockEmbedder::new(8);
        let document = Document {
            id: "doc".into(),
            content: "# Alpha\none two three four five six seven eight nine ten\n\n# Beta\nlast chunk here for sure\n"
                .into(),
            metadata_json: "{}".into(),
            ..Default::default()
        };

        let chunks = DocumentIndexer::new(&embedder, &config)
            .build_chunks(&document)
            .await
            .expect("chunks");
        assert!(chunks.len() > 2, "multiple chunks: {}", chunks.len());

        // First Alpha chunk is the designated parent for the rest of Alpha.
        let alpha: Vec<&Chunk> = chunks
            .iter()
            .filter(|c| metadata(c)["section"] == "Alpha")
            .collect();
        assert!(alpha.len() > 1, "alpha section spans several chunks");
        let leader = alpha[0];
        assert!(metadata(leader).get("parent_chunk_id").is_none());
        for member in &alpha[1..] {
            assert_eq!(
                metadata(member)["parent_chunk_id"],
                serde_json::Value::String(leader.id.clone())
            );
        }
        // Beta is a single-chunk section: no parent.
        let beta: Vec<&Chunk> = chunks
            .iter()
            .filter(|c| metadata(c)["section"] == "Beta")
            .collect();
        assert_eq!(beta.len(), 1);
        assert!(metadata(beta[0]).get("parent_chunk_id").is_none());
    }

    #[tokio::test]
    async fn plain_profile_never_splits_structurally() {
        let config = Config {
            embedding_dims: 8,
            chunk_size: 800,
            chunk_overlap: 120,
            ..Config::for_tests()
        };
        let embedder = MockEmbedder::new(8);
        let document = code_document("fn alpha() {}\n", "rust");

        let chunks = DocumentIndexer::new(&embedder, &config)
            .build_plain_chunks(&document)
            .await
            .expect("chunks");
        assert!(chunks.iter().all(|c| metadata(c).get("symbol").is_none()));
    }
}
