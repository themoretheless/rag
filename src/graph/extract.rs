//! Parse Obsidian-style `[[wikilinks]]` and `#tags` from markdown/text.
//!
//! - `[[Target]]` / `[[Target|alias]]` → `rel_type = "wikilink"`
//! - `![[embed]]` is skipped (not an edge in v1)
//! - `#tag` / `#multi/level` → `rel_type = "tagged"`
//! - Tags (and wikilinks) inside fenced ``` blocks and inline `` `code` `` are
//!   skipped best-effort by masking those spans before matching.

/// Edge type for parsed wikilinks.
pub const REL_WIKILINK: &str = "wikilink";

/// Edge type for parsed hashtags.
pub const REL_TAGGED: &str = "tagged";

/// Explicit cross-scope bridge edge (MemPalace tunnel; created via `create_tunnel` / `link_nodes`).
pub const REL_TUNNEL: &str = "tunnel";

/// Characters of surrounding text included in [`ExtractedLink::context`].
const CONTEXT_RADIUS: usize = 40;

/// One link or tag found in document text (not yet resolved to graph nodes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedLink {
    /// Target label (wikilink destination or tag name without `#`).
    pub target_label: String,
    /// `wikilink` or `tagged`.
    pub rel_type: String,
    /// Snippet around the match (up to 40 surrounding characters), when non-empty.
    pub context: Option<String>,
    /// Display alias from `[[Target|alias]]`, if any.
    pub alias: Option<String>,
}

/// Extract wikilinks and tags from `text` in document order.
///
/// - Wikilinks: `[[Target]]`, `[[Target|alias]]`. Image embeds `![[...]]` are skipped.
/// - Tags: `#tag`, `#multi/level` (Unicode letters/digits, `_`, `-`, `/`).
/// - Best-effort: skips fenced code blocks and inline `` `code` `` spans.
pub fn extract_links(text: &str) -> Vec<ExtractedLink> {
    if text.is_empty() {
        return Vec::new();
    }

    // Mask code so scanners never see tags/links inside fences/inline code,
    // while keeping byte length identical for context offsets.
    let masked = mask_code_regions(text);
    let mut hits: Vec<(usize, ExtractedLink)> = Vec::new();

    for (pos, link) in extract_wikilinks(&masked, text) {
        hits.push((pos, link));
    }
    for (pos, link) in extract_tags(&masked, text) {
        hits.push((pos, link));
    }

    hits.sort_by_key(|(pos, _)| *pos);
    hits.into_iter().map(|(_, link)| link).collect()
}

/// Replace fenced ``` and inline `code` with spaces (same UTF-8 byte length).
fn mask_code_regions(text: &str) -> String {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut out = text.to_string().into_bytes();
    let mut i = 0;

    while i < n {
        if bytes[i] == b'`' && starts_fence_at(bytes, i) {
            let fence_len = count_backticks(bytes, i);
            let open_start = i;
            let mut j = i + fence_len;
            while j < n && bytes[j] != b'\n' {
                j += 1;
            }
            if j < n {
                j += 1;
            }
            let mut k = j;
            let mut closed = false;
            while k < n {
                if is_line_start(bytes, k) {
                    let close_len = count_backticks(bytes, k);
                    if close_len >= fence_len {
                        let after = k + close_len;
                        let mut t = after;
                        while t < n && bytes[t] != b'\n' {
                            if bytes[t] != b' ' && bytes[t] != b'\t' && bytes[t] != b'\r' {
                                break;
                            }
                            t += 1;
                        }
                        if t == n || bytes[t] == b'\n' {
                            let end = if t < n { t + 1 } else { t };
                            blank_range(&mut out, open_start, end);
                            i = end;
                            closed = true;
                            break;
                        }
                    }
                }
                k += 1;
            }
            if closed {
                continue;
            }
            blank_range(&mut out, open_start, n);
            break;
        }

        if bytes[i] == b'`' {
            let run = count_backticks(bytes, i);
            if run == 1 || run == 2 {
                let content_start = i + run;
                if let Some(close) = find_inline_close(bytes, content_start, run) {
                    blank_range(&mut out, i, close + run);
                    i = close + run;
                    continue;
                }
            }
            i += run.max(1);
            continue;
        }

        i += 1;
    }

    // Safety: we only wrote ASCII spaces over existing bytes.
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

fn blank_range(buf: &mut [u8], start: usize, end: usize) {
    let end = end.min(buf.len());
    let start = start.min(end);
    for b in &mut buf[start..end] {
        // Preserve newlines so line-oriented fence detection remains sane if re-scanned.
        if *b != b'\n' && *b != b'\r' {
            *b = b' ';
        }
    }
}

fn starts_fence_at(bytes: &[u8], i: usize) -> bool {
    count_backticks(bytes, i) >= 3 && is_line_start(bytes, i)
}

fn is_line_start(bytes: &[u8], i: usize) -> bool {
    i == 0 || bytes[i - 1] == b'\n'
}

fn count_backticks(bytes: &[u8], i: usize) -> usize {
    let mut n = 0;
    while i + n < bytes.len() && bytes[i + n] == b'`' {
        n += 1;
    }
    n
}

fn find_inline_close(bytes: &[u8], from: usize, run: usize) -> Option<usize> {
    let mut j = from;
    while j + run <= bytes.len() {
        if bytes[j] == b'\n' {
            return None;
        }
        if count_backticks(bytes, j) == run {
            return Some(j);
        }
        j += 1;
    }
    None
}

/// Byte start positions of each wikilink match on `masked`, context from `original`.
fn extract_wikilinks(masked: &str, original: &str) -> Vec<(usize, ExtractedLink)> {
    let bytes = masked.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;

    while i + 3 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let content_start = i + 2;
            if i > 0 && bytes[i - 1] == b'!' {
                if let Some(close_rel) = find_wikilink_close(bytes, content_start) {
                    i = content_start + close_rel + 2;
                } else {
                    i += 2;
                }
                continue;
            }

            if let Some(close_rel) = find_wikilink_close(bytes, content_start) {
                let content_end = content_start + close_rel;
                let inner = &masked[content_start..content_end];
                let match_end = content_end + 2;
                if let Some(link) = parse_wikilink_inner(inner, original, i, match_end) {
                    out.push((i, link));
                }
                i = match_end;
                continue;
            }
        }
        i += 1;
    }

    out
}

