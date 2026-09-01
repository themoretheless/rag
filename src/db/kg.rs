//! Temporal knowledge-graph facts (`kg_facts`): add, query, invalidate, supersede, timeline, stats.

use chrono::{DateTime, Utc};
use duckdb::params;
use uuid::Uuid;

use super::store::Store;
use crate::error::{AppError, Result};
use crate::models::{KgFact, KgStats};
use crate::util::{format_db_timestamp as format_ts, parse_flexible_timestamp};

/// Shared SELECT list for kg_facts rows (order matches [`row_to_kg_fact`]).
const KG_FACT_SELECT: &str = r#"
    id, subject, predicate, object,
    CAST(valid_from AS VARCHAR), CAST(valid_to AS VARCHAR),
    status, superseded_by, source_document_id, source, confidence, metadata_json,
    CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR),
    CAST(invalidated_at AS VARCHAR)
"#;

impl Store {
    /// Insert a temporal fact. Idempotent for open active SPO matches (returns existing).
    ///
    /// Validity is half-open `[valid_from, valid_to)`. Rejects inverted windows.
    #[allow(clippy::too_many_arguments)]
    pub fn kg_add(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
        valid_from: Option<DateTime<Utc>>,
        valid_to: Option<DateTime<Utc>>,
        source_document_id: Option<&str>,
        confidence: Option<f64>,
        metadata_json: Option<&str>,
    ) -> Result<KgFact> {
        let subject = require_nonempty(subject, "subject")?;
        let predicate = normalize_predicate(predicate)?;
        let object = require_nonempty(object, "object")?;
        check_window(valid_from, valid_to)?;

        let source_document_id = opt_trim(source_document_id);
        let metadata = match metadata_json.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => s.to_string(),
            None => "{}".into(),
        };
        let now = Utc::now();
        let now_s = format_ts(now);

        let conn = self.lock()?;

        // Idempotent: same open active SPO already present.
        {
            let mut stmt = conn.prepare(
                r#"
                SELECT id FROM kg_facts
                WHERE subject = ? AND predicate = ? AND object = ?
                  AND status = 'active' AND valid_to IS NULL
                LIMIT 1
                "#,
            )?;
            let mut rows = stmt.query(params![subject, predicate, object])?;
            if let Some(row) = rows.next()? {
                let id: String = row.get(0)?;
                drop(rows);
                drop(stmt);
                return load_fact_locked(&conn, &id)?
                    .ok_or_else(|| AppError::db("kg_add: existing fact vanished"));
            }
        }

