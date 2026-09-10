//! Offline and gateway evaluation primitives: immutable dataset identity,
//! search setting profiles, A/B compare, checkpoints, and error labels.

mod apply;
mod checkpoint;
mod compare;
mod labels;
mod metrics;
mod settings;

pub use apply::apply_settings_profile;
pub use checkpoint::{load_checkpoint, save_checkpoint, Checkpoint};
pub use compare::{compare_query_metrics, CompareReport, QuerySideMetrics, RegressionEntry};
pub use labels::{ErrorLabel, QueryErrorLabel};
pub use metrics::{macro_average, score_ranked_titles, RankedTitle, ScoredQuery};
pub use settings::SearchSettingsProfile;

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
