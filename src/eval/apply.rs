//! Apply a settings profile onto a SearchQuery.

use super::SearchSettingsProfile;
use crate::db::search::{DiversityMode, SearchQuery};
use crate::error::{AppError, Result};
use crate::models::SearchMode;

pub fn apply_settings_profile(
    mut query: SearchQuery,
    profile: &SearchSettingsProfile,
) -> Result<SearchQuery> {
    if let Some(mode) = &profile.mode {
        query.mode = SearchMode::parse(mode).map_err(AppError::config)?;
    }
    if let Some(top_k) = profile.top_k {
        if top_k == 0 {
            return Err(AppError::config("settings top_k must be greater than zero"));
        }
        query.top_k = top_k;
    }
    if let Some(wing) = &profile.wing {
        query.wing = Some(wing.clone());
    }
    if let Some(room) = &profile.room {
        query.room = Some(room.clone());
    }
    if let Some(layer) = &profile.layer {
        query.layer = Some(layer.clone());
    }
    if let Some(min_score) = profile.min_score {
        query.min_score = Some(min_score);
    }
    if let Some(diversity) = &profile.diversity {
        query.diversity = Some(DiversityMode::parse(diversity)?);
    }
    if let Some(max_chunks) = profile.max_chunks_per_document {
        query.max_chunks_per_document = Some(max_chunks);
    }
    if let Some(rrf_k) = profile.rrf_k {
        query.rrf_k = rrf_k;
    }
    if let Some(half_life) = profile.recency_half_life_days {
        query.recency_half_life_days = Some(half_life);
    }
    Ok(query)
}
