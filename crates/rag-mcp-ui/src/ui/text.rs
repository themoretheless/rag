//! Small text helpers shared by the workspace views.

/// At most `max` characters, ending in `…` when cut.
pub fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// `YYYY-MM-DD HH:MM:SS` from an RFC 3339 / ISO timestamp, for table rows.
pub fn short_timestamp(value: &str) -> String {
    value
        .strip_suffix('Z')
        .unwrap_or(value)
        .replace('T', " ")
        .chars()
        .take(19)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_counts_the_ellipsis_and_short_timestamp_drops_fractions() {
        assert_eq!(truncate("абвгде", 4), "абв…");
        assert_eq!(truncate("abc", 3), "abc");
        assert_eq!(short_timestamp("2026-09-02T12:34:56.123+00:00"), "2026-09-02 12:34:56");
    }
}
