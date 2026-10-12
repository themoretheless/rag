//! Chunk reads/writes and embedding snapshots.

use super::*;

impl Store {
    /// List chunks for a document ordered by `chunk_index`.
    pub fn list_chunks_for_document(&self, doc_id: &str) -> Result<Vec<Chunk>> {
        let conn = self.lock()?;
        list_chunks_for_document_locked(&conn, doc_id)
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
        let active_clause = if active_only {
            "AND COALESCE(d.status, 'active') NOT IN ('archived', 'tombstone')"
        } else {
            ""
        };
        let wing_clause = if require_wing {
            "AND d.wing IS NOT NULL AND TRIM(d.wing) <> ''"
        } else {
            ""
        };
        let layer_clause = if exclude_system_layers {
            "AND COALESCE(d.layer, '') NOT IN ('schema', 'index', 'log')"
        } else {
            ""
        };
        let sql = format!(
            r#"
            WITH candidates AS (
                SELECT d.id, d.title, d.wing, d.room
                FROM documents d
                WHERE EXISTS (
                    SELECT 1 FROM chunks candidate_chunk
                    WHERE candidate_chunk.document_id = d.id
                )
                {active_clause}
                {wing_clause}
                {layer_clause}
                ORDER BY d.id ASC
                LIMIT ?
            )
            SELECT d.id, d.title, d.wing, d.room,
                   (
                       SELECT c.embedding_json
                       FROM chunks c
                       WHERE c.document_id = d.id
                       ORDER BY c.chunk_index ASC, c.id ASC
                       LIMIT 1
                   ) AS embedding_json
            FROM candidates d
            ORDER BY d.id ASC
            "#
        );
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let conn = self.lock()?;
        let mut statement = conn.prepare(&sql)?;
        let mut rows = statement.query(params![limit])?;
        let mut samples = Vec::new();
        while let Some(row) = rows.next()? {
            let document_id: String = row.get(0)?;
            let raw: String = row.get(4)?;
            let embedding = serde_json::from_str(&raw).map_err(|error| {
                AppError::db(format!(
                    "invalid representative embedding JSON for document {document_id}: {error}"
                ))
            })?;
            samples.push(DocumentEmbeddingSample {
                document_id,
                title: row.get(1)?,
                wing: row.get(2)?,
                room: row.get(3)?,
                embedding,
            });
        }
        Ok(samples)
    }

    /// Load only the first chunk vector for a specific document.
    pub(crate) fn first_chunk_embedding(&self, document_id: &str) -> Result<Option<Vec<f32>>> {
        let conn = self.lock()?;
        let raw = match conn.query_row(
            r#"
            SELECT embedding_json
            FROM chunks
            WHERE document_id = ?
            ORDER BY chunk_index ASC, id ASC
            LIMIT 1
            "#,
            params![document_id],
            |row| row.get::<_, String>(0),
        ) {
            Ok(raw) => raw,
            Err(duckdb::Error::QueryReturnedNoRows) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        serde_json::from_str(&raw).map(Some).map_err(|error| {
            AppError::db(format!(
                "invalid representative embedding JSON for document {document_id}: {error}"
            ))
        })
    }

    /// Load every chunk with its embedding (for in-process vector search).
    pub fn all_chunks_with_embeddings(&self) -> Result<Vec<Chunk>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, document_id, chunk_index, content, embedding_json, char_start, char_end, metadata_json
            FROM chunks
            ORDER BY document_id ASC, chunk_index ASC
            "#,
        )?;

        let out = crate::db::rows::query_vec(&mut stmt, [], rows::chunk)?;
        Ok(out)
    }

    /// Atomically replace all chunks for a document, preserving caller-supplied ids.
    pub fn replace_chunks_for_document(&self, document_id: &str, chunks: &[Chunk]) -> Result<()> {
        let embedding_json = embedding_json(chunks)?;
        self.with_tx(|tx| {
            crate::db::fts::mark_fts_dirty(tx)?;
            tx.execute(
                "DELETE FROM chunks WHERE document_id = ?",
                params![document_id],
            )?;
            insert_chunks_locked(tx, chunks, &embedding_json)?;
            Ok(())
        })
    }

    /// Atomically replace only the embeddings of an unchanged chunk set.
    ///
    /// `expected_chunks` is the snapshot embedded by the caller outside the
    /// database lock. Both the document revision and every persisted chunk
    /// field (including the prior vector) form the CAS boundary, so a concurrent
    /// content write or re-embed cannot be overwritten. Chunk ids, text,
    /// offsets, metadata, timestamps, and document revision are preserved.
    ///
    /// When supplied, `manifest` commits in the same transaction as the new
    /// vectors. The chunk generation advances to invalidate vector snapshots;
    /// a clean FTS generation advances with it because indexed text did not
    /// change, while an already-dirty FTS stays dirty.
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
            let current_revision = match tx.query_row(
                "SELECT COALESCE(revision, 1) FROM documents WHERE id = ?",
                params![document_id],
                |row| row.get::<_, i64>(0),
            ) {
                Ok(revision) => revision,
                Err(duckdb::Error::QueryReturnedNoRows) => {
                    return Err(AppError::not_found(format!(
                        "document not found: {document_id}"
                    )))
                }
                Err(error) => return Err(error.into()),
            };
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

            // A migration marker (or replacement manifest) becomes part of the
            // transaction before the first vector write. The transaction boundary
            // keeps it invisible until the vectors commit and rolls it back on any
            // later failure.
            if let Some(manifest) = manifest {
                set_embedding_manifest_locked(tx, manifest)?;
            }
            if !expected_chunks.is_empty() {
                crate::db::fts::mark_embeddings_changed(tx)?;
                let mut stmt = tx.prepare("UPDATE chunks SET embedding_json = ? WHERE id = ?")?;
                for (index, (chunk, embedding_json)) in
                    expected_chunks.iter().zip(&embedding_json).enumerate()
                {
                    let updated = stmt.execute(params![embedding_json, chunk.id])?;
                    if updated != 1 {
                        return Err(AppError::conflict(format!(
                            "chunk {} changed while re-embedding document {document_id}",
                            chunk.id
                        )));
                    }
                    after_chunk_update(index + 1)?;
                }
            }
            Ok(expected_chunks.len())
        })
    }

    // --- Karpathy wiki: ops_log + wiki_index + layer helpers ---
}
