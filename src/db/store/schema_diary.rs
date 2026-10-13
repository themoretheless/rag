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
        self.list_diary_entries_with(agent_name, limit)
    }

    /// Newest diary entries for activity summaries (`/v1/agents`, polled):
    /// only `id`, `agent_name`, `wing` and `created_at` are filled; `title`
    /// and `content` are empty and `updated_at` equals `created_at`.
    ///
    /// Reads just the placement, metadata and creation-time columns, and
    /// resolves the agent name with [`DiaryEntry::from_document`] itself, so
    /// it matches the full entries exactly (including Unicode whitespace).
    /// About 3x cheaper than loading the full row projection.
    pub(crate) fn list_diary_activity(
        &self,
        agent_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>> {
        let limit = limit.max(1);
        let agent = agent_name.map(str::trim).filter(|s| !s.is_empty());
        let (agent_filter, binds) = match agent {
            Some(name) => (
                "AND (room = ? OR wing = ?)",
                vec![name.to_string(), Self::diary_wing_for_agent(name)],
            ),
            None => ("", Vec::new()),
        };
        let sql = format!(
            "SELECT id, room, wing, \
                    metadata_json, \
                    CAST(created_at AS VARCHAR) \
             FROM documents \
             WHERE kind = 'diary' {agent_filter} \
               AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone') \
             ORDER BY created_at DESC, id DESC \
             LIMIT {limit}"
        );
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(duckdb::params_from_iter(binds.iter()))?;
        let mut out = Vec::new();
        crate::db::rows::visit_rows(&mut rows, |row| {
            let created_at = crate::db::rows::timestamp(&row.get::<_, String>(4)?)?;
            let doc = Document {
                id: row.get(0)?,
                room: row.get(1)?,
                wing: row.get(2)?,
                metadata_json: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                created_at,
                updated_at: created_at,
                ..Default::default()
            };
            out.push(DiaryEntry::from_document(&doc));
            Ok(())
        })?;
        Ok(out)
    }

    fn list_diary_entries_with(
        &self,
        agent_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>> {
        let select = DOCUMENT_SELECT;
        let limit = limit.max(1);
        let agent = agent_name.map(str::trim).filter(|s| !s.is_empty());

        let conn = self.lock()?;
        let mut out = Vec::new();
        if let Some(name) = agent {
            let wing = Self::diary_wing_for_agent(name);
            let sql = format!(
                "SELECT {select} FROM documents \
                 WHERE kind = 'diary' \
                   AND (room = ? OR wing = ?) \
                   AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone') \
                 ORDER BY created_at DESC, id DESC \
                 LIMIT {limit}"
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(params![name, wing])?;
            crate::db::rows::visit_rows(&mut rows, |row| {
                let doc = rows::document(row)?;
                out.push(DiaryEntry::from_document(&doc));
                Ok(())
            })?;
        } else {
            let sql = format!(
                "SELECT {select} FROM documents \
                 WHERE kind = 'diary' \
                   AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone') \
                 ORDER BY created_at DESC, id DESC \
                 LIMIT {limit}"
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            crate::db::rows::visit_rows(&mut rows, |row| {
                let doc = rows::document(row)?;
                out.push(DiaryEntry::from_document(&doc));
                Ok(())
            })?;
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
