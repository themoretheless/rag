use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

use crate::error::AppError;

const DATE_TIME_FORMATS: &[&str] = &[
    "%Y-%m-%d %H:%M:%S%.f",
    "%Y-%m-%d %H:%M:%S",
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%dT%H:%M:%S",
];

/// Format a UTC time as the database timestamp text (microsecond precision).
pub fn format_db_timestamp(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
}

/// Parse RFC 3339 or a database timestamp format as UTC; `None` when blank
/// or unrecognised.
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

/// Like [`parse_db_timestamp`], but also accepts a bare `YYYY-MM-DD`
/// (midnight UTC).
pub fn parse_flexible_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    parse_db_timestamp(raw).or_else(|| {
        NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")
            .ok()
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .map(|parsed| parsed.and_utc())
    })
}

/// Parse an optional caller-supplied timestamp; blank input is `None`.
///
/// Accepts RFC3339, `YYYY-MM-DD[T ]HH:MM:SS[.f]` and date-only (midnight UTC).
/// Anything else is an [`AppError::Config`] so transports report it as a bad
/// request / invalid params.
pub fn parse_optional_timestamp(raw: Option<&str>) -> Result<Option<DateTime<Utc>>, AppError> {
    let Some(value) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    parse_flexible_timestamp(value).map(Some).ok_or_else(|| {
        AppError::config(format!(
            "invalid timestamp (use RFC3339 or YYYY-MM-DD[ HH:MM:SS]): {value}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_timestamp_accepts_common_forms() {
        assert!(parse_optional_timestamp(None).unwrap().is_none());
        assert!(parse_optional_timestamp(Some("  ")).unwrap().is_none());
        let date = parse_optional_timestamp(Some("2024-03-15"))
            .unwrap()
            .unwrap();
        assert_eq!(date.date_naive().to_string(), "2024-03-15");
        let rfc = parse_optional_timestamp(Some("2024-03-15T12:00:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(rfc.timestamp(), 1_710_504_000);
        let err = parse_optional_timestamp(Some("not-a-date")).unwrap_err();
        assert!(matches!(err, AppError::Config(_)));
    }

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
