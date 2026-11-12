//! Schema document, diary listings and pinned documents.

use super::*;
use crate::db::lin_engine::{
    cell_bool, cell_nonempty_string, cell_opt_string, cell_str, cell_string, cell_ts,
};

impl Store {
    /// Schema document (`uri=schema://agents`) if present.
    pub fn get_schema_document(&self) -> Result<Option<Document>> {
        if let Some(d) = self.find_by_uri("schema://agents")? {
            return Ok(Some(d));
        }
        let list = self.list_documents_by_layer("schema")?;
        Ok(list.into_iter().next())
    }

    /// Default wing for an agent diary: `agents/<agent_name>`.
    pub fn diary_wing_for_agent(agent_name: &str) -> String {
        format!("agents/{}", agent_name.trim())
    }

    /// List recent diary documents (`kind=diary`) for an agent, newest first.
    pub fn list_diary_entries(
        &self,
        agent_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>> {
        self.list_diary_entries_with(agent_name, limit)
    }

    /// Newest diary entries for activity summaries (`/v1/agents`, polled).
    pub fn list_diary_activity(
        &self,
        agent_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>> {
        let limit = limit.max(1);
        let agent = agent_name.map(str::trim).filter(|s| !s.is_empty());
        let expected_wing = agent.map(Self::diary_wing_for_agent);

        let r = self.read()?;
        let rows = r.find_by_eq("documents", "kind", "diary")?;
        let mut docs: Vec<Document> = Vec::new();
        for row in &rows {
            let status = cell_nonempty_string(row, "status").unwrap_or_else(|| "active".into());
            if status == "archived" || status == "tombstone" {
                continue;
            }
            if let (Some(name), Some(exp_wing)) = (agent, &expected_wing) {
                let matches = cell_str(row, "room") == Some(name)
                    || cell_str(row, "wing") == Some(exp_wing.as_str());
                if !matches {
                    continue;
                }
            }
            let created_at = cell_ts(row, "created_at")?;
            docs.push(Document {
                id: cell_string(row, "id"),
                room: cell_opt_string(row, "room"),
                wing: cell_opt_string(row, "wing"),
                metadata_json: cell_opt_string(row, "metadata_json").unwrap_or_default(),
                created_at,
                updated_at: created_at,
                ..Default::default()
            });
        }
        docs.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        docs.truncate(limit);
        Ok(docs.iter().map(DiaryEntry::from_document).collect())
    }

    fn list_diary_entries_with(
        &self,
        agent_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DiaryEntry>> {
        let limit = limit.max(1);
        let agent = agent_name.map(str::trim).filter(|s| !s.is_empty());
        let expected_wing = agent.map(Self::diary_wing_for_agent);

        let r = self.read()?;
        let rows = r.find_by_eq("documents", "kind", "diary")?;
        let mut docs: Vec<Document> = Vec::new();
        for row in &rows {
            let status = cell_nonempty_string(row, "status").unwrap_or_else(|| "active".into());
            if status == "archived" || status == "tombstone" {
                continue;
            }
            if let (Some(name), Some(exp_wing)) = (agent, &expected_wing) {
                let matches = cell_str(row, "room") == Some(name)
                    || cell_str(row, "wing") == Some(exp_wing.as_str());
                if !matches {
                    continue;
                }
            }
            docs.push(rows::document(row)?);
        }
        docs.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        docs.truncate(limit);
        Ok(docs.iter().map(DiaryEntry::from_document).collect())
    }

    /// List pinned active documents (for `wake_up`), newest / highest boost first.
    pub fn list_pinned_documents(&self, limit: usize) -> Result<Vec<Document>> {
        let limit = limit.max(1);
        let r = self.read()?;
        let mut out = r.filter_map_rows("documents", |row| {
            if !cell_bool(row, "pinned", false) {
                return Ok(None);
            }
            let status = cell_nonempty_string(row, "status").unwrap_or_else(|| "active".into());
            if status == "archived" || status == "tombstone" {
                return Ok(None);
            }
            Ok(Some(rows::document(row)?))
        })?;
        out.sort_by(|a, b| {
            b.boost
                .partial_cmp(&a.boost)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.title.cmp(&b.title))
                .then_with(|| a.id.cmp(&b.id))
        });
        out.truncate(limit);
        Ok(out)
    }
}
