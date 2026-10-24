use super::*;
use crate::db;
use crate::embeddings::MockEmbedder;
use crate::llm::ChatClient;
use crate::models::SearchMode;
use crate::testkit::SeedDoc;
use crate::util::content_hash;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Seeded body the failing embedder refuses.
const FAILING_TEXT: &str = "second body";

struct FailOnTextEmbedder {
    calls: AtomicUsize,
    dims: usize,
}

#[async_trait]
impl EmbeddingProvider for FailOnTextEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        // Re-embedding groups documents into one call; failing on one
        // document's text (not on the Nth call) keeps the failure per
        // document whatever the grouping.
        if texts.iter().any(|text| text.contains(FAILING_TEXT)) {
            return Err(AppError::embeddings("injected provider failure"));
        }
        Ok(texts.iter().map(|_| vec![0.25; self.dims]).collect())
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

fn test_config(db_path: PathBuf, max_docs: usize) -> Config {
    Config {
        db_path,
        embedding_model: "mock".into(),
        embedding_dims: 8,
        llm_enabled: false,
        maint_max_docs: max_docs,
        ..Config::for_tests()
    }
}

fn seed_doc(store: &Store, id: &str, title: &str, content: &str, layer: &str) {
    SeedDoc::new(id, title, content)
        .layer(layer)
        .kind("document")
        .hashed()
        .insert(store);
}

fn item(
    action: MaintenanceAction,
    target: Option<&str>,
    params: serde_json::Value,
) -> MaintenancePlanItem {
    MaintenancePlanItem {
        action,
        reason: None,
        target_id: target.map(|s| s.to_string()),
        params,
    }
}

#[tokio::test]
async fn dry_run_default_does_not_mutate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("apply.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 50);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    seed_doc(&store, "d1", "Alpha", "hello body", "raw");

    let plan = vec![item(
        MaintenanceAction::Refile,
        Some("d1"),
        json!({"wing": "research", "room": "rag"}),
    )];
    let opts = ApplyPlanOptions::default();
    assert!(opts.dry_run);

    let report = apply_maintenance_plan(&store, &embedder, &config, None, plan, &opts)
        .await
        .unwrap();
    assert!(report.dry_run);
    assert_eq!(report.applied.len(), 1);
    assert!(report.applied[0].message.starts_with("would_apply:"));
    let doc = store.get_document("d1").unwrap().unwrap();
    assert!(doc.wing.is_none());
    assert!(report.ops_log_id.is_some());
}

#[tokio::test]
async fn plan_dry_run_cannot_be_disabled_by_refresh_stale_action() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("refresh-stale-authoritative-dry-run.duckdb");
    let store = Store::open(&path).unwrap();
    let mut config = test_config(path, 50);
    config.llm_enabled = true;
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));

    seed_doc(&store, "raw", "Raw", "new source body", "raw");
    seed_doc(&store, "wiki", "Wiki", "compiled body", "wiki");
    let mut raw = store.get_document("raw").unwrap().unwrap();
    raw.updated_at = Utc::now();
    store.upsert_document(&raw).unwrap();
    let mut wiki = store.get_document("wiki").unwrap().unwrap();
    wiki.updated_at = raw.updated_at - chrono::Duration::days(1);
    wiki.metadata_json = json!({"source_id": raw.id}).to_string();
    store.upsert_document(&wiki).unwrap();
    let wiki_before = store.get_document("wiki").unwrap().unwrap();

    // If the nested `dry_run=false` escaped the plan-wide boundary, this
    // unreachable client would be called and the action would fail instead
    // of returning a pure preview.
    let llm = ChatClient::with_limits("http://127.0.0.1:9/v1", "x", "m", 1, 16).unwrap();
    let report = apply_maintenance_plan(
        &store,
        &embedder,
        &config,
        Some(&llm),
        vec![item(
            MaintenanceAction::RefreshStaleWiki,
            None,
            json!({"dry_run": false, "max_docs": 10}),
        )],
        &ApplyPlanOptions::default(),
    )
    .await
    .unwrap();

    assert!(report.dry_run);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.applied.len(), 1);
    assert!(report.applied[0].message.starts_with("would_apply:"));
    assert_eq!(report.applied[0].detail.as_ref().unwrap()["dry_run"], true);
    let wiki_after = store.get_document("wiki").unwrap().unwrap();
    assert_eq!(wiki_after.revision, wiki_before.revision);
    assert_eq!(wiki_after.content, wiki_before.content);
}

