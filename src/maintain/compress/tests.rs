use super::*;
use crate::models::SearchMode;
use crate::testkit::{test_config, SeedDoc};

fn seed_doc(store: &Store, id: &str, title: &str, content: &str, layer: &str, emb: Vec<f32>) {
    SeedDoc::new(id, title, content)
        .layer(layer)
        .embedding(emb)
        .hashed()
        .insert(store);
}

fn assert_finalization_failure_audit(
    store: &Store,
    report: &MaintainCompressReport,
    expected_error: &str,
    expected_durable: bool,
) {
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].contains("FTS_FINALIZATION_FAILED"));
    if expected_durable {
        assert!(report.errors[0].contains("durable corpus work was committed"));
    } else {
        assert!(report.errors[0].contains("no durable corpus mutation was detected"));
    }
    assert!(report.errors[0].contains("retryable=true"));
    assert!(report.errors[0].contains(expected_error));

    let entries = store.ops_log().list_ops_log(10).unwrap();
    let summary_id = report.ops_log_id.as_deref().expect("terminal summary id");
    let summary = entries
        .iter()
        .find(|entry| entry.id == summary_id)
        .expect("terminal summary ops_log row");
    assert_eq!(summary.op, "maintain_compress");
    let summary_payload: serde_json::Value = serde_json::from_str(&summary.payload_json).unwrap();
    assert_eq!(summary_payload["errors"].as_array().unwrap().len(), 1);
    assert!(summary_payload["errors"][0]
        .as_str()
        .unwrap()
        .contains(expected_error));

    let failure = entries
        .iter()
        .find(|entry| entry.op == "fts_finalization_failed")
        .expect("durable FTS failure ops_log row");
    let failure_payload: serde_json::Value = serde_json::from_str(&failure.payload_json).unwrap();
    assert_eq!(failure_payload["operation"], "maintain_compress");
    assert_eq!(failure_payload["code"], "FTS_FINALIZATION_FAILED");
    assert_eq!(
        failure_payload["durable_mutation_committed"],
        expected_durable
    );
    assert_eq!(failure_payload["retryable"], true);
    assert_eq!(failure_payload["dirty_marker_written"], true);
}

#[test]
fn level_out_of_range_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.duckdb");
    let store = crate::testkit::open_test_store(&path).unwrap();
    let cfg = test_config(path, 8);
    let mut opts = CompressOptions::from_config(&cfg);
    opts.level = 9;
    assert!(maintain_compress(&store, &cfg, &opts).is_err());
}

#[test]
fn l0_dry_run_and_apply() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l0.duckdb");
    let store = crate::testkit::open_test_store(&path).unwrap();
    let cfg = test_config(path, 8);
    seed_doc(&store, "d1", "A", "hello world body", "wiki", vec![0.1; 8]);

    let mut opts = CompressOptions::from_config(&cfg);
    opts.level = 0;
    opts.dry_run = true;
    let dry = maintain_compress(&store, &cfg, &opts).unwrap();
    assert!(dry.dry_run);
    assert!(dry.l0.as_ref().unwrap().dry_run);
    assert_eq!(dry.docs_removed, 0);
    assert!(dry.ops_log_id.is_some());

    opts.dry_run = false;
    let applied = maintain_compress(&store, &cfg, &opts).unwrap();
    assert!(!applied.dry_run);
    assert!(applied.l0.as_ref().unwrap().checkpointed);
    assert!(store
        .ops_log()
        .list_ops_log(10)
        .unwrap()
        .iter()
        .any(|o| o.op == "maintain_compress"));
}

