//! Wiki pages filtered by their `knowledge_properties` metadata.

use duckdb::params_from_iter;

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
    let mut clauses = vec![
        "layer = 'wiki'".to_string(),
        "COALESCE(status,'active') NOT IN ('archived','tombstone')".into(),
    ];
    let mut values = Vec::new();
    if !filter.q.is_empty() {
        clauses.push("contains(lower(title || ' ' || uri), lower(?))".into());
        values.push(filter.q.clone());
    }
    if !filter.wing.is_empty() {
        clauses.push("wing = ?".into());
        values.push(filter.wing.clone());
    }
    for (name, value) in [
        ("document_type", &filter.document_type),
        ("review_status", &filter.review_status),
        ("owner", &filter.owner),
    ] {
        if !value.is_empty() {
            clauses.push(format!(
                "json_extract_string(TRY_CAST(metadata_json AS JSON), '$.knowledge_properties.{name}') = ?"
            ));
            values.push(value.clone());
        }
    }
    if !filter.due_before.is_empty() {
        clauses.push("TRY_CAST(json_extract_string(TRY_CAST(metadata_json AS JSON), '$.knowledge_properties.review_due') AS DATE) <= CAST(? AS DATE)".into());
        values.push(filter.due_before.clone());
    }
    let condition = clauses.join(" AND ");
    let conn = store.lock()?;
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM documents WHERE {condition}"),
        params_from_iter(values.iter()),
        |r| r.get(0),
    )?;
    let sql = format!(
        "SELECT id,uri,title,wing,revision,metadata_json FROM documents WHERE {condition} ORDER BY title,id LIMIT {KNOWLEDGE_PAGE_SIZE} OFFSET {}",
        filter.offset
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(values.iter()), |r| {
            Ok(KnowledgePageRow {
                id: r.get(0)?,
                uri: r.get(1)?,
                title: r.get(2)?,
                wing: r.get(3)?,
                revision: r.get(4)?,
                metadata_json: r.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok((total, rows))
}