#[tokio::test]
async fn apply_refile_pin_archive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("apply2.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 50);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    seed_doc(&store, "d1", "Alpha", "hello body", "wiki");

    let plan = vec![
        item(
            MaintenanceAction::Refile,
            Some("d1"),
            json!({"wing": "w1", "room": "r1"}),
        ),
        item(MaintenanceAction::Pin, Some("d1"), json!({})),
        item(
            MaintenanceAction::SetBoost,
            Some("d1"),
            json!({"boost": 2.5}),
        ),
        item(MaintenanceAction::Archive, Some("d1"), json!({})),
    ];
    let opts = ApplyPlanOptions {
        dry_run: false,
        max_docs: None,
        agent: Some("test".into()),
    };
    let report = apply_maintenance_plan(&store, &embedder, &config, None, plan, &opts)
        .await
        .unwrap();
    assert!(!report.dry_run);
    assert_eq!(report.errors.len(), 0);
    assert_eq!(report.applied.len(), 4);
    let doc = store.get_document("d1").unwrap().unwrap();
    assert_eq!(doc.wing.as_deref(), Some("w1"));
    assert_eq!(doc.room.as_deref(), Some("r1"));
    assert!(doc.pinned);
    assert!((doc.boost - 2.5).abs() < 1e-9);
    assert_eq!(doc.status, "archived");
}

#[tokio::test]
async fn respects_max_docs_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cap.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 2);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    for i in 0..4 {
        seed_doc(&store, &format!("d{i}"), &format!("D{i}"), "body", "raw");
    }
    let plan: Vec<_> = (0..4)
        .map(|i| item(MaintenanceAction::Pin, Some(&format!("d{i}")), json!({})))
        .collect();
    let opts = ApplyPlanOptions {
        dry_run: false,
        max_docs: Some(2),
        agent: None,
    };
    let report = apply_maintenance_plan(&store, &embedder, &config, None, plan, &opts)
        .await
        .unwrap();
    assert_eq!(report.max_docs, 2);
    assert_eq!(report.applied.len(), 2);
    assert_eq!(report.skipped.len(), 2);
    assert!(store.get_document("d0").unwrap().unwrap().pinned);
    assert!(store.get_document("d1").unwrap().unwrap().pinned);
    assert!(!store.get_document("d2").unwrap().unwrap().pinned);
}

#[tokio::test]
async fn merge_exact_dup_archives_sources() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dup.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 50);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    let body = "same content hash body";
    seed_doc(&store, "keep", "Keep", body, "wiki");
    seed_doc(&store, "dup1", "Dup1", body, "wiki");
    seed_doc(&store, "dup2", "Dup2", body, "wiki");

    let plan = vec![item(
        MaintenanceAction::MergeExactDup,
        Some("keep"),
        json!({"document_ids": ["keep", "dup1", "dup2"]}),
    )];
    let opts = ApplyPlanOptions {
        dry_run: false,
        ..Default::default()
    };
    let report = apply_maintenance_plan(&store, &embedder, &config, None, plan, &opts)
        .await
        .unwrap();
    assert_eq!(report.errors.len(), 0, "{:?}", report.errors);
    assert_eq!(report.applied.len(), 1);
    assert_eq!(
        store.get_document("dup1").unwrap().unwrap().status,
        "archived"
    );
    assert_eq!(
        store.get_document("dup2").unwrap().unwrap().status,
        "archived"
    );
    assert_eq!(
        store.get_document("keep").unwrap().unwrap().status,
        "active"
    );
}

