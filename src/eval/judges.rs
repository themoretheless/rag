//! Deterministic citation checks on top of retrieval metrics.
//!
//! Answer-level scoring lives in [`super::answer_judge`]: heuristic always,
//! optional local LLM behind a versioned prompt. This module only looks at
//! cited document ids / answer text markers against expected sources.

use serde::{Deserialize, Serialize};

/// Inputs for deterministic citation / coverage checks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CitationJudgeInput {
    /// Optional answer body; when set, citation presence also looks for a
    /// `## citations` section or `[[wikilink]]` / `source:` markers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_text: Option<String>,
    /// Document ids (or URIs) explicitly cited by the answer / pack.
    #[serde(default)]
    pub cited_document_ids: Vec<String>,
    /// Expected relevant document ids (or URIs) from the eval label set.
    #[serde(default)]
    pub expected_document_ids: Vec<String>,
}

/// Deterministic citation judge outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CitationJudgeResult {
    /// True when at least one citation id is present, or answer text contains
    /// a citation marker section.
    pub citation_present: bool,
    /// Fraction of expected docs that appear in `cited_document_ids` (0..=1).
    /// When expected is empty, coverage is `1.0` if no_answer semantics apply
    /// via empty expected, else `1.0` only when citations are also empty.
    pub expected_doc_coverage: f64,
    pub missing_expected: Vec<String>,
    pub cited_count: usize,
    pub expected_count: usize,
}

/// Score citation presence and expected-document coverage without an LLM.
pub fn judge_citations(input: &CitationJudgeInput) -> CitationJudgeResult {
    let cited: Vec<String> = input
        .cited_document_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let expected: Vec<String> = input
        .expected_document_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let text_marker = input.answer_text.as_deref().is_some_and(answer_has_citation_marker);
    let citation_present = !cited.is_empty() || text_marker;

    let mut missing = Vec::new();
    for exp in &expected {
        let found = cited.iter().any(|c| ids_match(c, exp));
        if !found {
            missing.push(exp.clone());
        }
    }
    let expected_count = expected.len();
    let coverage = if expected_count == 0 {
        1.0
    } else {
        (expected_count - missing.len()) as f64 / expected_count as f64
    };

    CitationJudgeResult {
        citation_present,
        expected_doc_coverage: coverage,
        missing_expected: missing,
        cited_count: cited.len(),
        expected_count,
    }
}

fn ids_match(a: &str, b: &str) -> bool {
    a == b || a.eq_ignore_ascii_case(b)
}

fn answer_has_citation_marker(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("## citations")
        || lower.contains("\n# citations")
        || text.contains("[[")
        || lower.contains("source:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfect_coverage() {
        let result = judge_citations(&CitationJudgeInput {
            answer_text: Some("see ## Citations\n- a".into()),
            cited_document_ids: vec!["doc-a".into(), "doc-b".into()],
            expected_document_ids: vec!["doc-a".into(), "doc-b".into()],
        });
        assert!(result.citation_present);
        assert_eq!(result.expected_doc_coverage, 1.0);
        assert!(result.missing_expected.is_empty());
    }

    #[test]
    fn reports_missing_expected() {
        let result = judge_citations(&CitationJudgeInput {
            answer_text: None,
            cited_document_ids: vec!["doc-a".into()],
            expected_document_ids: vec!["doc-a".into(), "doc-b".into()],
        });
        assert!(result.citation_present);
        assert!((result.expected_doc_coverage - 0.5).abs() < f64::EPSILON);
        assert_eq!(result.missing_expected, vec!["doc-b".to_string()]);
    }

    #[test]
    fn empty_citations_fail_presence() {
        let result = judge_citations(&CitationJudgeInput {
            answer_text: Some("no markers here".into()),
            cited_document_ids: vec![],
            expected_document_ids: vec!["doc-a".into()],
        });
        assert!(!result.citation_present);
        assert_eq!(result.expected_doc_coverage, 0.0);
    }
}
