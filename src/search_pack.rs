//! Token-budget packing for search hits.
//!
//! Estimates tokens as ~4 characters per token (ceil). Used by search
//! (`max_context_tokens`) and the optional `pack_context` MCP tool so ranked
//! hits cannot overflow a model window.

use crate::models::SearchHit;

/// Rough characters-per-token estimate (common English heuristic).
pub const CHARS_PER_TOKEN: usize = 4;

/// Estimate token count from text length: `ceil(char_count / 4)`.
///
/// Empty input yields `0`. Uses Unicode scalar count (`chars().count()`), not bytes.
pub fn estimate_tokens(text: &str) -> usize {
    let n = text.chars().count();
    if n == 0 {
        0
    } else {
        n.div_ceil(CHARS_PER_TOKEN)
    }
}

/// Result of packing ranked hits under a token budget.
#[derive(Debug, Clone)]
pub struct PackedContext {
    /// Hits that fit (order preserved; last may be content-truncated).
    pub hits: Vec<SearchHit>,
    /// Estimated tokens of [`Self::context_text`] (includes citation headers).
    pub total_tokens: usize,
    /// Budget passed to [`pack_hits`] (full formatted prompt block).
    pub max_tokens: usize,
    /// How many input hits were left out entirely.
    pub omitted_count: usize,
    /// Formatted citation block suitable for prompt stuffing.
    pub context_text: String,
}

/// Pack ranked `hits` under a complete prompt-block budget (~4 chars/token).
///
/// Citation headers, separators, and expanded source text all consume the same
/// budget. Rank order is preserved; the last retained body may be truncated.
/// If even the first citation and one content character cannot fit, no hit is
/// returned. `max_tokens == 0` also returns an empty pack.
///
/// Packed hits have a single body: `content`. Redundant `context` and `snippet`
/// fields are cleared so expansion cannot hide unbudgeted source text there.
/// This bounds the prompt block, not JSON syntax or ranking/provenance metadata.
/// The character heuristic is not a guarantee about any particular tokenizer.
pub fn pack_hits(hits: &[SearchHit], max_tokens: usize) -> PackedContext {
    let max_chars = max_tokens.saturating_mul(CHARS_PER_TOKEN);
    let mut packed = Vec::new();
    let mut used_chars = 0usize;
    for hit in hits {
        let header = format_citation_header(packed.len() + 1, hit);
        let separator = if packed.is_empty() { 0 } else { 2 };
        let overhead = header.chars().count().saturating_add(1 + separator);
        let remaining = max_chars.saturating_sub(used_chars);
        if overhead > remaining || (!hit.content.is_empty() && overhead == remaining) {
            break;
        }
        let body_limit = remaining - overhead;
        let content = truncate_to_chars(&hit.content, body_limit);
        let truncated = content != hit.content;
        used_chars += overhead + content.chars().count();
        let mut retained = hit.clone();
        retained.content = content;
        retained.context = None;
        retained.snippet = None;
        packed.push(retained);
        if truncated {
            break;
        }
    }
    let omitted = hits.len() - packed.len();
    finish_pack(packed, omitted, max_tokens)
}

fn finish_pack(packed: Vec<SearchHit>, omitted_count: usize, max_tokens: usize) -> PackedContext {
    let context_text = format_context_block(&packed);
    let total_tokens = estimate_tokens(&context_text);
    debug_assert!(total_tokens <= max_tokens);
    PackedContext {
        hits: packed,
        total_tokens,
        max_tokens,
        omitted_count,
        context_text,
    }
}

