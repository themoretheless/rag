//! Bounded, text-only chunk reads for document responses.
//!
//! The byte budget is checked in SQL before any chunk content is materialized,
//! and embedding vectors are never selected.

use duckdb::params;
use serde::Serialize;

use crate::db::Store;
use crate::error::{AppError, Result};

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

/// One page of chunk text plus the document's total chunk count.
pub struct ChunkTextPage {
    pub items: Vec<ChunkText>,
    pub total: u64,
    pub content_bytes: usize,
}

/// Read up to `limit` chunks of `document_id` in `(chunk_index, id)` order,
/// optionally restricted to an inclusive `chunk_index` range. When the
/// selected text exceeds `max_content_bytes`, returns `over_budget(bytes)`
/// without loading any content.
pub fn chunk_text_page(
    store: &Store,
    document_id: &str,
    index_range: Option<(i32, i32)>,
    limit: usize,
    max_content_bytes: usize,
    over_budget: impl FnOnce(usize) -> AppError,
) -> Result<ChunkTextPage> {
    let (low, high) = index_range.unwrap_or((i32::MIN, i32::MAX));
    let limit_sql = i64::try_from(limit).unwrap_or(i64::MAX);
    let conn = store.lock()?;
    let total: i64 = conn.query_row(
        "SELECT COUNT(*)::BIGINT FROM chunks WHERE document_id = ?",
        [document_id],
        |row| row.get(0),
    )?;
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
        params![document_id, low, high, limit_sql],
        |row| row.get(0),
    )?;
    let content_bytes = usize::try_from(content_bytes.max(0)).unwrap_or(usize::MAX);
    if content_bytes > max_content_bytes {
        return Err(over_budget(content_bytes));
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
    let items = statement
        .query_map(params![document_id, low, high, limit_sql], |row| {
            Ok(ChunkText {
                id: row.get(0)?,
                chunk_index: row.get(1)?,
                content: row.get(2)?,
                char_start: row.get(3)?,
                char_end: row.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(ChunkTextPage {
        items,
        total: total.max(0) as u64,
        content_bytes,
    })
}
