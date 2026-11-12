//! Bounded, text-only chunk reads for document responses over `lin`.
//!
//! The byte budget is checked before chunk strings are cloned into transport
//! projections, and embedding vectors are never decoded.

use serde::Serialize;

use super::lin_engine::{LinReader, cell_i32, cell_str, cell_string};
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
/// without materializing `ChunkText` items.
pub fn chunk_text_page(
    store: &Store,
    document_id: &str,
    index_range: Option<(i32, i32)>,
    limit: usize,
    max_content_bytes: usize,
    over_budget: impl FnOnce(usize) -> AppError,
) -> Result<ChunkTextPage> {
    let (low, high) = index_range.unwrap_or((i32::MIN, i32::MAX));
    let r = store.read()?;
    let rows = r.find_by_eq("chunks", "document_id", document_id)?;
    let total = rows.len() as u64;

    let mut selected: Vec<(i32, &str, &lin::Row)> = rows
        .iter()
        .filter_map(|row| {
            let idx = cell_i32(row, "chunk_index", 0);
            if idx < low || idx > high {
                return None;
            }
            let id = cell_str(row, "id")?;
            Some((idx, id, row))
        })
        .collect();

    selected.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    if selected.len() > limit {
        selected.truncate(limit);
    }

    let content_bytes: usize = selected
        .iter()
        .map(|(_, _, row)| cell_str(row, "content").map_or(0, str::len))
        .fold(0usize, usize::saturating_add);

    if content_bytes > max_content_bytes {
        return Err(over_budget(content_bytes));
    }

    let items = selected
        .into_iter()
        .map(|(chunk_index, _, row)| ChunkText {
            id: cell_string(row, "id"),
            chunk_index,
            content: cell_string(row, "content"),
            char_start: cell_i32(row, "char_start", 0),
            char_end: cell_i32(row, "char_end", 0),
        })
        .collect();

    Ok(ChunkTextPage {
        items,
        total,
        content_bytes,
    })
}
