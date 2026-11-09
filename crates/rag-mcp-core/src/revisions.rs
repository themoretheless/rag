//! Document revision restore and diff payloads.

use serde::{Deserialize, Serialize};

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