#[tokio::test]
async fn merge_exact_dup_rejects_one_unrelated_source_without_partial_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dup-unrelated.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 50);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    seed_doc(&store, "keep", "Keep", "same body", "wiki");
    seed_doc(&store, "dup", "Dup", "same body", "wiki");
    seed_doc(&store, "unrelated", "Other", "different body", "wiki");
    let mut unrelated = store.get_document("unrelated").unwrap().unwrap();
    unrelated.content_hash = Some(content_hash("same body"));
    store.upsert_document(&unrelated).unwrap();

    let report = apply_maintenance_plan(
        &store,
        &embedder,
        &config,
        None,
        vec![item(
            MaintenanceAction::MergeExactDup,
            Some("keep"),
            json!({"source_ids": ["dup", "unrelated"], "delete": true}),
        )],
        &ApplyPlanOptions {
            dry_run: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0]
        .message
        .contains("not an exact content duplicate"));
    for id in ["keep", "dup", "unrelated"] {
        let document = store.get_document(id).unwrap().expect("document retained");
        assert_eq!(document.status, "active");
    }
}

#[tokio::test]
async fn merge_exact_dup_fallback_hashes_canonical_body_instead_of_trusting_column() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dup-canonical-hash.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 50);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    seed_doc(&store, "keep", "Keep", "same body", "wiki");
    seed_doc(&store, "dup", "Dup", "same body", "wiki");
    let mut keep = store.get_document("keep").unwrap().unwrap();
    keep.content_hash = Some("forged-canonical-hash".into());
    store.upsert_document(&keep).unwrap();

    let report = apply_maintenance_plan(
        &store,
        &embedder,
        &config,
        None,
        vec![item(
            MaintenanceAction::MergeExactDup,
            Some("keep"),
            json!({}),
        )],
        &ApplyPlanOptions {
            dry_run: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        store.get_document("dup").unwrap().unwrap().status,
        "archived"
    );
}

