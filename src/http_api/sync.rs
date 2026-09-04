//! Durable multi-machine synchronization transport.

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::db::sync::{SyncChange, SyncChangeInput, MAX_SYNC_BATCH};
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

fn identity() -> (String, String, Option<String>) {
    let primary_url = std::env::var("RAG_PRIMARY_URL")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let role = if primary_url.is_some() {
        "replica"
    } else {
        "primary"
    }
    .to_string();
    let node_id = std::env::var("RAG_NODE_ID")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| {
            if role == "primary" {
                "primary".into()
            } else {
                "local".into()
            }
        });
    (role, node_id, primary_url)
}

async fn status(State(st): State<HttpState>) -> impl IntoResponse {
    let (role, node_id, primary_url) = identity();
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

#[derive(Deserialize)]
struct WikiPayload {
    slug: String,
    title: String,
    content: String,
    #[serde(default)]
    wing: Option<String>,
    #[serde(default)]
    room: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

async fn push(State(st): State<HttpState>, Json(body): Json<PushBody>) -> impl IntoResponse {
    if body.changes.len() > MAX_SYNC_BATCH {
        return api_err(AppError::config(format!(
            "sync batch exceeds {MAX_SYNC_BATCH} changes"
        )));
    }
    for change in &body.changes {
        match st.store.has_sync_change(&body.node_id, change.origin_seq) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(error) => return api_err(error),
        }
        if let Err(error) = apply_change(&st, &body.node_id, change).await {
            return api_err(error);
        }
    }
    match st.store.accept_sync_changes(&body.node_id, &body.changes) {
        Ok(primary_sequences) => api_ok(
            json!({"ok": true, "accepted": primary_sequences.len(), "primary_sequences": primary_sequences}),
        ),
        Err(error) => api_err(error),
    }
}

async fn apply_change(
    st: &HttpState,
    origin_node: &str,
    change: &SyncChangeInput,
) -> Result<(), AppError> {
    if change.entity_kind != "wiki" || change.operation != "upsert" {
        return Err(AppError::config(
            "first sync transport version accepts only wiki upsert changes",
        ));
    }
    let payload: WikiPayload = serde_json::from_str(&change.payload_json)
        .map_err(|error| AppError::config(format!("invalid wiki sync payload: {error}")))?;
    wiki::write_wiki_page_command(
        &st.store,
        &st.embedder,
        &st.config,
        WikiWriteCommand {
            slug: payload.slug,
            title: payload.title,
            content: payload.content,
            wing: payload.wing,
            room: payload.room,
            kind: payload.kind.unwrap_or_else(|| "wiki".into()),
            category: payload.category,
            summary: payload.summary,
            agent: Some(format!("sync:{origin_node}")),
            options: WriteWikiOpts::default(),
        },
    )
    .await?;
    Ok(())
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
    let (role, node_id, configured_primary) = identity();
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
    tokio::spawn(async move {
        let base = primary_url.trim_end_matches('/').to_string();
        let client = reqwest::Client::new();
        let _ = st.store.register_sync_node(&node_id, &hostname, "replica");
        loop {
            if let Err(error) = sync_once(&st, &client, &base, &node_id, &hostname).await {
                tracing::warn!(%error, %node_id, "database sync cycle failed");
            }
            tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
        }
    });
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

    let pending = st.store.pending_sync_outbox(100)?;
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
    for item in &reply.items {
        apply_change(st, &item.origin_node, &item.change).await?;
    }
    if reply.next_cursor > cursor {
        st.store
            .acknowledge_sync_cursor(node_id, reply.next_cursor)?;
        client
            .post(format!("{base}/v1/sync/ack"))
            .json(&json!({"node_id":node_id,"cursor":reply.next_cursor}))
            .send()
            .await
            .map_err(|error| AppError::db(format!("sync ack request: {error}")))?
            .error_for_status()
            .map_err(|error| AppError::db(format!("sync ack response: {error}")))?;
    }
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
