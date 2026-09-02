use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use axum::{extract::Query, routing::get, Json, Router};
use serde::{Deserialize, Serialize};

const MAX_ACTIVITY_EVENTS: usize = 1_000;

#[derive(Clone, Debug, Serialize)]
pub struct ActivityEvent {
    pub seq: u64,
    pub at: String,
    pub kind: &'static str,
    pub client: Option<String>,
    pub action: String,
    pub status: Option<u16>,
    pub elapsed_ms: Option<f64>,
    pub request_id: Option<String>,
}

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

static EVENTS: OnceLock<Mutex<VecDeque<ActivityEvent>>> = OnceLock::new();
static NEXT_SEQ: AtomicU64 = AtomicU64::new(1);

fn events() -> &'static Mutex<VecDeque<ActivityEvent>> {
    EVENTS.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_ACTIVITY_EVENTS)))
}

pub fn record(
    kind: &'static str,
    client: Option<String>,
    action: impl Into<String>,
    status: Option<u16>,
    elapsed_ms: Option<f64>,
    request_id: Option<String>,
) {
    let event = ActivityEvent {
        seq: NEXT_SEQ.fetch_add(1, Ordering::Relaxed),
        at: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
        kind,
        client,
        action: action.into(),
        status,
        elapsed_ms,
        request_id,
    };
    let mut queue = events()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if queue.len() == MAX_ACTIVITY_EVENTS {
        queue.pop_front();
    }
    queue.push_back(event);
}

async fn list_activity(Query(query): Query<ActivityQuery>) -> Json<ActivityResponse> {
    let queue = events()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let latest_seq = queue.back().map_or(0, |event| event.seq);
    let limit = query.limit.unwrap_or(200).clamp(1, MAX_ACTIVITY_EVENTS);
    let items = select_activity(&queue, limit, query.after);
    Json(ActivityResponse {
        items,
        latest_seq,
        capacity: MAX_ACTIVITY_EVENTS,
    })
}

fn select_activity(
    queue: &VecDeque<ActivityEvent>,
    limit: usize,
    after: Option<u64>,
) -> Vec<ActivityEvent> {
    let mut items: Vec<_> = queue
        .iter()
        .rev()
        .filter(|event| after.is_none_or(|after| event.seq > after))
        .take(limit)
        .cloned()
        .collect();
    items.reverse();
    items
}

pub fn routes() -> Router<super::HttpState> {
    Router::new().route("/v1/activity", get(list_activity))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_is_bounded() {
        for n in 0..(MAX_ACTIVITY_EVENTS + 5) {
            record("test", None, format!("event-{n}"), None, None, None);
        }
        let queue = events().lock().unwrap();
        assert_eq!(queue.len(), MAX_ACTIVITY_EVENTS);
        assert_eq!(
            queue.back().unwrap().action,
            format!("event-{}", MAX_ACTIVITY_EVENTS + 4)
        );
    }

    #[test]
    fn selection_is_incremental_and_keeps_chronological_order() {
        let queue = (1..=4)
            .map(|seq| ActivityEvent {
                seq,
                at: String::new(),
                kind: "test",
                client: None,
                action: format!("event-{seq}"),
                status: None,
                elapsed_ms: None,
                request_id: None,
            })
            .collect();
        let selected = select_activity(&queue, 2, Some(1));
        assert_eq!(
            selected.iter().map(|event| event.seq).collect::<Vec<_>>(),
            vec![3, 4]
        );
    }
}
