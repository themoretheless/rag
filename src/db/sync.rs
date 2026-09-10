//! Durable transport state for local-to-primary database synchronization.

use chrono::{SecondsFormat, Utc};
use duckdb::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::Store;
use crate::error::{AppError, Result};

pub const MAX_SYNC_BATCH: usize = 500;
/// Leave room for the push envelope/node identity under the gateway's 1 MiB limit.
pub const MAX_SYNC_EVENT_BYTES: usize = 1_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncChangeInput {
    pub origin_seq: i64,
    pub entity_kind: String,
    pub entity_id: String,
    pub operation: String,
    pub payload_json: String,
    #[serde(default)]
    pub content_hash: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncChange {
    pub primary_seq: i64,
    pub origin_node: String,
    #[serde(flatten)]
    pub change: SyncChangeInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncNode {
    pub node_id: String,
    pub hostname: String,
    pub role: String,
    pub last_seen_at: String,
    pub last_push_seq: i64,
    pub pull_cursor: i64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncStatus {
    pub role: String,
    pub node_id: String,
    pub primary_url: Option<String>,
    pub latest_primary_seq: i64,
    pub pending_outbox: i64,
    pub nodes: Vec<SyncNode>,
}

/// Stable process identity shared by local journalling and the HTTP worker.
#[derive(Debug, Clone)]
pub(crate) struct SyncIdentity {
    pub node_id: String,
    pub primary_url: Option<String>,
}

impl SyncIdentity {
    pub(crate) fn from_env() -> Result<Self> {
        let primary_url = std::env::var("RAG_PRIMARY_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let node_id = std::env::var("RAG_NODE_ID")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                std::env::var("HOSTNAME")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            })
            .unwrap_or_else(|| {
                if primary_url.is_some() {
                    "local".into()
                } else {
                    "primary".into()
                }
            });
        validate_token("node_id", &node_id)?;
        Ok(Self {
            node_id,
            primary_url,
        })
    }

    pub(crate) fn role(&self) -> &'static str {
        if self.primary_url.is_some() {
            "replica"
        } else {
            "primary"
        }
    }
}

/// Typed internal provenance; user-controlled agent strings never disable the journal.
#[derive(Debug, Clone)]
pub(crate) enum WikiSyncWrite<'a> {
    Local(SyncIdentity),
    Push {
        origin_node: &'a str,
        change: &'a SyncChangeInput,
    },
    Pull {
        node_id: &'a str,
        event: &'a SyncChange,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WikiSyncPayload {
    pub slug: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub wing: Option<String>,
    #[serde(default)]
    pub room: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    /// Complete wiki metadata snapshot; absent legacy events retain unrelated local keys.
    #[serde(default)]
    pub metadata_json: Option<String>,
}

/// Validate the complete immutable event before any page is prepared or written.
pub(crate) fn validate_wiki_change(change: &SyncChangeInput) -> Result<WikiSyncPayload> {
    validate_change(change)?;
    if change.entity_kind != "wiki" || change.operation != "upsert" {
        return Err(AppError::config("sync accepts only wiki upsert changes"));
    }
    let payload: WikiSyncPayload = serde_json::from_str(&change.payload_json)
        .map_err(|error| AppError::config(format!("invalid wiki sync payload: {error}")))?;
    if let Some(metadata) = &payload.metadata_json {
        let value: serde_json::Value = serde_json::from_str(metadata).map_err(|error| {
            AppError::config(format!("invalid wiki sync metadata JSON: {error}"))
        })?;
        if !value.is_object() {
            return Err(AppError::config("wiki sync metadata must be a JSON object"));
        }
    }
    let slug = crate::util::slugify(&payload.slug, crate::util::SlugPolicy::WikiPage);
    if slug.is_empty() || payload.slug != slug || change.entity_id != format!("wiki://{slug}") {
        return Err(AppError::config(
            "wiki sync entity_id must match the canonical payload slug",
        ));
    }
    if change
        .content_hash
        .as_deref()
        .is_some_and(|hash| hash != crate::util::content_hash(&payload.content))
    {
        return Err(AppError::config(
            "wiki sync content hash does not match payload",
        ));
    }
    Ok(payload)
}

fn canonical_change(change: &SyncChangeInput) -> Result<SyncChangeInput> {
    validate_change(change)?;
    let mut canonical = change.clone();
    canonical.created_at = chrono::DateTime::parse_from_rfc3339(&change.created_at)
        .map_err(|error| AppError::config(format!("created_at must be RFC3339: {error}")))?
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Micros, true);
    canonical.payload_json = serde_json::to_string(&serde_json::from_str::<serde_json::Value>(
        &change.payload_json,
    )?)?;
    // Normalizing a valid RFC3339 timestamp can add fractional-second bytes.
    // Enforce the transport limit on the exact representation persisted below.
    validate_change(&canonical)?;
    Ok(canonical)
}

fn stored_event_locked(
    conn: &Connection,
    origin_node: &str,
    origin_seq: i64,
) -> Result<Option<SyncChange>> {
    let mut stmt = conn.prepare(
        "SELECT primary_seq, entity_kind, entity_id, operation, payload_json, content_hash, strftime(created_at, '%Y-%m-%dT%H:%M:%S.%fZ') FROM sync_changes WHERE origin_node = ? AND origin_seq = ?"
    )?;
    let mut rows = stmt.query(params![origin_node, origin_seq])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(SyncChange {
        primary_seq: row.get(0)?,
        origin_node: origin_node.into(),
        change: SyncChangeInput {
            origin_seq,
            entity_kind: row.get(1)?,
            entity_id: row.get(2)?,
            operation: row.get(3)?,
            payload_json: row.get(4)?,
            content_hash: row.get(5)?,
            created_at: row.get(6)?,
        },
    }))
}

fn existing_change_locked(
    conn: &Connection,
    origin_node: &str,
    change: &SyncChangeInput,
) -> Result<Option<i64>> {
    let Some(existing) = stored_event_locked(conn, origin_node, change.origin_seq)? else {
        return Ok(None);
    };
    if canonical_change(&existing.change)? != canonical_change(change)? {
        return Err(AppError::conflict(
            "sync event identity was already used with different content",
        ));
    }
    Ok(Some(existing.primary_seq))
}

pub(crate) fn register_node_locked(
    conn: &Connection,
    node_id: &str,
    hostname: &str,
    role: &str,
) -> Result<()> {
    validate_token("node_id", node_id)?;
    validate_token("hostname", hostname)?;
    if !matches!(role, "primary" | "replica") {
        return Err(AppError::config("invalid sync role"));
    }
    let updated = conn.execute(
        "UPDATE sync_nodes SET hostname = ?, role = ?, last_seen_at = CURRENT_TIMESTAMP WHERE node_id = ?",
        params![hostname, role, node_id],
    )?;
    if updated == 0 {
        conn.execute("INSERT INTO sync_nodes (node_id, hostname, role, last_seen_at) VALUES (?, ?, ?, CURRENT_TIMESTAMP)", params![node_id, hostname, role])?;
    }
    Ok(())
}

fn insert_event_locked(
    conn: &Connection,
    primary_seq: i64,
    origin_node: &str,
    change: &SyncChangeInput,
) -> Result<()> {
    let change = canonical_change(change)?;
    conn.execute(
        "INSERT INTO sync_changes (primary_seq, origin_node, origin_seq, entity_kind, entity_id, operation, payload_json, content_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![primary_seq, origin_node, change.origin_seq, change.entity_kind, change.entity_id,
            change.operation, change.payload_json, change.content_hash, change.created_at],
    )?;
    Ok(())
}

