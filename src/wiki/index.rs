//! Index markdown rendering and small shared helpers.

use super::*;

/// Render index catalog as markdown (index.md analogue).
pub fn render_index_markdown(store: &Store) -> Result<String> {
    let entries = store.list_wiki_index()?;
    let mut out = String::from("# Wiki index\n\n");
    if entries.is_empty() {
        out.push_str("_empty catalog_\n");
        return Ok(out);
    }
    // group by category
    let mut by_cat: std::collections::BTreeMap<String, Vec<&WikiIndexEntry>> =
        std::collections::BTreeMap::new();
    for e in &entries {
        let cat = e.category.clone().unwrap_or_else(|| "uncategorized".into());
        by_cat.entry(cat).or_default().push(e);
    }
    for (cat, rows) in by_cat {
        out.push_str(&format!("## {cat}\n\n"));
        for e in rows {
            let sum = e.summary.as_deref().unwrap_or("");
            out.push_str(&format!(
                "- [[{}]] (`{}`, {}) - {}\n",
                e.title, e.slug, e.kind, sum
            ));
        }
        out.push('\n');
    }
    Ok(out)
}

pub(super) fn resolve_doc(store: &Store, id_or_uri: &str) -> Result<Document> {
    let s = id_or_uri.trim();
    if let Some(d) = store.get_document(s)? {
        return Ok(d);
    }
    if let Some(d) = store.find_by_uri(s)? {
        return Ok(d);
    }
    Err(AppError::not_found(format!(
        "document not found: {id_or_uri}"
    )))
}

pub(super) fn slugify(s: &str) -> String {
    shared_slugify(s, SlugPolicy::WikiPage)
}

pub(super) fn first_line(content: &str, max: usize) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let mut s: String = line.chars().take(max.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

// silence unused import if CompilePage only used via CompileResult
#[allow(dead_code)]
pub(super) fn _touch_compile_page(_: &crate::llm::CompilePage) {}
