//! Wiki index catalog: upsert, rebuild, field updates and markdown rendering.

use super::*;

impl Store {
    /// Upsert a wiki index catalog entry (by primary key `id` or by `slug`).
    pub fn upsert_wiki_index_entry(&self, entry: &WikiIndexEntry) -> Result<()> {
        let conn = self.lock()?;
        upsert_wiki_index_entry_locked(&conn, entry)
    }

    /// List all wiki index entries (catalog).
    pub fn list_wiki_index(&self) -> Result<Vec<WikiIndexEntry>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id,
                   COALESCE(slug, COALESCE(label, id)),
                   COALESCE(title, COALESCE(label, id)),
                   COALESCE(kind, 'wiki'),
                   category, summary,
                   COALESCE(page_id, document_id),
                   CAST(updated_at AS VARCHAR)
            FROM wiki_index
            ORDER BY COALESCE(title, label, id) ASC
            "#,
        )?;
        let out = crate::db::rows::query_vec(&mut stmt, [], row_to_wiki_index)?;
        Ok(out)
    }

    /// Find wiki index entry by slug.
    pub fn get_wiki_index_by_slug(&self, slug: &str) -> Result<Option<WikiIndexEntry>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id,
                   COALESCE(slug, COALESCE(label, id)),
                   COALESCE(title, COALESCE(label, id)),
                   COALESCE(kind, 'wiki'),
                   category, summary,
                   COALESCE(page_id, document_id),
                   CAST(updated_at AS VARCHAR)
            FROM wiki_index
            WHERE slug = ? OR label = ? OR id = ?
            LIMIT 1
            "#,
        )?;
        let mut rows = stmt.query(params![slug, slug, slug])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_wiki_index(row)?)),
            None => Ok(None),
        }
    }

    /// Delete wiki index entry by id.
    pub fn delete_wiki_index_entry(&self, id: &str) -> Result<bool> {
        let conn = self.lock()?;
        let n = conn.execute("DELETE FROM wiki_index WHERE id = ?", params![id])?;
        Ok(n > 0)
    }

    /// Clear and rebuild wiki_index from all `layer=wiki` documents.
    ///
    /// One catalog row per wiki page with a one-line summary taken from content
    /// (or `metadata_json.summary` when present). Category from
    /// `metadata_json.category` when set.
    pub fn rebuild_wiki_index_from_docs(&self) -> Result<usize> {
        let docs = self.list_documents_by_layer("wiki")?;
        {
            let conn = self.lock()?;
            conn.execute("DELETE FROM wiki_index", [])?;
        }
        let now = Utc::now();
        let mut n = 0usize;
        for doc in docs {
            let slug = wiki_slug_from_uri(&doc.uri).unwrap_or_else(|| slugify(&doc.title));
            let (meta_summary, meta_category) = meta_summary_category(&doc.metadata_json);
            let summary =
                meta_summary.unwrap_or_else(|| crate::util::first_prose_line(&doc.content, 240));
            let entry = WikiIndexEntry {
                id: format!("idx-{}", doc.id),
                slug: slug.clone(),
                title: doc.title.clone(),
                kind: if doc.kind.is_empty() {
                    "wiki".into()
                } else {
                    doc.kind.to_string()
                },
                category: meta_category,
                summary: Some(summary),
                page_id: Some(doc.id.clone()),
                updated_at: now,
            };
            self.upsert_wiki_index_entry(&entry)?;
            n += 1;
        }
        Ok(n)
    }

    /// Create or merge-update a catalog entry keyed by `slug`.
    ///
    /// Empty optional fields leave existing values unchanged. On insert, missing
    /// title defaults to slug; kind defaults to `wiki`.
    pub fn update_wiki_index_entry_fields(
        &self,
        slug: &str,
        title: Option<String>,
        summary: Option<String>,
        kind: Option<String>,
        category: Option<String>,
        page_id: Option<String>,
    ) -> Result<WikiIndexEntry> {
        let slug_clean = slug.trim();
        if slug_clean.is_empty() {
            return Err(AppError::config(
                "update_index_entry: slug must be non-empty",
            ));
        }
        let slug_key = slugify(slug_clean);
        if slug_key.is_empty() {
            return Err(AppError::config(
                "update_index_entry: slug is empty after normalize",
            ));
        }

        let now = Utc::now();
        let mut entry = if let Some(e) = self.get_wiki_index_by_slug(&slug_key)? {
            e
        } else if let Some(e) = self.get_wiki_index_by_slug(slug_clean)? {
            e
        } else {
            WikiIndexEntry {
                id: format!("idx-slug-{}", slug_key),
                slug: slug_key.clone(),
                title: slug_key.clone(),
                kind: "wiki".into(),
                category: None,
                summary: None,
                page_id: None,
                updated_at: now,
            }
        };

        entry.slug = slug_key;
        if let Some(t) = title {
            let t = t.trim();
            if !t.is_empty() {
                entry.title = t.to_string();
            }
        }
        if let Some(s) = summary {
            entry.summary = Some(s);
        }
        if let Some(k) = kind {
            let k = k.trim();
            if !k.is_empty() {
                entry.kind = k.to_string();
            }
        }
        if let Some(c) = category {
            let c = c.trim();
            entry.category = if c.is_empty() {
                None
            } else {
                Some(c.to_string())
            };
        }
        if let Some(p) = page_id {
            let p = p.trim();
            entry.page_id = if p.is_empty() {
                None
            } else {
                Some(p.to_string())
            };
        }
        entry.updated_at = now;
        self.upsert_wiki_index_entry(&entry)?;
        Ok(entry)
    }

    /// Render the wiki index catalog as markdown (Karpathy `index.md` analogue).
    pub fn render_wiki_index_markdown(&self) -> Result<String> {
        let entries = self.list_wiki_index()?;
        let mut out = String::from("# Wiki index\n\n");
        if entries.is_empty() {
            out.push_str("_empty catalog_\n");
            return Ok(out);
        }
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
                    "- [[{}]] (`{}`, {}) — {}\n",
                    e.title, e.slug, e.kind, sum
                ));
            }
            out.push('\n');
        }
        Ok(out)
    }

    /// Index-first catalog lookup: rank `wiki_index` rows by simple term overlap
    /// against slug / title / summary / category / kind (case-insensitive).
    ///
    /// Empty `query` returns catalog entries in title order (score 1.0), limited
    /// by `limit`. Used by the `query_with_index` MCP tool.
    pub fn query_wiki_index(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::models::IndexQueryMatch>> {
        let limit = limit.max(1);
        let entries = self.list_wiki_index()?;
        let terms = tokenize_index_query(query);
        if terms.is_empty() {
            return Ok(entries
                .into_iter()
                .take(limit)
                .map(|entry| crate::models::IndexQueryMatch { entry, score: 1.0 })
                .collect());
        }

        let mut scored: Vec<(f32, WikiIndexEntry)> = entries
            .into_iter()
            .filter_map(|entry| {
                let score = score_index_entry(&entry, &terms);
                if score > 0.0 {
                    Some((score, entry))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.title.cmp(&b.1.title))
        });
        Ok(scored
            .into_iter()
            .take(limit)
            .map(|(score, entry)| crate::models::IndexQueryMatch { entry, score })
            .collect())
    }
}

