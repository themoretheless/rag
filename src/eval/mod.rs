//! Offline and gateway evaluation primitives: immutable dataset identity,
//! search setting profiles, A/B compare, checkpoints, and error labels.

mod answer_judge;
mod apply;
mod checkpoint;
mod compare;
mod export;
mod judges;
mod labels;
mod metrics;
mod prompts;
mod settings;
mod trace;

pub use answer_judge::{
    judge_answer_heuristic, judge_answer_llm, merge_llm_scores, parse_llm_judge_json,
    AnswerJudgeInput, AnswerJudgeResult, AnswerJudgeScores, LlmJudgeScores, PromptRef,
};
pub use apply::apply_settings_profile;
pub use checkpoint::{load_checkpoint, save_checkpoint, Checkpoint};
pub use compare::{compare_query_metrics, CompareReport, QuerySideMetrics, RegressionEntry};
pub use export::{
    load_export_bundle, replay_bundle, replay_run_payload, EvalExportBundle, ReplayReport,
    EXPORT_BUNDLE_VERSION,
};
pub use judges::{judge_citations, CitationJudgeInput, CitationJudgeResult};
pub use labels::{ErrorLabel, QueryErrorLabel};
pub use metrics::{macro_average, score_ranked_titles, RankedTitle, ScoredQuery};
pub use prompts::{
    builtin_answer_judge, prompt_content_hash, validate_prompt, PromptVersion, ANSWER_JUDGE_V1_BODY,
    ANSWER_JUDGE_V1_NAME, ANSWER_JUDGE_V1_VERSION, PROMPT_KIND_ANSWER_JUDGE,
};
pub use settings::SearchSettingsProfile;
pub use trace::{
    source_versions_json, RetrievalTrace, SourceVersion, SpanBuilder, TraceSpan,
};

use crate::util::content_hash;
use std::path::Path;

/// Content-addressed identity of a dataset file (immutable version marker).
pub fn dataset_content_hash(path: &Path) -> crate::error::Result<String> {
    let raw = std::fs::read(path).map_err(|e| {
        crate::error::AppError::config(format!("read dataset {}: {e}", path.display()))
    })?;
    Ok(content_hash(&String::from_utf8_lossy(&raw)))
}

/// Hash arbitrary UTF-8 bytes (for in-memory or already-loaded datasets).
pub fn dataset_bytes_hash(raw: &str) -> String {
    content_hash(raw)
}