        let id = Uuid::new_v4().to_string();
        let conf = confidence.unwrap_or(1.0);
        conn.execute(
            r#"
            INSERT INTO kg_facts (
              id, subject, predicate, object,
              valid_from, valid_to, status, superseded_by,
              source_document_id, source, confidence, metadata_json,
              created_at, updated_at, invalidated_at
            ) VALUES (
              ?, ?, ?, ?,
              CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP), 'active', NULL,
              ?, ?, ?, ?,
              CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP), NULL
            )
            "#,
            params![
                id,
                subject,
                predicate,
                object,
                valid_from.map(format_ts),
                valid_to.map(format_ts),
                source_document_id.clone(),
                source_document_id.clone(),
                conf,
                metadata,
                now_s.as_str(),
                now_s.as_str(),
            ],
        )?;

        load_fact_locked(&conn, &id)?.ok_or_else(|| AppError::db("kg_add: insert not readable"))
    }

    /// Query facts by optional SPO filters and optional point-in-time.
    ///
    /// - Without `at_time`: returns `status = 'active'` rows matching filters.
    /// - With `at_time`: half-open temporal filter
    ///   `(valid_from IS NULL OR valid_from <= t) AND (valid_to IS NULL OR valid_to > t)`.
    pub fn kg_query(
        &self,
        subject: Option<&str>,
        predicate: Option<&str>,
        object: Option<&str>,
        at_time: Option<DateTime<Utc>>,
    ) -> Result<Vec<KgFact>> {
        let subject = opt_trim(subject);
        let predicate = predicate
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase().replace(' ', "_"));
        let object = opt_trim(object);

        let mut sql = format!("SELECT {KG_FACT_SELECT} FROM kg_facts WHERE 1=1");
        let mut binds: Vec<String> = Vec::new();

        if let Some(ref s) = subject {
            sql.push_str(" AND subject = ?");
            binds.push(s.clone());
        }
        if let Some(ref p) = predicate {
            sql.push_str(" AND predicate = ?");
            binds.push(p.clone());
        }
        if let Some(ref o) = object {
            sql.push_str(" AND object = ?");
            binds.push(o.clone());
        }

        if let Some(t) = at_time {
            let t_s = format_ts(t);
            sql.push_str(
                " AND (valid_from IS NULL OR valid_from <= CAST(? AS TIMESTAMP))\
                 AND (valid_to IS NULL OR valid_to > CAST(? AS TIMESTAMP))",
            );
            binds.push(t_s.clone());
            binds.push(t_s);
        } else {
            sql.push_str(" AND status = 'active'");
        }

        sql.push_str(" ORDER BY valid_from ASC NULLS FIRST, created_at ASC, id ASC");

        let conn = self.lock()?;
        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn duckdb::ToSql> =
            binds.iter().map(|s| s as &dyn duckdb::ToSql).collect();
        let mut rows = stmt.query(param_refs.as_slice())?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_kg_fact(row)?);
        }
        Ok(out)
    }

    /// Mark matching open active fact(s) as invalidated.
    ///
    /// Sets `valid_to = ended` (default now), `status = 'invalidated'`, and `invalidated_at`.
    pub fn kg_invalidate(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
        ended: Option<DateTime<Utc>>,
    ) -> Result<Vec<KgFact>> {
        let subject = require_nonempty(subject, "subject")?;
        let predicate = normalize_predicate(predicate)?;
        let object = require_nonempty(object, "object")?;
        let ended = ended.unwrap_or_else(Utc::now);
        let ended_s = format_ts(ended);
        let now_s = format_ts(Utc::now());

        let conn = self.lock()?;

        // Collect ids first so we can validate windows and return updated rows.
        let mut ids: Vec<String> = Vec::new();
        {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, CAST(valid_from AS VARCHAR) FROM kg_facts
                WHERE subject = ? AND predicate = ? AND object = ?
                  AND status = 'active' AND valid_to IS NULL
                "#,
            )?;
            let mut rows = stmt.query(params![subject, predicate, object])?;
            while let Some(row) = rows.next()? {
                let id: String = row.get(0)?;
                let vf_raw: Option<String> = row.get(1)?;
                if let Some(raw) = vf_raw {
                    let vf = parse_ts(&raw)?;
                    if ended < vf {
                        return Err(AppError::config(format!(
                            "kg_invalidate: ended={ended_s} is before valid_from={raw}"
                        )));
                    }
                }
                ids.push(id);
            }
        }

        if ids.is_empty() {
            return Ok(Vec::new());
        }

        for id in &ids {
            conn.execute(
                r#"
                UPDATE kg_facts
                SET valid_to = CAST(? AS TIMESTAMP),
                    status = 'invalidated',
                    invalidated_at = CAST(? AS TIMESTAMP),
                    updated_at = CAST(? AS TIMESTAMP)
                WHERE id = ?
                "#,
                params![ended_s.as_str(), ended_s.as_str(), now_s.as_str(), id],
            )?;
        }

        let mut out = Vec::with_capacity(ids.len());
        for id in &ids {
            if let Some(f) = load_fact_locked(&conn, id)? {
                out.push(f);
            }
        }
        Ok(out)
    }

    /// Atomically replace an open fact's object at a shared boundary instant.
    ///
    /// Closes matching `(subject, predicate, old_object)` with `valid_to = at` and
    /// `status = 'superseded'`, then opens `(subject, predicate, new_object)` with
    /// `valid_from = at`. Point-in-time queries at `at` return only the successor
    /// (half-open upper bound).
    ///
    /// If no open old fact exists, still opens the successor (degrades to add).
    #[allow(clippy::too_many_arguments)]
    pub fn kg_supersede(
        &self,
        subject: &str,
        predicate: &str,
        old_object: &str,
        new_object: &str,
        at: Option<DateTime<Utc>>,
        source_document_id: Option<&str>,
        confidence: Option<f64>,
    ) -> Result<KgFact> {
        let subject = require_nonempty(subject, "subject")?;
        let predicate = normalize_predicate(predicate)?;
        let old_object = require_nonempty(old_object, "old_object")?;
        let new_object = require_nonempty(new_object, "new_object")?;
        let at = at.unwrap_or_else(Utc::now);
        let at_s = format_ts(at);
        let now = Utc::now();
        let now_s = format_ts(now);
        let source_document_id = opt_trim(source_document_id);
        let conf = confidence.unwrap_or(1.0);

        let conn = self.lock()?;

        // Close open old facts at the shared boundary.
        let mut old_ids: Vec<String> = Vec::new();
        {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, CAST(valid_from AS VARCHAR) FROM kg_facts
                WHERE subject = ? AND predicate = ? AND object = ?
                  AND status = 'active' AND valid_to IS NULL
                "#,
            )?;
            let mut rows = stmt.query(params![subject, predicate, old_object])?;
            while let Some(row) = rows.next()? {
                let id: String = row.get(0)?;
                let vf_raw: Option<String> = row.get(1)?;
                if let Some(raw) = vf_raw {
                    let vf = parse_ts(&raw)?;
                    if at < vf {
                        return Err(AppError::config(format!(
                            "kg_supersede: at={at_s} is before valid_from={raw}"
                        )));
                    }
                }
                old_ids.push(id);
            }
        }

        // If successor already open and active, return it after closing olds.
        let existing_new_id: Option<String> = {
            let mut stmt = conn.prepare(
                r#"
                SELECT id FROM kg_facts
                WHERE subject = ? AND predicate = ? AND object = ?
                  AND status = 'active' AND valid_to IS NULL
                LIMIT 1
                "#,
            )?;
            let mut rows = stmt.query(params![subject, predicate, new_object])?;
            match rows.next()? {
                Some(row) => Some(row.get(0)?),
                None => None,
            }
        };

        let creating_new = existing_new_id.is_none();
        let new_id = existing_new_id.unwrap_or_else(|| Uuid::new_v4().to_string());

        for id in &old_ids {
            conn.execute(
                r#"
                UPDATE kg_facts
                SET valid_to = CAST(? AS TIMESTAMP),
                    status = 'superseded',
                    superseded_by = ?,
                    updated_at = CAST(? AS TIMESTAMP)
                WHERE id = ?
                "#,
                params![at_s.as_str(), new_id.as_str(), now_s.as_str(), id],
            )?;
        }

        if creating_new {
            conn.execute(
                r#"
                INSERT INTO kg_facts (
                  id, subject, predicate, object,
                  valid_from, valid_to, status, superseded_by,
                  source_document_id, source, confidence, metadata_json,
                  created_at, updated_at, invalidated_at
                ) VALUES (
                  ?, ?, ?, ?,
                  CAST(? AS TIMESTAMP), NULL, 'active', NULL,
                  ?, ?, ?, '{}',
                  CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP), NULL
                )
                "#,
                params![
                    new_id,
                    subject,
                    predicate,
                    new_object,
                    at_s.as_str(),
                    source_document_id.clone(),
                    source_document_id.clone(),
                    conf,
                    now_s.as_str(),
                    now_s.as_str(),
                ],
            )?;
        }

        load_fact_locked(&conn, &new_id)?
            .ok_or_else(|| AppError::db("kg_supersede: new fact not readable"))
    }

    /// Chronological facts for a subject (any status), ordered by `valid_from`.
    pub fn kg_timeline(&self, subject: &str) -> Result<Vec<KgFact>> {
        let subject = require_nonempty(subject, "subject")?;
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            r#"
            SELECT {KG_FACT_SELECT}
            FROM kg_facts
            WHERE subject = ?
            ORDER BY valid_from ASC NULLS FIRST, created_at ASC, id ASC
            "#
        ))?;
        let mut rows = stmt.query(params![subject])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_kg_fact(row)?);
        }
        Ok(out)
    }

    /// Aggregate counts and distinct relationship types.
    pub fn kg_stats(&self) -> Result<KgStats> {
        let conn = self.lock()?;

        let total_facts: i64 = conn.query_row("SELECT COUNT(*) FROM kg_facts", [], |r| r.get(0))?;
        let active_facts: i64 = conn.query_row(
            "SELECT COUNT(*) FROM kg_facts WHERE status = 'active'",
            [],
            |r| r.get(0),
        )?;
        let invalidated_facts: i64 = conn.query_row(
            "SELECT COUNT(*) FROM kg_facts WHERE status = 'invalidated'",
            [],
            |r| r.get(0),
        )?;
        let superseded_facts: i64 = conn.query_row(
            "SELECT COUNT(*) FROM kg_facts WHERE status = 'superseded'",
            [],
            |r| r.get(0),
        )?;
        let distinct_subjects: i64 =
            conn.query_row("SELECT COUNT(DISTINCT subject) FROM kg_facts", [], |r| {
                r.get(0)
            })?;
        let distinct_predicates: i64 =
            conn.query_row("SELECT COUNT(DISTINCT predicate) FROM kg_facts", [], |r| {
                r.get(0)
            })?;

        let mut relationship_types = Vec::new();
        {
            let mut stmt =
                conn.prepare("SELECT DISTINCT predicate FROM kg_facts ORDER BY predicate ASC")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                relationship_types.push(row.get::<_, String>(0)?);
            }
        }

        Ok(KgStats {
            total_facts: total_facts as u64,
            active_facts: active_facts as u64,
            invalidated_facts: invalidated_facts as u64,
            superseded_facts: superseded_facts as u64,
            distinct_subjects: distinct_subjects as u64,
            distinct_predicates: distinct_predicates as u64,
            relationship_types,
        })
    }

    /// Load one fact by primary key.
    pub fn kg_get(&self, fact_id: &str) -> Result<Option<KgFact>> {
        let conn = self.lock()?;
        load_fact_locked(&conn, fact_id)
    }
}

