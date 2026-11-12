//! Committed successful native index-run measurements, separate from revision facts.
use super::*;
use crate::db::lin_engine::{
    cell_bool, cell_i64, cell_i64_opt, cell_string, format_db_timestamp, LinReader, LinWriter,
    RecBuilder,
};
use chrono::Utc;
use rag_mcp_core::IndexStageTimings;

pub(super) fn record(
    w: &mut impl LinWriter,
    revision: i64,
    created: bool,
    m: &IndexStageTimings,
) -> Result<()> {
    let mut max_seq: i64 = 0;
    w.scan_each("code_index_runs", |row| {
        if let Some(s) = cell_i64_opt(row, "seq")
            && s > max_seq
        {
            max_seq = s;
        }
        Ok(true)
    })?;
    let seq = max_seq
        .checked_add(1)
        .ok_or_else(|| AppError::db("code_index_runs sequence overflow"))?;
    let id = seq.to_string();
    let now = format_db_timestamp(Utc::now());
    w.insert_one(
        "code_index_runs",
        RecBuilder::new()
            .id(&id)
            .i64("seq", seq)
            .str("run_id", &m.run_id)
            .i64("index_revision", revision)
            .bool("revision_created", created)
            .str("recorded_at", now)
            .str("metrics_json", serde_json::to_string(m)?)
            .build(),
    )?;
    Ok(())
}
fn read(r: &impl LinReader, revision: i64, publication: bool) -> Result<Option<serde_json::Value>> {
    let mut best: Option<(i64, String, String, bool, String)> = None;
    r.scan_each("code_index_runs", |row| {
        if cell_i64(row, "index_revision", 0) == revision {
            let created = cell_bool(row, "revision_created", false);
            if !publication || created {
                let seq = cell_i64(row, "seq", 0);
                if best.as_ref().is_none_or(|b| seq > b.0) {
                    best = Some((
                        seq,
                        cell_string(row, "run_id"),
                        cell_string(row, "recorded_at"),
                        created,
                        cell_string(row, "metrics_json"),
                    ));
                }
            }
        }
        Ok(true)
    })?;
    let Some((seq, id, at, created, json)) = best else {
        return Ok(None);
    };
    Ok(Some(decode(seq, &id, &at, created, revision, &json)?))
}
pub(super) fn decode(
    seq: i64,
    id: &str,
    at: &str,
    created: bool,
    revision: i64,
    json: &str,
) -> Result<serde_json::Value> {
    let metrics: IndexStageTimings = serde_json::from_str(json)?;
    let values = [
        metrics.lock_wait_ms,
        metrics.source_load_decode_ms,
        metrics.cache_parse_serialize_ms,
        metrics.cache_replace_ms,
        metrics.revision_publish_ms,
        metrics.transaction_body_ms,
        metrics.before_metrics_write_ms,
    ];
    let examined = metrics.cache_hits.checked_add(metrics.cache_misses);
    let loaded = examined.and_then(|n| n.checked_add(metrics.unsupported_documents));
    if loaded != Some(metrics.documents_loaded)
        || metrics.parse_error_documents > examined.unwrap_or(0)
        || metrics.contract_version != 1
        || metrics.run_id != id
        || values.iter().any(|v| !v.is_finite() || *v < 0.0)
        || metrics.commit_included
        || metrics.metrics_write_included
        || metrics.embedding_fts_included
    {
        return Err(AppError::Invalid("invalid index-run timing record".into()));
    }
    Ok(
        serde_json::json!({"seq":seq,"run_id":id,"recorded_at":at,"revision_created":created,"index_rev":revision,"metrics":metrics}),
    )
}
pub(super) fn status(r: &impl LinReader, revision: Option<i64>) -> serde_json::Value {
    let base = |reason: &str| serde_json::json!({"available":false,"reason":reason,"scope":"corpus_global","successful_committed_runs_only":true,"commit_measured":false});
    let Some(revision) = revision else {
        return base("no_published_revision");
    };
    if !r.has_collection("code_index_runs") {
        return base("run_ledger_unavailable");
    }
    match (read(r, revision, true), read(r, revision, false)) {
        (Ok(publication), Ok(latest)) => {
            serde_json::json!({"available":publication.is_some()||latest.is_some(),"reason":if publication.is_none()&&latest.is_none(){Some("no_measured_run_for_revision")}else{None},"scope":"corpus_global","index_rev":revision,"publication_run":publication,"latest_successful_run":latest,"successful_committed_runs_only":true,"commit_measured":false,"metrics_write_measured":false,"embedding_fts_measured":false,"mcp_queue_wait_measured":false})
        }
        _ => base("invalid_or_unreadable_run_metrics"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    fn setup() -> (tempfile::TempDir, Store) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("timings.lin")).unwrap();
        (root, store)
    }
    fn put(store: &Store, text: &str) {
        store
            .upsert_document(&Document {
                id: "a".into(),
                uri: "doc://a".into(),
                source_file: Some("/p/a.py".into()),
                content: text.into(),
                wing: Some("alpha".into()),
                ..Default::default()
            })
            .unwrap();
    }
    #[test]
    fn publication_and_noop_runs_are_distinct_persistent_and_do_not_change_revision() {
        let (root, store) = setup();
        put(&store, "def work(): pass\n");
        let first = store.index_code().unwrap();
        let a = first.stage_timings.as_ref().unwrap();
        assert_eq!(a.cache_misses, 1);
        assert_eq!(a.documents_loaded, 1);
        for n in [
            a.lock_wait_ms,
            a.source_load_decode_ms,
            a.cache_parse_serialize_ms,
            a.cache_replace_ms,
            a.revision_publish_ms,
            a.transaction_body_ms,
            a.before_metrics_write_ms,
        ] {
            assert!(n.is_finite() && n >= 0.0);
        }
        assert!(a.before_metrics_write_ms >= a.transaction_body_ms);
        assert!(!a.commit_included);
        let second = store.index_code().unwrap();
        let b = second.stage_timings.as_ref().unwrap();
        assert!(!second.revision_created);
        assert_eq!(second.index_revision, first.index_revision);
        assert_eq!(b.cache_hits, 1);
        assert_ne!(a.run_id, b.run_id);
        let report = status(&*store.read().unwrap(), first.index_revision);
        assert_eq!(report["publication_run"]["run_id"], a.run_id);
        assert_eq!(report["latest_successful_run"]["run_id"], b.run_id);
        put(&store, "def revised(): pass\n");
        let third = store.index_code().unwrap();
        assert_ne!(third.index_revision, first.index_revision);
        assert_eq!(status(&*store.read().unwrap(), first.index_revision), report);
        drop(store);
        let reopened = Store::open(&root.path().join("timings.lin")).unwrap();
        let persisted = status(&*reopened.read().unwrap(), first.index_revision);
        assert_eq!(persisted, report);
    }
    #[test]
    fn measurement_write_failure_rolls_back_cache_and_publication() {
        let (_root, store) = setup();
        put(&store, "def before(): pass\n");
        let first = store.index_code().unwrap();
        let before = {
            let r = store.read().unwrap();
            let row = r.get_by_id("code_index_documents", "a").unwrap().unwrap();
            cell_string(&row, "payload")
        };
        store
            .write()
            .unwrap()
            .insert_one(
                "code_index_runs",
                RecBuilder::new()
                    .id(i64::MAX.to_string())
                    .i64("seq", i64::MAX)
                    .str("run_id", "forced-overflow")
                    .i64("index_revision", -1)
                    .bool("revision_created", false)
                    .str("recorded_at", format_db_timestamp(Utc::now()))
                    .str("metrics_json", "{}")
                    .build(),
            )
            .unwrap();
        put(&store, "def after(): pass\n");
        assert!(store.index_code().is_err());
        let r = store.read().unwrap();
        let revision = crate::analysis_tasks::latest_code_index_rev(&*r).unwrap();
        assert_eq!(revision, first.index_revision);
        let payload = {
            let row = r.get_by_id("code_index_documents", "a").unwrap().unwrap();
            cell_string(&row, "payload")
        };
        assert_eq!(payload, before);
        let count = r.count_rows("code_index_runs", None).unwrap();
        assert_eq!(count, 2);
    }
    #[test]
    fn missing_legacy_and_corrupt_records_are_explicit_without_fabricated_timings() {
        let (_root, store) = setup();
        put(&store, "def work(): pass\n");
        let first = store.index_code().unwrap();
        store
            .write()
            .unwrap()
            .delete_where("code_index_runs", crate::db::lin_engine::pred_all())
            .unwrap();
        let empty = status(&*store.read().unwrap(), first.index_revision);
        assert_eq!(empty["available"], false);
        assert_eq!(empty["reason"], "no_measured_run_for_revision");
        store.index_code().unwrap();
        let measured = status(&*store.read().unwrap(), first.index_revision);
        assert!(measured["publication_run"].is_null());
        assert!(measured["latest_successful_run"].is_object());
        store
            .write()
            .unwrap()
            .update_where(
                "code_index_runs",
                crate::db::lin_engine::pred_all(),
                RecBuilder::new().str("metrics_json", "bad metrics").build(),
            )
            .unwrap();
        let corrupt = status(&*store.read().unwrap(), first.index_revision);
        assert_eq!(corrupt["available"], false);
        assert_eq!(corrupt["reason"], "invalid_or_unreadable_run_metrics");
        assert!(!corrupt.to_string().contains("bad metrics"));
        store
            .drop_collection_for_test("code_index_runs")
            .unwrap();
        assert_eq!(
            status(&*store.read().unwrap(), first.index_revision)["reason"],
            "run_ledger_unavailable"
        );
    }
    #[test]
    fn schema_24_upgrade_preserves_revision_and_global_metrics_are_not_project_timings() {
        let (root, store) = setup();
        put(&store, "def work(): pass\n");
        store
            .upsert_document(&Document {
                id: "b".into(),
                uri: "doc://b".into(),
                source_file: Some("/p/b.py".into()),
                content: "def other(): pass\n".into(),
                wing: Some("beta".into()),
                ..Default::default()
            })
            .unwrap();
        let original = store.index_code().unwrap();
        store
            .drop_collection_for_test("code_index_runs")
            .unwrap();
        {
            let mut w = store.write().unwrap();
            w.update_where(
                "schema_version",
                lin::query::pred::eq("version", 25_i64),
                RecBuilder::new().i64("version", 24).build(),
            )
            .unwrap();
            crate::db::schema::upsert_meta(&mut *w, "schema_version", "24").unwrap();
        }
        drop(store);
        let upgraded = Store::open(&root.path().join("timings.lin")).unwrap();
        assert_eq!(
            status(&*upgraded.read().unwrap(), original.index_revision)["reason"],
            "no_measured_run_for_revision"
        );
        let report = upgraded.index_code().unwrap();
        assert_eq!(report.index_revision, original.index_revision);
        assert!(!report.revision_created);
        assert_eq!(report.unchanged, 2);
        let observed = upgraded
            .code_index_status(
                &super::super::status::StatusQuery {
                    project: Some("alpha".into()),
                    ..Default::default()
                },
                &crate::Config::for_tests(),
            )
            .unwrap();
        assert_eq!(observed["versions"]["store_schema"], 25);
        assert_eq!(observed["coverage"]["indexable_documents"], 1);
        assert_eq!(observed["stage_timings"]["scope"], "corpus_global");
        assert_eq!(
            observed["stage_timings"]["latest_successful_run"]["metrics"]["documents_loaded"],
            2
        );
        assert!(observed["stage_timings"]["publication_run"].is_null());
    }
}
