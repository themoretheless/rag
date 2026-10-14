//! Filing a useful answer back into the wiki with citations.

use super::*;

/// Characters of the filed question quoted in a citation line (ellipsis included).
const QUESTION_PREVIEW_MAX_CHARS: usize = 160;

/// One citation attached to a filed answer (metadata + optional body lines).
#[derive(Debug, Clone, Serialize, serde::Deserialize, Default)]
pub struct FileAnswerCitation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl FileAnswerCitation {
    /// Human-readable bullet for the page body.
    pub fn display_line(&self) -> String {
        let label = self
            .title
            .as_deref()
            .or(self.uri.as_deref())
            .or(self.document_id.as_deref())
            .unwrap_or("source");
        let mut line = label.to_string();
        if let Some(uri) = self.uri.as_deref().filter(|u| !u.is_empty()) {
            if self.title.is_some() {
                line.push_str(&format!(" ({uri})"));
            }
        }
        if let Some(q) = self.quote.as_deref().filter(|s| !s.is_empty()) {
            let short = crate::util::truncate_chars(q, QUESTION_PREVIEW_MAX_CHARS);
            line.push_str(&format!(": \"{short}\""));
        }
        if let Some(n) = self.note.as_deref().filter(|s| !s.is_empty()) {
            line.push_str(&format!(" - {n}"));
        }
        line
    }
}

/// Persist a cited answer as a wiki page: body + citations metadata, graph rebuild, ops log, index touch.
// Compatibility adapter: preserve the established public call shape.
#[allow(clippy::too_many_arguments)]
pub async fn file_answer(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    title: &str,
    body: &str,
    slug: Option<&str>,
    citations: Option<Vec<FileAnswerCitation>>,
    agent: Option<&str>,
) -> Result<WikiWriteResult> {
    if title.trim().is_empty() {
        return Err(AppError::config("file_answer requires a non-empty title"));
    }
    if body.trim().is_empty() {
        return Err(AppError::config("file_answer requires a non-empty body"));
    }

    let slug_src = slug
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(title);
    let slug = slugify(slug_src);
    if slug.is_empty() {
        return Err(AppError::config("file_answer could not derive a wiki slug"));
    }

    let cites = citations.unwrap_or_default();
    let mut page_body = body.trim_end().to_string();
    if !cites.is_empty() {
        // Avoid duplicating an existing Citations section.
        if !page_body.to_ascii_lowercase().contains("## citations") {
            page_body.push_str("\n\n## Citations\n");
            for c in &cites {
                page_body.push_str(&format!("- {}\n", c.display_line()));
            }
        }
    }

    let cites_json = serde_json::to_value(&cites).unwrap_or_else(|_| serde_json::json!([]));
    let summary = first_line(body, 240);

    write_wiki_page_with_opts(
        store,
        embedder,
        config,
        &slug,
        title,
        &page_body,
        "wiki",
        Some("answers"),
        Some(&summary),
        agent,
        WriteWikiOpts {
            op: Some("file_answer".into()),
            metadata_json_override: None,
            prefix: Some("FILE".into()),
            message: Some(format!("filed answer wiki://{slug}")),
            extra_metadata: Some(serde_json::json!({
                "citations": cites_json,
                "filed_as": "answer",
            })),
            extra_payload: Some(serde_json::json!({
                "citation_count": cites.len(),
            })),
            if_match_revision: None,
        },
    )
    .await
}