fn load_fact_locked(conn: &duckdb::Connection, id: &str) -> Result<Option<KgFact>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {KG_FACT_SELECT} FROM kg_facts WHERE id = ? LIMIT 1"
    ))?;
    let mut rows = stmt.query(params![id])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_kg_fact(row)?)),
        None => Ok(None),
    }
}

fn row_to_kg_fact(row: &duckdb::Row<'_>) -> Result<KgFact> {
    let valid_from = opt_parse_ts(row.get::<_, Option<String>>(4)?)?;
    let valid_to = opt_parse_ts(row.get::<_, Option<String>>(5)?)?;
    let created_raw: String = row.get(12)?;
    let updated_raw: Option<String> = row.get(13)?;
    let invalidated_at = opt_parse_ts(row.get::<_, Option<String>>(14)?)?;

    let created_at = parse_ts(&created_raw)?;
    let updated_at = match updated_raw {
        Some(s) if !s.trim().is_empty() => parse_ts(&s)?,
        _ => created_at,
    };

    let status: Option<String> = row.get(6)?;
    let metadata: Option<String> = row.get(11)?;

    Ok(KgFact {
        id: row.get(0)?,
        subject: row.get(1)?,
        predicate: row.get(2)?,
        object: row.get(3)?,
        valid_from,
        valid_to,
        status: status
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "active".into()),
        superseded_by: row.get(7)?,
        source_document_id: row.get(8)?,
        source: row.get(9)?,
        confidence: row.get(10)?,
        metadata_json: metadata
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "{}".into()),
        created_at,
        updated_at,
        invalidated_at,
    })
}

