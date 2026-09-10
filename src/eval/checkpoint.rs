//! Durable per-query checkpoint so long eval runs can resume after process stop.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

use super::compare::QuerySideMetrics;
use super::labels::ErrorLabel;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Checkpoint {
    pub dataset_content_hash: String,
    pub settings_a_name: String,
    pub settings_b_name: String,
    pub completed_query_ids: BTreeSet<String>,
    pub pairs: Vec<CheckpointPair>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckpointPair {
    pub query_id: String,
    pub side_a: QuerySideMetrics,
    pub side_b: QuerySideMetrics,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_label: Option<ErrorLabel>,
}

pub fn load_checkpoint(path: &Path) -> crate::error::Result<Option<Checkpoint>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path).map_err(|e| {
        crate::error::AppError::config(format!("read checkpoint {}: {e}", path.display()))
    })?;
    let checkpoint: Checkpoint = serde_json::from_str(&raw).map_err(|e| {
        crate::error::AppError::config(format!("parse checkpoint {}: {e}", path.display()))
    })?;
    Ok(Some(checkpoint))
}

pub fn save_checkpoint(path: &Path, checkpoint: &Checkpoint) -> crate::error::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            crate::error::AppError::config(format!(
                "create checkpoint dir {}: {e}",
                parent.display()
            ))
        })?;
    }
    let tmp = path.with_extension("json.tmp");
    let raw = serde_json::to_string_pretty(checkpoint).map_err(|e| {
        crate::error::AppError::config(format!("serialize checkpoint: {e}"))
    })?;
    std::fs::write(&tmp, raw).map_err(|e| {
        crate::error::AppError::config(format!("write checkpoint {}: {e}", tmp.display()))
    })?;
    std::fs::rename(&tmp, path).map_err(|e| {
        crate::error::AppError::config(format!("finalize checkpoint {}: {e}", path.display()))
    })?;
    Ok(())
}

impl Checkpoint {
    pub fn compatible_with(
        &self,
        dataset_content_hash: &str,
        settings_a_name: &str,
        settings_b_name: &str,
    ) -> bool {
        self.dataset_content_hash == dataset_content_hash
            && self.settings_a_name == settings_a_name
            && self.settings_b_name == settings_b_name
    }

    pub fn record(
        &mut self,
        query_id: String,
        side_a: QuerySideMetrics,
        side_b: QuerySideMetrics,
        error_label: Option<ErrorLabel>,
    ) {
        self.completed_query_ids.insert(query_id.clone());
        if let Some(existing) = self.pairs.iter_mut().find(|p| p.query_id == query_id) {
            existing.side_a = side_a;
            existing.side_b = side_b;
            existing.error_label = error_label;
        } else {
            self.pairs.push(CheckpointPair {
                query_id,
                side_a,
                side_b,
                error_label,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_resume_skips_completed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ckpt.json");
        let mut ckpt = Checkpoint {
            dataset_content_hash: "abc".into(),
            settings_a_name: "A".into(),
            settings_b_name: "B".into(),
            ..Default::default()
        };
        ckpt.record(
            "q1".into(),
            QuerySideMetrics {
                recall_at_k: 1.0,
                reciprocal_rank: 1.0,
                ndcg_at_k: 1.0,
            },
            QuerySideMetrics {
                recall_at_k: 0.5,
                reciprocal_rank: 0.5,
                ndcg_at_k: 0.5,
            },
            None,
        );
        save_checkpoint(&path, &ckpt).unwrap();
        let loaded = load_checkpoint(&path).unwrap().unwrap();
        assert!(loaded.completed_query_ids.contains("q1"));
        assert!(loaded.compatible_with("abc", "A", "B"));
        assert!(!loaded.compatible_with("abc", "A", "C"));
    }
}
