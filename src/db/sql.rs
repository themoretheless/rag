//! Shared SQL fragments for repository code.

/// `?, ?, ?` for an `IN (...)` list of `n` bound values (empty for `n == 0`).
pub(crate) fn in_placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

/// The status predicate for "live" documents on an unaliased `documents` row:
/// archived and tombstoned documents are hidden.
pub(crate) const ACTIVE_DOCUMENT_SQL: &str =
    "COALESCE(status, 'active') NOT IN ('archived', 'tombstone')";

/// [`ACTIVE_DOCUMENT_SQL`] with the status column qualified by `alias`.
pub(crate) fn active_document_sql(alias: &str) -> String {
    format!("COALESCE({alias}.status, 'active') NOT IN ('archived', 'tombstone')")
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholders_and_active_predicate() {
        assert_eq!(super::in_placeholders(0), "");
        assert_eq!(super::in_placeholders(1), "?");
        assert_eq!(super::in_placeholders(3), "?, ?, ?");
        assert_eq!(
            super::active_document_sql("d"),
            super::ACTIVE_DOCUMENT_SQL.replace("COALESCE(status", "COALESCE(d.status")
        );
    }
}
