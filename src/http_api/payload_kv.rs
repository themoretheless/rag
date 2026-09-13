//! Shared `(id, payload)` JSON tables used by eval/jobs HTTP adapters.
//!
//! These tables share one DuckDB shape (`id VARCHAR PRIMARY KEY, payload VARCHAR`).
//! Domain types stay in their modules; this only removes copy-pasted SQL and
//! created_at pagination.

use duckdb::{params, OptionalExt};
use serde::Serialize;
use serde_json::{json, Value};

use crate::db::Store;
use crate::error::Result;

/// Allowlisted payload tables (never interpolate untrusted names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadTable {
    EvalRuns,
    EvalTraces,
    EvalPrompts,
    EvalLabelQueue,
    SearchFeedback,
    BackgroundJobs,
}

impl PayloadTable {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EvalRuns => "eval_runs",
            Self::EvalTraces => "eval_traces",
            Self::EvalPrompts => "eval_prompts",
            Self::EvalLabelQueue => "eval_label_queue",
            Self::SearchFeedback => "search_feedback",
            Self::BackgroundJobs => "background_jobs",
        }
    }
}

/// Insert or replace a JSON payload row.
pub fn upsert_payload(store: &Store, table: PayloadTable, id: &str, payload: &impl Serialize) -> Result<()> {
    let conn = store.lock()?;
    upsert_payload_conn(&conn, table, id, payload)
}

/// Connection-scoped upsert for RMW that already holds the Store lock.
pub fn upsert_payload_conn(
    conn: &duckdb::Connection,
    table: PayloadTable,
    id: &str,
    payload: &impl Serialize,
) -> Result<()> {
    let raw = serde_json::to_string(payload)?;
    let sql = format!("INSERT OR REPLACE INTO {} VALUES (?,?)", table.as_str());
    conn.execute(&sql, params![id, raw])?;
    Ok(())
}

/// Load raw payload JSON text by id.
pub fn get_payload_raw(store: &Store, table: PayloadTable, id: &str) -> Result<Option<String>> {
    let conn = store.lock()?;
    get_payload_raw_conn(&conn, table, id)
}

/// Connection-scoped get for RMW that already holds the Store lock.
pub fn get_payload_raw_conn(
    conn: &duckdb::Connection,
    table: PayloadTable,
    id: &str,
) -> Result<Option<String>> {
    let sql = format!("SELECT payload FROM {} WHERE id=?", table.as_str());
    let value: Option<String> = conn.query_row(&sql, [id], |r| r.get(0)).optional()?;
    Ok(value)
}

/// Count rows in a payload table.
pub fn count_payloads(store: &Store, table: PayloadTable) -> Result<i64> {
    let conn = store.lock()?;
    let sql = format!("SELECT COUNT(*) FROM {}", table.as_str());
    Ok(conn.query_row(&sql, [], |r| r.get(0))?)
}

/// Load every `(id, payload)` pair. Caller owns filtering/sorting of typed rows.
pub fn list_payload_pairs(store: &Store, table: PayloadTable) -> Result<Vec<(String, String)>> {
    let conn = store.lock()?;
    list_payload_pairs_conn(&conn, table)
}

/// Connection-scoped list for RMW that already holds the Store lock.
pub fn list_payload_pairs_conn(
    conn: &duckdb::Connection,
    table: PayloadTable,
) -> Result<Vec<(String, String)>> {
    let sql = format!("SELECT id, payload FROM {}", table.as_str());
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Load payloads as JSON objects, injecting `id` when missing.
pub fn list_payload_values(store: &Store, table: PayloadTable) -> Result<Vec<Value>> {
    list_payload_pairs(store, table)?
        .into_iter()
        .map(|(id, payload)| {
            let mut value = serde_json::from_str::<Value>(&payload)?;
            if let Some(obj) = value.as_object_mut() {
                obj.entry("id").or_insert(json!(id));
            }
            Ok(value)
        })
        .collect()
}

/// Newest `created_at` first; missing timestamps sort last; tie-break on `id`.
pub fn sort_by_created_at_desc(items: &mut [Value]) {
    items.sort_by(|a, b| {
        let ca = a.get("created_at").and_then(|v| v.as_str());
        let cb = b.get("created_at").and_then(|v| v.as_str());
        let id_a = a.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let id_b = b.get("id").and_then(|v| v.as_str()).unwrap_or("");
        match (ca, cb) {
            (Some(a), Some(b)) => b.cmp(a).then_with(|| id_b.cmp(id_a)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => id_b.cmp(id_a),
        }
    });
}

/// Offset/limit page of already-sorted values.
pub fn page_values(items: Vec<Value>, offset: u32, limit: usize) -> Vec<Value> {
    items
        .into_iter()
        .skip(offset as usize)
        .take(limit)
        .collect()
}

/// Count + created_at-desc page used by most eval list endpoints.
pub fn list_payload_page(
    store: &Store,
    table: PayloadTable,
    offset: u32,
    limit: usize,
) -> Result<(i64, Vec<Value>)> {
    let total = count_payloads(store, table)?;
    let mut items = list_payload_values(store, table)?;
    sort_by_created_at_desc(&mut items);
    Ok((total, page_values(items, offset, limit)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::sync::Arc;

    #[test]
    fn upsert_list_and_page_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            db_path: root.path().join("payload.duckdb"),
            ..Config::for_tests()
        };
        let store = Arc::new(Store::open(&config.db_path).unwrap());
        upsert_payload(
            &store,
            PayloadTable::EvalRuns,
            "a",
            &json!({"id":"a","created_at":"2026-09-12T10:00:00Z","kind":"x"}),
        )
        .unwrap();
        upsert_payload(
            &store,
            PayloadTable::EvalRuns,
            "b",
            &json!({"id":"b","created_at":"2026-09-12T12:00:00Z","kind":"y"}),
        )
        .unwrap();
        let (total, page) = list_payload_page(&store, PayloadTable::EvalRuns, 0, 10).unwrap();
        assert_eq!(total, 2);
        assert_eq!(page[0]["id"], "b");
        assert_eq!(page[1]["id"], "a");
        let raw = get_payload_raw(&store, PayloadTable::EvalRuns, "a").unwrap().unwrap();
        assert!(raw.contains("\"kind\":\"x\""));
    }

    #[test]
    fn missing_row_is_none() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("empty.duckdb")).unwrap();
        assert!(get_payload_raw(&store, PayloadTable::EvalTraces, "missing")
            .unwrap()
            .is_none());
    }
}
