//! Schema document, diary listings and pinned documents.

use super::*;

impl Store {
    /// Schema document (`uri=schema://agents`) if present.
    ///
    /// Prefer exact URI match; fall back to the first `layer=schema` document.
    pub fn get_schema_document(&self) -> Result<Option<Document>> {
        if let Some(d) = self.find_by_uri("schema://agents")? {
            return Ok(Some(d));
        }
        // Fallback: first layer=schema (legacy / non-canonical uri)
        let list = self.list_documents_by_layer("schema")?;
        Ok(list.into_iter().next())
    }

    /// Default wing for an agent diary: `agents/<agent_name>`.
    pub fn diary_wing_for_agent(agent_name: &str) -> String {
        format!("agents/{}", agent_name.trim())
    }

    /// List recent diary documents (`kind=diary`) for an agent, newest first.
    ///
    /// Matches rows where `room = agent_name` or `wing = agents/<agent_name>`.
    /// When `agent_name` is empty/`None`, returns recent diary across all agents.
    pub fn list_diary_entries(
        &self,
        agent_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>> {
        let limit = limit.max(1);
        let agent = agent_name.map(str::trim).filter(|s| !s.is_empty());

        let conn = self.lock()?;
        let mut out = Vec::new();
        if let Some(name) = agent {
            let wing = Self::diary_wing_for_agent(name);
            let sql = format!(
                "SELECT {DOCUMENT_SELECT} FROM documents \
                 WHERE kind = 'diary' \
                   AND (room = ? OR wing = ?) \
                   AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone') \
                 ORDER BY created_at DESC \
                 LIMIT {limit}"
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(params![name, wing])?;
            while let Some(row) = rows.next()? {
                let doc = rows::document(row)?;
                out.push(DiaryEntry::from_document(&doc));
            }
        } else {
            let sql = format!(
                "SELECT {DOCUMENT_SELECT} FROM documents \
                 WHERE kind = 'diary' \
                   AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone') \
                 ORDER BY created_at DESC \
                 LIMIT {limit}"
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let doc = rows::document(row)?;
                out.push(DiaryEntry::from_document(&doc));
            }
        }
        Ok(out)
    }

    /// List pinned active documents (for `wake_up`), newest / highest boost first.
    ///
    /// Order: boost DESC, updated_at DESC, title ASC. Excludes archived/tombstone.
    pub fn list_pinned_documents(&self, limit: usize) -> Result<Vec<Document>> {
        let limit = limit.max(1);
        let conn = self.lock()?;
        let sql = format!(
            "SELECT {DOCUMENT_SELECT} FROM documents \
             WHERE COALESCE(pinned, false) = true \
               AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone') \
             ORDER BY COALESCE(boost, 1.0) DESC, updated_at DESC, title ASC \
             LIMIT {limit}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let out = crate::db::rows::query_vec(&mut stmt, [], rows::document)?;
        Ok(out)
    }
}