#[tokio::test]
async fn merge_exact_dup_uses_unique_sources_and_remaining_global_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dup-cap.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 1);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    for id in ["keep", "dup1", "dup2"] {
        seed_doc(&store, id, id, "same body", "wiki");
    }

    let report = apply_maintenance_plan(
        &store,
        &embedder,
        &config,
        None,
        vec![item(
            MaintenanceAction::MergeExactDup,
            Some("keep"),
            json!({"source_ids": ["dup1", "dup1", "dup2"]}),
        )],
        &ApplyPlanOptions {
            dry_run: false,
            max_docs: Some(99),
            agent: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(report.max_docs, 1);
    assert_eq!(report.docs_touched, 1);
    assert_eq!(report.applied[0].detail.as_ref().unwrap()["skipped_cap"], 1);
    assert_eq!(
        store.get_document("dup1").unwrap().unwrap().status,
        "archived"
    );
    assert_eq!(
        store.get_document("dup2").unwrap().unwrap().status,
        "active"
    );
}

#[tokio::test]
async fn merge_exact_dup_raw_guard_is_case_insensitive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dup-raw-case.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 50);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    seed_doc(&store, "keep", "Keep", "same body", "wiki");
    seed_doc(&store, "raw-dup", "Raw dup", "same body", "RaW");

    let report = apply_maintenance_plan(
        &store,
        &embedder,
        &config,
        None,
        vec![item(
            MaintenanceAction::MergeExactDup,
            Some("keep"),
            json!({"source_ids": ["raw-dup"], "delete": true}),
        )],
        &ApplyPlanOptions {
            dry_run: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let source = store.get_document("raw-dup").unwrap().unwrap();
    assert_eq!(source.status, "archived");
    assert_eq!(
        report.applied[0].detail.as_ref().unwrap()["blocked"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn drop_tombstones_clamps_cap_and_guards_raw_case_insensitively() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drop-tombstones-cap.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 1);
    for (id, layer) in [("wiki-a", "wiki"), ("wiki-b", "wiki"), ("raw", "RaW")] {
        seed_doc(&store, id, id, "body", layer);
        let mut document = store.get_document(id).unwrap().unwrap();
        document.status = "tombstone".into();
        store.upsert_document(&document).unwrap();
    }

    let first = apply_drop_tombstones(
        &store,
        0,
        "drop_tombstones",
        &json!({"max_docs": 99, "allow_raw_delete": true}),
        &config,
        false,
    )
    .unwrap();
    assert_eq!(first.outcome.detail.as_ref().unwrap()["max_docs"], 1);
    let remaining_after_cap = ["wiki-a", "wiki-b", "raw"]
        .into_iter()
        .filter(|id| store.get_document(id).unwrap().is_some())
        .count();
    assert_eq!(remaining_after_cap, 2);

    let raw_only_path = dir.path().join("drop-tombstones-raw.duckdb");
    let raw_store = Store::open(&raw_only_path).unwrap();
    let raw_config = test_config(raw_only_path, 50);
    seed_doc(&raw_store, "raw", "Raw", "body", "rAw");
    let mut raw = raw_store.get_document("raw").unwrap().unwrap();
    raw.status = "tombstone".into();
    raw_store.upsert_document(&raw).unwrap();
    let guarded = apply_drop_tombstones(
        &raw_store,
        0,
        "drop_tombstones",
        &json!({"max_docs": 50, "allow_raw_delete": false}),
        &raw_config,
        false,
    )
    .unwrap();
    assert!(raw_store.get_document("raw").unwrap().is_some());
    assert_eq!(guarded.outcome.detail.as_ref().unwrap()["skipped_raw"], 1);
}

#[tokio::test]
async fn set_tags_links_graph() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tags.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 50);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    seed_doc(&store, "d1", "Tagged", "body #old", "raw");

    let plan = vec![item(
        MaintenanceAction::SetTags,
        Some("d1"),
        json!({"tags": ["alpha", "beta"], "mode": "replace"}),
    )];
    let opts = ApplyPlanOptions {
        dry_run: false,
        ..Default::default()
    };
    let report = apply_maintenance_plan(&store, &embedder, &config, None, plan, &opts)
        .await
        .unwrap();
    assert_eq!(report.errors.len(), 0, "{:?}", report.errors);
    assert_eq!(report.applied.len(), 1);
    let node = store.find_node_by_document_id("d1").unwrap().unwrap();
    let view = store.neighbors(&node.id, 1, 50).unwrap();
    let tag_labels: Vec<_> = view
        .nodes
        .iter()
        .filter(|n| n.kind == "tag")
        .map(|n| n.label.clone())
        .collect();
    assert!(tag_labels.iter().any(|t| t == "alpha"));
    assert!(tag_labels.iter().any(|t| t == "beta"));
}

#[tokio::test]
async fn set_tags_reuses_the_canonical_tag_hub_for_hash_and_case_variants() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tag-hub.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 50);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    seed_doc(&store, "d1", "One", "body", "raw");
    seed_doc(&store, "d2", "Two", "body", "raw");
    let plan = vec![
        item(
            MaintenanceAction::SetTags,
            Some("d1"),
            json!({"tags": ["#Alpha"], "mode": "replace"}),
        ),
        item(
            MaintenanceAction::SetTags,
            Some("d2"),
            json!({"tags": ["alpha"], "mode": "replace"}),
        ),
    ];
    let opts = ApplyPlanOptions {
        dry_run: false,
        ..Default::default()
    };
    let report = apply_maintenance_plan(&store, &embedder, &config, None, plan, &opts)
        .await
        .unwrap();
    assert_eq!(report.errors.len(), 0, "{:?}", report.errors);
    let conn = store.lock().unwrap();
    let uris: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT uri FROM graph_nodes WHERE kind = 'tag' ORDER BY uri")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    };
    assert_eq!(uris, vec!["tag://alpha".to_string()]);
}

