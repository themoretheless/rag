//! Wiki pages filtered by their `knowledge_properties` metadata over `lin`.

use chrono::NaiveDate;

use super::lin_engine::{cell_i64, cell_opt_string, cell_str, cell_string, LinReader};
use crate::db::Store;
use crate::error::Result;

/// Filter values are matched exactly; empty strings mean "any".
#[derive(Debug, Clone, Default)]
pub struct KnowledgePageQuery {
    pub q: String,
    pub wing: String,
    pub document_type: String,
    pub review_status: String,
    pub owner: String,
    pub due_before: String,
    pub offset: u32,
}

/// One active wiki page with its raw metadata JSON.
#[derive(Debug, Clone)]
pub struct KnowledgePageRow {
    pub id: String,
    pub uri: String,
    pub title: String,
    pub wing: Option<String>,
    pub revision: i64,
    pub metadata_json: String,
}

/// Page size of [`list_knowledge_pages`].
pub const KNOWLEDGE_PAGE_SIZE: u32 = 50;

/// Matching total and one title-ordered page of active wiki pages.
pub fn list_knowledge_pages(
    store: &Store,
    filter: &KnowledgePageQuery,
) -> Result<(i64, Vec<KnowledgePageRow>)> {
    let q_lower = (!filter.q.is_empty()).then(|| filter.q.to_lowercase());
    let due_cutoff = if !filter.due_before.is_empty() {
        NaiveDate::parse_from_str(filter.due_before.trim(), "%Y-%m-%d").ok()
    } else {
        None
    };

    let needs_json = !filter.document_type.is_empty()
        || !filter.review_status.is_empty()
        || !filter.owner.is_empty()
        || !filter.due_before.is_empty();

    let r = store.read()?;
    let wiki_rows = r.find_by_eq("documents", "layer", "wiki")?;
    let mut matched = Vec::new();

    for row in &wiki_rows {
        let status = cell_str(row, "status").unwrap_or("active");
        if matches!(status, "archived" | "tombstone") {
            continue;
        }
        if !filter.wing.is_empty() && cell_str(row, "wing") != Some(filter.wing.as_str()) {
            continue;
        }
        let title = cell_string(row, "title");
        let uri = cell_string(row, "uri");
        if let Some(ref ql) = q_lower {
            let haystack = format!("{title} {uri}").to_lowercase();
            if !haystack.contains(ql) {
                continue;
            }
        }
        let metadata_json = cell_string(row, "metadata_json");
        if needs_json {
            let parsed = serde_json::from_str::<serde_json::Value>(&metadata_json).ok();
            let kp = parsed
                .as_ref()
                .and_then(|v| v.get("knowledge_properties"))
                .and_then(|v| v.as_object());

            let prop_str = |name: &str| -> Option<String> {
                let val = kp?.get(name)?;
                match val {
                    serde_json::Value::String(s) => Some(s.clone()),
                    serde_json::Value::Null => None,
                    other => Some(other.to_string()),
                }
            };

            if !filter.document_type.is_empty()
                && prop_str("document_type").as_deref() != Some(filter.document_type.as_str())
            {
                continue;
            }
            if !filter.review_status.is_empty()
                && prop_str("review_status").as_deref() != Some(filter.review_status.as_str())
            {
                continue;
            }
            if !filter.owner.is_empty()
                && prop_str("owner").as_deref() != Some(filter.owner.as_str())
            {
                continue;
            }
            if !filter.due_before.is_empty() {
                let Some(cutoff) = due_cutoff else {
                    continue;
                };
                let Some(raw_due) = prop_str("review_due") else {
                    continue;
                };
                let due_prefix = raw_due.trim().get(..10).unwrap_or(raw_due.trim());
                let Ok(due_date) = NaiveDate::parse_from_str(due_prefix, "%Y-%m-%d") else {
                    continue;
                };
                if due_date > cutoff {
                    continue;
                }
            }
        }

        matched.push(KnowledgePageRow {
            id: cell_string(row, "id"),
            uri,
            title,
            wing: cell_opt_string(row, "wing"),
            revision: cell_i64(row, "revision", 1),
            metadata_json,
        });
    }

    matched.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.id.cmp(&b.id)));
    let total = matched.len() as i64;
    let rows = matched
        .into_iter()
        .skip(filter.offset as usize)
        .take(KNOWLEDGE_PAGE_SIZE as usize)
        .collect();
    Ok((total, rows))
}
