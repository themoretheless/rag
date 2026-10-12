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
    ellipsize(line.unwrap_or(""), max)
}

/// Like [`first_line`], but prefers the first line that is not a Markdown
/// heading, so summaries read as prose.
pub fn first_prose_line(content: &str, max: usize) -> String {
    let mut lines = content.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.clone().next().unwrap_or("");
    let prose = lines.find(|l| !l.starts_with('#')).unwrap_or(first);
    ellipsize(prose, max)
}

fn ellipsize(line: &str, max: usize) -> String {
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let mut out: String = line.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_values_become_none() {
        assert_eq!(non_blank(None), None);
        assert_eq!(non_blank(Some("  ".into())), None);
        assert_eq!(non_blank(Some(" a ".into())), Some(" a ".into()));
        assert_eq!(trimmed_non_blank(Some(" a ".into())), Some("a".into()));
        assert_eq!(trimmed_non_blank(Some("\t".into())), None);
    }
}