#[tokio::test]
async fn rebuild_index_and_noop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("idx.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 50);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    seed_doc(&store, "w1", "Wiki", "compiled", "wiki");

    let plan = vec![
        item(MaintenanceAction::Noop, None, json!({})),
        item(MaintenanceAction::RebuildIndex, None, json!({})),
    ];
    let opts = ApplyPlanOptions {
        dry_run: false,
        ..Default::default()
    };
    let report = apply_maintenance_plan(&store, &embedder, &config, None, plan, &opts)
        .await
        .unwrap();
    assert_eq!(report.applied.len(), 2);
    assert_eq!(store.list_wiki_index().unwrap().len(), 1);
}

#[tokio::test]
async fn reembed_plan_finishes_with_generation_clean_fts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reembed-plan.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 50);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    store.ensure_embedding_manifest(&config).unwrap();
    seed_doc(&store, "d1", "Alpha", "searchable reembed body", "wiki");
    store.ensure_fts(&config.fts_stemmer).unwrap();

    let plan = vec![item(MaintenanceAction::Reembed, Some("d1"), json!({}))];
    let report = apply_maintenance_plan(
        &store,
        &embedder,
        &config,
        None,
        plan,
        &ApplyPlanOptions {
            dry_run: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(report.errors.len(), 0, "{:?}", report.errors);
    assert_eq!(report.applied.len(), 1);
    let generation_after_plan = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(!generation_after_plan.dirty);

    let hits = crate::db::search::search(
        &store,
        &crate::db::search::SearchQuery {
            mode: SearchMode::Lex,
            query_text: Some("searchable".into()),
            top_k: 5,
            fts_stemmer: config.fts_stemmer.clone(),
            ..crate::db::search::SearchQuery::default()
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    let generation_after_search = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert_eq!(generation_after_search, generation_after_plan);
}

#[tokio::test]
async fn partial_reembed_failure_is_structured_error_and_keeps_migration_marker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partial-reembed-plan.duckdb");
    let store = Store::open(&path).unwrap();
    let mut config = test_config(path, 50);
    let mut old_config = config.clone();
    old_config.embedding_model = "old-model".into();
    config.embedding_model = "target-model".into();
    store
        .write_embedding_manifest_from_config(&old_config)
        .unwrap();
    seed_doc(&store, "d1", "One", "first body", "wiki");
    seed_doc(&store, "d2", "Two", "second body", "wiki");
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(FailOnTextEmbedder {
        calls: AtomicUsize::new(0),
        dims: 8,
    });

    let report = apply_maintenance_plan(
        &store,
        &embedder,
        &config,
        None,
        vec![item(
            MaintenanceAction::ReembedAll,
            None,
            json!({"max_docs": 2}),
        )],
        &ApplyPlanOptions {
            dry_run: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert!(report.applied.is_empty());
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    let error = &report.errors[0];
    assert_eq!(error.action, "reembed_all");
    assert_eq!(error.outcome, ActionOutcomeKind::Error);
    assert_eq!(
        error.detail.as_ref().unwrap()["code"],
        "REEMBED_ALL_INCOMPLETE"
    );
    assert_eq!(
        error.detail.as_ref().unwrap()["report"]["documents_failed"],
        1
    );
    assert_eq!(
        error.detail.as_ref().unwrap()["report"]["documents_succeeded"],
        1
    );
    let target_error = store
        .require_embedding_manifest_match(&config)
        .expect_err("target identity must stay blocked after partial migration");
    assert!(target_error
        .to_string()
        .contains("incomplete corpus migration"));
    store
        .require_embedding_manifest_match(&old_config)
        .expect_err("configuration rollback must also stay blocked");
}

#[tokio::test]
async fn capped_reembed_is_structured_plan_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("capped-reembed-plan.duckdb");
    let store = Store::open(&path).unwrap();
    let config = test_config(path, 50);
    store.write_embedding_manifest_from_config(&config).unwrap();
    seed_doc(&store, "d1", "One", "first body", "wiki");
    seed_doc(&store, "d2", "Two", "second body", "wiki");
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));

    let report = apply_maintenance_plan(
        &store,
        &embedder,
        &config,
        None,
        vec![item(
            MaintenanceAction::ReembedAll,
            None,
            json!({"max_docs": 1}),
        )],
        &ApplyPlanOptions {
            dry_run: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert!(report.applied.is_empty());
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    let detail = report.errors[0].detail.as_ref().unwrap();
    assert_eq!(detail["code"], "REEMBED_ALL_INCOMPLETE");
    assert_eq!(detail["report"]["skipped_cap"], 1);
    assert_eq!(detail["report"]["documents_failed"], 0);
}

#[tokio::test]
async fn final_fts_failure_returns_mutation_report_and_terminal_ops_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("final-fts-failure.duckdb");
    let store = Store::open(&path).unwrap();
    let mut config = test_config(path, 50);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
    let body = "same durable body before failed finalization";
    seed_doc(&store, "keep", "Keep", body, "wiki");
    seed_doc(&store, "drop", "Drop", body, "wiki");
    store.ensure_fts(&config.fts_stemmer).unwrap();
    config.fts_stemmer = db::fts::test_fault::fail("apply");

    let report = apply_maintenance_plan(
        &store,
        &embedder,
        &config,
        None,
        vec![item(
            MaintenanceAction::MergeExactDup,
            Some("keep"),
            json!({"document_ids": ["keep", "drop"], "delete": true}),
        )],
        &ApplyPlanOptions {
            dry_run: false,
            ..Default::default()
        },
    )
    .await
    .expect("aggregate report survives final FTS failure");

    assert!(store.get_document("keep").unwrap().is_some());
    assert!(store.get_document("drop").unwrap().is_none());
    assert_eq!(report.applied.len(), 1);
    let finalization = report
        .errors
        .iter()
        .find(|outcome| outcome.action == "finalize_fts")
        .expect("structured finalization error");
    assert_eq!(finalization.outcome, ActionOutcomeKind::Error);
    assert_eq!(
        finalization.detail.as_ref().unwrap()["code"],
        "FTS_FINALIZATION_FAILED"
    );
    assert_eq!(
        finalization.detail.as_ref().unwrap()["fallback_dirty_marked"],
        true
    );
    let generation = {
        let conn = store.lock().unwrap();
        db::fts_generation_state(&conn).unwrap()
    };
    assert!(generation.dirty, "next lexical read must retry FTS");

    let summary_id = report.ops_log_id.as_deref().expect("terminal summary id");
    let summary = store
        .list_ops_log(10)
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == summary_id)
        .expect("terminal summary ops_log row");
    assert_eq!(summary.op, "apply_maintenance_plan");
    assert!(store
        .list_ops_log(10)
        .unwrap()
        .iter()
        .any(|entry| entry.op == "fts_finalization_failed"));
    let payload: serde_json::Value = serde_json::from_str(&summary.payload_json).unwrap();
    assert!(payload["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|error| error["action"] == "finalize_fts"));
}

#[test]
fn param_usize_accepts_absent_and_non_negative() {
    let params = serde_json::json!({"a": 3, "b": "7", "c": 2.9});
    assert_eq!(param_usize(&params, "missing").unwrap(), None);
    assert_eq!(param_usize(&params, "a").unwrap(), Some(3));
    assert_eq!(param_usize(&params, "b").unwrap(), Some(7));
    assert_eq!(param_usize(&params, "c").unwrap(), Some(2));
}

#[test]
fn param_usize_rejects_negative_nan_and_infinite() {
    for bad in [
        serde_json::json!({"max_docs": -1}),
        serde_json::json!({"max_docs": "NaN"}),
        serde_json::json!({"max_docs": "inf"}),
        serde_json::json!({"max_docs": "-inf"}),
    ] {
        let err = param_usize(&bad, "max_docs").unwrap_err();
        assert!(err.to_string().contains("max_docs"), "{err}");
    }
}