fn accept_change_locked(
    conn: &Connection,
    origin_node: &str,
    change: &SyncChangeInput,
) -> Result<i64> {
    if let Some(existing) = existing_change_locked(conn, origin_node, change)? {
        return Ok(existing);
    }
    let next: i64 = conn.query_row(
        "SELECT COALESCE(MAX(primary_seq), 0) + 1 FROM sync_changes",
        [],
        |row| row.get(0),
    )?;
    insert_event_locked(conn, next, origin_node, change)?;
    conn.execute("UPDATE sync_nodes SET last_seen_at = CURRENT_TIMESTAMP, last_push_seq = GREATEST(last_push_seq, ?) WHERE node_id = ?", params![change.origin_seq, origin_node])?;
    Ok(next)
}

fn enqueue_change_locked(
    conn: &Connection,
    prototype: &SyncChangeInput,
    node_id: Option<&str>,
) -> Result<i64> {
    let next: i64 = conn.query_row(
        "SELECT GREATEST(COALESCE((SELECT MAX(local_seq) FROM sync_outbox), 0), COALESCE((SELECT MAX(origin_seq) FROM sync_changes WHERE origin_node = ?), 0)) + 1",
        params![node_id.unwrap_or("")], |row| row.get(0),
    )?;
    let mut change = prototype.clone();
    change.origin_seq = next;
    let change = canonical_change(&change)?;
    conn.execute(
        "INSERT INTO sync_outbox (local_seq, entity_kind, entity_id, operation, payload_json, content_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![next, change.entity_kind, change.entity_id, change.operation, change.payload_json, change.content_hash, change.created_at],
    )?;
    Ok(next)
}

