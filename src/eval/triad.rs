//! RAG-triad metric naming over measured eval signals.
//!
//! Emits retrieval-eval results under the RAG-triad names
//! (`context_precision`, `context_recall`, `answer_relevancy`,
//! `faithfulness`) for dashboards and CI that standardize on that vocabulary.
//!
//! Only measured signals are mapped; everything else is emitted as JSON
//! `null` with the reason recorded in `mapping`. The mapping is documented in
//! `docs/RETRIEVAL_EVALUATION.md`:
//!
//! - `context_recall` ≈ retrieval `recall@k` (label-based, top-k).
//! - `context_precision`: **not measured** — the eval scores ranked lists
//!   against graded labels (recall/MRR/nDCG), not per-chunk precision.
//! - `answer_relevancy`: **not measured** — the retrieval eval never
//!   generates answers.
//! - `faithfulness`: measured only when an answer judge ran (eval feedback
//!   runs carry `AnswerJudgeScores.faithful`); retrieval-only runs emit null.

use serde::Serialize;

/// One mapping note: triad metric → measured source or reason for null.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TriadMappingNote {
    pub metric: String,
    pub source: String,
}

/// Static mapping documentation included in every report so consumers can
/// tell "measured" apart from "not applicable to this run".
pub fn mapping_notes() -> Vec<TriadMappingNote> {
    vec![
        TriadMappingNote {
            metric: "context_recall".into(),
            source: "mapped from retrieval recall@k (graded labels, top-k)".into(),
        },
        TriadMappingNote {
            metric: "context_precision".into(),
            source: "null: not measured; eval scores ranked lists via recall@k/MRR/nDCG, not per-chunk precision".into(),
        },
        TriadMappingNote {
            metric: "answer_relevancy".into(),
            source: "null: retrieval eval does not generate answers".into(),
        },
        TriadMappingNote {
            metric: "faithfulness".into(),
            source: "null in retrieval-only runs; mapped from answer-judge `faithful` score when a judged answer run is exported".into(),
        },
    ]
}

/// Inputs for one retrieval mode's triad row.
#[derive(Debug, Clone)]
pub struct TriadModeInput {
    pub mode: String,
    pub recall_at_k: f64,
    /// Answer-judge faithfulness in 0..=1 when a judged run exists.
    pub faithfulness: Option<f64>,
}

/// Triad-named metrics for one retrieval mode. Unmeasured metrics are null.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TriadModeMetrics {
    pub mode: String,
    pub context_recall: f64,
    pub context_precision: Option<f64>,
    pub answer_relevancy: Option<f64>,
    pub faithfulness: Option<f64>,
}

/// Versioned triad report (JSON) for external dashboards.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TriadReport {
    pub kind: String,
    pub version: u32,
    pub dataset_name: String,
    pub dataset_content_hash: String,
    pub top_k: usize,
    pub mapping: Vec<TriadMappingNote>,
    pub modes: Vec<TriadModeMetrics>,
}

/// Build a triad report from measured retrieval metrics. Fabricates nothing:
/// signals with no measurement stay null.
pub fn triad_report(
    dataset_name: &str,
    dataset_content_hash: &str,
    top_k: usize,
    modes: Vec<TriadModeInput>,
) -> TriadReport {
    TriadReport {
        kind: "rag-triad".into(),
        version: 1,
        dataset_name: dataset_name.to_string(),
        dataset_content_hash: dataset_content_hash.to_string(),
        top_k,
        mapping: mapping_notes(),
        modes: modes
            .into_iter()
            .map(|input| TriadModeMetrics {
                mode: input.mode,
                context_recall: input.recall_at_k,
                context_precision: None,
                answer_relevancy: None,
                faithfulness: input.faithfulness,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_recall_and_leaves_unmeasured_null() {
        let report = triad_report(
            "example",
            "hash",
            5,
            vec![TriadModeInput {
                mode: "hybrid".into(),
                recall_at_k: 0.8,
                faithfulness: None,
            }],
        );
        let mode = &report.modes[0];
        assert_eq!(mode.context_recall, 0.8);
        assert_eq!(mode.context_precision, None);
        assert_eq!(mode.answer_relevancy, None);
        assert_eq!(mode.faithfulness, None);
        let json = serde_json::to_string(&report).unwrap();
        // Nulls are explicit so consumers can distinguish "not measured"
        // from a missing field.
        assert!(json.contains("\"context_precision\":null"));
        assert!(json.contains("\"answer_relevancy\":null"));
    }

    #[test]
    fn carries_faithfulness_when_judged() {
        let report = triad_report(
            "example",
            "hash",
            5,
            vec![TriadModeInput {
                mode: "vec".into(),
                recall_at_k: 0.5,
                faithfulness: Some(0.9),
            }],
        );
        assert_eq!(report.modes[0].faithfulness, Some(0.9));
        assert_eq!(report.mapping.len(), 4);
        assert_eq!(report.kind, "rag-triad");
    }
}
