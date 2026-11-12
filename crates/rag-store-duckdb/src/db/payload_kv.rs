//! Shared `(id, payload)` JSON tables used by the eval, feedback and jobs stores over `lin`.
//!
//! Domain types stay in their modules; this centralizes `(id, payload)` persistence
//! and `created_at` pagination.

use std::cell::RefCell;

use serde::Serialize;
use serde_json::{Value, json};

use super::lin_engine::{
    LinReadGuard, LinReader, LinWriteGuard, LinWriter, RecBuilder, cell_opt_string, cell_string,
};
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
    KnowledgeViews,
}

impl PayloadTable {
    /// Collection name for this payload table.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EvalRuns => "eval_runs",
            Self::EvalTraces => "eval_traces",
            Self::EvalPrompts => "eval_prompts",
            Self::EvalLabelQueue => "eval_label_queue",
            Self::SearchFeedback => "search_feedback",
            Self::BackgroundJobs => "background_jobs",
            Self::KnowledgeViews => "knowledge_views",
        }
    }
}

/// Insert or replace a JSON payload row.
pub fn upsert_payload(
    store: &Store,
    table: PayloadTable,
    id: &str,
    payload: &impl Serialize,
) -> Result<()> {
    crate::util::blocking_section(|| {
        let mut w = store.write()?;
        upsert_payload_locked(&mut w, table, id, payload)
    })
}

/// Lock-scoped upsert for RMW that already holds the Store write lock.
pub fn upsert_payload_locked(
    w: &mut impl LinWriter,
    table: PayloadTable,
    id: &str,
    payload: &impl Serialize,
) -> Result<()> {
    let raw = serde_json::to_string(payload)?;
    let rec = RecBuilder::new().id(id).str("payload", &raw).build();
    w.upsert(table.as_str(), rec)
}

/// Load raw payload JSON text by id.
pub fn get_payload_raw(store: &Store, table: PayloadTable, id: &str) -> Result<Option<String>> {
    crate::util::blocking_section(|| {
        let r: LinReadGuard<'_> = store.read()?;
        get_payload_raw_locked(&r, table, id)
    })
}

/// Lock-scoped get for RMW that already holds a Store guard.
pub fn get_payload_raw_locked(
    r: &impl LinReader,
    table: PayloadTable,
    id: &str,
) -> Result<Option<String>> {
    Ok(r.get_by_id(table.as_str(), id)?
        .and_then(|row| cell_opt_string(&row, "payload")))
}

/// Count rows in a payload table.
pub fn count_payloads(store: &Store, table: PayloadTable) -> Result<i64> {
    crate::util::blocking_section(|| {
        let r = store.read()?;
        Ok(r.count_all(table.as_str())? as i64)
    })
}

/// Load every `(id, payload)` pair. Caller owns filtering/sorting of typed rows.
pub fn list_payload_pairs(store: &Store, table: PayloadTable) -> Result<Vec<(String, String)>> {
    crate::util::blocking_section(|| {
        let r = store.read()?;
        list_payload_pairs_locked(&r, table)
    })
}

/// Lock-scoped list for RMW that already holds a Store guard.
pub fn list_payload_pairs_locked(
    r: &impl LinReader,
    table: PayloadTable,
) -> Result<Vec<(String, String)>> {
    r.filter_map_rows(table.as_str(), |row| {
        Ok(Some((cell_string(row, "id"), cell_string(row, "payload"))))
    })
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

/// Total row count and one page of payloads ordered by `id`.
pub fn page_payloads_by_id(
    store: &Store,
    table: PayloadTable,
    offset: u32,
    limit: usize,
) -> Result<(i64, Vec<Value>)> {
    let r = store.read()?;
    let mut pairs = list_payload_pairs_locked(&r, table)?;
    let total = pairs.len() as i64;
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let items = pairs
        .into_iter()
        .skip(offset as usize)
        .take(limit)
        .map(|(_, raw)| Ok(serde_json::from_str::<Value>(&raw)?))
        .collect::<Result<Vec<_>>>()?;
    Ok((total, items))
}

/// One locked session for read-modify-write over payload tables.
///
/// Lease claims and CAS updates must read and write under the same Store write lock;
/// callers get this handle instead of a raw database connection.
pub struct PayloadSession<'a> {
    w: RefCell<LinWriteGuard<'a>>,
}

impl PayloadSession<'_> {
    /// List `(id, payload)` pairs from a payload table.
    pub fn list_pairs(&self, table: PayloadTable) -> Result<Vec<(String, String)>> {
        let guard = self.w.borrow();
        list_payload_pairs_locked(&*guard, table)
    }

    /// Fetch one raw JSON payload by id.
    pub fn get_raw(&self, table: PayloadTable, id: &str) -> Result<Option<String>> {
        let guard = self.w.borrow();
        get_payload_raw_locked(&*guard, table, id)
    }

    /// Insert or replace one serialized payload by id.
    pub fn upsert(&self, table: PayloadTable, id: &str, payload: &impl Serialize) -> Result<()> {
        let mut guard = self.w.borrow_mut();
        upsert_payload_locked(&mut *guard, table, id, payload)
    }

    /// Count rows in a payload table.
    pub fn count(&self, table: PayloadTable) -> Result<i64> {
        let guard = self.w.borrow();
        Ok(guard.count_all(table.as_str())? as i64)
    }
}

/// Run `work` with one [`PayloadSession`] under the Store write lock.
pub fn with_session<R>(
    store: &Store,
    work: impl FnOnce(&PayloadSession<'_>) -> Result<R>,
) -> Result<R> {
    crate::util::blocking_section(|| {
        let w = store.write()?;
        work(&PayloadSession { w: RefCell::new(w) })
    })
}

/// Delete one payload row by id (no-op when absent).
pub fn delete_payload(store: &Store, table: PayloadTable, id: &str) -> Result<()> {
    crate::util::blocking_section(|| {
        let mut w = store.write()?;
        w.delete_by_id(table.as_str(), id)?;
        Ok(())
    })
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
            db_path: root.path().join("payload.lin"),
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
        let raw = get_payload_raw(&store, PayloadTable::EvalRuns, "a")
            .unwrap()
            .unwrap();
        assert!(raw.contains("\"kind\":\"x\""));
    }

    #[test]
    fn missing_row_is_none() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("empty.lin")).unwrap();
        assert!(
            get_payload_raw(&store, PayloadTable::EvalTraces, "missing")
                .unwrap()
                .is_none()
        );
    }
}
