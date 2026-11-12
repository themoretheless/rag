//! Chunk reads/writes and embedding snapshots.

use super::*;
use crate::db::lin_engine::{cell_i64, cell_nonempty_string, cell_opt_string, cell_str, cell_string, cell_vec_f32};

/// Chunk text plus section metadata, without the embedding vector.
#[derive(Debug, Clone)]
pub(crate) struct ChunkBody {
    pub id: String,
    pub chunk_index: i32,
    pub content: String,
    pub metadata_json: String,
}

impl Store {
    /// List chunks for a document ordered by `chunk_index`.
    pub fn list_chunks_for_document(&self, doc_id: &str) -> Result<Vec<Chunk>> {
        let r = self.read()?;
        list_chunks_for_document_locked(&r, doc_id)
    }

    /// Chunk bodies of a document in `chunk_index` order, without embeddings.
    pub(crate) fn list_chunk_bodies_for_document(&self, doc_id: &str) -> Result<Vec<ChunkBody>> {
        self.list_chunk_bodies_in_range(doc_id, i32::MIN, i32::MAX)
    }

    /// Chunk bodies of a document with `chunk_index` in `low..=high`, in order.
    pub(crate) fn list_chunk_bodies_in_range(
        &self,
        doc_id: &str,
        low: i32,
        high: i32,
    ) -> Result<Vec<ChunkBody>> {
        let r = self.read()?;
        let mut bodies: Vec<ChunkBody> = Vec::new();
        r.for_each_by_eq_projected(
            "chunks",
            "document_id",
            doc_id,
            Some(&["id", "chunk_index", "content", "metadata_json"]),
            |row| {
                let idx = cell_i64(row, "chunk_index", 0) as i32;
                if idx >= low && idx <= high {
                    bodies.push(ChunkBody {
                        id: cell_string(row, "id"),
                        chunk_index: idx,
                        content: cell_string(row, "content"),
                        metadata_json: cell_opt_string(row, "metadata_json").unwrap_or_default(),
                    });
                }
                Ok(true)
            },
        )?;
        bodies.sort_by(|a, b| {
            a.chunk_index
                .cmp(&b.chunk_index)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(bodies)
    }

    /// Stored vector for each distinct chunk text of `document_ids`, keyed by the text.
    pub(crate) fn chunk_vectors_by_content(
        &self,
        document_ids: &[&str],
    ) -> Result<std::collections::HashMap<String, Vec<f32>>> {
        let mut out = std::collections::HashMap::new();
        if document_ids.is_empty() {
            return Ok(out);
        }
        let r = self.read()?;
        for &doc_id in document_ids {
            r.for_each_by_eq_projected(
                "chunks",
                "document_id",
                doc_id,
                Some(&["content", "embedding"]),
                |row| {
                    let content = cell_string(row, "content");
                    if let std::collections::hash_map::Entry::Vacant(slot) = out.entry(content) {
                        let vec = cell_vec_f32(row, "embedding");
                        if !vec.is_empty() {
                            slot.insert(vec);
                        }
                    }
                    Ok(true)
                },
            )?;
        }
        Ok(out)
    }

    /// `metadata_json` for each of `chunk_ids` that exists, in one read guard.
    pub(crate) fn chunk_metadata_by_ids(
        &self,
        chunk_ids: &[&str],
    ) -> Result<std::collections::HashMap<String, String>> {
        let mut out = std::collections::HashMap::with_capacity(chunk_ids.len());
        if chunk_ids.is_empty() {
            return Ok(out);
        }
        let r = self.read()?;
        for &id in chunk_ids {
            let maybe_row = if let Some(eng) = r.disk_v2() {
                eng.get_projected_by_id("chunks", id, &["metadata_json"])?
            } else {
                r.get_by_id("chunks", id)?
            };
            if let Some(row) = maybe_row {
                out.insert(
                    id.to_string(),
                    cell_opt_string(&row, "metadata_json").unwrap_or_default(),
                );
            }
        }
        Ok(out)
    }

    /// Load one representative vector for at most `limit` deterministic
    /// documents without materializing full document bodies or the corpus.
    pub(crate) fn representative_document_embeddings(
        &self,
        limit: usize,
        active_only: bool,
        require_wing: bool,
        exclude_system_layers: bool,
    ) -> Result<Vec<DocumentEmbeddingSample>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let r = self.read()?;

        // Gather documents that have at least one chunk
        let mut docs_with_chunks = std::collections::HashSet::new();
        r.scan_each_projected("chunks", &["document_id"], |row| {
            if let Some(doc_id) = cell_str(row, "document_id") {
                docs_with_chunks.insert(doc_id.to_string());
            }
            Ok(true)
        })?;

        let mut candidates: Vec<(String, String, Option<String>, Option<String>)> = Vec::new();
        r.scan_each_projected(
            "documents",
            &["id", "title", "status", "wing", "room", "layer"],
            |row| {
                let Some(id) = cell_str(row, "id") else {
                    return Ok(true);
                };
                if !docs_with_chunks.contains(id) {
                    return Ok(true);
                }
                if active_only {
                    let status =
                        cell_nonempty_string(row, "status").unwrap_or_else(|| "active".into());
                    if status == "archived" || status == "tombstone" {
                        return Ok(true);
                    }
                }
                let wing = cell_opt_string(row, "wing");
                if require_wing && wing.as_deref().map(str::trim).unwrap_or("").is_empty() {
                    return Ok(true);
                }
                if exclude_system_layers {
                    let layer = cell_str(row, "layer").unwrap_or("");
                    if matches!(layer, "schema" | "index" | "log") {
                        return Ok(true);
                    }
                }
                candidates.push((
                    id.to_string(),
                    cell_string(row, "title"),
                    wing,
                    cell_opt_string(row, "room"),
                ));
                Ok(true)
            },
        )?;

        candidates.sort_by(|a, b| a.0.cmp(&b.0));
        candidates.truncate(limit);

        let mut samples = Vec::with_capacity(candidates.len());
        for (document_id, title, wing, room) in candidates {
            let mut best_row: Option<lin::Row> = None;
            r.for_each_by_eq_projected(
                "chunks",
                "document_id",
                document_id.as_str(),
                Some(&["id", "chunk_index", "embedding"]),
                |row| {
                    let replace = match &best_row {
                        None => true,
                        Some(prev) => {
                            (cell_i64(row, "chunk_index", 0), cell_str(row, "id"))
                                < (cell_i64(prev, "chunk_index", 0), cell_str(prev, "id"))
                        }
                    };
                    if replace {
                        best_row = Some(row.clone());
                    }
                    Ok(true)
                },
            )?;
            if let Some(first_row) = best_row {
                let embedding = cell_vec_f32(&first_row, "embedding");
                samples.push(DocumentEmbeddingSample {
                    document_id,
                    title,
                    wing,
                    room,
                    embedding,
                });
            }
        }
        Ok(samples)
    }

    /// First-chunk embedding of each of `document_ids` that has chunks.
    pub(crate) fn first_chunk_embeddings(
        &self,
        document_ids: &[&str],
    ) -> Result<std::collections::BTreeMap<String, Vec<f32>>> {
        let mut out = std::collections::BTreeMap::new();
        if document_ids.is_empty() {
            return Ok(out);
        }
        let r = self.read()?;
        for &doc_id in document_ids {
            let mut best_row: Option<lin::Row> = None;
            r.for_each_by_eq_projected(
                "chunks",
                "document_id",
                doc_id,
                Some(&["id", "chunk_index", "embedding"]),
                |row| {
                    let replace = match &best_row {
                        None => true,
                        Some(prev) => {
                            (cell_i64(row, "chunk_index", 0), cell_str(row, "id"))
                                < (cell_i64(prev, "chunk_index", 0), cell_str(prev, "id"))
                        }
                    };
                    if replace {
                        best_row = Some(row.clone());
                    }
                    Ok(true)
                },
            )?;
            if let Some(first_row) = best_row {
                out.insert(doc_id.to_string(), cell_vec_f32(&first_row, "embedding"));
            }
        }
        Ok(out)
    }

    /// Load every chunk with its embedding (for in-process vector search).
    pub fn all_chunks_with_embeddings(&self) -> Result<Vec<Chunk>> {
        let r = self.read()?;
        let mut out = r.filter_map_rows("chunks", |row| Ok(Some(rows::chunk(row)?)))?;
        out.sort_by(|a, b| {
            a.document_id
                .cmp(&b.document_id)
                .then_with(|| a.chunk_index.cmp(&b.chunk_index))
        });
        Ok(out)
    }

    /// Atomically replace all chunks for a document, preserving caller-supplied ids.
    pub fn replace_chunks_for_document(&self, document_id: &str, chunks: &[Chunk]) -> Result<()> {
        let embedding_json = embedding_json(chunks)?;
        self.with_tx(|tx| {
            crate::db::fts::mark_chunks_changed_for(tx, &[document_id])?;
            tx.delete_where("chunks", lin::query::pred::eq("document_id", document_id))?;
            insert_chunks_locked(tx, chunks, &embedding_json)?;
            Ok(())
        })
    }

    /// Atomically replace only the embeddings of an unchanged chunk set.
    pub(crate) fn update_chunk_embeddings_atomic(
        &self,
        document_id: &str,
        if_match_revision: i64,
        expected_chunks: &[Chunk],
        embeddings: &[Vec<f32>],
        manifest: Option<&EmbeddingManifest>,
    ) -> Result<usize> {
        self.update_chunk_embeddings_atomic_with(
            document_id,
            if_match_revision,
            expected_chunks,
            embeddings,
            manifest,
            |_| Ok(()),
        )
    }

    pub(super) fn update_chunk_embeddings_atomic_with(
        &self,
        document_id: &str,
        if_match_revision: i64,
        expected_chunks: &[Chunk],
        embeddings: &[Vec<f32>],
        manifest: Option<&EmbeddingManifest>,
        mut after_chunk_update: impl FnMut(usize) -> Result<()>,
    ) -> Result<usize> {
        let document_id = document_id.trim();
        if document_id.is_empty() {
            return Err(AppError::config("document_id must be non-empty"));
        }
        if if_match_revision < 1 {
            return Err(AppError::config("if_match_revision must be >= 1"));
        }
        if expected_chunks.len() != embeddings.len() {
            return Err(AppError::embeddings(format!(
                "embedder returned {} vectors for {} chunks",
                embeddings.len(),
                expected_chunks.len()
            )));
        }
        if let Some(chunk) = expected_chunks
            .iter()
            .find(|chunk| chunk.document_id != document_id)
        {
            return Err(AppError::config(format!(
                "chunk {} belongs to document {}, expected {}",
                chunk.id, chunk.document_id, document_id
            )));
        }
        let mut chunk_ids = std::collections::HashSet::with_capacity(expected_chunks.len());
        if let Some(chunk) = expected_chunks
            .iter()
            .find(|chunk| !chunk_ids.insert(chunk.id.as_str()))
        {
            return Err(AppError::config(format!(
                "duplicate chunk id in embedding update: {}",
                chunk.id
            )));
        }
        if let Some(manifest) = manifest {
            let expected_dims = usize::try_from(manifest.dims).map_err(|_| {
                AppError::embeddings(format!(
                    "embedding manifest dims must be positive (got {})",
                    manifest.dims
                ))
            })?;
            if expected_dims == 0 {
                return Err(AppError::embeddings(
                    "embedding manifest dims must be positive",
                ));
            }
            if let Some((index, embedding)) = embeddings
                .iter()
                .enumerate()
                .find(|(_, embedding)| embedding.len() != expected_dims)
            {
                return Err(AppError::embeddings(format!(
                    "embedding {} has dims={}, expected {} from manifest",
                    index,
                    embedding.len(),
                    expected_dims
                )));
            }
        }
        let embedding_json = embeddings
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?;

        self.with_tx(|tx| {
            let doc_row = tx.get_by_id("documents", document_id)?.ok_or_else(|| {
                AppError::not_found(format!("document not found: {document_id}"))
            })?;
            let current_revision = cell_i64(&doc_row, "revision", 1).max(1);
            if current_revision != if_match_revision {
                return Err(AppError::conflict(format!(
                    "etag mismatch for document {document_id}: expected revision {} ({}), current revision {} ({})",
                    if_match_revision,
                    crate::models::format_document_etag(if_match_revision),
                    current_revision,
                    crate::models::format_document_etag(current_revision),
                )));
            }

            let current_chunks = list_chunks_for_document_locked(tx, document_id)?;
            if !same_chunk_snapshot(&current_chunks, expected_chunks) {
                return Err(AppError::conflict(format!(
                    "chunk snapshot changed while re-embedding document {document_id}; retry from a fresh read"
                )));
            }

            if let Some(manifest) = manifest {
                set_embedding_manifest_locked(tx, manifest)?;
            }
            if !expected_chunks.is_empty() {
                crate::db::fts::mark_embeddings_changed_for(tx, Some(&[document_id]))?;
                let bridge = tx.bridge_embedder().clone();
                for (index, ((chunk, new_vec), json)) in expected_chunks
                    .iter()
                    .zip(embeddings)
                    .zip(&embedding_json)
                    .enumerate()
                {
                    let existing_row = tx.get_by_id("chunks", &chunk.id)?.ok_or_else(|| {
                        AppError::conflict(format!(
                            "chunk {} changed while re-embedding document {document_id}",
                            chunk.id
                        ))
                    })?;
                    let created_at = cell_opt_string(&existing_row, "created_at")
                        .unwrap_or_else(|| format_ts(Utc::now()));
                    let content_hash_val = cell_opt_string(&existing_row, "content_hash");
                    let parent_chunk_id = cell_opt_string(&existing_row, "parent_chunk_id")
                        .or_else(|| extract_parent_chunk_id(&chunk.metadata_json));
                    let digest = super::embedding_digest(json);

                    tx.delete_by_id("chunks", &chunk.id)?;
                    let has_vec = !new_vec.is_empty();
                    if has_vec {
                        bridge.stage(chunk.id.clone(), Arc::from(new_vec.as_slice()));
                    }
                    tx.insert_one(
                        "chunks",
                        RecBuilder::new()
                            .str("id", &chunk.id)
                            .str("document_id", &chunk.document_id)
                            .i64("chunk_index", i64::from(chunk.chunk_index))
                            .str("content", &chunk.content)
                            .opt_str("snippet", has_vec.then_some("1"))
                            .i64("char_start", i64::from(chunk.char_start))
                            .i64("char_end", i64::from(chunk.char_end))
                            .str("created_at", created_at)
                            .opt_string("content_hash", &content_hash_val)
                            .str("metadata_json", &chunk.metadata_json)
                            .i64("embedding_digest", digest)
                            .opt_string("parent_chunk_id", &parent_chunk_id)
                            .build(),
                    )?;
                    after_chunk_update(index + 1)?;
                }
            }
            Ok(expected_chunks.len())
        })
    }
}
