//! Temporal knowledge-graph facts (`kg_facts`): add, query, invalidate, supersede, timeline, stats over `lin`.

use std::collections::{BTreeSet, HashSet};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::lin_engine::{cell_nonempty_string, cell_str, cell_string, LinReader, LinWriter, RecBuilder};
use super::rows;
use super::store::Store;
use crate::error::{AppError, Result};
use crate::models::{KgFact, KgStats};
use crate::util::{format_db_timestamp as format_ts, parse_flexible_timestamp};

/// Fields for [`Store::kg_add`].
#[derive(Debug, Clone, Copy)]
pub struct KgFactInput<'a> {
    pub subject: &'a str,
    pub predicate: &'a str,
    pub object: &'a str,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub source_document_id: Option<&'a str>,
    pub confidence: Option<f64>,
    pub metadata_json: Option<&'a str>,
}

/// Fields for [`Store::kg_supersede`].
#[derive(Debug, Clone, Copy)]
pub struct KgSupersedeInput<'a> {
    pub subject: &'a str,
    pub predicate: &'a str,
    pub old_object: &'a str,
    pub new_object: &'a str,
    pub at: Option<DateTime<Utc>>,
    pub source_document_id: Option<&'a str>,
    pub confidence: Option<f64>,
}

impl Store {
    /// Insert a temporal fact. Idempotent for open active SPO matches (returns existing).
    ///
    /// Validity is half-open `[valid_from, valid_to)`. Rejects inverted windows.
    pub fn kg_add(&self, fact: KgFactInput<'_>) -> Result<KgFact> {
        let KgFactInput {
            subject,
            predicate,
            object,
            valid_from,
            valid_to,
            source_document_id,
            confidence,
            metadata_json,
        } = fact;
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

        let mut w = self.write()?;

        // Idempotent: same open active SPO already present.
        for row in w.find_by_eq("kg_facts", "subject", &subject)? {
            if is_open_active_spo(&row, &predicate, &object) {
                return rows::kg_fact(&row);
            }
        }

        let id = Uuid::new_v4().to_string();
        let conf = confidence.unwrap_or(1.0);
        let rec = RecBuilder::new()
            .id(&id)
            .str("subject", &subject)
            .str("predicate", &predicate)
            .str("object", &object)
            .opt_string("valid_from", valid_from.map(format_ts))
            .opt_string("valid_to", valid_to.map(format_ts))
            .str("status", "active")
            .opt_str("superseded_by", None)
            .opt_string("source_document_id", source_document_id.clone())
            .opt_string("source", source_document_id)
            .f64("confidence", conf)
            .str("metadata_json", &metadata)
            .str("created_at", &now_s)
            .str("updated_at", &now_s)
            .opt_str("invalidated_at", None)
            .build();
        w.upsert("kg_facts", rec)?;

        load_fact_locked(&w, &id)?.ok_or_else(|| AppError::db("kg_add: insert not readable"))
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
        Ok(self
            .kg_query_page(subject, predicate, object, at_time, None)?
            .0)
    }

    /// Facts matching the filters, at most `limit` of them in query order,
    /// plus the total number of matches.
    pub fn kg_query_page(
        &self,
        subject: Option<&str>,
        predicate: Option<&str>,
        object: Option<&str>,
        at_time: Option<DateTime<Utc>>,
        limit: Option<usize>,
    ) -> Result<(Vec<KgFact>, usize)> {
        let subject = opt_trim(subject);
        let predicate = predicate
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase().replace(' ', "_"));
        let object = opt_trim(object);

        let r = self.read()?;
        let candidate_rows: Vec<lin::Row> = if let Some(ref s) = subject {
            r.find_by_eq("kg_facts", "subject", s)?
        } else if let Some(ref p) = predicate {
            r.find_by_eq("kg_facts", "predicate", p)?
        } else if at_time.is_none() {
            r.find_by_eq("kg_facts", "status", "active")?
        } else {
            r.filter_map_rows("kg_facts", |row| Ok(Some(row.clone())))?
        };

