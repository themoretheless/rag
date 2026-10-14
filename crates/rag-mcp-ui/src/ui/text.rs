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

/// `HH:MM:SS` from an ISO timestamp (first 8 characters when there is no `T`).
pub fn short_time(value: &str) -> String {
    value
        .split_once('T')
        .map(|(_, time)| time.trim_end_matches('Z').chars().take(8).collect())
        .unwrap_or_else(|| value.chars().take(8).collect())
}

/// First 8 bytes of an id for compact display; the whole id when shorter or
/// when byte 8 is not a char boundary.
pub fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_counts_the_ellipsis_and_short_timestamp_drops_fractions() {
        assert_eq!(truncate("абвгде", 4), "абв…");
        assert_eq!(truncate("abc", 3), "abc");
        assert_eq!(
            short_timestamp("2026-09-02T12:34:56.123+00:00"),
            "2026-09-02 12:34:56"
        );
    }

    #[test]
    fn short_time_and_short_id() {
        assert_eq!(short_time("2026-09-02T12:34:56Z"), "12:34:56");
        assert_eq!(short_id("abc"), "abc");
        assert_eq!(short_id("123456789"), "12345678");
    }
}
