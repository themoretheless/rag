//! Document revision restore and diff payloads.

use serde::{Deserialize, Serialize};

/// Lean immutable revision row for timelines. Full bodies are deliberately
/// excluded and loaded one-at-a-time through the revision snapshot endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocumentRevisionSummary {
    pub document_id: String,
    pub uri: String,
    pub title: String,
    #[serde(default)]
    pub wing: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
    pub layer: String,
    pub kind: String,
    pub status: String,
    pub updated_at: String,
    pub superseded_at: String,
    pub revision: i64,
    pub content_chars: u64,
    pub content_lines: u64,
}

/// Outcome of restoring an older revision as the new head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreRevisionResult {
    pub document_id: String,
    pub restored_from_revision: i64,
    pub revision: i64,
    pub etag: String,
    pub chunk_count: usize,
    pub node_id: String,
    pub edge_count: usize,
}

/// One changed line in a bounded revision diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionLineChange {
    /// `added` or `removed`.
    pub kind: String,
    pub line: usize,
    pub content: String,
    pub content_truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::DocumentRevisionSummary;

    #[test]
    fn revision_summary_wire_format_is_stable() {
        let wire = r#"{"document_id":"d1","uri":"wiki://a","title":"A","wing":"alpha","room":null,"layer":"wiki","kind":"wiki","status":"active","updated_at":"2026-09-01T10:00:00Z","superseded_at":"2026-09-02T10:00:00Z","revision":3,"content_chars":12,"content_lines":2}"#;
        let row: DocumentRevisionSummary = serde_json::from_str(wire).unwrap();
        assert_eq!(row.wing.as_deref(), Some("alpha"));
        assert_eq!(row.room, None);
        // Field order and null handling are part of the `/v1/revisions` contract.
        assert_eq!(serde_json::to_string(&row).unwrap(), wire);

        let without_placement = wire.replace(r#""wing":"alpha","room":null,"#, "");
        let row: DocumentRevisionSummary = serde_json::from_str(&without_placement).unwrap();
        assert_eq!((row.wing, row.room), (None, None));
    }
}
