//! Re-embed migration progress.

use super::*;

/// Re-embed migration progress per target fingerprint.
///
/// A repository over the shared Store connection; get one from [`Store::reembed_progress`].
#[derive(Clone, Copy)]
pub struct ReembedProgress<'a> {
    pub(in crate::db) store: &'a Store,
}

impl ReembedProgress<'_> {
    pub(crate) fn list_reembed_pending_document_ids(
        &self,
        target_fingerprint: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        let conn = self.store.lock()?;
        let mut stmt = conn.prepare(
            "SELECT d.id FROM documents d \
             WHERE (? IS NULL OR d.id > ?) AND NOT EXISTS ( \
               SELECT 1 FROM reembed_progress p \
               WHERE p.target_fingerprint = ? AND p.document_id = d.id \
             ) \
             ORDER BY d.id LIMIT ?",
        )?;
        let rows = stmt.query_map(
            params![after, after, target_fingerprint, limit as i64],
            |row| row.get(0),
        )?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    pub(crate) fn count_reembed_pending(&self, target_fingerprint: &str) -> Result<usize> {
        let conn = self.store.lock()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*)::BIGINT FROM documents d \
             WHERE NOT EXISTS ( \
               SELECT 1 FROM reembed_progress p \
               WHERE p.target_fingerprint = ? AND p.document_id = d.id \
             )",
            params![target_fingerprint],
            |row| row.get(0),
        )?;
        usize::try_from(count).map_err(|_| AppError::db(format!("invalid pending count: {count}")))
    }

    pub(crate) fn count_reembed_done(&self, target_fingerprint: &str) -> Result<usize> {
        let conn = self.store.lock()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*)::BIGINT FROM reembed_progress WHERE target_fingerprint = ?",
            params![target_fingerprint],
            |row| row.get(0),
        )?;
        usize::try_from(count).map_err(|_| AppError::db(format!("invalid done count: {count}")))
    }

    pub(crate) fn mark_reembed_done(
        &self,
        target_fingerprint: &str,
        document_id: &str,
        chunks_reembedded: usize,
    ) -> Result<()> {
        let conn = self.store.lock()?;
        conn.execute(
            "INSERT OR REPLACE INTO reembed_progress \
             (target_fingerprint, document_id, chunks_reembedded, completed_at) \
             VALUES (?, ?, ?, CURRENT_TIMESTAMP)",
            params![target_fingerprint, document_id, chunks_reembedded as i64],
        )?;
        Ok(())
    }

    pub(crate) fn clear_reembed_progress(&self, target_fingerprint: &str) -> Result<usize> {
        let conn = self.store.lock()?;
        let removed = conn.execute(
            "DELETE FROM reembed_progress WHERE target_fingerprint = ?",
            params![target_fingerprint],
        )?;
        Ok(removed)
    }
}
