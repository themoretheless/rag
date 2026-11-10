//! Durable multi-machine synchronization transport.

use std::sync::Arc;

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
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::wiki::{self, WikiWriteCommand, WriteWikiOpts};

use super::{
    error::{api_err, api_ok},
    HttpState,
};

/// Whole-request timeout for database sync HTTP calls to peers.
const SYNC_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/sync/status", get(status))
        .route("/v1/sync/register", post(register))
        .route("/v1/sync/push", post(push))
        .route("/v1/sync/pull", get(pull))
        .route("/v1/sync/ack", post(ack))
}

fn identity() -> Result<(String, String, Option<String>), AppError> {
    let identity = SyncIdentity::current()?;
    Ok((
        identity.role().into(),
        identity.node_id,
        identity.primary_url,
    ))
}

async fn status(State(st): State<HttpState>) -> impl IntoResponse {
    st.blocking_response("status", move |store| {
        let (role, node_id, primary_url) = match identity() {
            Ok(value) => value,
            Err(error) => return api_err(error),
        };
        match store.sync().sync_status(&role, &node_id, primary_url) {
            Ok(value) => api_ok(json!({"ok": true, "sync": value})),
            Err(error) => api_err(error),
        }
    })
    .await
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
    st.blocking_response("register", move |store| {
        match store
            .sync()
            .register_sync_node(&body.node_id, &body.hostname, &body.role)
        {
            Ok(()) => api_ok(json!({"ok": true, "node_id": body.node_id})),
            Err(error) => api_err(error),
        }
    })
    .await
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
    let (node_id, changes) = (body.node_id.clone(), body.changes.clone());
    if let Err(error) = st
        .blocking("sync preflight", move |store| {
            store.sync().preflight_sync_batch(&node_id, &changes)
        })
        .await
    {
        return api_err(error);
    }
    let embedder = batch_embedder(&st, body.changes.iter()).await;
    let mut primary_sequences = Vec::with_capacity(body.changes.len());
    for change in &body.changes {
        match apply_change_with(
            &st,
            &embedder,
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

#[cfg(test)]
async fn apply_change(st: &HttpState, mode: WikiSyncWrite<'_>) -> Result<i64, AppError> {
    apply_change_with(st, &st.embedder, mode).await
}

/// Embed the chunk texts of every page in a replication batch with one
/// provider call; the per-event writes then read those vectors. Falls back to
/// the plain embedder when the batch call fails (each write retries alone).
async fn batch_embedder<'a>(
    st: &HttpState,
    changes: impl Iterator<Item = &'a SyncChangeInput>,
) -> Arc<dyn EmbeddingProvider> {
    let indexer = crate::document_indexer::DocumentIndexer::new(st.embedder.as_ref(), &st.config);
    let texts: Vec<String> = changes
        .filter_map(|change| validate_wiki_change(change).ok())
        .flat_map(|payload| {
            indexer.plain_chunk_texts(&crate::models::Document {
                content: payload.content,
                ..Default::default()
            })
        })
        .collect();
    match crate::embeddings::PrecomputedEmbedder::prepare(st.embedder.clone(), texts).await {
        Ok(embedder) => Arc::new(embedder),
        Err(_) => st.embedder.clone(),
    }
}

async fn apply_change_with(
    st: &HttpState,
    embedder: &Arc<dyn EmbeddingProvider>,
    mode: WikiSyncWrite<'_>,
) -> Result<i64, AppError> {
    let (origin_node, change) = match &mode {
        WikiSyncWrite::Push {
            origin_node,
            change,
        } => (*origin_node, *change),
        WikiSyncWrite::Pull { event, .. } => (event.origin_node.as_str(), &event.change),
        WikiSyncWrite::Local(_) => {
            return Err(AppError::invalid(
                "transport requires incoming sync provenance",
            ))
        }
    };
    let payload = validate_wiki_change(change)?;
    for attempt in 0..3 {
        if let Some(sequence) =
            crate::util::blocking_section(|| st.store.sync().previously_applied_sync(&mode))?
        {
            return Ok(sequence);
        }
        let result = wiki::write_wiki_page_synced_command(
            &st.store,
            embedder,
            &st.config,
            WikiWriteCommand {
                slug: payload.slug.clone(),
                title: payload.title.clone(),
                content: payload.content.clone(),
                wing: payload.wing.clone(),
                room: payload.room.clone(),
                kind: payload
                    .kind
                    .clone()
                    .unwrap_or_else(|| crate::models::DocumentKind::Wiki.as_str().into()),
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
pub(super) fn spawn_worker(st: HttpState, tasks: &crate::background::BackgroundTasks) {
    let (role, node_id, configured_primary) = match identity() {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "cannot initialize database sync identity");
            return;
        }
    };
    let hostname = st
        .config
        .sync_node_name
        .clone()
        .unwrap_or_else(|| node_id.clone());
    if let Err(error) = crate::util::blocking_section(|| {
        st.store
            .sync()
            .register_sync_node(&node_id, &hostname, &role)
    }) {
        tracing::error!(%error, %node_id, "cannot initialize database sync node");
    }
    let Some(primary_url) = configured_primary else {
        return;
    };
    let interval_secs = st.config.sync_interval_secs;
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(token) = st.config.sync_primary_token.as_ref().map(|t| t.expose()) {
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
        .timeout(SYNC_HTTP_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            tracing::error!("cannot initialize database sync HTTP client");
            return;
        }
    };
    let loop_tasks = tasks.clone();
    tasks.spawn("sync-worker", async move {
        let base = primary_url.trim_end_matches('/').to_string();
        let (register_node, register_host) = (node_id.clone(), hostname.clone());
        let _ = st
            .blocking("sync register", move |store| {
                store
                    .sync()
                    .register_sync_node(&register_node, &register_host, "replica")
            })
            .await;
        loop {
            if let Err(error) = sync_once(&st, &client, &base, &node_id, &hostname).await {
                tracing::warn!(%error, %node_id, "database sync cycle failed");
            }
            if !loop_tasks
                .pause(std::time::Duration::from_secs(interval_secs))
                .await
            {
                break;
            }
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
                return Err(AppError::invalid(
                    "pending sync event exceeds the HTTP body limit; preserve the legacy outbox \
                     and repair the oversized entry before retrying",
                ));
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

    let outbox = st
        .blocking("sync outbox", |store| store.sync().pending_sync_outbox(100))
        .await?;
    let pending = bounded_sync_batch(node_id, &outbox)?;
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
        st.blocking("sync outbox sent", move |store| {
            store.sync().mark_sync_outbox_sent(&assignments)
        })
        .await?;
    }

    let cursor_node = node_id.to_string();
    let cursor = st
        .blocking("sync cursor", move |store| {
            store.sync().sync_pull_cursor(&cursor_node)
        })
        .await?;
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
        return Err(AppError::invalid(
            "sync reply cursor does not match its events",
        ));
    }
    let embedder = batch_embedder(st, reply.items.iter().map(|item| &item.change)).await;
    for item in &reply.items {
        apply_change_with(
            st,
            &embedder,
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
    st.blocking_response("pull", move |store| {
        match store
            .sync()
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
    })
    .await
}

#[derive(Deserialize)]
struct AckBody {
    node_id: String,
    cursor: i64,
}

async fn ack(State(st): State<HttpState>, Json(body): Json<AckBody>) -> impl IntoResponse {
    st.blocking_response("ack", move |store| {
        match store
            .sync()
            .acknowledge_sync_cursor(&body.node_id, body.cursor)
        {
            Ok(()) => api_ok(json!({"ok": true, "node_id": body.node_id, "cursor": body.cursor})),
            Err(error) => api_err(error),
        }
    })
    .await
}

#[cfg(test)]
mod tests;