/// Whitespace / punctuation tokenization for index query matching.
pub(super) fn tokenize_index_query(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

/// Field-weighted catalog score so top_k discriminates (slug > title > summary > kind).
///
/// Whole-token match (same tokenizer as query). Exact phrase on slug/title boosts hard.
pub(super) fn score_index_entry(entry: &WikiIndexEntry, terms: &[String]) -> f32 {
    if terms.is_empty() {
        return 1.0;
    }
    let slug_tokens = tokenize_index_query(&entry.slug);
    let title_tokens = tokenize_index_query(&entry.title);
    let summary_tokens = tokenize_index_query(entry.summary.as_deref().unwrap_or(""));
    let kind_tokens = tokenize_index_query(&entry.kind);
    let cat_tokens = tokenize_index_query(entry.category.as_deref().unwrap_or(""));

    let slug_lc = entry.slug.to_ascii_lowercase();
    let title_lc = entry.title.to_ascii_lowercase();
    let summary_lc = entry.summary.as_deref().unwrap_or("").to_ascii_lowercase();

    let mut score = 0.0f32;
    let mut terms_hit = 0u32;
    for t in terms {
        let mut hit = false;
        if slug_tokens.iter().any(|h| h == t) {
            score += 4.0;
            hit = true;
        }
        if title_tokens.iter().any(|h| h == t) {
            score += 3.0;
            hit = true;
        }
        if summary_tokens.iter().any(|h| h == t) {
            score += 1.5;
            hit = true;
        }
        if kind_tokens.iter().any(|h| h == t) || cat_tokens.iter().any(|h| h == t) {
            score += 0.5;
            hit = true;
        }
        // Substring on slug/title only (e.g. "spine" in "spine-tools")
        if !hit {
            if slug_lc.contains(t.as_str()) {
                score += 2.0;
                hit = true;
            } else if title_lc.contains(t.as_str()) {
                score += 1.5;
                hit = true;
            } else if summary_lc.contains(t.as_str()) {
                score += 0.75;
                hit = true;
            }
        }
        if hit {
            terms_hit += 1;
        }
    }
    if terms_hit == 0 {
        return 0.0;
    }

    // Average per query term so longer queries do not dominate by raw sum alone.
    score /= terms.len() as f32;

    // Full-phrase identity / containment boosts ranking head.
    let phrase = terms.join(" ");
    let slug_phrase = slug_tokens.join(" ");
    let title_phrase = title_tokens.join(" ");
    if slug_phrase == phrase || title_phrase == phrase {
        score += 2.0;
    } else if slug_lc.contains(&phrase) || title_lc.contains(&phrase) {
        score += 1.0;
    }
    // Prefer covering all query terms.
    if terms_hit as usize == terms.len() {
        score += 0.5;
    }
    score
}

pub(super) fn slugify(s: &str) -> String {
    shared_slugify(s, SlugPolicy::IndexLookup)
}

// ---------------------------------------------------------------------------
// Wiki catalog listing: SQL filter/count/page + lean DTO map
// (kept free of search ranking and CAS upsert paths)
// ---------------------------------------------------------------------------

pub(super) fn upsert_wiki_index_entry_locked(
    conn: &Connection,
    entry: &WikiIndexEntry,
) -> Result<()> {
    let title = if entry.title.is_empty() {
        entry.slug.clone()
    } else {
        entry.title.clone()
    };
    let label = title.clone();
    let page_id = entry.page_id.clone();
    let document_id = page_id.clone();
    conn.execute(
        r#"
        INSERT OR REPLACE INTO wiki_index
          (id, slug, title, label, kind, summary, category, document_id, page_id, updated_at)
        VALUES
          (?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))
        "#,
        params![
            entry.id,
            entry.slug,
            title,
            label,
            entry.kind,
            entry.summary,
            entry.category,
            document_id,
            page_id,
            format_ts(entry.updated_at),
        ],
    )?;
    Ok(())
}

pub(super) fn row_to_wiki_index(row: &duckdb::Row<'_>) -> Result<WikiIndexEntry> {
    let updated_raw: String = row.get(7)?;
    Ok(WikiIndexEntry {
        id: row.get(0)?,
        slug: row.get(1)?,
        title: row.get(2)?,
        kind: row.get(3)?,
        category: row.get(4)?,
        summary: row.get(5)?,
        page_id: row.get(6)?,
        updated_at: parse_ts(&updated_raw)?,
    })
}