pub(crate) fn local_change_locked(
    conn: &Connection,
    identity: &SyncIdentity,
    prototype: &SyncChangeInput,
) -> Result<i64> {
    validate_wiki_change(prototype)?;
    register_node_locked(conn, &identity.node_id, &identity.node_id, identity.role())?;
    if identity.primary_url.is_some() {
        return enqueue_change_locked(conn, prototype, Some(&identity.node_id));
    }
    // Both the origin and canonical sequence are allocated under this same transaction.
    let next: i64 = conn.query_row(
        "SELECT GREATEST(COALESCE((SELECT MAX(origin_seq) FROM sync_changes WHERE origin_node = ?), 0), COALESCE((SELECT MAX(local_seq) FROM sync_outbox), 0)) + 1",
        params![&identity.node_id], |row| row.get(0),
    )?;
    let mut change = prototype.clone();
    change.origin_seq = next;
    accept_change_locked(conn, &identity.node_id, &change)
}

fn cursor_locked(conn: &Connection, node_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(pull_cursor), 0) FROM sync_nodes WHERE node_id = ?",
        params![node_id],
        |row| row.get(0),
    )?)
}

/// Called inside the page transaction, before touching document/chunks/graph.
pub(crate) fn sync_duplicate_locked(
    conn: &Connection,
    mode: &WikiSyncWrite<'_>,
) -> Result<Option<i64>> {
    match mode {
        WikiSyncWrite::Local(identity) => {
            validate_token("node_id", &identity.node_id)?;
            Ok(None)
        }
        WikiSyncWrite::Push {
            origin_node,
            change,
        } => {
            validate_token("origin_node", origin_node)?;
            validate_wiki_change(change)?;
            existing_change_locked(conn, origin_node, change)
        }
        WikiSyncWrite::Pull { node_id, event } => {
            validate_token("node_id", node_id)?;
            validate_token("origin_node", &event.origin_node)?;
            validate_wiki_change(&event.change)?;
            if event.primary_seq < 1 {
                return Err(AppError::config("primary_seq must be >= 1"));
            }
            if let Some(existing) = existing_change_locked(conn, &event.origin_node, &event.change)?
            {
                if existing != event.primary_seq {
                    return Err(AppError::conflict("sync event canonical sequence changed"));
                }
                if cursor_locked(conn, node_id)? < existing {
                    return Err(AppError::conflict(
                        "sync journal and applied cursor disagree",
                    ));
                }
                return Ok(Some(existing));
            }
            let cursor = cursor_locked(conn, node_id)?;
            if event.primary_seq != cursor + 1 {
                return Err(AppError::conflict(
                    "sync stream must apply consecutive canonical sequences",
                ));
            }
            let occupied: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sync_changes WHERE primary_seq = ?",
                params![event.primary_seq],
                |row| row.get(0),
            )?;
            if occupied != 0 {
                return Err(AppError::conflict(
                    "canonical sync sequence already belongs to another event",
                ));
            }
            Ok(None)
        }
    }
}

