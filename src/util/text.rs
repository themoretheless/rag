//! Optional-string normalization shared by transports and services.

/// Drop `None`, empty and whitespace-only values; keep the surviving string as is.
pub fn non_blank(value: Option<String>) -> Option<String> {
    value.filter(|item| !item.trim().is_empty())
}

/// Like [`non_blank`], but return the trimmed string.
pub fn trimmed_non_blank(value: Option<String>) -> Option<String> {
    value.and_then(|item| {
        let trimmed = item.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

/// First non-blank line, trimmed and cut to `max` characters with `…`.
pub fn first_line(content: &str, max: usize) -> String {
    let line = content.lines().map(str::trim).find(|l| !l.is_empty());
    truncate_chars(line.unwrap_or(""), max)
}

/// Like [`first_line`], but prefers the first line that is not a Markdown
/// heading, so summaries read as prose.
pub fn first_prose_line(content: &str, max: usize) -> String {
    let mut lines = content.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.clone().next().unwrap_or("");
    let prose = lines.find(|l| !l.starts_with('#')).unwrap_or(first);
    truncate_chars(prose, max)
}

/// At most `max` characters, ending in `…` when cut (the ellipsis counts).
pub fn truncate_chars(line: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    cut_with_ellipsis(line, max, max - 1)
}

/// First `max` characters of `s`, with `…` appended when cut (the ellipsis
/// does not count, so a cut result has `max + 1` characters). Used for error
/// excerpts and prompt budgets.
pub fn excerpt_chars(s: &str, max: usize) -> String {
    cut_with_ellipsis(s, max, max)
}

/// `s` unchanged when it has at most `limit` characters, else its first
/// `keep` characters followed by `…`.
fn cut_with_ellipsis(s: &str, limit: usize, keep: usize) -> String {
    if s.chars().count() <= limit {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(keep).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_counts_characters_not_bytes() {
        assert_eq!(excerpt_chars("привет", 3), "при…");
        assert_eq!(excerpt_chars("abc", 3), "abc");
    }

    #[test]
    fn blank_values_become_none() {
        assert_eq!(non_blank(None), None);
        assert_eq!(non_blank(Some("  ".into())), None);
        assert_eq!(non_blank(Some(" a ".into())), Some(" a ".into()));
        assert_eq!(trimmed_non_blank(Some(" a ".into())), Some("a".into()));
        assert_eq!(trimmed_non_blank(Some("\t".into())), None);
    }
}