        let mut matches = Vec::new();
        for row in &candidate_rows {
            if let Some(ref s) = subject {
                if cell_str(row, "subject") != Some(s.as_str()) {
                    continue;
                }
            }
            if let Some(ref p) = predicate {
                if cell_str(row, "predicate") != Some(p.as_str()) {
                    continue;
                }
            }
            if let Some(ref o) = object {
                if cell_str(row, "object") != Some(o.as_str()) {
                    continue;
                }
            }
            let fact = rows::kg_fact(row)?;
            if let Some(t) = at_time {
                let starts_ok = fact.valid_from.is_none_or(|vf| vf <= t);
                let ends_ok = fact.valid_to.is_none_or(|vt| vt > t);
                if !starts_ok || !ends_ok {
                    continue;
                }
            } else if fact.status != "active" {
                continue;
            }
            matches.push(fact);
        }

        sort_kg_facts(&mut matches);
        let total = matches.len();
        if let Some(lim) = limit {
            matches.truncate(lim);
        }
        Ok((matches, total))
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

        self.with_tx(|tx| {
            let mut ids: Vec<String> = Vec::new();
            for row in tx.find_by_eq("kg_facts", "subject", &subject)? {
                if !is_open_active_spo(&row, &predicate, &object) {
                    continue;
                }
                if let Some(raw) = cell_nonempty_string(&row, "valid_from") {
                    let vf = parse_ts(&raw)?;
                    if ended < vf {
                        return Err(AppError::config(format!(
                            "kg_invalidate: ended={ended_s} is before valid_from={raw}"
                        )));
                    }
                }
                ids.push(cell_string(&row, "id"));
            }

            if ids.is_empty() {
                return Ok(Vec::new());
            }

            for id in &ids {
                tx.update_where(
                    "kg_facts",
                    lin::query::pred::eq("id", id.as_str()),
                    RecBuilder::new()
                        .str("valid_to", &ended_s)
                        .str("status", "invalidated")
                        .str("invalidated_at", &ended_s)
                        .str("updated_at", &now_s)
                        .build(),
                )?;
            }

            let mut out = Vec::with_capacity(ids.len());
            for id in &ids {
                if let Some(f) = load_fact_locked(tx, id)? {
                    out.push(f);
                }
            }
            Ok(out)
        })
    }

    /// Atomically replace an open fact's object at a shared boundary instant.
    ///
    /// Closes matching `(subject, predicate, old_object)` with `valid_to = at` and
    /// `status = 'superseded'`, then opens `(subject, predicate, new_object)` with
    /// `valid_from = at`. Point-in-time queries at `at` return only the successor
    /// (half-open upper bound).
    ///
    /// If no open old fact exists, still opens the successor (degrades to add).
    pub fn kg_supersede(&self, change: KgSupersedeInput<'_>) -> Result<KgFact> {
        let KgSupersedeInput {
            subject,
            predicate,
            old_object,
            new_object,
            at,
            source_document_id,
            confidence,
        } = change;
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

        self.with_tx(|tx| {
            let subject_rows = tx.find_by_eq("kg_facts", "subject", &subject)?;

            let mut old_ids: Vec<String> = Vec::new();
            let mut existing_new_id: Option<String> = None;

            for row in &subject_rows {
                if is_open_active_spo(row, &predicate, &old_object) {
                    if let Some(raw) = cell_nonempty_string(row, "valid_from") {
                        let vf = parse_ts(&raw)?;
                        if at < vf {
                            return Err(AppError::config(format!(
                                "kg_supersede: at={at_s} is before valid_from={raw}"
                            )));
                        }
                    }
                    old_ids.push(cell_string(row, "id"));
                }
                if existing_new_id.is_none() && is_open_active_spo(row, &predicate, &new_object) {
                    existing_new_id = Some(cell_string(row, "id"));
                }
            }

            let creating_new = existing_new_id.is_none();
            let new_id = existing_new_id.unwrap_or_else(|| Uuid::new_v4().to_string());

            for id in &old_ids {
                tx.update_where(
                    "kg_facts",
                    lin::query::pred::eq("id", id.as_str()),
                    RecBuilder::new()
                        .str("valid_to", &at_s)
                        .str("status", "superseded")
                        .str("superseded_by", &new_id)
                        .str("updated_at", &now_s)
                        .build(),
                )?;
            }

            if creating_new {
                let rec = RecBuilder::new()
                    .id(&new_id)
                    .str("subject", &subject)
                    .str("predicate", &predicate)
                    .str("object", &new_object)
                    .str("valid_from", &at_s)
                    .opt_str("valid_to", None)
                    .str("status", "active")
                    .opt_str("superseded_by", None)
                    .opt_string("source_document_id", source_document_id.clone())
                    .opt_string("source", source_document_id)
                    .f64("confidence", conf)
                    .str("metadata_json", "{}")
                    .str("created_at", &now_s)
                    .str("updated_at", &now_s)
                    .opt_str("invalidated_at", None)
                    .build();
                tx.upsert("kg_facts", rec)?;
            }

            load_fact_locked(tx, &new_id)?
                .ok_or_else(|| AppError::db("kg_supersede: new fact not readable"))
        })
    }

    /// Chronological facts for a subject (any status), ordered by `valid_from`.
    pub fn kg_timeline(&self, subject: &str) -> Result<Vec<KgFact>> {
        let subject = require_nonempty(subject, "subject")?;
        let r = self.read()?;
        let mut out: Vec<KgFact> = r
            .find_by_eq("kg_facts", "subject", &subject)?
            .iter()
            .map(rows::kg_fact)
            .collect::<Result<Vec<_>>>()?;
        sort_kg_facts(&mut out);
        Ok(out)
    }

    /// Aggregate counts and distinct relationship types.
    pub fn kg_stats(&self) -> Result<KgStats> {
        let r = self.read()?;
        let mut total_facts = 0u64;
        let mut active_facts = 0u64;
        let mut invalidated_facts = 0u64;
        let mut superseded_facts = 0u64;
        let mut subjects: HashSet<String> = HashSet::new();
        let mut predicates: BTreeSet<String> = BTreeSet::new();

        r.scan_each("kg_facts", |row| {
            total_facts += 1;
            match cell_str(row, "status").unwrap_or("active") {
                "active" | "" => active_facts += 1,
                "invalidated" => invalidated_facts += 1,
                "superseded" => superseded_facts += 1,
                _ => {}
            }
            if let Some(s) = cell_str(row, "subject") {
                subjects.insert(s.to_string());
            }
            if let Some(p) = cell_str(row, "predicate") {
                predicates.insert(p.to_string());
            }
            Ok(true)
        })?;

        let distinct_subjects = subjects.len() as u64;
        let distinct_predicates = predicates.len() as u64;
        let relationship_types = predicates.into_iter().collect();

        Ok(KgStats {
            total_facts,
            active_facts,
            invalidated_facts,
            superseded_facts,
            distinct_subjects,
            distinct_predicates,
            relationship_types,
        })
    }

    /// Load one fact by primary key.
    pub fn kg_get(&self, fact_id: &str) -> Result<Option<KgFact>> {
        let r = self.read()?;
        load_fact_locked(&r, fact_id)
    }
}