/// Keep a newer offline local edit visible while its immutable outbox event is pending.
/// It is reconciled by its own canonical event after the next successful push.
pub(crate) fn defer_pulled_document_locked(
    conn: &Connection,
    mode: &WikiSyncWrite<'_>,
    uri: &str,
) -> Result<bool> {
    if !matches!(mode, WikiSyncWrite::Pull { .. }) {
        return Ok(false);
    }
    let pending: i64 = conn.query_row("SELECT COUNT(*) FROM sync_outbox WHERE entity_kind = 'wiki' AND entity_id = ? AND sent_at IS NULL", params![uri], |row| row.get(0))?;
    Ok(pending > 0)
}

pub(crate) fn commit_sync_write_locked(
    conn: &Connection,
    mode: &WikiSyncWrite<'_>,
    local: &SyncChangeInput,
) -> Result<i64> {
    match mode {
        WikiSyncWrite::Local(identity) => local_change_locked(conn, identity, local),
        WikiSyncWrite::Push {
            origin_node,
            change,
        } => accept_change_locked(conn, origin_node, change),
        WikiSyncWrite::Pull { node_id, event } => {
            insert_event_locked(conn, event.primary_seq, &event.origin_node, &event.change)?;
            register_node_locked(conn, node_id, node_id, "replica")?;
            conn.execute("UPDATE sync_nodes SET pull_cursor = ?, last_seen_at = CURRENT_TIMESTAMP WHERE node_id = ?", params![event.primary_seq, node_id])?;
            Ok(event.primary_seq)
        }
    }
}

impl Store {
    pub fn has_sync_change(&self, origin_node: &str, origin_seq: i64) -> Result<bool> {
        let conn = self.lock()?;
        Ok(stored_event_locked(&conn, origin_node, origin_seq)?.is_some())
    }

    pub(crate) fn previously_applied_sync(&self, mode: &WikiSyncWrite<'_>) -> Result<Option<i64>> {
        let conn = self.lock()?;
        sync_duplicate_locked(&conn, mode)
    }

    pub(crate) fn preflight_sync_batch(
        &self,
        origin_node: &str,
        changes: &[SyncChangeInput],
    ) -> Result<()> {
        validate_token("origin_node", origin_node)?;
        if changes.len() > MAX_SYNC_BATCH {
            return Err(AppError::config(format!(
                "sync batch exceeds {MAX_SYNC_BATCH} changes"
            )));
        }
        let mut identities = HashMap::new();
        let conn = self.lock()?;
        for change in changes {
            validate_wiki_change(change)?;
            let canonical = canonical_change(change)?;
            if identities
                .insert(change.origin_seq, canonical.clone())
                .is_some_and(|previous| previous != canonical)
            {
                return Err(AppError::conflict(
                    "sync batch reuses event identity with different content",
                ));
            }
            existing_change_locked(&conn, origin_node, change)?;
        }
        Ok(())
    }

    /// Low-level journal append. Application wiki writes use the atomic page+journal seam.
    pub fn journal_local_sync_change(
        &self,
        entity_kind: &str,
        entity_id: &str,
        operation: &str,
        payload_json: &str,
        content_hash: Option<&str>,
    ) -> Result<i64> {
        let prototype = new_change(
            entity_kind,
            entity_id,
            operation,
            payload_json,
            content_hash,
        );
        validate_wiki_change(&prototype)?;
        let identity = SyncIdentity::from_env()?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let assigned = local_change_locked(&tx, &identity, &prototype)?;
        tx.commit()?;
        Ok(assigned)
    }

    pub fn enqueue_sync_change(
        &self,
        entity_kind: &str,
        entity_id: &str,
        operation: &str,
        payload_json: &str,
        content_hash: Option<&str>,
    ) -> Result<i64> {
        let prototype = new_change(
            entity_kind,
            entity_id,
            operation,
            payload_json,
            content_hash,
        );
        validate_wiki_change(&prototype)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let next = enqueue_change_locked(&tx, &prototype, None)?;
        tx.commit()?;
        Ok(next)
    }

