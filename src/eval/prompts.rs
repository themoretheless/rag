//! Content-addressed prompt versions for eval judges (local, no remote prompt hub).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::util::content_hash;

/// Built-in answer-judge prompt name.
pub const ANSWER_JUDGE_V1_NAME: &str = "answer_judge";
/// Built-in answer-judge prompt version label.
pub const ANSWER_JUDGE_V1_VERSION: &str = "v1";
/// Kind stored on judge prompts.
pub const PROMPT_KIND_ANSWER_JUDGE: &str = "answer_judge";

/// Immutable system text for the v1 answer judge. Changing this string is a new version.
pub const ANSWER_JUDGE_V1_BODY: &str = r#"You are a retrieval-answer judge. Score only from the supplied question, answer, citations, and optional gold/context. Do not invent sources.

Return a single JSON object with this exact shape:
{"supported":0.0,"complete":0.0,"faithful":0.0,"verdict":"pass","rationale":"..."}

Rules:
- supported: 0..1 how well the answer is backed by cited ids / context.
- complete: 0..1 how well expected sources / gold facts are covered.
- faithful: 0..1 how little the answer adds unsupported claims.
- verdict: pass (>=0.8 overall), partial (>=0.4), or fail.
- rationale: one short sentence, no markdown.

If the answer is empty, all scores are 0 and verdict is fail.
"#;

/// Named, content-addressed prompt template.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptVersion {
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub body: String,
    pub content_hash: String,
    #[serde(default)]
    pub builtin: bool,
    pub created_at: DateTime<Utc>,
}

impl PromptVersion {
    /// Build a prompt; `id` / `content_hash` are derived from name+version+body.
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        kind: impl Into<String>,
        body: impl Into<String>,
        builtin: bool,
    ) -> Self {
        let name = name.into();
        let version = version.into();
        let kind = kind.into();
        let body = body.into();
        let content_hash = prompt_content_hash(&name, &version, &body);
        Self {
            id: content_hash.clone(),
            name,
            version,
            kind,
            body,
            content_hash,
            builtin,
            created_at: Utc::now(),
        }
    }
}

/// Stable identity of a named prompt body.
pub fn prompt_content_hash(name: &str, version: &str, body: &str) -> String {
    content_hash(&format!("{name}\n{version}\n{body}"))
}

/// Built-in answer-judge v1.
pub fn builtin_answer_judge() -> PromptVersion {
    PromptVersion::new(
        ANSWER_JUDGE_V1_NAME,
        ANSWER_JUDGE_V1_VERSION,
        PROMPT_KIND_ANSWER_JUDGE,
        ANSWER_JUDGE_V1_BODY,
        true,
    )
}

/// Validate a caller-supplied prompt before persist.
pub fn validate_prompt(name: &str, version: &str, kind: &str, body: &str) -> crate::error::Result<()> {
    if name.trim().is_empty() || name.len() > 64 {
        return Err(crate::error::AppError::config(
            "prompt name must be 1..64 bytes",
        ));
    }
    if version.trim().is_empty() || version.len() > 32 {
        return Err(crate::error::AppError::config(
            "prompt version must be 1..32 bytes",
        ));
    }
    if kind.trim().is_empty() || kind.len() > 32 {
        return Err(crate::error::AppError::config(
            "prompt kind must be 1..32 bytes",
        ));
    }
    if body.trim().is_empty() || body.len() > 16_384 {
        return Err(crate::error::AppError::config(
            "prompt body must be 1..16384 bytes",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_hash_is_stable() {
        let a = builtin_answer_judge();
        let b = builtin_answer_judge();
        assert_eq!(a.id, b.id);
        assert_eq!(a.content_hash.len(), 64);
        assert_eq!(a.name, ANSWER_JUDGE_V1_NAME);
        assert!(a.builtin);
    }

    #[test]
    fn body_change_is_a_new_id() {
        let a = PromptVersion::new("answer_judge", "v1", "answer_judge", "aaa", false);
        let b = PromptVersion::new("answer_judge", "v1", "answer_judge", "bbb", false);
        assert_ne!(a.id, b.id);
    }
}
