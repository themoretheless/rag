//! Committed successful native index-run measurements, separate from revision facts.
use super::*;
use duckdb::{Connection, OptionalExt};
use rag_mcp_core::IndexStageTimings;

pub(super) fn record(
    conn: &Connection,
    revision: i64,
    created: bool,
    m: &IndexStageTimings,
) -> Result<()> {
    let seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq),0)+1 FROM code_index_runs",
        [],
        |r| r.get(0),
    )?;
    conn.execute("INSERT INTO code_index_runs(seq,run_id,index_revision,revision_created,metrics_json) VALUES (?,?,?,?,?)",params![seq,m.run_id,revision,created,serde_json::to_string(m)?])?;
    Ok(())
}
fn read(conn: &Connection, revision: i64, publication: bool) -> Result<Option<serde_json::Value>> {
    let row:Option<(i64,String,String,bool,String)>=conn.query_row("SELECT seq,run_id,CAST(recorded_at AS VARCHAR),revision_created,metrics_json FROM code_index_runs WHERE index_revision=? AND (?=false OR revision_created=true) ORDER BY seq DESC LIMIT 1",params![revision,publication],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let Some((seq, id, at, created, json)) = row else {
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
pub(super) fn status(conn: &Connection, revision: Option<i64>) -> serde_json::Value {
    let base = |reason: &str| serde_json::json!({"available":false,"reason":reason,"scope":"corpus_global","successful_committed_runs_only":true,"commit_measured":false});
    let Some(revision) = revision else {
        return base("no_published_revision");
    };
    let exists=conn.query_row("SELECT COUNT(*) FROM information_schema.tables WHERE table_name='code_index_runs' AND table_schema='main'",[],|r|r.get::<_,i64>(0));
    if !matches!(exists, Ok(1)) {
        return base("run_ledger_unavailable");
    }
    match (read(conn, revision, true), read(conn, revision, false)) {
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
        let store = Store::open(&root.path().join("timings.duckdb")).unwrap();
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
        let report = status(&store.lock().unwrap(), first.index_revision);
        assert_eq!(report["publication_run"]["run_id"], a.run_id);
        assert_eq!(report["latest_successful_run"]["run_id"], b.run_id);
        put(&store, "def revised(): pass\n");
        let third = store.index_code().unwrap();
        assert_ne!(third.index_revision, first.index_revision);
        assert_eq!(status(&store.lock().unwrap(), first.index_revision), report);
        drop(store);
        let reopened = Store::open(&root.path().join("timings.duckdb")).unwrap();
        let persisted = status(&reopened.lock().unwrap(), first.index_revision);
        assert_eq!(persisted, report);
    }
    #[test]
    fn measurement_write_failure_rolls_back_cache_and_publication() {
        let (_root, store) = setup();
        put(&store, "def before(): pass\n");
        let first = store.index_code().unwrap();
        let before: String = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM code_index_documents WHERE document_id='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        store.lock().unwrap().execute("INSERT INTO code_index_runs(seq,run_id,index_revision,revision_created,metrics_json) VALUES (9223372036854775807,'forced-overflow',-1,false,'{}')",[]).unwrap();
        put(&store, "def after(): pass\n");
        assert!(store.index_code().is_err());
        let conn = store.lock().unwrap();
        let revision: Option<i64> = conn
            .query_row("SELECT MAX(id) FROM code_index_revisions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(revision, first.index_revision);
        let payload: String = conn
            .query_row(
                "SELECT payload FROM code_index_documents WHERE document_id='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(payload, before);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM code_index_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
    #[test]
    fn missing_legacy_and_corrupt_records_are_explicit_without_fabricated_timings() {
        let (_root, store) = setup();
        put(&store, "def work(): pass\n");
        let first = store.index_code().unwrap();
        store
            .lock()
            .unwrap()
            .execute("DELETE FROM code_index_runs", [])
            .unwrap();
        let empty = status(&store.lock().unwrap(), first.index_revision);
        assert_eq!(empty["available"], false);
        assert_eq!(empty["reason"], "no_measured_run_for_revision");
        store.index_code().unwrap();
        let measured = status(&store.lock().unwrap(), first.index_revision);
        assert!(measured["publication_run"].is_null());
        assert!(measured["latest_successful_run"].is_object());
        store
            .lock()
            .unwrap()
            .execute("UPDATE code_index_runs SET metrics_json='bad metrics'", [])
            .unwrap();
        let corrupt = status(&store.lock().unwrap(), first.index_revision);
        assert_eq!(corrupt["available"], false);
        assert_eq!(corrupt["reason"], "invalid_or_unreadable_run_metrics");
        assert!(!corrupt.to_string().contains("bad metrics"));
        store
            .lock()
            .unwrap()
            .execute("DROP TABLE code_index_runs", [])
            .unwrap();
        assert_eq!(
            status(&store.lock().unwrap(), first.index_revision)["reason"],
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
        {
            let conn = store.lock().unwrap();
            conn.execute("DROP TABLE code_index_runs", []).unwrap();
            conn.execute("UPDATE schema_version SET version=24 WHERE version=25", [])
                .unwrap();
            conn.execute("UPDATE meta SET value='24' WHERE key='schema_version'", [])
                .unwrap();
        }
        drop(store);
        let upgraded = Store::open(&root.path().join("timings.duckdb")).unwrap();
        assert_eq!(
            status(&upgraded.lock().unwrap(), original.index_revision)["reason"],
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
