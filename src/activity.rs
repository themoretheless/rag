//! Process-local, bounded activity feed shared by every transport.
//!
//! HTTP middleware and the MCP dispatcher both record here; only the
//! `/v1/activity` route lives in `http_api`. Events keep anonymous client ids
//! and action names, never request bodies, paths or queries.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Ring-buffer capacity; the oldest event is dropped first.
pub const MAX_ACTIVITY_EVENTS: usize = 1_000;

pub use rag_mcp_core::ActivityEvent;

static EVENTS: OnceLock<Mutex<VecDeque<ActivityEvent>>> = OnceLock::new();
static NEXT_SEQ: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

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
        kind: kind.into(),
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

/// Newest-last page of events after `after`, plus the latest sequence number.
pub fn snapshot(limit: usize, after: Option<u64>) -> (Vec<ActivityEvent>, u64) {
    let queue = events()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let latest_seq = queue.back().map_or(0, |event| event.seq);
    (select_activity(&queue, limit, after), latest_seq)
}

/// One MCP tool call (name, status, latency).
pub fn record_mcp_tool(action: &str, status: u16, elapsed_ms: f64) {
    record(
        "mcp_tool",
        None,
        action,
        Some(status),
        Some(elapsed_ms),
        None,
    );
}

/// A named product action that has no HTTP status of its own.
pub fn record_rag_action(action: &str) {
    record("rag", None, action, Some(200), None, None);
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

#[cfg(test)]
pub(crate) fn latest_matching(kind: &str, action: &str) -> Option<ActivityEvent> {
    events()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .rev()
        .find(|event| event.kind == kind && event.action == action)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_is_bounded() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
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
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let queue = (1..=4)
            .map(|seq| ActivityEvent {
                seq,
                at: String::new(),
                kind: "test".into(),
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