fn require_nonempty(s: &str, field: &str) -> Result<String> {
    let t = s.trim();
    if t.is_empty() {
        return Err(AppError::config(format!("kg: {field} must be non-empty")));
    }
    Ok(t.to_string())
}

fn normalize_predicate(predicate: &str) -> Result<String> {
    let t = require_nonempty(predicate, "predicate")?;
    Ok(t.to_ascii_lowercase().replace(' ', "_"))
}

fn opt_trim(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
}

fn check_window(valid_from: Option<DateTime<Utc>>, valid_to: Option<DateTime<Utc>>) -> Result<()> {
    if let (Some(from), Some(to)) = (valid_from, valid_to) {
        if to < from {
            return Err(AppError::config(format!(
                "kg: valid_to ({}) is before valid_from ({})",
                format_ts(to),
                format_ts(from)
            )));
        }
    }
    Ok(())
}

fn opt_parse_ts(raw: Option<String>) -> Result<Option<DateTime<Utc>>> {
    match raw {
        Some(s) if !s.trim().is_empty() => Ok(Some(parse_ts(&s)?)),
        _ => Ok(None),
    }
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    parse_flexible_timestamp(s)
        .ok_or_else(|| AppError::db(format!("invalid timestamp value: {}", s.trim())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn open_temp() -> Store {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("kg.duckdb");
        // Keep tempdir alive by leaking (tests are short-lived).
        std::mem::forget(dir);
        Store::open(&path).expect("open store")
    }

    fn ts(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn kg_add_query_active_and_idempotent() {
        let store = open_temp();
        let f1 = store
            .kg_add(
                "Alice",
                "works_at",
                "Acme",
                Some(ts(2020, 1, 1)),
                None,
                Some("doc:1"),
                None,
                None,
            )
            .expect("add");
        assert_eq!(f1.subject, "Alice");
        assert_eq!(f1.predicate, "works_at");
        assert_eq!(f1.object, "Acme");
        assert_eq!(f1.status, "active");

        let again = store
            .kg_add("Alice", "works_at", "Acme", None, None, None, None, None)
            .expect("idempotent add");
        assert_eq!(again.id, f1.id);

        let hits = store
            .kg_query(Some("Alice"), Some("works_at"), None, None)
            .expect("query");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, f1.id);
    }

    #[test]
    fn kg_invalidate_and_at_time_query() {
        let store = open_temp();
        store
            .kg_add(
                "Bob",
                "lives_in",
                "Paris",
                Some(ts(2019, 1, 1)),
                None,
                None,
                Some(0.9),
                None,
            )
            .expect("add");

        let ended = ts(2022, 6, 1);
        let inv = store
            .kg_invalidate("Bob", "lives_in", "Paris", Some(ended))
            .expect("invalidate");
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].status, "invalidated");
        assert!(inv[0].valid_to.is_some());

        let active = store
            .kg_query(Some("Bob"), None, None, None)
            .expect("active query");
        assert!(active.is_empty());

        let mid = store
            .kg_query(Some("Bob"), None, None, Some(ts(2021, 1, 1)))
            .expect("at_time mid");
        assert_eq!(mid.len(), 1);

        let after = store
            .kg_query(Some("Bob"), None, None, Some(ts(2023, 1, 1)))
            .expect("at_time after");
        assert!(after.is_empty());

        // Half-open: at exact valid_to the fact is no longer valid.
        let boundary = store
            .kg_query(Some("Bob"), None, None, Some(ended))
            .expect("at_time boundary");
        assert!(boundary.is_empty());
    }

    #[test]
    fn kg_supersede_closes_old_and_opens_new() {
        let store = open_temp();
        store
            .kg_add(
                "Carol",
                "employer",
                "OldCo",
                Some(ts(2018, 1, 1)),
                None,
                None,
                None,
                None,
            )
            .expect("add");

        let at = ts(2024, 3, 15);
        let new_f = store
            .kg_supersede("Carol", "employer", "OldCo", "NewCo", Some(at), None, None)
            .expect("supersede");
        assert_eq!(new_f.object, "NewCo");
        assert_eq!(new_f.status, "active");
        assert_eq!(new_f.valid_from, Some(at));

        let timeline = store.kg_timeline("Carol").expect("timeline");
        assert_eq!(timeline.len(), 2);
        let old = timeline.iter().find(|f| f.object == "OldCo").expect("old");
        assert_eq!(old.status, "superseded");
        assert_eq!(old.valid_to, Some(at));
        assert_eq!(old.superseded_by.as_deref(), Some(new_f.id.as_str()));

        // At boundary: only successor (half-open).
        let at_b = store
            .kg_query(Some("Carol"), Some("employer"), None, Some(at))
            .expect("query at");
        assert_eq!(at_b.len(), 1);
        assert_eq!(at_b[0].object, "NewCo");

        let before = store
            .kg_query(Some("Carol"), Some("employer"), None, Some(ts(2020, 1, 1)))
            .expect("query before");
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].object, "OldCo");
    }

    #[test]
    fn kg_stats_counts() {
        let store = open_temp();
        store
            .kg_add("X", "rel_a", "Y", None, None, None, None, None)
            .unwrap();
        store
            .kg_add("X", "rel_b", "Z", None, None, None, None, None)
            .unwrap();
        store.kg_invalidate("X", "rel_a", "Y", None).unwrap();

        let s = store.kg_stats().expect("stats");
        assert_eq!(s.total_facts, 2);
        assert_eq!(s.active_facts, 1);
        assert_eq!(s.invalidated_facts, 1);
        assert_eq!(s.superseded_facts, 0);
        assert_eq!(s.distinct_subjects, 1);
        assert!(s.relationship_types.iter().any(|p| p == "rel_a"));
        assert!(s.relationship_types.iter().any(|p| p == "rel_b"));
    }

    #[test]
    fn kg_add_rejects_empty_and_inverted() {
        let store = open_temp();
        assert!(store
            .kg_add("", "p", "o", None, None, None, None, None)
            .is_err());
        assert!(store
            .kg_add(
                "s",
                "p",
                "o",
                Some(ts(2022, 1, 1)),
                Some(ts(2020, 1, 1)),
                None,
                None,
                None
            )
            .is_err());
    }
}
