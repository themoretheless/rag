//! Aggregate row codecs. SQL projection order remains owned by each repository.

use crate::error::{AppError, Result};
use crate::models::{Chunk, Document, EmbeddingManifest};
use crate::util::parse_db_timestamp;

fn timestamp(value: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    parse_db_timestamp(value)
        .ok_or_else(|| AppError::db(format!("invalid timestamp value: {}", value.trim())))
}

pub(crate) fn document(row: &duckdb::Row<'_>) -> Result<Document> {
    let created_raw: String = row.get(11)?;
    let updated_raw: String = row.get(12)?;
    Ok(Document {
        id: row.get(0)?, uri: row.get(1)?, title: row.get(2)?, content: row.get(3)?,
        metadata_json: row.get(4)?, content_hash: row.get(5)?, wing: row.get(6)?,
        room: row.get(7)?, source_file: row.get(8)?,
        layer: row.get::<_, Option<String>>(9)?.filter(|value| !value.is_empty()).unwrap_or_else(|| "raw".into()),
        kind: row.get::<_, Option<String>>(10)?.filter(|value| !value.is_empty()).unwrap_or_else(|| "document".into()),
        created_at: timestamp(&created_raw)?, updated_at: timestamp(&updated_raw)?,
        status: row.get::<_, Option<String>>(13)?.filter(|value| !value.is_empty()).unwrap_or_else(|| "active".into()),
        pinned: row.get::<_, Option<bool>>(14)?.unwrap_or(false),
        boost: row.get::<_, Option<f64>>(15)?.unwrap_or(1.0),
        revision: row.get::<_, Option<i64>>(16)?.unwrap_or(1),
    })
}

pub(crate) fn chunk(row: &duckdb::Row<'_>) -> Result<Chunk> {
    let embedding_json: String = row.get(4)?;
    Ok(Chunk {
        id: row.get(0)?, document_id: row.get(1)?, chunk_index: row.get(2)?, content: row.get(3)?,
        embedding: serde_json::from_str(&embedding_json)?, char_start: row.get(5)?, char_end: row.get(6)?,
        metadata_json: row.get::<_, Option<String>>(7)?.filter(|value| !value.is_empty()).unwrap_or_else(|| "{}".into()),
    })
}

pub(crate) fn embedding_manifest(row: &duckdb::Row<'_>) -> Result<EmbeddingManifest> {
    let updated_raw: String = row.get(6)?;
    Ok(EmbeddingManifest {
        id: row.get(0)?, provider: row.get(1)?, model: row.get(2)?, dims: row.get(3)?,
        base_url: row.get(4)?, content_fingerprint: row.get(5)?, updated_at: timestamp(&updated_raw)?,
    })
}
