//! Human error taxonomy for retrieval regressions (not answer correctness).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorLabel {
    WrongTopHit,
    MissingRelevant,
    StaleCorpus,
    BadSettings,
    NoAnswerExpected,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryErrorLabel {
    pub query_id: String,
    pub label: ErrorLabel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl QueryErrorLabel {
    pub fn load_jsonl(path: &std::path::Path) -> crate::error::Result<Vec<Self>> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            crate::error::AppError::config(format!("read error labels {}: {e}", path.display()))
        })?;
        let mut out = Vec::new();
        for (line_no, line) in raw.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let item: Self = serde_json::from_str(line).map_err(|e| {
                crate::error::AppError::config(format!(
                    "parse error labels {}:{}: {e}",
                    path.display(),
                    line_no + 1
                ))
            })?;
            out.push(item);
        }
        Ok(out)
    }
}
