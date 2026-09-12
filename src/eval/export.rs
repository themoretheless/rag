//! Versioned eval export bundle and offline judge replay (no live search).

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::answer_judge::{judge_answer_heuristic, AnswerJudgeInput, AnswerJudgeResult};
use crate::error::{AppError, Result};

/// Bundle format version. Unknown versions are refused on replay.
pub const EXPORT_BUNDLE_VERSION: u32 = 1;

/// Gateway / CLI snapshot of prompts, runs, traces, and labelled questions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalExportBundle {
    pub version: u32,
    pub exported_at: String,
    #[serde(default)]
    pub prompts: Vec<Value>,
    #[serde(default)]
    pub runs: Vec<Value>,
    #[serde(default)]
    pub traces: Vec<Value>,
    #[serde(default)]
    pub feedback: Vec<Value>,
}

/// One replayed judge compared to the score stored in the bundle (if any).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplayItem {
    pub run_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored_overall: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replayed_overall: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer_judge: Option<AnswerJudgeResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
}

/// Offline replay report. Does not re-search or re-embed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplayReport {
    pub kind: String,
    pub bundle_version: u32,
    pub replayed_at: String,
    pub judged: usize,
    pub skipped: usize,
    pub items: Vec<ReplayItem>,
}

impl EvalExportBundle {
    pub fn new(
        prompts: Vec<Value>,
        runs: Vec<Value>,
        traces: Vec<Value>,
        feedback: Vec<Value>,
    ) -> Self {
        Self {
            version: EXPORT_BUNDLE_VERSION,
            exported_at: Utc::now().to_rfc3339(),
            prompts,
            runs,
            traces,
            feedback,
        }
    }
}

/// Recompute heuristic (+ citation) judges from stored payloads. No live corpus.
pub fn replay_bundle(bundle: &EvalExportBundle) -> Result<ReplayReport> {
    if bundle.version != EXPORT_BUNDLE_VERSION {
        return Err(AppError::config(format!(
            "unsupported eval export version {}; expected {EXPORT_BUNDLE_VERSION}",
            bundle.version
        )));
    }
    let mut items = Vec::new();
    let mut judged = 0usize;
    let mut skipped = 0usize;
    for run in &bundle.runs {
        let run_id = run
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        match extract_judge_input(run) {
            Ok(input) => {
                if input.answer_text.trim().is_empty()
                    && input.cited_document_ids.is_empty()
                    && input.expected_document_ids.is_empty()
                {
                    skipped += 1;
                    items.push(ReplayItem {
                        run_id,
                        status: "skipped".into(),
                        stored_overall: None,
                        replayed_overall: None,
                        delta: None,
                        answer_judge: None,
                        skip_reason: Some("no answer or citation fields to replay".into()),
                    });
                    continue;
                }
                let judge = judge_answer_heuristic(&input);
                let stored = stored_overall(run);
                let replayed = judge.combined.overall;
                judged += 1;
                items.push(ReplayItem {
                    run_id,
                    status: "judged".into(),
                    stored_overall: stored,
                    replayed_overall: Some(replayed),
                    delta: stored.map(|s| replayed - s),
                    answer_judge: Some(judge),
                    skip_reason: None,
                });
            }
            Err(reason) => {
                skipped += 1;
                items.push(ReplayItem {
                    run_id,
                    status: "skipped".into(),
                    stored_overall: None,
                    replayed_overall: None,
                    delta: None,
                    answer_judge: None,
                    skip_reason: Some(reason),
                });
            }
        }
    }
    Ok(ReplayReport {
        kind: "replay".into(),
        bundle_version: bundle.version,
        replayed_at: Utc::now().to_rfc3339(),
        judged,
        skipped,
        items,
    })
}

/// Load a bundle from a JSON file (CLI `--replay`).
pub fn load_export_bundle(path: &std::path::Path) -> Result<EvalExportBundle> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        AppError::config(format!("read export bundle {}: {e}", path.display()))
    })?;
    serde_json::from_str(&raw).map_err(|e| {
        AppError::config(format!("parse export bundle {}: {e}", path.display()))
    })
}