fn find_wikilink_close(bytes: &[u8], from: usize) -> Option<usize> {
    let mut j = from;
    while j + 1 < bytes.len() {
        if bytes[j] == b']' && bytes[j + 1] == b']' {
            return Some(j - from);
        }
        if bytes[j] == b'[' && bytes[j + 1] == b'[' {
            return None;
        }
        j += 1;
    }
    None
}

fn parse_wikilink_inner(
    inner: &str,
    full_text: &str,
    match_start: usize,
    match_end: usize,
) -> Option<ExtractedLink> {
    let inner = inner.trim();
    if inner.is_empty() {
        return None;
    }

    let (target_raw, alias_raw) = match inner.find('|') {
        Some(pipe) => {
            let (t, a) = inner.split_at(pipe);
            (t.trim(), Some(a[1..].trim()))
        }
        None => (inner, None),
    };

    // A fragment identifies a section inside the target document, not a
    // separate graph object. A fragment-only link stays within this document.
    let target_raw = target_raw
        .split_once('#')
        .map_or(target_raw, |(base, _)| base)
        .trim();
    if target_raw.is_empty() {
        return None;
    }

    let alias = alias_raw.filter(|a| !a.is_empty()).map(|a| a.to_string());

    Some(ExtractedLink {
        target_label: target_raw.to_string(),
        rel_type: REL_WIKILINK.to_string(),
        context: context_snippet(full_text, match_start, match_end),
        alias,
    })
}

fn extract_tags(masked: &str, original: &str) -> Vec<(usize, ExtractedLink)> {
    let mut out = Vec::new();
    let mut prev: Option<char> = None;
    let mut chars = masked.char_indices().peekable();

    while let Some((idx, ch)) = chars.next() {
        if ch == '#' && is_tag_boundary(prev) {
            let mut body = String::new();
            let mut body_end = idx + ch.len_utf8();

            while let Some(&(next_idx, next_ch)) = chars.peek() {
                if is_tag_body_char(next_ch) {
                    body.push(next_ch);
                    body_end = next_idx + next_ch.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }

            let label = body.trim_matches('/').to_string();
            if is_meaningful_tag(&label) {
                out.push((
                    idx,
                    ExtractedLink {
                        target_label: label,
                        rel_type: REL_TAGGED.to_string(),
                        context: context_snippet(original, idx, body_end),
                        alias: None,
                    },
                ));
            }
            prev = body.chars().last().or(Some('#'));
            continue;
        }
        prev = Some(ch);
    }

    out
}

fn is_meaningful_tag(label: &str) -> bool {
    if label.is_empty() || label.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }
    if !label.chars().any(char::is_alphanumeric) {
        return false;
    }
    // In prose and generated reports these are overwhelmingly CSS colours,
    // not knowledge tags. Keeping them creates thousands of meaningless nodes.
    matches!(label.len(), 3 | 4 | 6 | 8).then(|| label.chars().all(|ch| ch.is_ascii_hexdigit()))
        != Some(true)
}

fn is_tag_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => !(c.is_alphanumeric() || c == '_' || c == '&'),
    }
}

/// Tag body: Unicode letters/digits plus `_`, `-`, `/` (SPEC).
fn is_tag_body_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-' || c == '/'
}

