//! Local-first retrieval traces for eval debugging (no remote Opik stack).
//!
//! Spans cover search → pack/context → optional answer. Source document ids and
//! content hashes are recorded when available so regressions can pin versions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceSpan {
    pub name: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_ms: f64,
    #[serde(default)]
    pub attrs: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrievalTrace {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub spans: Vec<TraceSpan>,
    /// Optional free-form path label (`search`, `search_pack`, `search_pack_answer`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SpanBuilder {
    name: String,
    started_at: DateTime<Utc>,
    started_instant: std::time::Instant,
    attrs: Map<String, Value>,
}

impl SpanBuilder {
    pub fn start(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            started_at: Utc::now(),
            started_instant: std::time::Instant::now(),
            attrs: Map::new(),
        }
    }

    pub fn attr(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.attrs.insert(key.into(), value.into());
        self
    }

    pub fn finish(self) -> TraceSpan {
        let ended_at = Utc::now();
        TraceSpan {
            name: self.name,
            started_at: self.started_at,
            ended_at,
            duration_ms: self.started_instant.elapsed().as_secs_f64() * 1000.0,
            attrs: self.attrs,
        }
    }
}

impl RetrievalTrace {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            created_at: Utc::now(),
            spans: Vec::new(),
            path: Some(path.into()),
        }
    }

    pub fn push(&mut self, span: TraceSpan) {
        self.spans.push(span);
    }
}

/// Compact source version rows suitable for span attrs / eval compare.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceVersion {
    pub document_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

pub fn source_versions_json(versions: &[SourceVersion]) -> Value {
    json!(versions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_records_duration_and_attrs() {
        let span = SpanBuilder::start("search")
            .attr("mode", "hybrid")
            .attr("hit_count", 3)
            .finish();
        assert_eq!(span.name, "search");
        assert!(span.duration_ms >= 0.0);
        assert_eq!(span.attrs.get("mode").and_then(|v| v.as_str()), Some("hybrid"));
    }
}
