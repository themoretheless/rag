//! Document revision history reads.

use super::*;
use crate::db::lin_engine::{
    cell_i64, cell_nonempty_string, cell_opt_string, cell_str, cell_string,
};

/// Largest page of document revisions read at once.
const MAX_REVISIONS_PAGE: usize = 200;

impl Store {
    /// Historical document snapshots, newest revision first.
    pub fn list_document_revisions(&self, document_id: &str) -> Result<Vec<Document>> {
        let r = self.read()?;
        let rows = r.find_by_eq("document_revisions", "document_id", document_id.trim())?;
        let mut out: Vec<Document> = rows
            .iter()
            .map(rows::document_revision)
            .collect::<Result<Vec<_>>>()?;
        out.sort_by_key(|a| std::cmp::Reverse(a.revision));
        Ok(out)
    }

    /// Paginated revision timeline metadata without materializing historical bodies.
    pub fn list_document_revision_summaries(
        &self,
        document_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<DocumentRevisionPage> {
        let document_id = document_id.trim();
        if document_id.is_empty() {
            return Err(AppError::config("document_id must be non-empty"));
        }
        let limit = limit.clamp(1, MAX_REVISIONS_PAGE);
        let r = self.read()?;
        let mut rows = r.find_by_eq("document_revisions", "document_id", document_id)?;
        let total = rows.len() as u64;
        rows.sort_by_key(|a| std::cmp::Reverse(cell_i64(a, "revision", 1)));

        let items: Vec<DocumentRevisionSummary> = rows
            .iter()
            .skip(offset)
            .take(limit)
            .map(|row| {
                let content = cell_str(row, "content").unwrap_or("");
                let content_chars = content.chars().count() as u64;
                let content_lines = if content.is_empty() {
                    0
                } else {
                    1 + content.bytes().filter(|&b| b == b'\n').count() as u64
                };
                Ok(DocumentRevisionSummary {
                    document_id: cell_string(row, "document_id"),
                    uri: cell_string(row, "uri"),
                    title: cell_string(row, "title"),
                    wing: cell_opt_string(row, "wing"),
                    room: cell_opt_string(row, "room"),
                    layer: cell_nonempty_string(row, "layer").unwrap_or_else(|| "raw".into()),
                    kind: cell_nonempty_string(row, "kind").unwrap_or_else(|| "document".into()),
                    status: cell_nonempty_string(row, "status").unwrap_or_else(|| "active".into()),
                    updated_at: cell_string(row, "updated_at"),
                    superseded_at: cell_string(row, "superseded_at"),
                    revision: cell_i64(row, "revision", 1),
                    content_chars,
                    content_lines,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(DocumentRevisionPage { items, total })
    }

    /// Load one historical document snapshot by its original revision number.
    pub fn get_document_revision(
        &self,
        document_id: &str,
        revision: i64,
    ) -> Result<Option<Document>> {
        if revision < 1 {
            return Err(AppError::config("revision must be >= 1"));
        }
        let document_id = document_id.trim();
        let r = self.read()?;
        let composite = composite_id_2(document_id, &revision.to_string());
        if let Some(row) = r.get_by_id("document_revisions", &composite)? {
            return Ok(Some(rows::document_revision(&row)?));
        }
        let rows = r.find_by_eq("document_revisions", "document_id", document_id)?;
        rows.iter()
            .find(|row| cell_i64(row, "revision", 0) == revision)
            .map(rows::document_revision)
            .transpose()
    }
}
