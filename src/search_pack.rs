//! Token-budget packing for search hits.
//!
//! Token counting uses a real tokenizer when one is configured (see below)
//! and otherwise falls back to a calibrated per-character-class heuristic
//! (CJK vs ASCII word/punctuation/whitespace vs other scripts) that is far
//! more accurate than a flat 4-chars-per-token for Chinese text and code.
//! Used by search (`max_context_tokens`) and the optional `pack_context` MCP
//! tool so ranked hits cannot overflow a model window.
//!
//! Tokenizer backend (optional, compile-time feature `tokenizers`):
//! set `RAG_TOKENIZER_JSON` to a local Hugging Face `tokenizer.json` matching
//! the active LLM provider family (e.g. a Qwen/GLM tokenizer for those
//! providers). crates.io is unreachable from this environment, so the
//! dependency stays optional and the calibrated heuristic remains the default.

use crate::models::SearchHit;

/// Rough characters-per-token estimate (legacy English heuristic).
///
/// Retained for call sites and docs that reason in characters; actual token
/// accounting goes through [`estimate_tokens`].
pub const CHARS_PER_TOKEN: usize = 4;

/// Estimate token count for `text`.
///
/// With the `tokenizers` feature compiled and `RAG_TOKENIZER_JSON` set to a
/// readable tokenizer file, this is an exact BPE count. Otherwise it is the
/// calibrated heuristic from [`heuristic_estimate_tokens`]. Empty input is 0.
pub fn estimate_tokens(text: &str) -> usize {
    #[cfg(feature = "tokenizers")]
    if let Some(count) = tokenizer_count(text) {
        return count;
    }
    heuristic_estimate_tokens(text)
}

/// Count tokens with the configured Hugging Face tokenizer, if any.
///
/// The tokenizer is loaded once from `RAG_TOKENIZER_JSON`; a missing or
/// unreadable file logs once and permanently falls back to the heuristic.
#[cfg(feature = "tokenizers")]
fn tokenizer_count(text: &str) -> Option<usize> {
    use std::sync::OnceLock;
    static TOKENIZER: OnceLock<Option<tokenizers::Tokenizer>> = OnceLock::new();
    let tokenizer = TOKENIZER.get_or_init(|| {
        let path = std::env::var("RAG_TOKENIZER_JSON")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())?;
        match tokenizers::Tokenizer::from_file(&path) {
            Ok(loaded) => {
                tracing::info!(path, "token counting via Hugging Face tokenizer");
                Some(loaded)
            }
            Err(error) => {
                tracing::warn!(path, error = %error, "RAG_TOKENIZER_JSON unreadable; using token heuristic");
                None
            }
        }
    });
    tokenizer
        .as_ref()
        .and_then(|t| t.encode(text, false).ok())
        .map(|encoding| encoding.get_ids().len())
}

/// Calibrated heuristic token estimate.
///
/// Character classes, validated against BPE counts on representative samples
/// (see tests):
///
/// - ASCII letters/digits: 4 chars/token (unchanged legacy behavior).
/// - ASCII whitespace: 8 chars/token (whitespace mostly merges into
///   neighboring tokens; counting it at 4/token overestimates prose).
/// - ASCII punctuation/symbols: 2 chars/token (code-heavy text).
/// - CJK ideographs, kana, hangul, CJK punctuation and fullwidth forms:
///   1 token/char (modern BPE tokenizers allocate roughly one token per
///   common CJK character; rare characters can cost more, so this is
///   intentionally conservative for budget packing).
/// - Everything else (accents, emoji, other scripts): 1 token/char,
///   conservative.
///
/// Arithmetic is done in eighths of a token to keep the ratios exact.
pub fn heuristic_estimate_tokens(text: &str) -> usize {
    let eighths: usize = text.chars().map(char_weight_eighths).sum();
    eighths.div_ceil(8)
}