fn context_snippet(text: &str, byte_start: usize, byte_end: usize) -> Option<String> {
    if byte_start > text.len() || byte_end > text.len() || byte_start > byte_end {
        return None;
    }
    let prefix: String = text[..byte_start]
        .chars()
        .rev()
        .take(CONTEXT_RADIUS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let middle = &text[byte_start..byte_end];
    let suffix: String = text[byte_end..].chars().take(CONTEXT_RADIUS).collect();
    let snippet = format!("{prefix}{middle}{suffix}");
    if snippet.is_empty() {
        None
    } else {
        Some(snippet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels_of<'a>(links: &'a [ExtractedLink], rel: &str) -> Vec<&'a str> {
        links
            .iter()
            .filter(|l| l.rel_type == rel)
            .map(|l| l.target_label.as_str())
            .collect()
    }

    #[test]
    fn empty_text_yields_nothing() {
        assert!(extract_links("").is_empty());
    }

    #[test]
    fn wikilinks_basic_and_alias() {
        let links = extract_links("See [[Note A]] and [[Target|display]].");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target_label, "Note A");
        assert_eq!(links[0].rel_type, REL_WIKILINK);
        assert!(links[0].alias.is_none());
        assert_eq!(links[1].target_label, "Target");
        assert_eq!(links[1].alias.as_deref(), Some("display"));
        let ctx = links[0].context.as_deref().unwrap_or("");
        assert!(ctx.contains("[[Note A]]"), "{ctx}");
    }

    #[test]
    fn skips_image_embeds() {
        let links = extract_links("Pic ![[image.png]] and [[Real]].");
        assert_eq!(labels_of(&links, REL_WIKILINK), vec!["Real"]);
    }

    #[test]
    fn empty_and_whitespace_wikilinks_skipped() {
        assert!(extract_links("[[]] [[   ]]").is_empty());
    }

    #[test]
    fn trims_wikilink_target_and_alias() {
        let links = extract_links("[[  Foo  |  Bar  ]]");
        assert_eq!(links[0].target_label, "Foo");
        assert_eq!(links[0].alias.as_deref(), Some("Bar"));
    }

    #[test]
    fn wikilink_fragments_resolve_to_documents_not_section_stubs() {
        let links = extract_links("[[Page#Section|read]] [[#Local section]]");
        assert_eq!(labels_of(&links, REL_WIKILINK), vec!["Page"]);
        assert_eq!(links[0].alias.as_deref(), Some("read"));
    }

    #[test]
    fn tags_and_nested() {
        let links = extract_links("Hello #rust and #multi/level-tag end");
        assert_eq!(
            labels_of(&links, REL_TAGGED),
            vec!["rust", "multi/level-tag"]
        );
    }

    #[test]
    fn tag_unicode_and_punctuation_body() {
        let links = extract_links("#café #tag_1 #a-b");
        assert_eq!(labels_of(&links, REL_TAGGED), vec!["café", "tag_1", "a-b"]);
    }

    #[test]
    fn rejects_numeric_css_colour_and_punctuation_pseudo_tags() {
        let links = extract_links("#12345 #000000 #abc #deadbeef #--- #idea #rust-2026");
        assert_eq!(labels_of(&links, REL_TAGGED), vec!["idea", "rust-2026"]);
    }

    #[test]
    fn tag_not_mid_token() {
        let links = extract_links("C# language and foo#bar and #real");
        assert_eq!(labels_of(&links, REL_TAGGED), vec!["real"]);
    }

    #[test]
    fn skips_fenced_and_inline_code() {
        let text = "Before\n```\n[[Hidden]] #hidden\n```\nAfter [[Visible]] and `[[code]]` #ok";
        let links = extract_links(text);
        let labels: Vec<_> = links.iter().map(|l| l.target_label.as_str()).collect();
        assert!(labels.contains(&"Visible"), "{labels:?}");
        assert!(labels.contains(&"ok"), "{labels:?}");
        assert!(!labels.contains(&"Hidden"), "{labels:?}");
        assert!(!labels.contains(&"hidden"), "{labels:?}");
        assert!(!labels.contains(&"code"), "{labels:?}");
    }

    #[test]
    fn document_order_mixed() {
        let text = "#first then [[Link]] then #second";
        let links = extract_links(text);
        assert_eq!(
            links
                .iter()
                .map(|l| (l.rel_type.as_str(), l.target_label.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (REL_TAGGED, "first"),
                (REL_WIKILINK, "Link"),
                (REL_TAGGED, "second"),
            ]
        );
    }

    #[test]
    fn multiple_wikilinks() {
        let links = extract_links("[[A]] and [[B|bee]] and ![[skip]] and [[C]]");
        assert_eq!(labels_of(&links, REL_WIKILINK), vec!["A", "B", "C"]);
        assert_eq!(
            links
                .iter()
                .filter(|l| l.rel_type == REL_WIKILINK)
                .map(|l| l.alias.clone())
                .collect::<Vec<_>>(),
            vec![None, Some("bee".into()), None]
        );
    }

    #[test]
    fn unclosed_fence_consumes_rest_for_tags() {
        let text = "ok #yes\n```\n#no_more";
        let links = extract_links(text);
        assert_eq!(labels_of(&links, REL_TAGGED), vec!["yes"]);
    }

    #[test]
    fn context_is_local_snippet() {
        let pad = "x".repeat(50);
        let text = format!("{pad}[[Mid]]{pad}");
        let links = extract_links(&text);
        let ctx = links[0].context.as_ref().unwrap();
        assert!(ctx.contains("[[Mid]]"));
        assert!(ctx.len() < text.len());
        assert!(ctx.chars().count() <= 40 + "[[Mid]]".chars().count() + 40);
    }
}
