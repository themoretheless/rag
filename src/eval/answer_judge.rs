//! Answer-level judge: deterministic heuristic plus optional local LLM.
//!
//! The heuristic always runs. LLM scoring is opt-in (`use_llm`) and only used
//! when `RAG_LLM_ENABLED` and a chat client are available. Empty retrieval is
//! not treated as answer correctness.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::judges::{judge_citations, CitationJudgeInput, CitationJudgeResult};
use super::prompts::{builtin_answer_judge, PromptVersion};
use crate::error::{AppError, Result};
use crate::llm::{ChatClient, ChatMessage};

/// Inputs for an answer judge (heuristic and/or LLM).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AnswerJudgeInput {
    #[serde(default)]
    pub question: String,
    #[serde(default)]
    pub answer_text: String,
    #[serde(default)]
    pub cited_document_ids: Vec<String>,
    #[serde(default)]
    pub expected_document_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gold_answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_text: Option<String>,
}

/// Numeric scores in `0..=1` plus a coarse verdict.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnswerJudgeScores {
    pub supported: f64,
    pub complete: f64,
    pub faithful: f64,
    pub overall: f64,
    pub verdict: String,
}

/// Parsed LLM judge payload (same score fields as the heuristic).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmJudgeScores {
    pub supported: f64,
    pub complete: f64,
    pub faithful: f64,
    pub verdict: String,
    #[serde(default)]
    pub rationale: String,
}

/// Combined judge outcome recorded on eval runs / export bundles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnswerJudgeResult {
    pub mode: String,
    pub citation: CitationJudgeResult,
    pub heuristic: AnswerJudgeScores,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmJudgeScores>,
    pub combined: AnswerJudgeScores,
    pub rationale: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<PromptRef>,
}

/// Prompt identity attached to a judged answer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptRef {
    pub id: String,
    pub name: String,
    pub version: String,
    pub content_hash: String,
}

impl From<&PromptVersion> for PromptRef {
    fn from(p: &PromptVersion) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            version: p.version.clone(),
            content_hash: p.content_hash.clone(),
        }
    }
}

/// Deterministic answer + citation scores. Never calls a model.
pub fn judge_answer_heuristic(input: &AnswerJudgeInput) -> AnswerJudgeResult {
    let citation = judge_citations(&CitationJudgeInput {
        answer_text: Some(input.answer_text.clone()),
        cited_document_ids: input.cited_document_ids.clone(),
        expected_document_ids: input.expected_document_ids.clone(),
    });
    let heuristic = heuristic_scores(input, &citation);
    AnswerJudgeResult {
        mode: "heuristic".into(),
        citation,
        combined: heuristic.clone(),
        rationale: heuristic_rationale(input, &heuristic),
        heuristic,
        llm: None,
        prompt: Some((&builtin_answer_judge()).into()),
    }
}

/// Parse the structured JSON object produced by the answer-judge prompt.
pub fn parse_llm_judge_json(text: &str) -> Result<LlmJudgeScores> {
    let value = extract_json_object(text)?;
    let supported = clamp01(json_f64(&value, "supported")?);
    let complete = clamp01(json_f64(&value, "complete")?);
    let faithful = clamp01(json_f64(&value, "faithful")?);
    let verdict = value
        .get("verdict")
        .and_then(|v| v.as_str())
        .map(normalize_verdict)
        .unwrap_or_else(|| verdict_from_overall(mean3(supported, complete, faithful)));
    let rationale = value
        .get("rationale")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .chars()
        .take(500)
        .collect();
    Ok(LlmJudgeScores {
        supported,
        complete,
        faithful,
        verdict,
        rationale,
    })
}

/// Merge heuristic (always) with optional LLM scores. LLM wins combined when present.
pub fn merge_llm_scores(
    mut result: AnswerJudgeResult,
    llm: LlmJudgeScores,
    prompt: &PromptVersion,
) -> AnswerJudgeResult {
    let overall = mean3(llm.supported, llm.complete, llm.faithful);
    result.combined = AnswerJudgeScores {
        supported: llm.supported,
        complete: llm.complete,
        faithful: llm.faithful,
        overall,
        verdict: if llm.verdict == "pass" || llm.verdict == "partial" || llm.verdict == "fail" {
            llm.verdict.clone()
        } else {
            verdict_from_overall(overall)
        },
    };
    result.rationale = if llm.rationale.is_empty() {
        result.rationale
    } else {
        llm.rationale.clone()
    };
    result.mode = "heuristic_llm".into();
    result.prompt = Some(prompt.into());
    result.llm = Some(llm);
    result
}

/// Call the local chat model with a versioned judge prompt.
pub async fn judge_answer_llm(
    client: &ChatClient,
    prompt: &PromptVersion,
    input: &AnswerJudgeInput,
) -> Result<LlmJudgeScores> {
    let user = json!({
        "question": input.question,
        "answer_text": input.answer_text,
        "cited_document_ids": input.cited_document_ids,
        "expected_document_ids": input.expected_document_ids,
        "gold_answer": input.gold_answer,
        "context_text": input.context_text,
    });
    let messages = [
        ChatMessage {
            role: "system".into(),
            content: prompt.body.clone(),
        },
        ChatMessage {
            role: "user".into(),
            content: user.to_string(),
        },
    ];
    let raw = client.complete_with_max_tokens(&messages, 256).await?;
    parse_llm_judge_json(&raw)
}