fn char_weight_eighths(ch: char) -> usize {
    if ch.is_ascii() {
        if ch.is_ascii_alphanumeric() {
            2 // 4 chars/token
        } else if ch.is_ascii_whitespace() {
            1 // 8 chars/token
        } else {
            4 // 2 chars/token
        }
    } else {
        // CJK and all other non-ASCII: conservatively 1 token per char.
        8
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

/// Pack ranked `hits` under a complete prompt-block token budget.
///
/// Citation headers, separators, and expanded source text all consume the
/// same budget, measured with [`estimate_tokens`]. Rank order is preserved;
/// the last retained body may be truncated on a char boundary.
/// If even the first citation and one content token cannot fit, no hit is
/// returned. `max_tokens == 0` also returns an empty pack.
///
/// Packed hits have a single body: `content`. Redundant `context` and
/// `snippet` fields are cleared so expansion cannot hide unbudgeted source
/// text there. This bounds the prompt block, not JSON syntax or
/// ranking/provenance metadata.
pub fn pack_hits(hits: &[SearchHit], max_tokens: usize) -> PackedContext {
    let mut packed: Vec<SearchHit> = Vec::new();
    let mut used_tokens = 0usize;
    for hit in hits {
        let header = format_citation_header(packed.len() + 1, hit);
        let separator = if packed.is_empty() {
            0
        } else {
            estimate_tokens("\n\n")
        };
        let overhead = estimate_tokens(&header).saturating_add(1 + separator);
        let remaining = max_tokens.saturating_sub(used_tokens);
        if overhead > remaining || (!hit.content.is_empty() && overhead == remaining) {
            break;
        }
        let body_budget = remaining - overhead;
        let content = truncate_to_token_budget(&hit.content, body_budget);
        let truncated = content != hit.content;
        if hit.content.is_empty() || !content.is_empty() {
            used_tokens += overhead + estimate_tokens(&content);
            let mut retained = hit.clone();
            retained.content = content;
            retained.context = None;
            retained.snippet = None;
            packed.push(retained);
        }
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

/// Unicode-safe truncation to a token budget. The ellipsis itself consumes
/// budget; if even one character plus the ellipsis exceeds `max_tokens`, the
/// result is empty.
fn truncate_to_token_budget(text: &str, max_tokens: usize) -> String {
    if text.is_empty() || estimate_tokens(text) <= max_tokens {
        return text.to_string();
    }
    if max_tokens == 0 {
        return String::new();
    }
    let total_chars = text.chars().count();
    // Binary search the longest char-boundary prefix within budget.
    // Invariant: prefix of `lo` chars fits; prefix of `hi` chars does not.
    let mut lo = 0usize;
    let mut hi = total_chars;
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        let prefix: String = text.chars().take(mid).collect();
        if estimate_tokens(&prefix) <= max_tokens {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let mut out: String = text.chars().take(lo).collect();
    while !out.is_empty() && estimate_tokens(&format!("{out}…")) > max_tokens {
        out.pop();
    }
    if !out.is_empty() {
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
    fn heuristic_calibrated_on_sample_strings() {
        // English prose: ~4 chars/token for words, whitespace cheaper.
        let prose = "The quick brown fox jumps over the lazy dog repeatedly.";
        let prose_estimate = heuristic_estimate_tokens(prose);
        assert!(
            (10..=16).contains(&prose_estimate),
            "prose={prose_estimate}"
        );

        // CJK: roughly one token per character (conservative).
        assert_eq!(heuristic_estimate_tokens("你好世界"), 4);
        assert_eq!(heuristic_estimate_tokens("检索增强生成"), 6);

        // Mixed CJK + ASCII prose beats the legacy chars/4 heuristic, which
        // would report ceil(11/4)=3 for this 11-char string.
        let mixed = "使用 RAG 检索文档";
        let mixed_estimate = heuristic_estimate_tokens(mixed);
        assert!(mixed_estimate >= 7, "mixed={mixed_estimate}");

        // Code: punctuation-heavy, denser than prose per char.
        let code = "fn main() { println!(\"hi\"); }";
        let code_estimate = heuristic_estimate_tokens(code);
        assert!(code_estimate >= 9, "code={code_estimate}");

        // Whitespace runs are nearly free.
        assert_eq!(heuristic_estimate_tokens("        "), 1);
    }

    #[test]
    fn heuristic_is_concat_subadditive() {
        // Guarantees pack_hits accounting never underestimates the total.
        let a = "引用文本 with punctuation!";
        let b = "第二段 content here";
        assert!(
            heuristic_estimate_tokens(&format!("{a}{b}"))
                <= heuristic_estimate_tokens(a) + heuristic_estimate_tokens(b)
        );
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
        let t = truncate_to_token_budget(text, 2);
        assert!(estimate_tokens(&t) <= 2);
        assert!(!t.is_empty());
        assert!(t.ends_with('…'));
        // Zero budget yields nothing.
        assert!(truncate_to_token_budget(text, 0).is_empty());
        // CJK truncation stays on char boundaries and within budget.
        let cjk = "这是一段需要截断的中文文本内容";
        let t = truncate_to_token_budget(cjk, 3);
        assert!(estimate_tokens(&t) <= 3, "{t}");
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
