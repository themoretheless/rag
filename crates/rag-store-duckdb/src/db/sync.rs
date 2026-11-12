//! Durable transport state for local-to-primary database synchronization over `lin`.

use std::collections::HashMap;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use super::Store;
use super::lin_engine::{
    LinReader, LinWriter, RecBuilder, cell_i64, cell_i64_opt, cell_nonempty_string,
    cell_opt_string, cell_str, cell_string,
};
use crate::error::{AppError, Result};
use crate::util::{format_db_timestamp, parse_flexible_timestamp};

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
pub struct SyncIdentity {
    pub node_id: String,
    pub primary_url: Option<String>,
}

/// `(node_id, primary_url)` from `Config`, set once at start-up.
static CONFIGURED_IDENTITY: std::sync::OnceLock<(Option<String>, Option<String>)> =
    std::sync::OnceLock::new();

/// Record this process's sync identity from `Config::{sync_node_id, sync_primary_url}`.
/// The first value wins.
pub fn configure_identity(node_id: Option<String>, primary_url: Option<String>) {
    let _ = CONFIGURED_IDENTITY.set((node_id, primary_url));
}

impl SyncIdentity {
    pub fn current() -> Result<Self> {
        let (configured_node, configured_primary) =
            CONFIGURED_IDENTITY.get_or_init(|| (None, None)).clone();
        let primary_url = configured_primary
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let node_id = configured_node
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

    pub fn role(&self) -> &'static str {
        if self.primary_url.is_some() {
            "replica"
        } else {
            "primary"
        }
    }
}

/// Typed internal provenance; user-controlled agent strings never disable the journal.
#[derive(Debug, Clone)]
pub enum WikiSyncWrite<'a> {
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
pub struct WikiSyncPayload {
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
pub fn validate_wiki_change(change: &SyncChangeInput) -> Result<WikiSyncPayload> {
    validate_change(change)?;
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
    canonical.created_at = DateTime::parse_from_rfc3339(&change.created_at)
        .map_err(|error| AppError::config(format!("created_at must be RFC3339: {error}")))?
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Micros, true);
    canonical.payload_json = serde_json::to_string(&serde_json::from_str::<serde_json::Value>(
        &change.payload_json,
    )?)?;
    validate_change(&canonical)?;
    Ok(canonical)
}

fn format_rfc3339_micros(raw: &str) -> String {
    parse_flexible_timestamp(raw)
        .map(|ts| ts.to_rfc3339_opts(SecondsFormat::Micros, true))
        .unwrap_or_else(|| raw.to_string())
}

fn row_to_sync_change(row: &lin::Row) -> SyncChange {
    SyncChange {
        primary_seq: cell_i64(row, "primary_seq", 0),
        origin_node: cell_string(row, "origin_node"),
        change: SyncChangeInput {
            origin_seq: cell_i64(row, "origin_seq", 0),
            entity_kind: cell_string(row, "entity_kind"),
            entity_id: cell_string(row, "entity_id"),
            operation: cell_string(row, "operation"),
            payload_json: cell_string(row, "payload_json"),
            content_hash: cell_opt_string(row, "content_hash"),
            created_at: format_rfc3339_micros(&cell_string(row, "created_at")),
        },
    }
}

fn stored_event_locked(
    r: &impl LinReader,
    origin_node: &str,
    origin_seq: i64,
) -> Result<Option<SyncChange>> {
    let pred = lin::query::pred::and(
        lin::query::pred::eq("origin_node", origin_node),
        lin::query::pred::eq("origin_seq", origin_seq),
    );
    let matches = r.query_filter("sync_changes", Some(pred), Some(1))?;
    if let Some(row) = matches.first() {
        return Ok(Some(row_to_sync_change(row)));
    }
    Ok(None)
}