fn heuristic_scores(input: &AnswerJudgeInput, citation: &CitationJudgeResult) -> AnswerJudgeScores {
    let answer = input.answer_text.trim();
    if answer.is_empty() {
        return AnswerJudgeScores {
            supported: 0.0,
            complete: 0.0,
            faithful: 0.0,
            overall: 0.0,
            verdict: "fail".into(),
        };
    }
    let complete = if !input.expected_document_ids.is_empty() {
        citation.expected_doc_coverage
    } else if let Some(gold) = input.gold_answer.as_deref() {
        token_coverage(gold, answer)
    } else {
        if citation.citation_present { 1.0 } else { 0.5 }
    };
    let reference = input
        .gold_answer
        .as_deref()
        .or(input.context_text.as_deref())
        .unwrap_or("");
    let supported = if !reference.is_empty() {
        token_coverage(answer, reference)
    } else if citation.citation_present {
        1.0
    } else {
        0.0
    };
    let faithful = if let Some(ctx) = input.context_text.as_deref() {
        token_coverage(answer, ctx)
    } else {
        supported
    };
    let overall = mean3(supported, complete, faithful);
    AnswerJudgeScores {
        supported,
        complete,
        faithful,
        overall,
        verdict: verdict_from_overall(overall),
    }
}

fn heuristic_rationale(input: &AnswerJudgeInput, scores: &AnswerJudgeScores) -> String {
    if input.answer_text.trim().is_empty() {
        return "empty answer; heuristic fail".into();
    }
    format!(
        "heuristic overall {:.2} (supported {:.2}, complete {:.2}, faithful {:.2})",
        scores.overall, scores.supported, scores.complete, scores.faithful
    )
}

fn token_coverage(needle: &str, haystack: &str) -> f64 {
    let tokens = tokenize(needle);
    if tokens.is_empty() {
        return 1.0;
    }
    let hay: std::collections::HashSet<&str> = tokenize(haystack).into_iter().collect();
    let hit = tokens.iter().filter(|t| hay.contains(*t)).count();
    hit as f64 / tokens.len() as f64
}

fn tokenize(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 2)
        .collect()
}

fn mean3(a: f64, b: f64, c: f64) -> f64 {
    (a + b + c) / 3.0
}

fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

fn verdict_from_overall(overall: f64) -> String {
    if overall >= 0.8 {
        "pass".into()
    } else if overall >= 0.4 {
        "partial".into()
    } else {
        "fail".into()
    }
}

fn normalize_verdict(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "pass" | "ok" | "yes" => "pass".into(),
        "partial" | "mixed" => "partial".into(),
        _ => "fail".into(),
    }
}

fn json_f64(value: &serde_json::Value, key: &str) -> Result<f64> {
    value
        .get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
        .ok_or_else(|| AppError::config(format!("LLM judge JSON missing numeric `{key}`")))
}

fn extract_json_object(text: &str) -> Result<serde_json::Value> {
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if v.is_object() {
            return Ok(v);
        }
    }
    let start = trimmed.find('{').ok_or_else(|| {
        AppError::config("LLM judge response did not contain a JSON object")
    })?;
    let end = trimmed.rfind('}').ok_or_else(|| {
        AppError::config("LLM judge response did not contain a JSON object")
    })?;
    serde_json::from_str(&trimmed[start..=end])
        .map_err(|e| AppError::config(format!("LLM judge JSON parse: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_answer_fails() {
        let result = judge_answer_heuristic(&AnswerJudgeInput {
            question: "what?".into(),
            answer_text: "".into(),
            expected_document_ids: vec!["doc-a".into()],
            ..Default::default()
        });
        assert_eq!(result.heuristic.verdict, "fail");
        assert_eq!(result.mode, "heuristic");
        assert!(result.prompt.is_some());
    }

    #[test]
    fn gold_overlap_and_citations_pass() {
        let result = judge_answer_heuristic(&AnswerJudgeInput {
            question: "What is the sync rule?".into(),
            answer_text: "replica outbox wiki offline ## Citations".into(),
            cited_document_ids: vec!["doc-a".into()],
            expected_document_ids: vec!["doc-a".into()],
            gold_answer: Some("replica outbox wiki offline".into()),
            context_text: Some("replica outbox wiki offline".into()),
        });
        assert_eq!(result.heuristic.verdict, "pass");
        assert!(result.citation.citation_present);
        assert_eq!(result.citation.expected_doc_coverage, 1.0);
    }

    #[test]
    fn parse_llm_json_from_fenced_text() {
        let parsed = parse_llm_judge_json(
            "sure\n```json\n{\"supported\":0.9,\"complete\":1,\"faithful\":0.8,\"verdict\":\"pass\",\"rationale\":\"grounded\"}\n```",
        )
        .unwrap();
        assert!((parsed.supported - 0.9).abs() < f64::EPSILON);
        assert_eq!(parsed.verdict, "pass");
        assert_eq!(parsed.rationale, "grounded");
    }

    #[test]
    fn merge_prefers_llm_combined() {
        let heuristic = judge_answer_heuristic(&AnswerJudgeInput {
            answer_text: "alpha beta".into(),
            gold_answer: Some("alpha beta".into()),
            ..Default::default()
        });
        let prompt = builtin_answer_judge();
        let merged = merge_llm_scores(
            heuristic,
            LlmJudgeScores {
                supported: 0.2,
                complete: 0.2,
                faithful: 0.2,
                verdict: "fail".into(),
                rationale: "model says no".into(),
            },
            &prompt,
        );
        assert_eq!(merged.mode, "heuristic_llm");
        assert_eq!(merged.combined.verdict, "fail");
        assert!((merged.combined.overall - 0.2).abs() < 1e-9);
        assert_eq!(merged.rationale, "model says no");
    }
}