#[test]
fn l1_merges_exact_hash_wiki_keeps_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l1.duckdb");
    let store = crate::testkit::open_test_store(&path).unwrap();
    let cfg = test_config(path, 8);
    let body = "identical wiki body for compress";
    seed_doc(&store, "w1", "Keep", body, "wiki", vec![0.2; 8]);
    // slight delay not needed; ids order: pick_canonical uses created_at then id
    seed_doc(&store, "w2", "Dup", body, "wiki", vec![0.2; 8]);

    let mut opts = CompressOptions::from_config(&cfg);
    opts.level = 1;
    opts.dry_run = false;
    opts.allow_raw_delete = false;
    let r = maintain_compress(&store, &cfg, &opts).unwrap();
    assert_eq!(r.docs_removed, 1);
    assert_eq!(r.exact_dup_groups.len(), 1);
    let remaining = store.documents().list_documents().unwrap();
    assert_eq!(remaining.len(), 1);
    // oldest / smaller id kept
    assert_eq!(remaining[0].id, "w1");
    let generation_after_compress = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(!generation_after_compress.dirty);
    assert_eq!(
        generation_after_compress.index_generation,
        Some(generation_after_compress.chunks_generation)
    );

    let hits = crate::db::search::search(
        &store,
        &crate::db::search::SearchQuery {
            mode: SearchMode::Lex,
            query_text: Some("identical wiki".into()),
            top_k: 5,
            fts_stemmer: cfg.fts_stemmer.clone(),
            ..crate::db::search::SearchQuery::default()
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    let generation_after_search = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert_eq!(generation_after_search, generation_after_compress);
}

#[test]
fn l1_raw_dups_tombstone_without_allow_raw_delete() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("raw.duckdb");
    let store = crate::testkit::open_test_store(&path).unwrap();
    let cfg = test_config(path, 8);
    let body = "raw twin content";
    seed_doc(&store, "r1", "RawA", body, "raw", vec![0.3; 8]);
    seed_doc(&store, "r2", "RawB", body, "raw", vec![0.3; 8]);

    let mut opts = CompressOptions::from_config(&cfg);
    opts.level = 1;
    opts.dry_run = false;
    opts.allow_raw_delete = false;
    let r = maintain_compress(&store, &cfg, &opts).unwrap();
    assert_eq!(r.docs_removed, 0);
    assert_eq!(r.docs_tombstoned, 1);
    assert!(r.raw_protected >= 1);
    assert!(store.documents().get_document("r1").unwrap().is_some());
    assert!(store.documents().get_document("r2").unwrap().is_some());
    let statuses: Vec<_> = store
        .documents()
        .list_documents()
        .unwrap()
        .into_iter()
        .map(|d| (d.id, d.status))
        .collect();
    // one active, one tombstone
    let active = statuses.iter().filter(|(_, s)| s != "tombstone").count();
    let tomb = statuses.iter().filter(|(_, s)| s == "tombstone").count();
    assert_eq!(active, 1);
    assert_eq!(tomb, 1);
}

#[test]
fn l1_raw_hard_delete_with_flag() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rawdel.duckdb");
    let store = crate::testkit::open_test_store(&path).unwrap();
    let cfg = test_config(path, 8);
    let body = "raw twin hard delete";
    seed_doc(&store, "r1", "RawA", body, "raw", vec![0.3; 8]);
    seed_doc(&store, "r2", "RawB", body, "raw", vec![0.3; 8]);

    let mut opts = CompressOptions::from_config(&cfg);
    opts.level = 1;
    opts.dry_run = false;
    opts.allow_raw_delete = true;
    let r = maintain_compress(&store, &cfg, &opts).unwrap();
    assert_eq!(r.docs_removed, 1);
    assert_eq!(store.documents().list_documents().unwrap().len(), 1);
}

#[test]
fn l2_lists_near_dups_without_confirm() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l2list.duckdb");
    let store = crate::testkit::open_test_store(&path).unwrap();
    let cfg = test_config(path, 8);
    // Same embedding → cosine 1.0
    let emb = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    seed_doc(
        &store,
        "a",
        "Alpha",
        "body alpha unique",
        "wiki",
        emb.clone(),
    );
    seed_doc(&store, "b", "Beta", "body beta unique", "wiki", emb);
    store
        .embedding_manifest()
        .write_embedding_manifest_from_config(&cfg)
        .unwrap();

    let mut opts = CompressOptions::from_config(&cfg);
    opts.level = 2;
    opts.dry_run = false;
    opts.confirm = false;
    opts.near_dup_threshold = 0.9;
    let r = maintain_compress(&store, &cfg, &opts).unwrap();
    assert!(!r.near_duplicates.is_empty());
    assert_eq!(r.docs_removed, 0);
    assert!(r.notes.iter().any(|n| n.contains("confirm")));
    assert_eq!(store.documents().list_documents().unwrap().len(), 2);
    assert!(r.merges.iter().any(|m| m.disposition == "listed"));
}

