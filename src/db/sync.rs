//! Durable transport state for local-to-primary database synchronization.

use duckdb::params;
use serde::{Deserialize, Serialize};

use super::Store;
use crate::error::{AppError, Result};

pub const MAX_SYNC_BATCH: usize = 500;

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

impl Store {
    pub fn has_sync_change(&self, origin_node: &str, origin_seq: i64) -> Result<bool> {
        let conn = self.lock()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_changes WHERE origin_node = ? AND origin_seq = ?",
            params![origin_node, origin_seq],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Journal one application mutation according to this process' role.
    /// Replicas retain it in the outbox; a primary assigns a canonical cursor.
    pub fn journal_local_sync_change(
        &self,
        entity_kind: &str,
        entity_id: &str,
        operation: &str,
        payload_json: &str,
        content_hash: Option<&str>,
    ) -> Result<i64> {
        let primary_url = std::env::var("RAG_PRIMARY_URL")
            .ok()
            .filter(|v| !v.trim().is_empty());
        if primary_url.is_some() {
            return self.enqueue_sync_change(
                entity_kind,
                entity_id,
                operation,
                payload_json,
                content_hash,
            );
        }
        let node_id = std::env::var("RAG_NODE_ID")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "primary".into());
        self.register_sync_node(&node_id, &node_id, "primary")?;
        let origin_seq = {
            let conn = self.lock()?;
            conn.query_row(
                "SELECT COALESCE(MAX(origin_seq), 0) + 1 FROM sync_changes WHERE origin_node = ?",
                params![&node_id],
                |row| row.get(0),
            )?
        };
        let change = SyncChangeInput {
            origin_seq,
            entity_kind: entity_kind.into(),
            entity_id: entity_id.into(),
            operation: operation.into(),
            payload_json: payload_json.into(),
            content_hash: content_hash.map(str::to_string),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        let assigned = self.accept_sync_changes(&node_id, &[change])?;
        Ok(assigned[0])
    }

    pub fn enqueue_sync_change(
        &self,
        entity_kind: &str,
        entity_id: &str,
        operation: &str,
        payload_json: &str,
        content_hash: Option<&str>,
    ) -> Result<i64> {
        let prototype = SyncChangeInput {
            origin_seq: 1,
            entity_kind: entity_kind.into(),
            entity_id: entity_id.into(),
            operation: operation.into(),
            payload_json: payload_json.into(),
            content_hash: content_hash.map(str::to_string),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        validate_change(&prototype)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let next: i64 = tx.query_row(
            "SELECT COALESCE(MAX(local_seq), 0) + 1 FROM sync_outbox",
            [],
            |row| row.get(0),
        )?;
        tx.execute(
            r#"INSERT INTO sync_outbox
               (local_seq, entity_kind, entity_id, operation, payload_json, content_hash, created_at)
               VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)"#,
            params![next, entity_kind, entity_id, operation, payload_json, content_hash],
        )?;
        tx.commit()?;
        Ok(next)
    }

    pub fn pending_sync_outbox(&self, limit: usize) -> Result<Vec<SyncChangeInput>> {
        let limit = limit.clamp(1, MAX_SYNC_BATCH);
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            r#"SELECT local_seq, entity_kind, entity_id, operation, payload_json,
                      content_hash, CAST(created_at AS VARCHAR)
               FROM sync_outbox WHERE sent_at IS NULL ORDER BY local_seq LIMIT {limit}"#,
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
            tx.execute(
                "UPDATE sync_outbox SET sent_at = CURRENT_TIMESTAMP, primary_seq = ? WHERE local_seq = ? AND sent_at IS NULL",
                params![primary_seq, local_seq],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn register_sync_node(&self, node_id: &str, hostname: &str, role: &str) -> Result<()> {
        validate_token("node_id", node_id)?;
        validate_token("hostname", hostname)?;
        let role = match role.trim() {
            "primary" => "primary",
            "replica" => "replica",
            other => return Err(AppError::config(format!("invalid sync role '{other}'"))),
        };
        let conn = self.lock()?;
        let updated = conn.execute(
            "UPDATE sync_nodes SET hostname = ?, role = ?, last_seen_at = CURRENT_TIMESTAMP WHERE node_id = ?",
            params![hostname.trim(), role, node_id.trim()],
        )?;
        if updated == 0 {
            conn.execute(
                "INSERT INTO sync_nodes (node_id, hostname, role, last_seen_at) VALUES (?, ?, ?, CURRENT_TIMESTAMP)",
                params![node_id.trim(), hostname.trim(), role],
            )?;
        }
        Ok(())
    }

    /// Idempotently accept a replica batch and assign canonical primary cursors.
    pub fn accept_sync_changes(
        &self,
        origin_node: &str,
        changes: &[SyncChangeInput],
    ) -> Result<Vec<i64>> {
        validate_token("origin_node", origin_node)?;
        if changes.len() > MAX_SYNC_BATCH {
            return Err(AppError::config(format!(
                "sync batch exceeds {MAX_SYNC_BATCH} changes"
            )));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let mut assigned = Vec::with_capacity(changes.len());
        let mut latest_origin_seq = 0i64;
        for change in changes {
            validate_change(change)?;
            latest_origin_seq = latest_origin_seq.max(change.origin_seq);
            let existing: Option<i64> = tx.query_row(
                "SELECT MAX(primary_seq) FROM sync_changes WHERE origin_node = ? AND origin_seq = ?",
                params![origin_node, change.origin_seq],
                |row| row.get(0),
            )?;
            if let Some(primary_seq) = existing {
                assigned.push(primary_seq);
                continue;
            }
            let next: i64 = tx.query_row(
                "SELECT COALESCE(MAX(primary_seq), 0) + 1 FROM sync_changes",
                [],
                |row| row.get(0),
            )?;
            tx.execute(
                r#"INSERT INTO sync_changes
                   (primary_seq, origin_node, origin_seq, entity_kind, entity_id,
                    operation, payload_json, content_hash, created_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
                params![
                    next,
                    origin_node,
                    change.origin_seq,
                    change.entity_kind,
                    change.entity_id,
                    change.operation,
                    change.payload_json,
                    change.content_hash,
                    change.created_at
                ],
            )?;
            assigned.push(next);
        }
        tx.execute(
            "UPDATE sync_nodes SET last_seen_at = CURRENT_TIMESTAMP, last_push_seq = GREATEST(last_push_seq, ?) WHERE node_id = ?",
            params![latest_origin_seq, origin_node],
        )?;
        tx.commit()?;
        Ok(assigned)
    }

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
            r#"SELECT primary_seq, origin_node, origin_seq, entity_kind, entity_id,
                      operation, payload_json, content_hash, CAST(created_at AS VARCHAR)
               FROM sync_changes WHERE primary_seq > ? AND origin_node <> ?
               ORDER BY primary_seq LIMIT {limit}"#,
        ))?;
        let rows = stmt.query_map(params![after, node_id], |row| {
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

    pub fn acknowledge_sync_cursor(&self, node_id: &str, cursor: i64) -> Result<()> {
        validate_token("node_id", node_id)?;
        if cursor < 0 {
            return Err(AppError::config("sync cursor must be >= 0"));
        }
        let conn = self.lock()?;
        conn.execute(
            "UPDATE sync_nodes SET last_seen_at = CURRENT_TIMESTAMP, pull_cursor = GREATEST(pull_cursor, ?) WHERE node_id = ?",
            params![cursor, node_id],
        )?;
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

fn validate_token(name: &str, value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
        return Err(AppError::config(format!(
            "{name} must be 1..255 printable characters"
        )));
    }
    Ok(())
}

fn validate_change(change: &SyncChangeInput) -> Result<()> {
    if change.origin_seq < 1 {
        return Err(AppError::config("origin_seq must be >= 1"));
    }
    validate_token("entity_kind", &change.entity_kind)?;
    validate_token("entity_id", &change.entity_id)?;
    if !matches!(change.operation.as_str(), "upsert" | "tombstone") {
        return Err(AppError::config(
            "sync operation must be upsert or tombstone",
        ));
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
    use tempfile::tempdir;

    #[test]
    fn push_is_idempotent_and_pull_excludes_origin() {
        let dir = tempdir().unwrap();
        let store = Store::open(&dir.path().join("sync.duckdb")).unwrap();
        store
            .register_sync_node("m4", "m4.local", "replica")
            .unwrap();
        store
            .register_sync_node("main", "main.local", "primary")
            .unwrap();
        let change = SyncChangeInput {
            origin_seq: 1,
            entity_kind: "wiki".into(),
            entity_id: "wiki://one".into(),
            operation: "upsert".into(),
            payload_json: "{\"title\":\"One\"}".into(),
            content_hash: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        assert_eq!(
            store
                .accept_sync_changes("m4", std::slice::from_ref(&change))
                .unwrap(),
            vec![1]
        );
        assert_eq!(store.accept_sync_changes("m4", &[change]).unwrap(), vec![1]);
        assert!(store.pull_sync_changes("m4", 0, 10).unwrap().is_empty());
        assert_eq!(store.pull_sync_changes("main", 0, 10).unwrap().len(), 1);
        let status = store.sync_status("primary", "main", None).unwrap();
        assert_eq!(status.latest_primary_seq, 1);
        assert_eq!(status.nodes.len(), 2);

        let local_seq = store
            .enqueue_sync_change("wiki", "wiki://two", "upsert", "{}", None)
            .unwrap();
        assert_eq!(local_seq, 1);
        assert_eq!(store.pending_sync_outbox(10).unwrap().len(), 1);
        store.mark_sync_outbox_sent(&[(local_seq, 2)]).unwrap();
        assert!(store.pending_sync_outbox(10).unwrap().is_empty());
    }
}
