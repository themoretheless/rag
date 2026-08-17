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
    /// Budget passed to [`pack_hits`] (applied to hit **content**).
    pub max_tokens: usize,
    /// How many input hits were left out entirely.
    pub omitted_count: usize,
    /// Formatted citation block suitable for prompt stuffing.
    pub context_text: String,
}

/// Pack ranked `hits` under a **content** token budget using ~4 chars/token.
///
/// Walks hits in the given order (caller should pass score-descending results).
/// Each hit costs `estimate_tokens(content)`. When the next full hit would
/// exceed the remaining budget, its content is truncated to fit if any budget
/// remains; otherwise packing stops. `max_tokens == 0` yields an empty pack.
///
/// Citation headers are included in [`PackedContext::context_text`] and
/// [`PackedContext::total_tokens`] but do not consume the content budget so
/// search `max_context_tokens` stays predictable.
pub fn pack_hits(hits: &[SearchHit], max_tokens: usize) -> PackedContext {
    if max_tokens == 0 || hits.is_empty() {
        return PackedContext {
            hits: Vec::new(),
            total_tokens: 0,
            max_tokens,
            omitted_count: hits.len(),
            context_text: String::new(),
        };
    }

    let mut packed: Vec<SearchHit> = Vec::new();
    let mut used = 0usize;

    for (i, hit) in hits.iter().enumerate() {
        let cost = estimate_tokens(&hit.content);

        if used + cost <= max_tokens {
            used += cost;
            packed.push(hit.clone());
            continue;
        }

        // Not enough room for the full hit. Only truncate when this is the first
        // hit (must return something useful); otherwise stop and omit the rest.
        if !packed.is_empty() {
            return finish_pack(packed, hits.len() - i, max_tokens);
        }

        let remaining = max_tokens; // used == 0
        let truncated_body = truncate_to_tokens(&hit.content, remaining);
        if truncated_body.is_empty() {
            return finish_pack(packed, hits.len() - i, max_tokens);
        }

        let mut truncated = hit.clone();
        truncated.snippet = Some(truncated_body.clone());
        truncated.content = truncated_body;
        packed.push(truncated);
        return finish_pack(packed, hits.len() - i - 1, max_tokens);
    }

    finish_pack(packed, 0, max_tokens)
}

fn finish_pack(packed: Vec<SearchHit>, omitted_count: usize, max_tokens: usize) -> PackedContext {
    let context_text = format_context_block(&packed);
    let total_tokens = estimate_tokens(&context_text);
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

/// Truncate `text` so `estimate_tokens` of the result is `<= max_tokens`.
///
/// Appends an ellipsis (`…`) when truncated and budget allows.
fn truncate_to_tokens(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 || text.is_empty() {
        return String::new();
    }
    if estimate_tokens(text) <= max_tokens {
        return text.to_string();
    }

    let max_chars = max_tokens.saturating_mul(CHARS_PER_TOKEN);
    if max_chars == 0 {
        return String::new();
    }

    let ellipsis = '…';
    let take = if max_chars > 1 {
        max_chars - 1
    } else {
        max_chars
    };

    let mut out: String = text.chars().take(take).collect();
    if take < text.chars().count() {
        if max_chars > 1 {
            out.push(ellipsis);
        }
        while estimate_tokens(&out) > max_tokens && !out.is_empty() {
            out.pop();
        }
        if !out.ends_with(ellipsis)
            && !out.is_empty()
            && estimate_tokens(&(out.clone() + "…")) <= max_tokens
        {
            out.push(ellipsis);
        }
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
    fn pack_content_budget_like_search() {
        // 40 chars → 10 tokens each; budget 15 fits only the first.
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
        let packed = pack_hits(&hits, 15);
        assert_eq!(packed.hits.len(), 1);
        assert_eq!(packed.hits[0].chunk_id, "1");
        assert_eq!(packed.omitted_count, 2);
    }

    #[test]
    fn pack_preserves_rank_order() {
        let hits = vec![
            hit("first", 1.0),
            hit("second", 0.5),
            hit("third", 0.1),
        ];
        let packed = pack_hits(&hits, 10_000);
        assert_eq!(packed.hits[0].content, "first");
        assert_eq!(packed.hits[1].content, "second");
        assert_eq!(packed.hits[2].content, "third");
    }

    #[test]
    fn truncate_respects_budget() {
        let text = "abcdefghijklmnopqrstuvwxyz";
        let t = truncate_to_tokens(text, 2);
        assert!(estimate_tokens(&t) <= 2);
        assert!(!t.is_empty());
    }

    #[test]
    fn truncates_oversized_first_hit() {
        let long = "x".repeat(100); // 25 tokens
        let hits = vec![hit(&long, 0.99)];
        let packed = pack_hits(&hits, 5);
        assert_eq!(packed.hits.len(), 1);
        assert!(estimate_tokens(&packed.hits[0].content) <= 5);
        assert!(packed.hits[0].content.chars().count() < 100);
    }

    #[test]
    fn total_tokens_matches_context_estimate() {
        let hits = vec![hit("hello world packed", 0.7)];
        let packed = pack_hits(&hits, 500);
        assert_eq!(packed.total_tokens, estimate_tokens(&packed.context_text));
        assert!(!packed.context_text.is_empty());
    }
}
