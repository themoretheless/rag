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