fn is_open_active_spo(row: &lin::Row, predicate: &str, object: &str) -> bool {
    let status = cell_str(row, "status").unwrap_or("active");
    (status == "active" || status.is_empty())
        && cell_nonempty_string(row, "valid_to").is_none()
        && cell_str(row, "predicate") == Some(predicate)
        && cell_str(row, "object") == Some(object)
}

fn sort_kg_facts(facts: &mut [KgFact]) {
    // ORDER BY valid_from ASC NULLS FIRST, created_at ASC, id ASC
    facts.sort_by(|a, b| {
        a.valid_from
            .cmp(&b.valid_from)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn load_fact_locked(r: &impl LinReader, id: &str) -> Result<Option<KgFact>> {
    r.get_by_id("kg_facts", id)?
        .as_ref()
        .map(rows::kg_fact)
        .transpose()
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

fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    parse_flexible_timestamp(s)
        .ok_or_else(|| AppError::db(format!("invalid timestamp value: {}", s.trim())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn open_temp() -> Store {
        crate::testkit::temp_store()
    }

    fn ts(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn kg_query_page_limits_items_and_reports_the_total() {
        let store = open_temp();
        for object in ["Acme", "Globex", "Initech"] {
            store
                .kg_add(KgFactInput {
                    subject: "Dana",
                    predicate: "worked_at",
                    object,
                    valid_from: None,
                    valid_to: None,
                    source_document_id: None,
                    confidence: None,
                    metadata_json: None,
                })
                .expect("add");
        }
        let (items, total) = store
            .kg_query_page(Some("Dana"), None, None, None, Some(2))
            .expect("page");
        assert_eq!(items.len(), 2);
        assert_eq!(total, 3);
        let all = store.kg_query(Some("Dana"), None, None, None).expect("all");
        assert_eq!(all.len(), 3);
        let ids = |facts: &[KgFact]| facts.iter().map(|fact| fact.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&items), ids(&all[..2]));
    }

    #[test]
    fn kg_add_query_active_and_idempotent() {
        let store = open_temp();
        let f1 = store
            .kg_add(KgFactInput {
                subject: "Alice",
                predicate: "works_at",
                object: "Acme",
                valid_from: Some(ts(2020, 1, 1)),
                valid_to: None,
                source_document_id: Some("doc:1"),
                confidence: None,
                metadata_json: None,
            })
            .expect("add");
        assert_eq!(f1.subject, "Alice");
        assert_eq!(f1.predicate, "works_at");
        assert_eq!(f1.object, "Acme");
        assert_eq!(f1.status, "active");

        let again = store
            .kg_add(KgFactInput {
                subject: "Alice",
                predicate: "works_at",
                object: "Acme",
                valid_from: None,
                valid_to: None,
                source_document_id: None,
                confidence: None,
                metadata_json: None,
            })
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
            .kg_add(KgFactInput {
                subject: "Bob",
                predicate: "lives_in",
                object: "Paris",
                valid_from: Some(ts(2019, 1, 1)),
                valid_to: None,
                source_document_id: None,
                confidence: Some(0.9),
                metadata_json: None,
            })
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
            .kg_add(KgFactInput {
                subject: "Carol",
                predicate: "employer",
                object: "OldCo",
                valid_from: Some(ts(2018, 1, 1)),
                valid_to: None,
                source_document_id: None,
                confidence: None,
                metadata_json: None,
            })
            .expect("add");

        let at = ts(2024, 3, 15);
        let new_f = store
            .kg_supersede(KgSupersedeInput {
                subject: "Carol",
                predicate: "employer",
                old_object: "OldCo",
                new_object: "NewCo",
                at: Some(at),
                source_document_id: None,
                confidence: None,
            })
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
            .kg_add(KgFactInput {
                subject: "X",
                predicate: "rel_a",
                object: "Y",
                valid_from: None,
                valid_to: None,
                source_document_id: None,
                confidence: None,
                metadata_json: None,
            })
            .unwrap();
        store
            .kg_add(KgFactInput {
                subject: "X",
                predicate: "rel_b",
                object: "Z",
                valid_from: None,
                valid_to: None,
                source_document_id: None,
                confidence: None,
                metadata_json: None,
            })
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
            .kg_add(KgFactInput {
                subject: "",
                predicate: "p",
                object: "o",
                valid_from: None,
                valid_to: None,
                source_document_id: None,
                confidence: None,
                metadata_json: None
            })
            .is_err());
        assert!(store
            .kg_add(KgFactInput {
                subject: "s",
                predicate: "p",
                object: "o",
                valid_from: Some(ts(2022, 1, 1)),
                valid_to: Some(ts(2020, 1, 1)),
                source_document_id: None,
                confidence: None,
                metadata_json: None
            })
            .is_err());
    }
}