/// Build a multi-hit citation block from packed hits.
pub fn format_context_block(hits: &[SearchHit]) -> String {
    hits.iter()
        .enumerate()
        .map(|(i, hit)| {
            let header = format_citation_header(i + 1, hit);
            format!("{header}\n{}", hit.content)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Format a single-hit citation line: `[n] title (uri) score=… chunk=…`.
fn format_citation_header(index: usize, hit: &SearchHit) -> String {
    let title = if hit.document_title.is_empty() {
        "untitled"
    } else {
        hit.document_title.as_str()
    };
    let uri = if hit.document_uri.is_empty() {
        "-"
    } else {
        hit.document_uri.as_str()
    };
    format!(
        "[{index}] {title} ({uri}) score={:.4} chunk={}",
        hit.score, hit.chunk_index
    )
}

/// Unicode-safe truncation; the ellipsis itself consumes one character.
fn truncate_to_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        out.pop();
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(content: &str, score: f32) -> SearchHit {
        SearchHit {
            chunk_id: "c1".into(),
            document_id: "d1".into(),
            document_title: "Doc".into(),
            document_uri: "doc://1".into(),
            chunk_index: 0,
            content: content.into(),
            score,
            ..Default::default()
        }
    }

    #[test]
    fn estimate_tokens_ceil_div() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        assert_eq!(estimate_tokens("abcdefghi"), 3);
    }

    #[test]
    fn pack_empty_and_zero_budget() {
        let hits = vec![hit("hello world", 0.9)];
        let empty = pack_hits(&[], 100);
        assert!(empty.hits.is_empty());
        assert_eq!(empty.omitted_count, 0);

        let zero = pack_hits(&hits, 0);
        assert!(zero.hits.is_empty());
        assert_eq!(zero.omitted_count, 1);
        assert!(zero.context_text.is_empty());
    }

    #[test]
    fn pack_fits_all_under_large_budget() {
        let hits = vec![
            hit("alpha content here", 0.9),
            hit("beta content here!", 0.8),
        ];
        let packed = pack_hits(&hits, 10_000);
        assert_eq!(packed.hits.len(), 2);
        assert_eq!(packed.omitted_count, 0);
        assert!(packed.context_text.contains("[1]"));
        assert!(packed.context_text.contains("[2]"));
        assert!(packed.context_text.contains("alpha content"));
        assert!(packed.context_text.contains("beta content"));
    }

    #[test]
    fn pack_complete_budget_like_search() {
        // Citation overhead plus body consumes the same budget.
        let hits = vec![
            SearchHit {
                chunk_id: "1".into(),
                content: "abcd".repeat(10),
                score: 1.0,
                ..Default::default()
            },
            SearchHit {
                chunk_id: "2".into(),
                content: "efgh".repeat(10),
                score: 0.9,
                ..Default::default()
            },
            SearchHit {
                chunk_id: "3".into(),
                content: "ijkl".repeat(10),
                score: 0.8,
                ..Default::default()
            },
        ];
        let packed = pack_hits(&hits, 20);
        assert_eq!(packed.hits.len(), 1);
        assert_eq!(packed.hits[0].chunk_id, "1");
        assert_eq!(packed.omitted_count, 2);
    }

    #[test]
    fn pack_preserves_rank_order() {
        let hits = vec![hit("first", 1.0), hit("second", 0.5), hit("third", 0.1)];
        let packed = pack_hits(&hits, 10_000);
        assert_eq!(packed.hits[0].content, "first");
        assert_eq!(packed.hits[1].content, "second");
        assert_eq!(packed.hits[2].content, "third");
    }

    #[test]
    fn truncate_respects_budget() {
        let text = "abcdefghijklmnopqrstuvwxyz";
        let t = truncate_to_chars(text, 8);
        assert!(estimate_tokens(&t) <= 2);
        assert!(!t.is_empty());
    }

    #[test]
    fn truncates_oversized_first_hit() {
        let long = "x".repeat(100); // 25 tokens
        let hits = vec![hit(&long, 0.99)];
        let packed = pack_hits(&hits, 20);
        assert_eq!(packed.hits.len(), 1);
        assert!(packed.total_tokens <= 20);
        assert!(packed.hits[0].content.chars().count() < 100);
    }

    #[test]
    fn total_tokens_matches_context_estimate() {
        let hits = vec![hit("hello world packed", 0.7)];
        let packed = pack_hits(&hits, 500);
        assert_eq!(packed.total_tokens, estimate_tokens(&packed.context_text));
        assert!(!packed.context_text.is_empty());
    }

    #[test]
    fn tiny_budget_does_not_return_unbudgeted_citations() {
        let hits = vec![hit("source text", 0.7)];
        for budget in [0, 1, 2, 5] {
            let packed = pack_hits(&hits, budget);
            assert!(packed.hits.is_empty());
            assert_eq!(packed.total_tokens, 0);
            assert_eq!(packed.omitted_count, 1);
        }
    }

    #[test]
    fn complete_budget_includes_unicode_headers_separators_and_last_body() {
        let mut source = hit(&"Русский текст 🦀 ".repeat(60), 0.8);
        source.document_title = "Заголовок 🧪".into();
        source.document_uri = "wiki://тест".into();
        let hits = vec![hit("first short excerpt", 1.0), source];
        for budget in 0..200 {
            let packed = pack_hits(&hits, budget);
            assert!(packed.total_tokens <= budget, "budget={budget}");
            assert_eq!(packed.total_tokens, estimate_tokens(&packed.context_text));
            assert_eq!(packed.context_text, format_context_block(&packed.hits));
            assert_eq!(packed.omitted_count + packed.hits.len(), hits.len());
        }
    }

    #[test]
    fn packed_expansion_has_no_hidden_full_context_or_snippet() {
        let full = "private source paragraph ".repeat(2_000);
        let mut source = hit(&full, 0.8);
        source.snippet = Some(full.clone());
        source.context = Some(vec![crate::models::SearchContextChunk {
            chunk_id: "expanded".into(),
            chunk_index: 1,
            content: full,
            heading_path: None,
            section: None,
        }]);
        let packed = pack_hits(&[source], 32);
        assert_eq!(packed.hits.len(), 1);
        assert!(packed.total_tokens <= 32);
        assert!(packed.hits[0].context.is_none());
        assert!(packed.hits[0].snippet.is_none());
        assert!(packed.hits[0].content.chars().count() < 128);
        let wire = serde_json::to_string(&packed.hits).unwrap();
        assert!(!wire.contains("expanded"));
        assert!(!wire.contains("\"context\""));
    }
}