fn existing_change_locked(
    r: &impl LinReader,
    origin_node: &str,
    change: &SyncChangeInput,
) -> Result<Option<i64>> {
    let Some(existing) = stored_event_locked(r, origin_node, change.origin_seq)? else {
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
    w: &mut impl LinWriter,
    node_id: &str,
    hostname: &str,
    role: &str,
) -> Result<()> {
    validate_token("node_id", node_id)?;
    validate_token("hostname", hostname)?;
    if !matches!(role, "primary" | "replica") {
        return Err(AppError::config("invalid sync role"));
    }
    let now_s = format_db_timestamp(Utc::now());
    if let Some(existing) = w.get_by_id("sync_nodes", node_id)? {
        let last_push_seq = cell_i64(&existing, "last_push_seq", 0);
        let pull_cursor = cell_i64(&existing, "pull_cursor", 0);
        let last_error = cell_opt_string(&existing, "last_error");
        let rec = RecBuilder::new()
            .id(node_id)
            .str("node_id", node_id)
            .str("hostname", hostname)
            .str("role", role)
            .str("last_seen_at", &now_s)
            .i64("last_push_seq", last_push_seq)
            .i64("pull_cursor", pull_cursor)
            .opt_string("last_error", last_error)
            .build();
        w.upsert("sync_nodes", rec)?;
    } else {
        let rec = RecBuilder::new()
            .id(node_id)
            .str("node_id", node_id)
            .str("hostname", hostname)
            .str("role", role)
            .str("last_seen_at", &now_s)
            .i64("last_push_seq", 0)
            .i64("pull_cursor", 0)
            .opt_str("last_error", None)
            .build();
        w.upsert("sync_nodes", rec)?;
    }
    Ok(())
}

fn max_primary_seq_locked(r: &impl LinReader) -> Result<i64> {
    let mut max_seq = 0i64;
    r.scan_each("sync_changes", |row| {
        max_seq = max_seq.max(cell_i64(row, "primary_seq", 0));
        Ok(true)
    })?;
    Ok(max_seq)
}

fn max_outbox_local_seq_locked(r: &impl LinReader) -> Result<i64> {
    let mut max_seq = 0i64;
    r.scan_each("sync_outbox", |row| {
        max_seq = max_seq.max(cell_i64(row, "local_seq", 0));
        Ok(true)
    })?;
    Ok(max_seq)
}

fn max_origin_seq_for_node_locked(r: &impl LinReader, origin_node: &str) -> Result<i64> {
    if origin_node.is_empty() {
        return Ok(0);
    }
    let mut max_seq = 0i64;
    r.scan_each("sync_changes", |row| {
        if cell_str(row, "origin_node") == Some(origin_node) {
            max_seq = max_seq.max(cell_i64(row, "origin_seq", 0));
        }
        Ok(true)
    })?;
    Ok(max_seq)
}

fn insert_event_locked(
    w: &mut impl LinWriter,
    primary_seq: i64,
    origin_node: &str,
    change: &SyncChangeInput,
) -> Result<()> {
    let change = canonical_change(change)?;
    let id = primary_seq.to_string();
    let rec = RecBuilder::new()
        .id(&id)
        .i64("primary_seq", primary_seq)
        .str("origin_node", origin_node)
        .i64("origin_seq", change.origin_seq)
        .str("entity_kind", &change.entity_kind)
        .str("entity_id", &change.entity_id)
        .str("operation", &change.operation)
        .str("payload_json", &change.payload_json)
        .opt_string("content_hash", change.content_hash)
        .str("created_at", &change.created_at)
        .build();
    w.upsert("sync_changes", rec)
}

fn accept_change_locked(
    w: &mut impl LinWriter,
    origin_node: &str,
    change: &SyncChangeInput,
) -> Result<i64> {
    if let Some(existing) = existing_change_locked(w, origin_node, change)? {
        return Ok(existing);
    }
    let next = max_primary_seq_locked(w)? + 1;
    insert_event_locked(w, next, origin_node, change)?;
    if let Some(node_row) = w.get_by_id("sync_nodes", origin_node)? {
        let prev_push = cell_i64(&node_row, "last_push_seq", 0);
        let now_s = format_db_timestamp(Utc::now());
        w.update_where(
            "sync_nodes",
            lin::query::pred::eq("id", origin_node),
            RecBuilder::new()
                .str("last_seen_at", &now_s)
                .i64("last_push_seq", prev_push.max(change.origin_seq))
                .build(),
        )?;
    }
    Ok(next)
}

fn enqueue_change_locked(
    w: &mut impl LinWriter,
    prototype: &SyncChangeInput,
    node_id: Option<&str>,
) -> Result<i64> {
    let max_local = max_outbox_local_seq_locked(w)?;
    let max_origin = max_origin_seq_for_node_locked(w, node_id.unwrap_or(""))?;
    let next = max_local.max(max_origin) + 1;
    let mut change = prototype.clone();
    change.origin_seq = next;
    let change = canonical_change(&change)?;
    let id = next.to_string();
    let rec = RecBuilder::new()
        .id(&id)
        .i64("local_seq", next)
        .str("entity_kind", &change.entity_kind)
        .str("entity_id", &change.entity_id)
        .str("operation", &change.operation)
        .str("payload_json", &change.payload_json)
        .opt_string("content_hash", change.content_hash)
        .str("created_at", &change.created_at)
        .opt_str("sent_at", None)
        .opt_i64("primary_seq", None)
        .build();
    w.upsert("sync_outbox", rec)?;
    Ok(next)
}

pub(crate) fn local_change_locked(
    w: &mut impl LinWriter,
    identity: &SyncIdentity,
    prototype: &SyncChangeInput,
) -> Result<i64> {
    validate_wiki_change(prototype)?;
    register_node_locked(w, &identity.node_id, &identity.node_id, identity.role())?;
    if identity.primary_url.is_some() {
        return enqueue_change_locked(w, prototype, Some(&identity.node_id));
    }
    let max_origin = max_origin_seq_for_node_locked(w, &identity.node_id)?;
    let max_local = max_outbox_local_seq_locked(w)?;
    let next = max_origin.max(max_local) + 1;
    let mut change = prototype.clone();
    change.origin_seq = next;
    accept_change_locked(w, &identity.node_id, &change)
}

fn cursor_locked(r: &impl LinReader, node_id: &str) -> Result<i64> {
    if let Some(row) = r.get_by_id("sync_nodes", node_id)? {
        return Ok(cell_i64(&row, "pull_cursor", 0));
    }
    let mut max_cursor = 0i64;
    r.scan_each("sync_nodes", |row| {
        if cell_str(row, "node_id") == Some(node_id) {
            max_cursor = max_cursor.max(cell_i64(row, "pull_cursor", 0));
        }
        Ok(true)
    })?;
    Ok(max_cursor)
}

/// Called inside the page transaction, before touching document/chunks/graph.
pub(crate) fn sync_duplicate_locked(
    r: &impl LinReader,
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
            existing_change_locked(r, origin_node, change)
        }
        WikiSyncWrite::Pull { node_id, event } => {
            validate_token("node_id", node_id)?;
            validate_token("origin_node", &event.origin_node)?;
            validate_wiki_change(&event.change)?;
            if event.primary_seq < 1 {
                return Err(AppError::config("primary_seq must be >= 1"));
            }
            if let Some(existing) = existing_change_locked(r, &event.origin_node, &event.change)? {
                if existing != event.primary_seq {
                    return Err(AppError::conflict("sync event canonical sequence changed"));
                }
                if cursor_locked(r, node_id)? < existing {
                    return Err(AppError::conflict(
                        "sync journal and applied cursor disagree",
                    ));
                }
                return Ok(Some(existing));
            }
            let cursor = cursor_locked(r, node_id)?;
            if event.primary_seq != cursor + 1 {
                return Err(AppError::conflict(
                    "sync stream must apply consecutive canonical sequences",
                ));
            }
            let mut occupied = false;
            let seq_id = event.primary_seq.to_string();
            if r.get_by_id("sync_changes", &seq_id)?.is_some() {
                occupied = true;
            } else {
                r.scan_each("sync_changes", |row| {
                    if cell_i64(row, "primary_seq", 0) == event.primary_seq {
                        occupied = true;
                        return Ok(false);
                    }
                    Ok(true)
                })?;
            }
            if occupied {
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
    r: &impl LinReader,
    mode: &WikiSyncWrite<'_>,
    uri: &str,
) -> Result<bool> {
    if !matches!(mode, WikiSyncWrite::Pull { .. }) {
        return Ok(false);
    }
    let mut pending = false;
    r.scan_each("sync_outbox", |row| {
        if cell_str(row, "entity_kind") == Some("wiki")
            && cell_str(row, "entity_id") == Some(uri)
            && cell_nonempty_string(row, "sent_at").is_none()
        {
            pending = true;
            return Ok(false);
        }
        Ok(true)
    })?;
    Ok(pending)
}

pub(crate) fn commit_sync_write_locked(
    w: &mut impl LinWriter,
    mode: &WikiSyncWrite<'_>,
    local: &SyncChangeInput,
) -> Result<i64> {
    match mode {
        WikiSyncWrite::Local(identity) => local_change_locked(w, identity, local),
        WikiSyncWrite::Push {
            origin_node,
            change,
        } => accept_change_locked(w, origin_node, change),
        WikiSyncWrite::Pull { node_id, event } => {
            insert_event_locked(w, event.primary_seq, &event.origin_node, &event.change)?;
            register_node_locked(w, node_id, node_id, "replica")?;
            let now_s = format_db_timestamp(Utc::now());
            w.update_where(
                "sync_nodes",
                lin::query::pred::eq("id", *node_id),
                RecBuilder::new()
                    .i64("pull_cursor", event.primary_seq)
                    .str("last_seen_at", &now_s)
                    .build(),
            )?;
            Ok(event.primary_seq)
        }
    }
}

impl Store {
    /// True when a sync change from this origin node and sequence is already stored.
    pub fn has_sync_change(&self, origin_node: &str, origin_seq: i64) -> Result<bool> {
        let r = self.read()?;
        Ok(stored_event_locked(&r, origin_node, origin_seq)?.is_some())
    }

    pub fn previously_applied_sync(&self, mode: &WikiSyncWrite<'_>) -> Result<Option<i64>> {
        let r = self.read()?;
        sync_duplicate_locked(&r, mode)
    }

    pub fn preflight_sync_batch(
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
        let r = self.read()?;
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
            existing_change_locked(&r, origin_node, change)?;
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
        let identity = SyncIdentity::current()?;
        self.with_tx(|tx| local_change_locked(tx, &identity, &prototype))
    }

    /// Append a local change to the sync outbox and return its local sequence.
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
        self.with_tx(|tx| enqueue_change_locked(tx, &prototype, None))
    }

    /// Unsent sync outbox changes in local order, up to `limit`.
    pub fn pending_sync_outbox(&self, limit: usize) -> Result<Vec<SyncChangeInput>> {
        let limit = limit.clamp(1, MAX_SYNC_BATCH);
        let r = self.read()?;
        let mut pending: Vec<SyncChangeInput> = Vec::new();
        r.scan_each("sync_outbox", |row| {
            if cell_nonempty_string(row, "sent_at").is_none() {
                pending.push(SyncChangeInput {
                    origin_seq: cell_i64(row, "local_seq", 0),
                    entity_kind: cell_string(row, "entity_kind"),
                    entity_id: cell_string(row, "entity_id"),
                    operation: cell_string(row, "operation"),
                    payload_json: cell_string(row, "payload_json"),
                    content_hash: cell_opt_string(row, "content_hash"),
                    created_at: format_rfc3339_micros(&cell_string(row, "created_at")),
                });
            }
            Ok(true)
        })?;
        pending.sort_by_key(|c| c.origin_seq);
        pending.truncate(limit);
        Ok(pending)
    }

    /// Record primary sequence numbers for sent outbox entries.
    pub fn mark_sync_outbox_sent(&self, assignments: &[(i64, i64)]) -> Result<()> {
        self.with_tx(|tx| {
            let now_s = format_db_timestamp(Utc::now());
            for &(local_seq, primary_seq) in assignments {
                if local_seq < 1 || primary_seq < 1 {
                    return Err(AppError::config("sync assignments must be positive"));
                }
                let mut existing_primary: Option<i64> = None;
                let mut found = false;
                tx.scan_each("sync_outbox", |row| {
                    if cell_i64(row, "local_seq", 0) == local_seq {
                        found = true;
                        if let Some(ps) = cell_i64_opt(row, "primary_seq") {
                            existing_primary =
                                Some(existing_primary.map_or(ps, |prev| prev.max(ps)));
                        }
                    }
                    Ok(true)
                })?;
                if existing_primary.is_some_and(|existing| existing != primary_seq) {
                    return Err(AppError::conflict("outbox canonical sequence changed"));
                }
                if !found {
                    return Err(AppError::conflict(
                        "outbox acknowledgement has no local event",
                    ));
                }
                let updated = tx.update_where(
                    "sync_outbox",
                    lin::query::pred::eq("local_seq", local_seq),
                    RecBuilder::new()
                        .str("sent_at", &now_s)
                        .i64("primary_seq", primary_seq)
                        .build(),
                )?;
                if updated == 0 {
                    return Err(AppError::conflict(
                        "outbox acknowledgement has no local event",
                    ));
                }
            }
            Ok(())
        })
    }

    /// Register or refresh a sync node's hostname and role.
    pub fn register_sync_node(&self, node_id: &str, hostname: &str, role: &str) -> Result<()> {
        self.with_tx(|tx| register_node_locked(tx, node_id, hostname, role))
    }

    /// Low-level journal import; HTTP uses the atomic page+journal apply path.
    pub fn accept_sync_changes(
        &self,
        origin_node: &str,
        changes: &[SyncChangeInput],
    ) -> Result<Vec<i64>> {
        self.preflight_sync_batch(origin_node, changes)?;
        self.with_tx(|tx| {
            let mut assigned = Vec::with_capacity(changes.len());
            for change in changes {
                assigned.push(accept_change_locked(tx, origin_node, change)?);
            }
            Ok(assigned)
        })
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
        let r = self.read()?;
        let mut out: Vec<SyncChange> = Vec::new();
        r.scan_each("sync_changes", |row| {
            if cell_i64(row, "primary_seq", 0) > after {
                out.push(row_to_sync_change(row));
            }
            Ok(true)
        })?;
        out.sort_by_key(|c| c.primary_seq);
        out.truncate(limit);
        Ok(out)
    }

    /// Primary-side delivery acknowledgement. Replica cursors advance only in
    /// the transaction that records and applies (or defers) the canonical event.
    pub fn acknowledge_sync_cursor(&self, node_id: &str, cursor: i64) -> Result<()> {
        validate_token("node_id", node_id)?;
        if cursor < 0 {
            return Err(AppError::config("sync cursor must be >= 0"));
        }
        let mut w = self.write()?;
        let latest = max_primary_seq_locked(&w)?;
        if cursor > latest {
            return Err(AppError::config("sync cursor exceeds canonical journal"));
        }
        let Some(existing) = w.get_by_id("sync_nodes", node_id)? else {
            return Err(AppError::not_found("sync node is not registered"));
        };
        let prev_cursor = cell_i64(&existing, "pull_cursor", 0);
        let now_s = format_db_timestamp(Utc::now());
        w.update_where(
            "sync_nodes",
            lin::query::pred::eq("id", node_id),
            RecBuilder::new()
                .str("last_seen_at", &now_s)
                .i64("pull_cursor", prev_cursor.max(cursor))
                .build(),
        )?;
        Ok(())
    }

    /// This node's pull cursor: what a replica's sync cycle needs, without
    /// the counts and node listing of [`Store::sync_status`].
    pub fn sync_pull_cursor(&self, node_id: &str) -> Result<i64> {
        let r = self.read()?;
        cursor_locked(&r, node_id)
    }

    /// Summarize sync journal, outbox and cursor state for this node.
    pub fn sync_status(
        &self,
        role: &str,
        node_id: &str,
        primary_url: Option<String>,
    ) -> Result<SyncStatus> {
        let r = self.read()?;
        let latest_primary_seq = max_primary_seq_locked(&r)?;
        let mut nodes: Vec<SyncNode> = r.filter_map_rows("sync_nodes", |row| {
            Ok(Some(SyncNode {
                node_id: cell_string(row, "node_id"),
                hostname: cell_string(row, "hostname"),
                role: cell_string(row, "role"),
                last_seen_at: cell_string(row, "last_seen_at"),
                last_push_seq: cell_i64(row, "last_push_seq", 0),
                pull_cursor: cell_i64(row, "pull_cursor", 0),
                last_error: cell_opt_string(row, "last_error"),
            }))
        })?;
        nodes.sort_by(|a, b| {
            a.role
                .cmp(&b.role)
                .then_with(|| a.hostname.cmp(&b.hostname))
        });

        let mut pending_outbox = 0i64;
        r.scan_each("sync_outbox", |row| {
            if cell_nonempty_string(row, "sent_at").is_none() {
                pending_outbox += 1;
            }
            Ok(true)
        })?;

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
    DateTime::parse_from_rfc3339(&change.created_at)
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
        let replica = Store::open(&root.path().join("replica.lin")).unwrap();
        let primary = Store::open(&root.path().join("primary.lin")).unwrap();
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
        assert!(DateTime::parse_from_rfc3339(&pending[0].created_at).is_ok());
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
        let store = Store::open(&root.path().join("idempotency.lin")).unwrap();
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
        assert!(
            store
                .accept_sync_changes("replica", &[next, invalid])
                .is_err()
        );
        assert_eq!(store.pull_sync_changes("replica", 0, 10).unwrap().len(), 1);
    }

    #[test]
    fn sync_event_size_limit_includes_the_canonical_timestamp() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("timestamp-boundary.lin")).unwrap();
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
        assert!(
            store
                .pull_sync_changes("replica", 0, 10)
                .unwrap()
                .is_empty()
        );

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
        let store = Store::open(&root.path().join("concurrent.lin")).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(12));
        let tasks = (0..12)
            .map(|i| {
                let store = store.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let event = change(&format!("page-{i}"), "body");
                    barrier.wait();
                    store
                        .with_tx(|tx| {
                            local_change_locked(
                                tx,
                                &SyncIdentity {
                                    node_id: "primary".into(),
                                    primary_url: None,
                                },
                                &event,
                            )
                        })
                        .unwrap()
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
