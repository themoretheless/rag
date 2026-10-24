//! Document revision history reads.

use super::*;

/// Largest page of document revisions read at once.
const MAX_REVISIONS_PAGE: usize = 200;

impl Store {
    /// Historical document snapshots, newest revision first.
    pub fn list_document_revisions(&self, document_id: &str) -> Result<Vec<Document>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT document_id AS id, uri, title, content, metadata_json, content_hash, wing, \
            room, source_file, layer, kind, CAST(created_at AS VARCHAR), CAST(updated_at AS \
            VARCHAR), COALESCE(status, 'active'), COALESCE(pinned, false), COALESCE(boost, 1.0), \
            revision FROM document_revisions WHERE document_id = ? ORDER BY revision DESC",
        )?;
        let out =
            crate::db::rows::query_vec(&mut stmt, params![document_id.trim()], rows::document)?;
        Ok(out)
    }

    /// Paginated revision timeline metadata without materializing historical
    /// bodies. Use [`Self::get_document_revision`] for one selected snapshot.
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
        let conn = self.lock()?;
        let total: i64 = conn.query_row(
            "SELECT COUNT(*)::BIGINT FROM document_revisions WHERE document_id = ?",
            params![document_id],
            |row| row.get(0),
        )?;
        let sql = format!(
            r#"
            SELECT document_id, uri, title, wing, room,
                   COALESCE(NULLIF(layer, ''), 'raw'),
                   COALESCE(NULLIF(kind, ''), 'document'),
                   COALESCE(NULLIF(status, ''), 'active'),
                   CAST(updated_at AS VARCHAR), CAST(superseded_at AS VARCHAR), revision,
                   LENGTH(content)::BIGINT,
                   CASE WHEN content = '' THEN 0
                        ELSE 1 + LENGTH(content) - LENGTH(REPLACE(content, '\n', '')) END
            FROM document_revisions
            WHERE document_id = ?
            ORDER BY revision DESC
            LIMIT {limit} OFFSET {offset}
            "#
        );
        let mut stmt = conn.prepare(&sql)?;
        let items = crate::db::rows::query_vec(&mut stmt, params![document_id], |row| {
            Ok(DocumentRevisionSummary {
                document_id: row.get(0)?,
                uri: row.get(1)?,
                title: row.get(2)?,
                wing: row.get(3)?,
                room: row.get(4)?,
                layer: row.get(5)?,
                kind: row.get(6)?,
                status: row.get(7)?,
                updated_at: row.get(8)?,
                superseded_at: row.get(9)?,
                revision: row.get(10)?,
                content_chars: row.get::<_, i64>(11)?.max(0) as u64,
                content_lines: row.get::<_, i64>(12)?.max(0) as u64,
            })
        })?;
        Ok(DocumentRevisionPage {
            items,
            total: total.max(0) as u64,
        })
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
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT document_id AS id, uri, title, content, metadata_json, content_hash, wing, \
            room, source_file, layer, kind, CAST(created_at AS VARCHAR), CAST(updated_at AS \
            VARCHAR), COALESCE(status, 'active'), COALESCE(pinned, false), COALESCE(boost, 1.0), \
            revision FROM document_revisions WHERE document_id = ? AND revision = ?",
        )?;
        let mut rows = stmt.query(params![document_id.trim(), revision])?;
        match rows.next()? {
            Some(row) => Ok(Some(rows::document(row)?)),
            None => Ok(None),
        }
    }
}
