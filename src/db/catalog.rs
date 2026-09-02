//! Lean document catalog and project dashboard queries.

use chrono::{DateTime, Utc};
use duckdb::types::ToSql;
use serde::{Deserialize, Serialize};

use super::{source_manifest::SourceRootSummary, store::Store};
use crate::error::{AppError, Result};
use crate::models::DrawerListItem;
use crate::util::parse_db_timestamp;

#[derive(Debug, Clone, Default)]
pub struct DocumentCatalogFilter {
    pub q: Option<String>,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub layer: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub include_archived: bool,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentCatalogPage {
    pub items: Vec<DrawerListItem>,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectHome {
    pub project_id: String,
    pub documents: u64,
    pub chunks: u64,
    pub raw_documents: u64,
    pub wiki_documents: u64,
    pub diary_documents: u64,
    pub archived_documents: u64,
    pub rooms: u64,
    pub source_files: u64,
    pub graph_nodes: u64,
    pub graph_edges: u64,
    pub unresolved_links: u64,
    pub source_roots: Vec<SourceRootSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<DateTime<Utc>>,
}

impl Store {
    /// Return one lean catalog page without loading document bodies.
    pub fn list_document_catalog(
        &self,
        filter: &DocumentCatalogFilter,
    ) -> Result<DocumentCatalogPage> {
        let (where_sql, binds) = catalog_where(filter, "d");
        let conn = self.lock()?;
        let count_sql = format!("SELECT COUNT(*)::BIGINT FROM documents d {where_sql}");
        let total: i64 = conn.query_row(&count_sql, dynamic_params(&binds).as_slice(), |row| {
            row.get(0)
        })?;

        let limit = filter.limit.clamp(1, 200);
        let sql = format!(
            r#"
            SELECT
              d.id, d.uri, d.title, d.wing, d.room, d.source_file,
              COALESCE(NULLIF(d.layer, ''), 'raw'),
              COALESCE(NULLIF(d.kind, ''), 'document'),
              d.content_hash, COALESCE(d.pinned, false),
              COALESCE(d.boost, 1.0), COALESCE(NULLIF(d.status, ''), 'active'),
              CAST(d.created_at AS VARCHAR), CAST(d.updated_at AS VARCHAR)
            FROM documents d
            {where_sql}
            ORDER BY COALESCE(d.pinned, false) DESC,
                     COALESCE(d.boost, 1.0) DESC,
                     d.updated_at DESC, d.title ASC, d.id ASC
            LIMIT {limit} OFFSET {}
            "#,
            filter.offset
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(dynamic_params(&binds).as_slice())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            items.push(DrawerListItem {
                id: row.get(0)?,
                uri: row.get(1)?,
                title: row.get(2)?,
                wing: row.get(3)?,
                room: row.get(4)?,
                source_file: row.get(5)?,
                layer: row.get(6)?,
                kind: row.get(7)?,
                content_hash: row.get(8)?,
                pinned: row.get(9)?,
                boost: row.get(10)?,
                status: row.get(11)?,
                created_at: timestamp(row.get(12)?)?,
                updated_at: timestamp(row.get(13)?)?,
            });
        }
        Ok(DocumentCatalogPage {
            items,
            total: total.max(0) as u64,
        })
    }

    /// Aggregate the useful health/freshness inventory for one project.
    pub fn project_home(&self, project_id: &str) -> Result<ProjectHome> {
        let project_id = project_id.trim();
        if project_id.is_empty() {
            return Err(AppError::config("project must be non-empty"));
        }
        let conn = self.lock()?;
        let row = conn.query_row(
            r#"
            SELECT
              COUNT(*)::BIGINT,
              COALESCE(SUM(CASE WHEN COALESCE(layer, 'raw') = 'raw' THEN 1 ELSE 0 END), 0)::BIGINT,
              COALESCE(SUM(CASE WHEN COALESCE(layer, '') = 'wiki' THEN 1 ELSE 0 END), 0)::BIGINT,
              COALESCE(SUM(CASE WHEN COALESCE(layer, '') = 'diary' OR COALESCE(kind, '') = 'diary' THEN 1 ELSE 0 END), 0)::BIGINT,
              COALESCE(SUM(CASE WHEN COALESCE(status, 'active') IN ('archived', 'tombstone') THEN 1 ELSE 0 END), 0)::BIGINT,
              COUNT(DISTINCT NULLIF(TRIM(room), ''))::BIGINT,
              COUNT(DISTINCT NULLIF(TRIM(source_file), ''))::BIGINT,
              CAST(MAX(updated_at) AS VARCHAR)
            FROM documents
            WHERE wing = ?
            "#,
            [project_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )?;
        let chunks: i64 = conn.query_row(
            "SELECT COUNT(*)::BIGINT FROM chunks c JOIN documents d ON d.id = c.document_id WHERE d.wing = ?",
            [project_id],
            |row| row.get(0),
        )?;
        let graph_nodes: i64 = conn.query_row(
            "SELECT COUNT(*)::BIGINT FROM graph_nodes n JOIN documents d ON d.id = n.document_id WHERE d.wing = ?",
            [project_id],
            |row| row.get(0),
        )?;
        let graph_edges: i64 = conn.query_row(
            r#"
            SELECT COUNT(*)::BIGINT
            FROM graph_edges e
            JOIN graph_nodes source ON source.id = e.source_id
            JOIN documents d ON d.id = source.document_id
            WHERE d.wing = ?
            "#,
            [project_id],
            |row| row.get(0),
        )?;
        let unresolved_links: i64 = conn.query_row(
            r#"
            SELECT COUNT(DISTINCT target.id)::BIGINT
            FROM graph_edges e
            JOIN graph_nodes source ON source.id = e.source_id
            JOIN documents d ON d.id = source.document_id
            JOIN graph_nodes target ON target.id = e.target_id
            WHERE d.wing = ? AND COALESCE(target.resolved, false) = false
            "#,
            [project_id],
            |row| row.get(0),
        )?;
        drop(conn);
        let source_roots = self.list_source_roots(Some(project_id))?;
        Ok(ProjectHome {
            project_id: project_id.to_string(),
            documents: count(row.0),
            chunks: count(chunks),
            raw_documents: count(row.1),
            wiki_documents: count(row.2),
            diary_documents: count(row.3),
            archived_documents: count(row.4),
            rooms: count(row.5),
            source_files: count(row.6),
            graph_nodes: count(graph_nodes),
            graph_edges: count(graph_edges),
            unresolved_links: count(unresolved_links),
            source_roots,
            last_updated_at: row.7.map(timestamp).transpose()?,
        })
    }
}

fn catalog_where(filter: &DocumentCatalogFilter, alias: &str) -> (String, Vec<String>) {
    let mut clauses = Vec::new();
    let mut binds = Vec::new();
    for (column, value) in [
        ("wing", filter.wing.as_deref()),
        ("room", filter.room.as_deref()),
        ("layer", filter.layer.as_deref()),
        ("kind", filter.kind.as_deref()),
        ("status", filter.status.as_deref()),
    ] {
        if let Some(value) = value.and_then(clean) {
            clauses.push(format!("{alias}.{column} = ?"));
            binds.push(value.to_string());
        }
    }
    if filter.status.as_deref().and_then(clean).is_none() && !filter.include_archived {
        clauses.push(format!(
            "COALESCE({alias}.status, 'active') NOT IN ('archived', 'tombstone')"
        ));
    }
    if let Some(query) = filter.q.as_deref().and_then(clean) {
        clauses.push(format!(
            "LOWER(COALESCE({alias}.title, '') || '\n' || COALESCE({alias}.uri, '') || '\n' || COALESCE({alias}.source_file, '')) LIKE ?"
        ));
        binds.push(format!("%{}%", query.to_lowercase()));
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    (where_sql, binds)
}

fn dynamic_params(values: &[String]) -> Vec<&dyn ToSql> {
    values.iter().map(|value| value as &dyn ToSql).collect()
}

fn clean(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn timestamp(raw: String) -> Result<DateTime<Utc>> {
    parse_db_timestamp(&raw)
        .ok_or_else(|| AppError::db(format!("invalid catalog timestamp: {raw}")))
}

fn count(value: i64) -> u64 {
    value.max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Chunk, Document};

    #[test]
    fn catalog_is_lean_filtered_and_paginated_and_home_is_scoped() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("catalog.duckdb")).unwrap();
        for (id, project, layer, status) in [
            ("a", "alpha", "raw", "active"),
            ("b", "alpha", "wiki", "active"),
            ("c", "alpha", "raw", "archived"),
            ("d", "beta", "raw", "active"),
        ] {
            let document = Document {
                id: id.into(),
                uri: format!("file:///{id}.md"),
                title: format!("Document {id}"),
                content: format!("private body {id}"),
                wing: Some(project.into()),
                room: Some("root".into()),
                source_file: Some(format!("/{project}/{id}.md")),
                layer: layer.into(),
                status: status.into(),
                ..Default::default()
            };
            store.upsert_document(&document).unwrap();
            store
                .insert_chunks(&[Chunk {
                    id: format!("chunk-{id}"),
                    document_id: id.into(),
                    chunk_index: 0,
                    content: document.content,
                    embedding: vec![1.0, 0.0],
                    char_start: 0,
                    char_end: 14,
                    metadata_json: "{}".into(),
                }])
                .unwrap();
        }

        let first = store
            .list_document_catalog(&DocumentCatalogFilter {
                wing: Some("alpha".into()),
                limit: 1,
                ..DocumentCatalogFilter::default()
            })
            .unwrap();
        assert_eq!(first.total, 2);
        assert_eq!(first.items.len(), 1);
        let second = store
            .list_document_catalog(&DocumentCatalogFilter {
                wing: Some("alpha".into()),
                q: Some("b.md".into()),
                limit: 50,
                ..DocumentCatalogFilter::default()
            })
            .unwrap();
        assert_eq!(second.total, 1);
        assert_eq!(second.items[0].id, "b");
        let home = store.project_home("alpha").unwrap();
        assert_eq!(home.documents, 3);
        assert_eq!(home.chunks, 3);
        assert_eq!(home.raw_documents, 2);
        assert_eq!(home.wiki_documents, 1);
        assert_eq!(home.archived_documents, 1);
        assert_eq!(home.source_files, 3);
    }
}
