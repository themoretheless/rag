use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

const DATE_TIME_FORMATS: &[&str] = &[
    "%Y-%m-%d %H:%M:%S%.f",
    "%Y-%m-%d %H:%M:%S",
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%dT%H:%M:%S",
];

pub fn format_db_timestamp(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
}

pub fn parse_db_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Utc));
    }
    DATE_TIME_FORMATS.iter().find_map(|format| {
        NaiveDateTime::parse_from_str(value, format)
            .ok()
            .map(|parsed| parsed.and_utc())
    })
}

pub fn parse_flexible_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    parse_db_timestamp(raw).or_else(|| {
        NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")
            .ok()
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .map(|parsed| parsed.and_utc())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_rfc3339_and_duckdb_forms() {
        assert!(parse_db_timestamp("2024-01-02T03:04:05Z").is_some());
        assert!(parse_db_timestamp("2024-01-02 03:04:05.123456").is_some());
        assert!(parse_db_timestamp("").is_none());
    }

    #[test]
    fn date_only_is_explicitly_flexible() {
        assert!(parse_db_timestamp("2024-01-02").is_none());
        assert!(parse_flexible_timestamp("2024-01-02").is_some());
    }
}
