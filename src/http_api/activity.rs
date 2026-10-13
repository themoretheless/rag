//! `/v1/activity`: read-only view over [`crate::activity`].

use axum::{extract::Query, routing::get, Json, Router};
use serde::{Deserialize, Serialize};

use crate::activity::{self, ActivityEvent, MAX_ACTIVITY_EVENTS};

#[derive(Debug, Serialize)]
struct ActivityResponse {
    items: Vec<ActivityEvent>,
    latest_seq: u64,
    capacity: usize,
}

#[derive(Debug, Default, Deserialize)]
struct ActivityQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    after: Option<u64>,
}

async fn list_activity(Query(query): Query<ActivityQuery>) -> Json<ActivityResponse> {
    let limit = query.limit.unwrap_or(200).clamp(1, MAX_ACTIVITY_EVENTS);
    let (items, latest_seq) = activity::snapshot(limit, query.after);
    Json(ActivityResponse {
        items,
        latest_seq,
        capacity: MAX_ACTIVITY_EVENTS,
    })
}

pub fn routes() -> Router<super::HttpState> {
    Router::new().route("/v1/activity", get(list_activity))
}