#[test]
fn l2_merges_near_dups_with_confirm() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l2merge.duckdb");
    let store = crate::testkit::open_test_store(&path).unwrap();
    let cfg = test_config(path, 8);
    let emb = vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    seed_doc(
        &store,
        "a",
        "Alpha",
        "body alpha unique",
        "wiki",
        emb.clone(),
    );
    seed_doc(&store, "b", "Beta", "body beta unique", "wiki", emb);
    store
        .embedding_manifest()
        .write_embedding_manifest_from_config(&cfg)
        .unwrap();

    let mut opts = CompressOptions::from_config(&cfg);
    opts.level = 2;
    opts.dry_run = false;
    opts.confirm = true;
    opts.near_dup_threshold = 0.9;
    let r = maintain_compress(&store, &cfg, &opts).unwrap();
    assert_eq!(r.docs_removed, 1);
    assert_eq!(store.documents().list_documents().unwrap().len(), 1);
    assert!(r.merges.iter().any(|m| m.action == "merge_near_dup"));
}

#[test]
fn l2_apply_fails_before_any_mutation_during_embedding_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l2-migration-incomplete.duckdb");
    let store = crate::testkit::open_test_store(&path).unwrap();
    let cfg = test_config(path, 8);
    let emb = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    seed_doc(
        &store,
        "keep",
        "Keep",
        "unique body one",
        "wiki",
        emb.clone(),
    );
    seed_doc(&store, "drop", "Drop", "unique body two", "wiki", emb);
    let migration = crate::db::store::embedding_migration_manifest(&cfg);
    store
        .embedding_manifest()
        .set_embedding_manifest(&migration)
        .unwrap();
    // Compare against the persisted round-trip: DuckDB keeps microseconds,
    // while `Utc::now()` carries nanoseconds on Linux.
    let manifest_before =
        serde_json::to_value(store.embedding_manifest().get_embedding_manifest().unwrap()).unwrap();
    let stats_before = store.maintenance().stats().unwrap();
    let documents_before =
        serde_json::to_value(store.documents().list_documents().unwrap()).unwrap();
    let chunks_before =
        serde_json::to_value(store.chunks().all_chunks_with_embeddings().unwrap()).unwrap();
    assert!(store.ops_log().list_ops_log(10).unwrap().is_empty());

    let mut opts = CompressOptions::from_config(&cfg);
    opts.level = 2;
    opts.dry_run = false;
    opts.confirm = true;
    opts.allow_raw_delete = true;
    opts.near_dup_threshold = 0.9;
    let error = maintain_compress(&store, &cfg, &opts).unwrap_err();

    assert!(matches!(error, AppError::Embeddings(_)));
    assert!(error.to_string().contains("incomplete corpus migration"));
    assert_eq!(store.maintenance().stats().unwrap(), stats_before);
    assert_eq!(
        serde_json::to_value(store.documents().list_documents().unwrap()).unwrap(),
        documents_before
    );
    assert_eq!(
        serde_json::to_value(store.chunks().all_chunks_with_embeddings().unwrap()).unwrap(),
        chunks_before
    );
    assert!(store.ops_log().list_ops_log(10).unwrap().is_empty());
    assert_eq!(
        serde_json::to_value(store.embedding_manifest().get_embedding_manifest().unwrap()).unwrap(),
        manifest_before
    );
    assert_eq!(
        manifest_before["content_fingerprint"],
        serde_json::to_value(migration.content_fingerprint).unwrap()
    );
}

#[test]
fn l1_dry_run_does_not_delete() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l1dry.duckdb");
    let store = crate::testkit::open_test_store(&path).unwrap();
    let cfg = test_config(path, 8);
    let body = "dup dry run body";
    seed_doc(&store, "w1", "A", body, "wiki", vec![0.1; 8]);
    seed_doc(&store, "w2", "B", body, "wiki", vec![0.1; 8]);

    let mut opts = CompressOptions::from_config(&cfg);
    opts.level = 1;
    opts.dry_run = true;
    let r = maintain_compress(&store, &cfg, &opts).unwrap();
    assert!(r.dry_run);
    assert_eq!(r.docs_removed, 1); // planned
    assert_eq!(store.documents().list_documents().unwrap().len(), 2);
    assert!(r.merges.iter().any(|m| m.disposition == "planned"));
    assert!(store
        .ops_log()
        .list_ops_log(5)
        .unwrap()
        .iter()
        .any(|o| o.op == "maintain_compress_dry_run"));
}