fn stored_overall(run: &Value) -> Option<f64> {
    run.pointer("/result/answer_judge/combined/overall")
        .or_else(|| run.pointer("/answer_judge/combined/overall"))
        .and_then(|v| v.as_f64())
}

fn extract_judge_input(run: &Value) -> std::result::Result<AnswerJudgeInput, String> {
    let result = run.get("result").unwrap_or(run);
    let question = first_str(&[
        result.get("question"),
        run.get("question"),
        run.pointer("/settings/query"),
        result.pointer("/search/query"),
    ])
    .unwrap_or_default();
    let answer_text = first_str(&[
        result.get("answer_text"),
        run.get("answer_text"),
        result.pointer("/answer_judge/answer_text"),
    ])
    .unwrap_or_default();
    let cited = first_string_list(&[
        result.get("cited_document_ids"),
        run.get("cited_document_ids"),
    ]);
    let expected = first_string_list(&[
        result.get("expected_document_ids"),
        run.get("expected_document_ids"),
    ]);
    if question.is_empty() && answer_text.is_empty() && cited.is_empty() && expected.is_empty() {
        return Err("run payload has no judgeable fields".into());
    }
    Ok(AnswerJudgeInput {
        question,
        answer_text,
        cited_document_ids: cited,
        expected_document_ids: expected,
        gold_answer: first_str(&[result.get("gold_answer"), run.get("gold_answer")]),
        context_text: first_str(&[result.get("context_text"), run.get("context_text")]),
    })
}

fn first_str(candidates: &[Option<&Value>]) -> Option<String> {
    candidates
        .iter()
        .flatten()
        .find_map(|v| v.as_str())
        .map(|s| s.to_string())
}

fn first_string_list(candidates: &[Option<&Value>]) -> Vec<String> {
    for value in candidates.iter().flatten() {
        if let Some(arr) = value.as_array() {
            return arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    Vec::new()
}

/// Compact JSON used by HTTP persist of a replay report.
pub fn replay_run_payload(report: &ReplayReport) -> Value {
    json!({
        "kind": "replay",
        "id": uuid::Uuid::new_v4().to_string(),
        "created_at": report.replayed_at,
        "bundle_version": report.bundle_version,
        "judged": report.judged,
        "skipped": report.skipped,
        "items": report.items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_unknown_version() {
        let bundle = EvalExportBundle {
            version: 99,
            exported_at: "now".into(),
            prompts: vec![],
            runs: vec![],
            traces: vec![],
            feedback: vec![],
        };
        assert!(replay_bundle(&bundle).is_err());
    }

    #[test]
    fn replays_stored_feedback_run() {
        let run = json!({
            "id": "run-1",
            "kind": "feedback_run",
            "settings": {"query": "sync rule"},
            "result": {
                "answer_text": "Replica outbox wiki offline ## Citations",
                "cited_document_ids": ["doc-a"],
                "expected_document_ids": ["doc-a"],
                "gold_answer": "replica outbox wiki",
                "answer_judge": {"combined": {"overall": 0.5}}
            }
        });
        let bundle = EvalExportBundle::new(vec![], vec![run], vec![], vec![]);
        let report = replay_bundle(&bundle).unwrap();
        assert_eq!(report.judged, 1);
        assert_eq!(report.skipped, 0);
        let item = &report.items[0];
        assert_eq!(item.status, "judged");
        assert_eq!(item.stored_overall, Some(0.5));
        assert!(item.replayed_overall.unwrap() > 0.5);
        assert!(item.delta.unwrap() > 0.0);
    }

    #[test]
    fn skips_empty_run() {
        let bundle = EvalExportBundle::new(
            vec![],
            vec![json!({"id":"x","kind":"feedback_compare"})],
            vec![],
            vec![],
        );
        let report = replay_bundle(&bundle).unwrap();
        assert_eq!(report.skipped, 1);
        assert_eq!(report.judged, 0);
    }
}