    pub fn pending_sync_outbox(&self, limit: usize) -> Result<Vec<SyncChangeInput>> {
        let limit = limit.clamp(1, MAX_SYNC_BATCH);
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT local_seq, entity_kind, entity_id, operation, payload_json, content_hash, strftime(created_at, '%Y-%m-%dT%H:%M:%S.%fZ') FROM sync_outbox WHERE sent_at IS NULL ORDER BY local_seq LIMIT {limit}"
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok(SyncChangeInput {
                origin_seq: row.get(0)?,
                entity_kind: row.get(1)?,
                entity_id: row.get(2)?,
                operation: row.get(3)?,
                payload_json: row.get(4)?,
                content_hash: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn mark_sync_outbox_sent(&self, assignments: &[(i64, i64)]) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        for (local_seq, primary_seq) in assignments {
            if *local_seq < 1 || *primary_seq < 1 {
                return Err(AppError::config("sync assignments must be positive"));
            }
            let existing: Option<i64> = tx.query_row(
                "SELECT MAX(primary_seq) FROM sync_outbox WHERE local_seq = ?",
                params![local_seq],
                |row| row.get(0),
            )?;
            if existing.is_some_and(|existing| existing != *primary_seq) {
                return Err(AppError::conflict("outbox canonical sequence changed"));
            }
            let updated = tx.execute("UPDATE sync_outbox SET sent_at = CURRENT_TIMESTAMP, primary_seq = ? WHERE local_seq = ?", params![primary_seq, local_seq])?;
            if updated == 0 {
                return Err(AppError::conflict(
                    "outbox acknowledgement has no local event",
                ));
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn register_sync_node(&self, node_id: &str, hostname: &str, role: &str) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        register_node_locked(&tx, node_id, hostname, role)?;
        tx.commit()?;
        Ok(())
    }

    /// Low-level journal import; HTTP uses the atomic page+journal apply path.
    pub fn accept_sync_changes(
        &self,
        origin_node: &str,
        changes: &[SyncChangeInput],
    ) -> Result<Vec<i64>> {
        self.preflight_sync_batch(origin_node, changes)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let mut assigned = Vec::with_capacity(changes.len());
        for change in changes {
            assigned.push(accept_change_locked(&tx, origin_node, change)?);
        }
        tx.commit()?;
        Ok(assigned)
    }

    /// The canonical stream includes a node's own events, so every replica replays
    /// the same ordering and converges after conflicting offline edits.
    pub fn pull_sync_changes(
        &self,
        node_id: &str,
        after: i64,
        limit: usize,
    ) -> Result<Vec<SyncChange>> {
        validate_token("node_id", node_id)?;
        if after < 0 {
            return Err(AppError::config("sync cursor must be >= 0"));
        }
        let limit = limit.clamp(1, MAX_SYNC_BATCH);
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT primary_seq, origin_node, origin_seq, entity_kind, entity_id, operation, payload_json, content_hash, strftime(created_at, '%Y-%m-%dT%H:%M:%S.%fZ') FROM sync_changes WHERE primary_seq > ? ORDER BY primary_seq LIMIT {limit}"
        ))?;
        let rows = stmt.query_map(params![after], |row| {
            Ok(SyncChange {
                primary_seq: row.get(0)?,
                origin_node: row.get(1)?,
                change: SyncChangeInput {
                    origin_seq: row.get(2)?,
                    entity_kind: row.get(3)?,
                    entity_id: row.get(4)?,
                    operation: row.get(5)?,
                    payload_json: row.get(6)?,
                    content_hash: row.get(7)?,
                    created_at: row.get(8)?,
                },
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Primary-side delivery acknowledgement. Replica cursors advance only in
    /// the transaction that records and applies (or defers) the canonical event.
    pub fn acknowledge_sync_cursor(&self, node_id: &str, cursor: i64) -> Result<()> {
        validate_token("node_id", node_id)?;
        if cursor < 0 {
            return Err(AppError::config("sync cursor must be >= 0"));
        }
        let conn = self.lock()?;
        let latest: i64 = conn.query_row(
            "SELECT COALESCE(MAX(primary_seq), 0) FROM sync_changes",
            [],
            |row| row.get(0),
        )?;
        if cursor > latest {
            return Err(AppError::config("sync cursor exceeds canonical journal"));
        }
        let updated = conn.execute("UPDATE sync_nodes SET last_seen_at = CURRENT_TIMESTAMP, pull_cursor = GREATEST(pull_cursor, ?) WHERE node_id = ?", params![cursor, node_id])?;
        if updated == 0 {
            return Err(AppError::not_found("sync node is not registered"));
        }
        Ok(())
    }
    pub fn sync_status(
        &self,
        role: &str,
        node_id: &str,
        primary_url: Option<String>,
    ) -> Result<SyncStatus> {
        let conn = self.lock()?;
        let latest_primary_seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(primary_seq), 0) FROM sync_changes",
            [],
            |row| row.get(0),
        )?;
        let mut stmt = conn.prepare(
            "SELECT node_id, hostname, role, CAST(last_seen_at AS VARCHAR), last_push_seq, pull_cursor, last_error FROM sync_nodes ORDER BY role, hostname",
        )?;
        let nodes = stmt
            .query_map([], |row| {
                Ok(SyncNode {
                    node_id: row.get(0)?,
                    hostname: row.get(1)?,
                    role: row.get(2)?,
                    last_seen_at: row.get(3)?,
                    last_push_seq: row.get(4)?,
                    pull_cursor: row.get(5)?,
                    last_error: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let pending_outbox: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_outbox WHERE sent_at IS NULL",
            [],
            |row| row.get(0),
        )?;
        Ok(SyncStatus {
            role: role.into(),
            node_id: node_id.into(),
            primary_url,
            latest_primary_seq,
            pending_outbox,
            nodes,
        })
    }
}

fn new_change(
    entity_kind: &str,
    entity_id: &str,
    operation: &str,
    payload_json: &str,
    content_hash: Option<&str>,
) -> SyncChangeInput {
    SyncChangeInput {
        origin_seq: 1,
        entity_kind: entity_kind.into(),
        entity_id: entity_id.into(),
        operation: operation.into(),
        payload_json: payload_json.into(),
        content_hash: content_hash.map(str::to_string),
        created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
    }
}

fn validate_token(name: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value != value.trim()
        || value.len() > 255
        || value.chars().any(char::is_control)
    {
        return Err(AppError::config(format!(
            "{name} must be 1..255 printable characters without surrounding whitespace"
        )));
    }
    Ok(())
}

fn validate_change(change: &SyncChangeInput) -> Result<()> {
    if serde_json::to_vec(change)?.len() > MAX_SYNC_EVENT_BYTES {
        return Err(AppError::config(format!(
            "sync event exceeds {MAX_SYNC_EVENT_BYTES} bytes; shorten wiki content or metadata"
        )));
    }
    if change.origin_seq < 1 {
        return Err(AppError::config("origin_seq must be >= 1"));
    }
    validate_token("entity_kind", &change.entity_kind)?;
    validate_token("entity_id", &change.entity_id)?;
    if change.entity_kind != "wiki" || change.operation != "upsert" {
        return Err(AppError::config("sync accepts only wiki upsert changes"));
    }
    serde_json::from_str::<serde_json::Value>(&change.payload_json)
        .map_err(|error| AppError::config(format!("payload_json must be valid JSON: {error}")))?;
    chrono::DateTime::parse_from_rfc3339(&change.created_at)
        .map_err(|error| AppError::config(format!("created_at must be RFC3339: {error}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(slug: &str, body: &str) -> SyncChangeInput {
        new_change(
            "wiki",
            &format!("wiki://{slug}"),
            "upsert",
            &serde_json::json!({"slug":slug,"title":slug,"content":body}).to_string(),
            None,
        )
    }

    #[test]
    fn sync_outbox_timestamps_roundtrip_and_origin_events_are_included() {
        let root = tempfile::tempdir().unwrap();
        let replica = Store::open(&root.path().join("replica.duckdb")).unwrap();
        let primary = Store::open(&root.path().join("primary.duckdb")).unwrap();
        let event = change("one", "offline body");
        replica
            .enqueue_sync_change(
                &event.entity_kind,
                &event.entity_id,
                &event.operation,
                &event.payload_json,
                None,
            )
            .unwrap();
        let pending = replica.pending_sync_outbox(10).unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(&pending[0].created_at).is_ok());
        let assigned = primary.accept_sync_changes("replica", &pending).unwrap();
        assert_eq!(assigned, vec![1]);
        assert_eq!(
            primary.accept_sync_changes("replica", &pending).unwrap(),
            assigned
        );
        let pulled = primary.pull_sync_changes("replica", 0, 10).unwrap();
        assert_eq!(pulled.len(), 1);
        assert_eq!(
            canonical_change(&pulled[0].change).unwrap(),
            canonical_change(&pending[0]).unwrap()
        );
        replica.mark_sync_outbox_sent(&[(1, 1)]).unwrap();
        replica.mark_sync_outbox_sent(&[(1, 1)]).unwrap();
        assert!(replica.mark_sync_outbox_sent(&[(1, 2)]).is_err());
        assert!(replica.pending_sync_outbox(10).unwrap().is_empty());
    }

    #[test]
    fn sync_idempotency_rejects_changed_payload_and_checks_the_entire_batch() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("idempotency.duckdb")).unwrap();
        let original = change("one", "original");
        store
            .accept_sync_changes("replica", std::slice::from_ref(&original))
            .unwrap();
        let mut changed = original.clone();
        changed.payload_json =
            serde_json::json!({"slug":"one","title":"one","content":"replacement"}).to_string();
        assert!(matches!(
            store.accept_sync_changes("replica", &[changed]),
            Err(AppError::Conflict(_))
        ));
        let mut next = change("two", "next");
        next.origin_seq = 2;
        let mut invalid = change("three", "invalid");
        invalid.origin_seq = 3;
        invalid.created_at = "2026-09-05 12:00:00".into();
        assert!(store
            .accept_sync_changes("replica", &[next, invalid])
            .is_err());
        assert_eq!(store.pull_sync_changes("replica", 0, 10).unwrap().len(), 1);
    }

    #[test]
    fn sync_event_size_limit_includes_the_canonical_timestamp() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("timestamp-boundary.duckdb")).unwrap();
        let mut event = change("boundary", "");
        event.created_at = "2026-09-05T12:00:00Z".into();
        let padding = MAX_SYNC_EVENT_BYTES - serde_json::to_vec(&event).unwrap().len();
        event.payload_json = serde_json::json!({
            "slug":"boundary", "title":"boundary", "content":"x".repeat(padding)
        })
        .to_string();
        assert_eq!(
            serde_json::to_vec(&event).unwrap().len(),
            MAX_SYNC_EVENT_BYTES
        );
        let error = store
            .accept_sync_changes("replica", &[event.clone()])
            .unwrap_err();
        assert!(error.to_string().contains("sync event exceeds"));
        assert!(store
            .pull_sync_changes("replica", 0, 10)
            .unwrap()
            .is_empty());

        // Canonical UTC microseconds add exactly seven bytes to this timestamp.
        event.payload_json = serde_json::json!({
            "slug":"boundary", "title":"boundary", "content":"x".repeat(padding - 7)
        })
        .to_string();
        store.accept_sync_changes("replica", &[event]).unwrap();
        let stored = store.pull_sync_changes("replica", 0, 10).unwrap().remove(0);
        assert_eq!(stored.change.created_at, "2026-09-05T12:00:00.000000Z");
        assert_eq!(
            serde_json::to_vec(&stored.change).unwrap().len(),
            MAX_SYNC_EVENT_BYTES
        );
        validate_wiki_change(&stored.change).unwrap();
    }

    #[test]
    fn concurrent_local_primary_journal_sequences_are_allocated_in_one_transaction() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("concurrent.duckdb")).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(12));
        let tasks = (0..12)
            .map(|i| {
                let store = store.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let event = change(&format!("page-{i}"), "body");
                    barrier.wait();
                    let mut conn = store.lock().unwrap();
                    let tx = conn.transaction().unwrap();
                    let assigned = local_change_locked(
                        &tx,
                        &SyncIdentity {
                            node_id: "primary".into(),
                            primary_url: None,
                        },
                        &event,
                    )
                    .unwrap();
                    tx.commit().unwrap();
                    assigned
                })
            })
            .collect::<Vec<_>>();
        let mut assigned = tasks
            .into_iter()
            .map(|task| task.join().unwrap())
            .collect::<Vec<_>>();
        assigned.sort_unstable();
        assert_eq!(assigned, (1..=12).collect::<Vec<_>>());
        let events = store.pull_sync_changes("primary", 0, 20).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.change.origin_seq)
                .collect::<Vec<_>>(),
            (1..=12).collect::<Vec<_>>()
        );
    }
}