#[test]
fn final_fts_failure_returns_durable_merge_report_and_terminal_ops_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("final-fts-failure.duckdb");
    let store = crate::testkit::open_test_store(&path).unwrap();
    let mut cfg = test_config(path, 8);
    let body = "same durable compression body";
    seed_doc(&store, "keep", "Keep", body, "wiki", vec![0.2; 8]);
    seed_doc(&store, "drop", "Drop", body, "wiki", vec![0.2; 8]);
    store.maintenance().ensure_fts(&cfg.fts_stemmer).unwrap();
    cfg.fts_stemmer = db::fts::test_fault::fail("compress");

    let mut opts = CompressOptions::from_config(&cfg);
    opts.level = 1;
    opts.dry_run = false;
    let report = maintain_compress(&store, &cfg, &opts)
        .expect("aggregate report survives final FTS failure");

    assert_eq!(report.docs_removed, 1);
    assert_eq!(store.documents().list_documents().unwrap().len(), 1);
    assert_finalization_failure_audit(
        &store,
        &report,
        "injected compress FTS finalization failure",
        true,
    );
    let generation = {
        let conn = store.lock().unwrap();
        db::fts_generation_state(&conn).unwrap()
    };
    assert!(generation.dirty, "next lexical read must retry FTS");

    let mut legacy_payload = serde_json::to_value(&report).unwrap();
    legacy_payload.as_object_mut().unwrap().remove("errors");
    let decoded: MaintainCompressReport = serde_json::from_value(legacy_payload).unwrap();
    assert!(decoded.errors.is_empty());
}

#[test]
fn final_fts_panic_returns_durable_merge_report_and_terminal_ops_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("final-fts-panic.duckdb");
    let store = crate::testkit::open_test_store(&path).unwrap();
    let mut cfg = test_config(path, 8);
    let body = "same durable compression panic body";
    seed_doc(&store, "keep", "Keep", body, "wiki", vec![0.2; 8]);
    seed_doc(&store, "drop", "Drop", body, "wiki", vec![0.2; 8]);
    store.maintenance().ensure_fts(&cfg.fts_stemmer).unwrap();
    cfg.fts_stemmer = db::fts::test_fault::panic("compress");

    let mut opts = CompressOptions::from_config(&cfg);
    opts.level = 1;
    opts.dry_run = false;
    let report =
        maintain_compress(&store, &cfg, &opts).expect("aggregate report survives final FTS panic");

    assert_eq!(report.docs_removed, 1);
    assert_eq!(store.documents().list_documents().unwrap().len(), 1);
    assert_finalization_failure_audit(
        &store,
        &report,
        "maintain_compress FTS finalization panicked",
        true,
    );
    let generation = {
        let conn = store.lock().unwrap();
        db::fts_generation_state(&conn).unwrap()
    };
    assert!(generation.dirty, "next lexical read must retry FTS");
}

#[test]
fn final_fts_panic_without_merge_preserves_noop_report_and_audit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("final-fts-panic-noop.duckdb");
    let store = crate::testkit::open_test_store(&path).unwrap();
    let mut cfg = test_config(path, 8);
    seed_doc(
        &store,
        "only",
        "Only",
        "unique compression body",
        "wiki",
        vec![0.2; 8],
    );
    store.maintenance().ensure_fts(&cfg.fts_stemmer).unwrap();
    cfg.fts_stemmer = db::fts::test_fault::panic("compress");

    let mut opts = CompressOptions::from_config(&cfg);
    opts.level = 1;
    opts.dry_run = false;
    let report =
        maintain_compress(&store, &cfg, &opts).expect("no-op aggregate survives final FTS panic");

    assert_eq!(report.docs_removed, 0);
    assert_eq!(report.docs_tombstoned, 0);
    assert_eq!(store.documents().list_documents().unwrap().len(), 1);
    assert_finalization_failure_audit(
        &store,
        &report,
        "maintain_compress FTS finalization panicked",
        false,
    );
}
