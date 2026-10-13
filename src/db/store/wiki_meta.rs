//! Wiki catalog listing: SQL filters, count, bounded page and lean row mapping.

use super::*;

/// SQL filters, total count, bounded page, and lean row projection.
///
/// SELECT intentionally omits `content` so list stays lean (ISP vs full [`Document`]).
pub(super) fn fetch_wiki_page_meta_items(
    conn: &Connection,
    filter: &WikiPageMetaFilter,
) -> Result<(Vec<crate::models::WikiPageListItem>, usize)> {
    let mut predicates = vec!["layer = 'wiki'".to_string()];
    let mut binds: Vec<String> = Vec::new();

    if let Some(ref kind) = filter.kind {
        let kind = kind.trim();
        if !kind.is_empty() {
            // Match stored kind; empty DB kind is projected as "wiki" in the DTO map.
            if crate::models::DocumentKind::Wiki == kind {
                predicates.push("(COALESCE(kind, '') = '' OR kind = ?)".into());
            } else {
                predicates.push("kind = ?".into());
            }
            binds.push(kind.to_string());
        }
    }
    if let Some(ref wing) = filter.wing {
        let wing = wing.trim();
        if !wing.is_empty() {
            predicates.push("wing = ?".into());
            binds.push(wing.to_string());
        }
    }
    if let Some(ref room) = filter.room {
        let room = room.trim();
        if !room.is_empty() {
            predicates.push("room = ?".into());
            binds.push(room.to_string());
        }
    }
    if let Some(ref category) = filter.category {
        let category = category.trim();
        if !category.is_empty() {
            predicates.push(
                "LOWER(TRIM(COALESCE(json_extract_string(TRY_CAST(metadata_json AS JSON), '$.category'), ''))) = ?"
                    .into(),
            );
            binds.push(category.to_lowercase());
        }
    }
    if let Some(ref query) = filter.q {
        let query = query.trim();
        if !query.is_empty() {
            predicates.push(
                r#"contains(
                    LOWER(
                        COALESCE(title, '') || '\n' ||
                        COALESCE(uri, '') || '\n' ||
                        COALESCE(NULLIF(kind, ''), 'wiki') || '\n' ||
                        COALESCE(json_extract_string(TRY_CAST(metadata_json AS JSON), '$.summary'), '') || '\n' ||
                        COALESCE(json_extract_string(TRY_CAST(metadata_json AS JSON), '$.category'), '')
                    ),
                    ?
                )"#
                .into(),
            );
            binds.push(query.to_lowercase());
        }
    }

    let where_sql = format!("WHERE {}", predicates.join(" AND "));
    let count_sql = format!("SELECT COUNT(*)::BIGINT FROM documents {where_sql}");
    let params_dyn: Vec<&dyn duckdb::types::ToSql> = binds
        .iter()
        .map(|s| s as &dyn duckdb::types::ToSql)
        .collect();
    let total: i64 = conn.query_row(&count_sql, params_dyn.as_slice(), |row| row.get(0))?;

    let mut sql = format!(
        r#"
        SELECT id, uri, title, kind,
               metadata_json,
               COALESCE(revision, 1),
               CAST(updated_at AS VARCHAR)
        FROM documents
        {where_sql}
        ORDER BY title ASC, id ASC
        "#
    );
    if let Some(limit) = filter.limit {
        let limit = limit.clamp(1, crate::db::MAX_CATALOG_PAGE_SIZE);
        sql.push_str(&format!(" LIMIT {limit}"));
    }
    if let Some(offset) = filter.offset.filter(|offset| *offset > 0) {
        if filter.limit.is_none() {
            sql.push_str(" LIMIT 9223372036854775807");
        }
        sql.push_str(&format!(" OFFSET {offset}"));
    }

    let mut stmt = conn.prepare(&sql)?;
    let out = crate::db::rows::query_vec(&mut stmt, params_dyn.as_slice(), map_wiki_page_meta_row)?;
    Ok((out, total.max(0) as usize))
}

/// Project one catalog SELECT row into lean [`crate::models::WikiPageListItem`] (no body).
pub(super) fn map_wiki_page_meta_row(
    row: &duckdb::Row<'_>,
) -> Result<crate::models::WikiPageListItem> {
    let id: String = row.get(0)?;
    let uri: String = row.get(1)?;
    let title: String = row.get(2)?;
    let kind: String = row
        .get::<_, Option<String>>(3)?
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::models::DocumentKind::Wiki.as_str().into());
    let metadata_json: String = row.get(4)?;
    let revision: i64 = row.get::<_, Option<i64>>(5)?.unwrap_or(1);
    let updated_raw: String = row.get(6)?;
    let updated_at = parse_ts(&updated_raw)?;
    let slug = uri
        .strip_prefix("wiki://")
        .unwrap_or(uri.as_str())
        .to_string();
    let (summary, category) = meta_summary_category(&metadata_json);
    Ok(crate::models::WikiPageListItem {
        id,
        uri,
        slug,
        title,
        kind,
        summary,
        category,
        revision,
        etag: crate::models::format_document_etag(revision),
        updated_at: updated_at.to_rfc3339(),
    })
}

/// Pull optional `summary` / `category` strings from document metadata JSON.
pub(super) fn meta_summary_category(metadata_json: &str) -> (Option<String>, Option<String>) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(metadata_json) else {
        return (None, None);
    };
    let summary = v
        .get("summary")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let category = v
        .get("category")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    (summary, category)
}
