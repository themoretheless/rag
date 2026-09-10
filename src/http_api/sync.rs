//! Durable multi-machine synchronization transport.

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::db::sync::{
    validate_wiki_change, SyncChange, SyncChangeInput, SyncIdentity, WikiSyncWrite,
};
use crate::error::AppError;
use crate::wiki::{self, WikiWriteCommand, WriteWikiOpts};

use super::{
    error::{api_err, api_ok},
    HttpState,
};

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/sync/status", get(status))
        .route("/v1/sync/register", post(register))
        .route("/v1/sync/push", post(push))
        .route("/v1/sync/pull", get(pull))
        .route("/v1/sync/ack", post(ack))
}

fn identity() -> Result<(String, String, Option<String>), AppError> {
    let identity = SyncIdentity::from_env()?;
    Ok((
        identity.role().into(),
        identity.node_id,
        identity.primary_url,
    ))
}

async fn status(State(st): State<HttpState>) -> impl IntoResponse {
    let (role, node_id, primary_url) = match identity() {
        Ok(value) => value,
        Err(error) => return api_err(error),
    };
    match st.store.sync_status(&role, &node_id, primary_url) {
        Ok(value) => api_ok(json!({"ok": true, "sync": value})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct RegisterBody {
    node_id: String,
    hostname: String,
    #[serde(default = "replica_role")]
    role: String,
}
fn replica_role() -> String {
    "replica".into()
}

async fn register(
    State(st): State<HttpState>,
    Json(body): Json<RegisterBody>,
) -> impl IntoResponse {
    match st
        .store
        .register_sync_node(&body.node_id, &body.hostname, &body.role)
    {
        Ok(()) => api_ok(json!({"ok": true, "node_id": body.node_id})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct PushBody {
    node_id: String,
    changes: Vec<SyncChangeInput>,
}

async fn push(State(st): State<HttpState>, Json(body): Json<PushBody>) -> impl IntoResponse {
    match identity() {
        Ok((role, _, _)) if role == "primary" => {}
        Ok(_) => {
            return api_err(AppError::forbidden(
                "replica does not accept primary push operations",
            ))
        }
        Err(error) => return api_err(error),
    }
    // Validate the whole batch before the first mutation. Each accepted event then
    // commits page+journal together; a retry safely resumes any committed prefix.
    if let Err(error) = st.store.preflight_sync_batch(&body.node_id, &body.changes) {
        return api_err(error);
    }
    let mut primary_sequences = Vec::with_capacity(body.changes.len());
    for change in &body.changes {
        match apply_change(
            &st,
            WikiSyncWrite::Push {
                origin_node: &body.node_id,
                change,
            },
        )
        .await
        {
            Ok(sequence) => primary_sequences.push(sequence),
            Err(error) => return api_err(error),
        }
    }
    api_ok(
        json!({"ok": true, "accepted": primary_sequences.len(), "primary_sequences": primary_sequences}),
    )
}

async fn apply_change(st: &HttpState, mode: WikiSyncWrite<'_>) -> Result<i64, AppError> {
    let (origin_node, change) = match &mode {
        WikiSyncWrite::Push {
            origin_node,
            change,
        } => (*origin_node, *change),
        WikiSyncWrite::Pull { event, .. } => (event.origin_node.as_str(), &event.change),
        WikiSyncWrite::Local(_) => {
            return Err(AppError::config(
                "transport requires incoming sync provenance",
            ))
        }
    };
    let payload = validate_wiki_change(change)?;
    for attempt in 0..3 {
        if let Some(sequence) = st.store.previously_applied_sync(&mode)? {
            return Ok(sequence);
        }
        let result = wiki::write_wiki_page_synced_command(
            &st.store,
            &st.embedder,
            &st.config,
            WikiWriteCommand {
                slug: payload.slug.clone(),
                title: payload.title.clone(),
                content: payload.content.clone(),
                wing: payload.wing.clone(),
                room: payload.room.clone(),
                kind: payload.kind.clone().unwrap_or_else(|| "wiki".into()),
                category: payload.category.clone(),
                summary: payload.summary.clone(),
                agent: Some(format!("sync:{origin_node}")),
                options: WriteWikiOpts {
                    metadata_json_override: payload.metadata_json.clone(),
                    ..Default::default()
                },
            },
            mode.clone(),
            false,
        )
        .await;
        match result {
            Ok((_, sequence)) => return Ok(sequence),
            // Concurrent first writes may race on the URI owner. Re-prepare against
            // the now-current row; event collision checks run again inside the tx.
            Err(AppError::Conflict(_)) if attempt < 2 => continue,
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded retry returns on its last attempt")
}

#[derive(Deserialize)]
struct PushReply {
    primary_sequences: Vec<i64>,
}
#[derive(Deserialize)]
struct PullReply {
    items: Vec<SyncChange>,
    next_cursor: i64,
}

/// Start the replica loop when `RAG_PRIMARY_URL` is configured. The primary
/// intentionally does not call itself.
pub(super) fn spawn_worker(st: HttpState) {
    let (role, node_id, configured_primary) = match identity() {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "cannot initialize database sync identity");
            return;
        }
    };
    let hostname = std::env::var("RAG_NODE_NAME").unwrap_or_else(|_| node_id.clone());
    if let Err(error) = st.store.register_sync_node(&node_id, &hostname, &role) {
        tracing::error!(%error, %node_id, "cannot initialize database sync node");
    }
    let Some(primary_url) = configured_primary else {
        return;
    };
    let interval_secs = std::env::var("RAG_SYNC_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(5u64)
        .max(1);
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(token) = std::env::var("RAG_PRIMARY_TOKEN") {
        let Ok(mut value) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
        else {
            tracing::error!("RAG_PRIMARY_TOKEN is not a valid credential header");
            return;
        };
        value.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, value);
    }
    let client = match reqwest::Client::builder()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            tracing::error!("cannot initialize database sync HTTP client");
            return;
        }
    };
    tokio::spawn(async move {
        let base = primary_url.trim_end_matches('/').to_string();
        let _ = st.store.register_sync_node(&node_id, &hostname, "replica");
        loop {
            if let Err(error) = sync_once(&st, &client, &base, &node_id, &hostname).await {
                tracing::warn!(%error, %node_id, "database sync cycle failed");
            }
            tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
        }
    });
}

/// Bound serialized bytes as well as event count. Old oversized outbox rows stay
/// intact and produce a diagnostic instead of being deleted or marked delivered.
fn bounded_sync_batch(
    node_id: &str,
    pending: &[SyncChangeInput],
) -> Result<Vec<SyncChangeInput>, AppError> {
    let mut bytes = serde_json::to_vec(&json!({"node_id":node_id,"changes":[]}))?.len();
    let mut batch = Vec::new();
    for event in pending {
        let event_bytes = serde_json::to_vec(event)?.len();
        let next = bytes + event_bytes + usize::from(!batch.is_empty());
        if next > super::MAX_HTTP_BODY_BYTES || event_bytes > crate::db::sync::MAX_SYNC_EVENT_BYTES
        {
            if batch.is_empty() {
                return Err(AppError::config("pending sync event exceeds the HTTP body limit; preserve the legacy outbox and repair the oversized entry before retrying"));
            }
            break;
        }
        batch.push(event.clone());
        bytes = next;
    }
    Ok(batch)
}

async fn sync_once(
    st: &HttpState,
    client: &reqwest::Client,
    base: &str,
    node_id: &str,
    hostname: &str,
) -> Result<(), AppError> {
    client
        .post(format!("{base}/v1/sync/register"))
        .json(&json!({"node_id":node_id,"hostname":hostname,"role":"replica"}))
        .send()
        .await
        .map_err(|error| AppError::db(format!("sync register request: {error}")))?
        .error_for_status()
        .map_err(|error| AppError::db(format!("sync register response: {error}")))?;

    let pending = bounded_sync_batch(node_id, &st.store.pending_sync_outbox(100)?)?;
    if !pending.is_empty() {
        let reply = client
            .post(format!("{base}/v1/sync/push"))
            .json(&json!({"node_id":node_id,"changes":pending}))
            .send()
            .await
            .map_err(|error| AppError::db(format!("sync push request: {error}")))?
            .error_for_status()
            .map_err(|error| AppError::db(format!("sync push response: {error}")))?
            .json::<PushReply>()
            .await
            .map_err(|error| AppError::db(format!("sync push decode: {error}")))?;
        if reply.primary_sequences.len() != pending.len() {
            return Err(AppError::db("sync push acknowledgement length mismatch"));
        }
        let assignments = pending
            .iter()
            .zip(reply.primary_sequences)
            .map(|(change, primary)| (change.origin_seq, primary))
            .collect::<Vec<_>>();
        st.store.mark_sync_outbox_sent(&assignments)?;
    }

    let local = st
        .store
        .sync_status("replica", node_id, Some(base.into()))?;
    let cursor = local
        .nodes
        .iter()
        .find(|node| node.node_id == node_id)
        .map(|node| node.pull_cursor)
        .unwrap_or(0);
    let reply = client
        .get(format!("{base}/v1/sync/pull"))
        .query(&[
            ("node_id", node_id),
            ("after", &cursor.to_string()),
            ("limit", "100"),
        ])
        .send()
        .await
        .map_err(|error| AppError::db(format!("sync pull request: {error}")))?
        .error_for_status()
        .map_err(|error| AppError::db(format!("sync pull response: {error}")))?
        .json::<PullReply>()
        .await
        .map_err(|error| AppError::db(format!("sync pull decode: {error}")))?;
    // Reject incomplete/reordered replies before applying any event. Every
    // successful local event records its own canonical cursor in the page tx.
    let mut expected = cursor;
    for item in &reply.items {
        validate_wiki_change(&item.change)?;
        expected += 1;
        if item.primary_seq != expected {
            return Err(AppError::conflict(
                "primary returned a non-consecutive sync stream",
            ));
        }
    }
    if reply.next_cursor != expected {
        return Err(AppError::config(
            "sync reply cursor does not match its events",
        ));
    }
    for item in &reply.items {
        apply_change(
            st,
            WikiSyncWrite::Pull {
                node_id,
                event: item,
            },
        )
        .await?;
    }
    // Always retry the acknowledgement, even when a previous ack failed after
    // the last local event committed and the next pull has no new items.
    client
        .post(format!("{base}/v1/sync/ack"))
        .json(&json!({"node_id":node_id,"cursor":expected}))
        .send()
        .await
        .map_err(|error| AppError::db(format!("sync ack request: {error}")))?
        .error_for_status()
        .map_err(|error| AppError::db(format!("sync ack response: {error}")))?;
    Ok(())
}

#[derive(Deserialize)]
struct PullQuery {
    node_id: String,
    #[serde(default)]
    after: i64,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    100
}

async fn pull(State(st): State<HttpState>, Query(query): Query<PullQuery>) -> impl IntoResponse {
    match st
        .store
        .pull_sync_changes(&query.node_id, query.after, query.limit)
    {
        Ok(items) => {
            let next_cursor = items
                .last()
                .map(|item| item.primary_seq)
                .unwrap_or(query.after);
            api_ok(
                json!({"ok": true, "count": items.len(), "items": items, "next_cursor": next_cursor}),
            )
        }
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct AckBody {
    node_id: String,
    cursor: i64,
}

async fn ack(State(st): State<HttpState>, Json(body): Json<AckBody>) -> impl IntoResponse {
    match st.store.acknowledge_sync_cursor(&body.node_id, body.cursor) {
        Ok(()) => api_ok(json!({"ok": true, "node_id": body.node_id, "cursor": body.cursor})),
        Err(error) => api_err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    fn state(root: &std::path::Path, name: &str) -> HttpState {
        let config = crate::Config {
            db_path: root.join(format!("{name}.duckdb")),
            embedding_dims: 16,
            ..crate::Config::for_tests()
        };
        HttpState::new(
            Arc::new(crate::Store::open(&config.db_path).unwrap()),
            false,
            config,
            Arc::new(crate::embeddings::MockEmbedder::new(16)),
        )
    }

    fn command(body: &str) -> WikiWriteCommand {
        WikiWriteCommand {
            slug: "shared".into(),
            title: "Shared".into(),
            content: body.into(),
            wing: None,
            room: None,
            kind: "wiki".into(),
            category: None,
            summary: None,
            agent: Some("sync:untrusted-user".into()),
            options: WriteWikiOpts::default(),
        }
    }

    async fn local_write(st: &HttpState, node_id: &str, replica: bool, body: &str) {
        wiki::write_wiki_page_synced_command(
            &st.store,
            &st.embedder,
            &st.config,
            command(body),
            WikiSyncWrite::Local(SyncIdentity {
                node_id: node_id.into(),
                primary_url: replica.then(|| "http://primary".into()),
            }),
            false,
        )
        .await
        .unwrap();
    }

    fn body(st: &HttpState) -> String {
        st.store
            .find_by_uri("wiki://shared")
            .unwrap()
            .unwrap()
            .content
    }
    fn revision(st: &HttpState) -> i64 {
        st.store
            .find_by_uri("wiki://shared")
            .unwrap()
            .unwrap()
            .revision
    }
    fn cursor(st: &HttpState, node: &str) -> i64 {
        st.store
            .sync_status("replica", node, None)
            .unwrap()
            .nodes
            .into_iter()
            .find(|item| item.node_id == node)
            .map(|item| item.pull_cursor)
            .unwrap_or(0)
    }

    fn event(seq: i64, body: &str) -> SyncChangeInput {
        SyncChangeInput {
            origin_seq: seq,
            entity_kind: "wiki".into(),
            entity_id: "wiki://shared".into(),
            operation: "upsert".into(),
            payload_json: json!({"slug":"shared","title":"Shared","content":body}).to_string(),
            content_hash: None,
            created_at: "2026-09-05T00:00:00.123456789Z".into(),
        }
    }

    async fn primary_server(st: HttpState) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, routes().with_state(st))
                .await
                .unwrap();
        });
        (base, task)
    }

    #[tokio::test]
    async fn sync_http_validates_all_events_before_writing_and_rejects_identity_reuse() {
        let root = tempfile::tempdir().unwrap();
        let primary = state(root.path(), "primary");
        let app = routes().with_state(primary.clone());
        let first = event(1, "first");
        let mut invalid = event(2, "bad");
        invalid.created_at = "2026-09-05 00:00:00".into();
        for (events, expected) in [
            (vec![first.clone(), invalid], StatusCode::BAD_REQUEST),
            (vec![first.clone()], StatusCode::OK),
            (vec![event(1, "different")], StatusCode::CONFLICT),
        ] {
            let request = Request::builder()
                .method("POST")
                .uri("/v1/sync/push")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"node_id":"replica","changes":events}).to_string(),
                ))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                expected,
                "{}",
                String::from_utf8_lossy(&to_bytes(response.into_body(), 65536).await.unwrap())
            );
            if expected == StatusCode::BAD_REQUEST {
                assert!(primary
                    .store
                    .find_by_uri("wiki://shared")
                    .unwrap()
                    .is_none());
                assert!(!primary.store.has_sync_change("replica", 1).unwrap());
            }
        }
        assert_eq!(body(&primary), "first");
        assert_eq!(revision(&primary), 1);
    }

    #[tokio::test]
    async fn sync_page_journal_and_pull_cursor_rollback_together_and_retry_once() {
        let root = tempfile::tempdir().unwrap();
        let replica = state(root.path(), "replica");
        let input = event(1, "canonical");
        let canonical = SyncChange {
            primary_seq: 1,
            origin_node: "primary".into(),
            change: input,
        };
        let mut failing = command("canonical");
        failing.options.op = Some("__test_fail_wiki_after_sync_journal__".into());
        let result = wiki::write_wiki_page_synced_command(
            &replica.store,
            &replica.embedder,
            &replica.config,
            failing,
            WikiSyncWrite::Pull {
                node_id: "replica",
                event: &canonical,
            },
            false,
        )
        .await;
        assert!(result.is_err());
        assert!(replica
            .store
            .find_by_uri("wiki://shared")
            .unwrap()
            .is_none());
        assert!(!replica.store.has_sync_change("primary", 1).unwrap());
        assert_eq!(cursor(&replica, "replica"), 0);
        apply_change(
            &replica,
            WikiSyncWrite::Pull {
                node_id: "replica",
                event: &canonical,
            },
        )
        .await
        .unwrap();
        let committed = revision(&replica);
        apply_change(
            &replica,
            WikiSyncWrite::Pull {
                node_id: "replica",
                event: &canonical,
            },
        )
        .await
        .unwrap();
        assert_eq!(revision(&replica), committed);
        assert_eq!(cursor(&replica, "replica"), 1);
        assert!(replica.store.pending_sync_outbox(10).unwrap().is_empty());

        let local = state(root.path(), "local");
        let mut failing = command("local");
        failing.options.op = Some("__test_fail_wiki_after_sync_journal__".into());
        let result = wiki::write_wiki_page_synced_command(
            &local.store,
            &local.embedder,
            &local.config,
            failing,
            WikiSyncWrite::Local(SyncIdentity {
                node_id: "local".into(),
                primary_url: Some("http://primary".into()),
            }),
            false,
        )
        .await;
        assert!(result.is_err());
        assert!(local.store.pending_sync_outbox(10).unwrap().is_empty());
        assert!(local.store.find_by_uri("wiki://shared").unwrap().is_none());
        // Public agent strings cannot opt out of journalling.
        wiki::write_wiki_page_command(
            &local.store,
            &local.embedder,
            &local.config,
            command("public"),
        )
        .await
        .unwrap();
        assert_eq!(
            local
                .store
                .sync_status("primary", "local", None)
                .unwrap()
                .latest_primary_seq,
            1
        );
    }

    #[tokio::test]
    async fn sync_real_cycles_converge_offline_conflicts_and_retries_without_duplicate_revisions() {
        let root = tempfile::tempdir().unwrap();
        let primary = state(root.path(), "primary");
        let first = state(root.path(), "first");
        let second = state(root.path(), "second");
        local_write(&first, "first", true, "offline first").await;
        local_write(&second, "second", true, "offline second").await;
        let (base, server) = primary_server(primary.clone()).await;
        let client = reqwest::Client::new();
        sync_once(&first, &client, &base, "first", "first.local")
            .await
            .unwrap();
        sync_once(&second, &client, &base, "second", "second.local")
            .await
            .unwrap();
        sync_once(&first, &client, &base, "first", "first.local")
            .await
            .unwrap();
        assert_eq!(body(&primary), "offline second");
        assert_eq!(body(&first), body(&primary));
        assert_eq!(body(&second), body(&primary));
        let revisions = (revision(&first), revision(&second), revision(&primary));
        sync_once(&first, &client, &base, "first", "first.local")
            .await
            .unwrap();
        sync_once(&second, &client, &base, "second", "second.local")
            .await
            .unwrap();
        assert_eq!(
            (revision(&first), revision(&second), revision(&primary)),
            revisions
        );
        assert_eq!(cursor(&first, "first"), 2);
        assert_eq!(cursor(&second, "second"), 2);
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn sync_replays_more_than_one_page_of_events_without_losing_pending_drafts() {
        let root = tempfile::tempdir().unwrap();
        let primary = state(root.path(), "primary");
        let replica = state(root.path(), "replica");
        for i in 1..=105 {
            local_write(&replica, "replica", true, &format!("offline {i}")).await;
        }
        let (base, server) = primary_server(primary.clone()).await;
        let client = reqwest::Client::new();
        sync_once(&replica, &client, &base, "replica", "replica.local")
            .await
            .unwrap();
        assert_eq!(body(&replica), "offline 105"); // The first 100 older own events cannot erase five unsent edits.
        assert_eq!(replica.store.pending_sync_outbox(100).unwrap().len(), 5);
        assert_eq!(cursor(&replica, "replica"), 100);
        sync_once(&replica, &client, &base, "replica", "replica.local")
            .await
            .unwrap();
        assert_eq!(body(&replica), "offline 105");
        assert_eq!(body(&primary), body(&replica));
        assert_eq!(cursor(&replica, "replica"), 105);
        assert!(replica.store.pending_sync_outbox(100).unwrap().is_empty());
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn sync_edit_between_push_and_pull_survives_and_a_lost_push_reply_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let primary = state(root.path(), "primary");
        let replica = state(root.path(), "replica");
        local_write(&replica, "replica", true, "submitted").await;
        let submitted = replica.store.pending_sync_outbox(10).unwrap().remove(0);
        let sequence = apply_change(
            &primary,
            WikiSyncWrite::Push {
                origin_node: "replica",
                change: &submitted,
            },
        )
        .await
        .unwrap();
        let primary_revision = revision(&primary);
        local_write(&replica, "replica", true, "new draft").await;
        // Simulate a lost push response: retry the same event before marking sent.
        assert_eq!(
            apply_change(
                &primary,
                WikiSyncWrite::Push {
                    origin_node: "replica",
                    change: &submitted
                }
            )
            .await
            .unwrap(),
            sequence
        );
        assert_eq!(revision(&primary), primary_revision);
        replica
            .store
            .mark_sync_outbox_sent(&[(submitted.origin_seq, sequence)])
            .unwrap();
        let first = primary
            .store
            .pull_sync_changes("replica", 0, 10)
            .unwrap()
            .remove(0);
        apply_change(
            &replica,
            WikiSyncWrite::Pull {
                node_id: "replica",
                event: &first,
            },
        )
        .await
        .unwrap();
        assert_eq!(body(&replica), "new draft");
        assert_eq!(cursor(&replica, "replica"), 1);
        let (base, server) = primary_server(primary.clone()).await;
        let client = reqwest::Client::new();
        sync_once(&replica, &client, &base, "replica", "replica.local")
            .await
            .unwrap();
        assert_eq!(body(&primary), "new draft");
        assert_eq!(body(&replica), "new draft");
        assert_eq!(cursor(&replica, "replica"), 2);
        server.abort();
        let _ = server.await;
    }

    struct ConcurrentEmbedder {
        barrier: tokio::sync::Barrier,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl crate::embeddings::EmbeddingProvider for ConcurrentEmbedder {
        async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
                self.barrier.wait().await;
            }
            Ok(vec![vec![1.0; 16]; texts.len()])
        }
        fn dimensions(&self) -> usize {
            16
        }
    }

    #[tokio::test]
    async fn sync_concurrent_first_writes_follow_the_committed_canonical_order() {
        let root = tempfile::tempdir().unwrap();
        let mut primary = state(root.path(), "primary");
        primary.embedder = Arc::new(ConcurrentEmbedder {
            barrier: tokio::sync::Barrier::new(2),
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let left = event(1, "left");
        let right = event(1, "right");
        let (a, b) = tokio::join!(
            apply_change(
                &primary,
                WikiSyncWrite::Push {
                    origin_node: "left",
                    change: &left
                }
            ),
            apply_change(
                &primary,
                WikiSyncWrite::Push {
                    origin_node: "right",
                    change: &right
                }
            ),
        );
        let mut assignments = vec![a.unwrap(), b.unwrap()];
        assignments.sort_unstable();
        assert_eq!(assignments, vec![1, 2]);
        let canonical = primary.store.pull_sync_changes("observer", 0, 10).unwrap();
        assert_eq!(
            body(&primary),
            validate_wiki_change(&canonical.last().unwrap().change)
                .unwrap()
                .content
        );
        let replica = state(root.path(), "replica");
        for event in &canonical {
            apply_change(
                &replica,
                WikiSyncWrite::Pull {
                    node_id: "replica",
                    event,
                },
            )
            .await
            .unwrap();
        }
        assert_eq!(body(&replica), body(&primary));
        assert_eq!(cursor(&replica, "replica"), 2);
    }

    #[tokio::test]
    async fn sync_metadata_snapshot_preserves_source_versions_across_nodes() {
        let root = tempfile::tempdir().unwrap();
        let first = state(root.path(), "first");
        let primary = state(root.path(), "primary");
        let second = state(root.path(), "second");
        let metadata = json!({"source_versions":[{"document_id":"source","content_hash":"captured-hash"}],"custom":{"preserved":true}});
        let mut page = command("compiled body");
        page.options.extra_metadata = Some(metadata.clone());
        wiki::write_wiki_page_synced_command(
            &first.store,
            &first.embedder,
            &first.config,
            page,
            WikiSyncWrite::Local(SyncIdentity {
                node_id: "first".into(),
                primary_url: Some("http://primary".into()),
            }),
            false,
        )
        .await
        .unwrap();
        let outgoing = first.store.pending_sync_outbox(1).unwrap().remove(0);
        let payload = validate_wiki_change(&outgoing).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&payload.metadata_json.unwrap()).unwrap(),
            metadata
        );
        apply_change(
            &primary,
            WikiSyncWrite::Push {
                origin_node: "first",
                change: &outgoing,
            },
        )
        .await
        .unwrap();
        let incoming = primary
            .store
            .pull_sync_changes("second", 0, 1)
            .unwrap()
            .remove(0);
        apply_change(
            &second,
            WikiSyncWrite::Pull {
                node_id: "second",
                event: &incoming,
            },
        )
        .await
        .unwrap();
        for state in [&primary, &second] {
            let doc = state.store.find_by_uri("wiki://shared").unwrap().unwrap();
            let received: serde_json::Value = serde_json::from_str(&doc.metadata_json).unwrap();
            assert_eq!(received, metadata);
        }
    }

    #[test]
    fn sync_batches_fit_the_gateway_body_limit_and_do_not_skip_oversized_entries() {
        let medium = event(1, &"x".repeat(500_000));
        let batch =
            bounded_sync_batch("replica", &[medium.clone(), medium.clone(), medium]).unwrap();
        assert_eq!(batch.len(), 2);
        assert!(
            serde_json::to_vec(&json!({"node_id":"replica","changes":batch}))
                .unwrap()
                .len()
                <= super::super::MAX_HTTP_BODY_BYTES
        );
        let large = event(1, &"x".repeat(super::super::MAX_HTTP_BODY_BYTES));
        assert!(bounded_sync_batch("replica", &[large]).is_err());
    }

    #[tokio::test]
    async fn sync_rejects_untransportable_new_wiki_without_committing_page_or_outbox() {
        let root = tempfile::tempdir().unwrap();
        let replica = state(root.path(), "replica");
        let mut page = command("small body");
        page.options.extra_metadata =
            Some(json!({"oversized":"x".repeat(crate::db::sync::MAX_SYNC_EVENT_BYTES)}));
        let result = wiki::write_wiki_page_synced_command(
            &replica.store,
            &replica.embedder,
            &replica.config,
            page,
            WikiSyncWrite::Local(SyncIdentity {
                node_id: "replica".into(),
                primary_url: Some("http://primary".into()),
            }),
            false,
        )
        .await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("sync event exceeds"));
        assert!(replica
            .store
            .find_by_uri("wiki://shared")
            .unwrap()
            .is_none());
        assert!(replica.store.pending_sync_outbox(10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn sync_event_size_limit_uses_the_allocated_outbox_sequence_and_rolls_back() {
        let root = tempfile::tempdir().unwrap();
        let replica = state(root.path(), "replica");
        let mode = WikiSyncWrite::Local(SyncIdentity {
            node_id: "replica".into(),
            primary_url: Some("http://primary".into()),
        });
        let mut page = command("small body");
        page.options.extra_metadata = Some(json!({"padding":""}));
        for _ in 0..8 {
            wiki::write_wiki_page_synced_command(
                &replica.store,
                &replica.embedder,
                &replica.config,
                page.clone(),
                mode.clone(),
                false,
            )
            .await
            .unwrap();
        }
        let pending = replica.store.pending_sync_outbox(10).unwrap();
        let padding = crate::db::sync::MAX_SYNC_EVENT_BYTES
            - serde_json::to_vec(pending.last().unwrap()).unwrap().len();
        page.options.extra_metadata = Some(json!({"padding":"x".repeat(padding)}));
        wiki::write_wiki_page_synced_command(
            &replica.store,
            &replica.embedder,
            &replica.config,
            page.clone(),
            mode.clone(),
            false,
        )
        .await
        .unwrap();
        let pending = replica.store.pending_sync_outbox(10).unwrap();
        assert_eq!(pending.last().unwrap().origin_seq, 9);
        assert_eq!(
            serde_json::to_vec(pending.last().unwrap()).unwrap().len(),
            crate::db::sync::MAX_SYNC_EVENT_BYTES
        );
        let before = replica.store.find_by_uri("wiki://shared").unwrap().unwrap();
        let error = wiki::write_wiki_page_synced_command(
            &replica.store,
            &replica.embedder,
            &replica.config,
            page,
            mode,
            false,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("sync event exceeds"));
        let after = replica.store.find_by_uri("wiki://shared").unwrap().unwrap();
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.content, before.content);
        assert_eq!(after.metadata_json, before.metadata_json);
        assert_eq!(replica.store.pending_sync_outbox(10).unwrap(), pending);
    }

    fn replica_metadata_edit(st: &HttpState, metadata: serde_json::Value) {
        let document = st.store.find_by_uri("wiki://shared").unwrap().unwrap();
        st.store
            .update_document_meta_for_sync_test(
                &document.id,
                &crate::models::DocumentMetaUpdate {
                    metadata_json: Some(metadata.to_string()),
                    ..Default::default()
                },
                &SyncIdentity {
                    node_id: "replica".into(),
                    primary_url: Some("http://primary".into()),
                },
            )
            .unwrap()
            .unwrap();
    }

    fn metadata(st: &HttpState) -> serde_json::Value {
        serde_json::from_str(
            &st.store
                .find_by_uri("wiki://shared")
                .unwrap()
                .unwrap()
                .metadata_json,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn sync_metadata_edits_around_push_ack_and_pull_converge() {
        let root = tempfile::tempdir().unwrap();
        let primary = state(root.path(), "primary");
        let replica = state(root.path(), "replica");
        let observer = state(root.path(), "observer");
        let mut page = command("body");
        page.options.extra_metadata = Some(json!({"version":"M1"}));
        wiki::write_wiki_page_synced_command(
            &replica.store,
            &replica.embedder,
            &replica.config,
            page,
            WikiSyncWrite::Local(SyncIdentity {
                node_id: "replica".into(),
                primary_url: Some("http://primary".into()),
            }),
            false,
        )
        .await
        .unwrap();
        replica_metadata_edit(&replica, json!({"version":"M2"}));
        let outgoing = replica.store.pending_sync_outbox(10).unwrap();
        assert_eq!(outgoing.len(), 2);
        let (base, server) = primary_server(primary.clone()).await;
        let client = reqwest::Client::new();
        let response: serde_json::Value = client
            .post(format!("{base}/v1/sync/push"))
            .json(&json!({"node_id":"replica", "changes":outgoing}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["primary_sequences"], json!([1, 2]));
        // An edit while the push reply is in flight, then another after ack.
        replica_metadata_edit(&replica, json!({"version":"M3"}));
        replica
            .store
            .mark_sync_outbox_sent(&[(1, 1), (2, 2)])
            .unwrap();
        replica_metadata_edit(&replica, json!({"version":"M4"}));
        for event in primary.store.pull_sync_changes("replica", 0, 10).unwrap() {
            apply_change(
                &replica,
                WikiSyncWrite::Pull {
                    node_id: "replica",
                    event: &event,
                },
            )
            .await
            .unwrap();
            assert_eq!(metadata(&replica), json!({"version":"M4"}));
        }
        sync_once(&replica, &client, &base, "replica", "replica.local")
            .await
            .unwrap();
        sync_once(&observer, &client, &base, "observer", "observer.local")
            .await
            .unwrap();
        for node in [&primary, &replica, &observer] {
            assert_eq!(metadata(node), json!({"version":"M4"}));
        }
        // A further metadata-only edit after the previous pull must also publish.
        replica_metadata_edit(&replica, json!({"version":"M5", "source_versions":[]}));
        for _ in 0..2 {
            sync_once(&replica, &client, &base, "replica", "replica.local")
                .await
                .unwrap();
            sync_once(&observer, &client, &base, "observer", "observer.local")
                .await
                .unwrap();
        }
        for node in [&primary, &replica, &observer] {
            assert_eq!(
                metadata(node),
                json!({"version":"M5", "source_versions":[]})
            );
        }
        assert_eq!(
            primary
                .store
                .pull_sync_changes("observer", 0, 10)
                .unwrap()
                .len(),
            5
        );
        assert!(replica.store.pending_sync_outbox(10).unwrap().is_empty());
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn sync_direct_metadata_update_rolls_back_when_journal_fails() {
        let root = tempfile::tempdir().unwrap();
        let replica = state(root.path(), "replica");
        local_write(&replica, "replica", true, "body").await;
        let before = replica.store.find_by_uri("wiki://shared").unwrap().unwrap();
        let catalog = replica
            .store
            .get_wiki_index_by_slug("shared")
            .unwrap()
            .unwrap();
        let pending = replica.store.pending_sync_outbox(10).unwrap();
        replica
            .store
            .lock()
            .unwrap()
            .execute_batch("DROP TABLE sync_nodes")
            .unwrap();
        let result = replica.store.update_document_meta_for_sync_test(
            &before.id,
            &crate::models::DocumentMetaUpdate {
                title: Some("should roll back".into()),
                metadata_json: Some(json!({"version":"M2", "category":"changed"}).to_string()),
                ..Default::default()
            },
            &SyncIdentity {
                node_id: "replica".into(),
                primary_url: Some("http://primary".into()),
            },
        );
        assert!(result.is_err());
        let after = replica.store.get_document(&before.id).unwrap().unwrap();
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.title, before.title);
        assert_eq!(after.metadata_json, before.metadata_json);
        let after_catalog = replica
            .store
            .get_wiki_index_by_slug("shared")
            .unwrap()
            .unwrap();
        assert_eq!(after_catalog.title, catalog.title);
        assert_eq!(after_catalog.category, catalog.category);
        assert_eq!(replica.store.pending_sync_outbox(10).unwrap(), pending);
    }

    #[tokio::test]
    async fn sync_generic_atomic_wiki_write_journals_only_replicated_fields() {
        let root = tempfile::tempdir().unwrap();
        let primary = state(root.path(), "primary");
        local_write(&primary, "primary", false, "body").await;
        let mut doc = primary.store.find_by_uri("wiki://shared").unwrap().unwrap();
        doc.title = "Changed title".into();
        doc.wing = Some("project".into());
        doc.room = Some("architecture".into());
        doc.kind = "reference".into();
        doc.metadata_json = json!({"version":"M2", "category":"design"}).to_string();
        primary
            .store
            .write_document_atomic(
                &doc,
                Some(doc.revision),
                crate::db::store::DocumentDerivedWrite::Preserve,
            )
            .unwrap();
        let events = primary.store.pull_sync_changes("observer", 0, 10).unwrap();
        assert_eq!(events.len(), 2);
        let payload = validate_wiki_change(&events[1].change).unwrap();
        assert_eq!(payload.title, doc.title);
        assert_eq!(payload.wing, doc.wing);
        assert_eq!(payload.room, doc.room);
        assert_eq!(payload.kind.as_deref(), Some("reference"));
        assert_eq!(
            payload.metadata_json.as_deref(),
            Some(doc.metadata_json.as_str())
        );
        // Generic metadata-only lifecycle changes remain local and do not add events.
        let update = crate::models::DocumentMetaUpdate {
            status: Some("archived".into()),
            pinned: Some(true),
            boost: Some(2.0),
            source_file: Some("local/path.md".into()),
            ..Default::default()
        };
        primary
            .store
            .update_document_meta(&doc.id, &update)
            .unwrap()
            .unwrap();
        let mut local = primary.store.get_document(&doc.id).unwrap().unwrap();
        local.status = "active".into();
        local.pinned = false;
        local.boost = 3.0;
        local.source_file = None;
        primary
            .store
            .write_document_atomic(
                &local,
                Some(local.revision),
                crate::db::store::DocumentDerivedWrite::Preserve,
            )
            .unwrap();
        let raw = crate::models::Document {
            id: "raw-with-wiki-uri".into(),
            uri: "wiki://raw-with-wiki-uri".into(),
            title: "Raw source".into(),
            content: "Immutable source text".into(),
            layer: "raw".into(),
            ..Default::default()
        };
        primary
            .store
            .write_document_atomic(&raw, None, crate::db::store::DocumentDerivedWrite::Preserve)
            .unwrap();
        primary
            .store
            .update_document_meta(
                &raw.id,
                &crate::models::DocumentMetaUpdate {
                    metadata_json: Some(json!({"raw-only":true}).to_string()),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            primary.store.pull_sync_changes("observer", 0, 10).unwrap(),
            events
        );
    }
}
