//! Wiki catalog listing: filters, count, bounded page and lean row mapping.

use super::*;
use crate::db::lin_engine::{cell_i64, cell_nonempty_string, cell_opt_string, cell_str, cell_string, cell_ts};

/// Filter, count, sort, and page `layer='wiki'` documents without loading `content`.
pub(super) fn fetch_wiki_page_meta_items(
    r: &impl LinReader,
    filter: &WikiPageMetaFilter,
) -> Result<(Vec<crate::models::WikiPageListItem>, usize)> {
    let want_kind = filter.kind.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let want_wing = filter.wing.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let want_room = filter.room.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let want_category_lc = filter
        .category
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase);
    let want_q_lc = filter
        .q
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase);

    let wiki_rows = r.find_by_eq("documents", "layer", "wiki")?;
    let mut matched: Vec<crate::models::WikiPageListItem> = Vec::new();

    for row in &wiki_rows {
        let row_kind = cell_str(row, "kind").unwrap_or("");
        if let Some(k) = want_kind {
            if crate::models::DocumentKind::Wiki == k {
                if !row_kind.is_empty() && row_kind != k {
                    continue;
                }
            } else if row_kind != k {
                continue;
            }
        }
        if let Some(w) = want_wing
            && cell_str(row, "wing") != Some(w)
        {
            continue;
        }
        if let Some(rm) = want_room
            && cell_str(row, "room") != Some(rm)
        {
            continue;
        }

        let metadata_json = cell_opt_string(row, "metadata_json").unwrap_or_else(|| "{}".into());
        let (summary, category) = meta_summary_category(&metadata_json);

        if let Some(ref cat_lc) = want_category_lc {
            let row_cat_lc = category
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .to_lowercase();
            if &row_cat_lc != cat_lc {
                continue;
            }
        }

        let id = cell_string(row, "id");
        let uri = cell_string(row, "uri");
        let title = cell_string(row, "title");
        let eff_kind = cell_nonempty_string(row, "kind")
            .unwrap_or_else(|| crate::models::DocumentKind::Wiki.as_str().into());

        if let Some(ref q_lc) = want_q_lc {
            let haystack = format!(
                "{}\n{}\n{}\n{}\n{}",
                title,
                uri,
                eff_kind,
                summary.as_deref().unwrap_or(""),
                category.as_deref().unwrap_or("")
            )
            .to_lowercase();
            if !haystack.contains(q_lc.as_str()) {
                continue;
            }
        }

        let revision = cell_i64(row, "revision", 1).max(1);
        let updated_at = cell_ts(row, "updated_at")?;
        let slug = uri
            .strip_prefix("wiki://")
            .unwrap_or(uri.as_str())
            .to_string();

        matched.push(crate::models::WikiPageListItem {
            id,
            uri,
            slug,
            title,
            kind: eff_kind,
            summary,
            category,
            revision,
            etag: crate::models::format_document_etag(revision),
            updated_at: updated_at.to_rfc3339(),
        });
    }

    let total = matched.len();
    matched.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.id.cmp(&b.id)));

    let offset = filter.offset.unwrap_or(0);
    let iter = matched.into_iter().skip(offset);
    let page: Vec<crate::models::WikiPageListItem> = if let Some(limit) = filter.limit {
        let limit = limit.clamp(1, crate::db::MAX_CATALOG_PAGE_SIZE);
        iter.take(limit).collect()
    } else {
        iter.collect()
    };

    Ok((page, total))
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
