//! `GET /v1/activity`.

use crate::dto::ActivityEvent;
use crate::gateway::{client as gateway_client, GatewayApi};

#[derive(Debug, serde::Deserialize)]
struct ActivityResponse {
    #[serde(default)]
    items: Vec<ActivityEvent>,
}

/// Fetch recent gateway activity events from `GET /v1/activity`.
pub fn fetch_activity_http(base: &str) -> Result<Vec<ActivityEvent>, String> {
    let client = gateway_client(5)?;
    GatewayApi::new(&client, base)
        .get::<ActivityResponse>("v1/activity", "Activity")
        .map(|response| response.items)
}
