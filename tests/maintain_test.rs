//! Integration tests for the local maintenance loop (offline, mock embedder).
//!
//! Pipeline under test (docs/LOCAL_LLM_MAINTENANCE.md):
//!   analyze_corpus → plan_maintenance (heuristic, no network)
//!   → apply_maintenance_plan (dry_run) → maintain_compress L0 → vacuum_store
//!
//! No LLM HTTP calls: `llm_enabled=false`, `force_heuristic=true`, mock embedder.

use std::sync::Arc;

use chrono::Utc;
use rag_mcp::config::EmbeddingProviderKind;
use rag_mcp::embeddings::{EmbeddingProvider, MockEmbedder};
use rag_mcp::models::{
    Chunk, Document, GraphNode, SearchMode,
};
use rag_mcp::util::content_hash;
use rag_mcp::{
    analyze_corpus, apply_maintenance_plan, heuristic_plan, log_plan, maintain_compress,
    plan_maintenance, ActionOutcomeKind, AnalyzeOptions, ApplyPlanOptions, CompressOptions,
    Config, MaintainCompressReport, MaintenanceAction, PlanOptions, PlanSource, Store,
};
use tempfile::TempDir;

const DIMS: usize = 32;

fn open_temp_store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("maintain_test.duckdb");
    let store = Store::open(&path).expect("open store");
    (dir, store)
}

fn test_config(db_path: std::path::PathBuf) -> Config {
    Config {
        db_path,
        embedding_provider: EmbeddingProviderKind::Mock,
        embedding_base_url: "https://api.openai.com/v1".into(),
        embedding_api_key: String::new(),
        embedding_model: "mock".into(),
        embedding_dims: DIMS,
        chunk_size: 200,
        chunk_overlap: 20,
        default_top_k: 5,
        ingest_roots: Vec::new(),
        max_context_tokens: 4096,
        max_chunks_per_doc: 3,
        fts_stemmer: "porter".into(),
        default_search_mode: SearchMode::Vec,
        llm_provider: rag_mcp::llm::LlmProviderKind::Ollama,
        llm_base_url: "http://127.0.0.1:11434/v1".into(),
        llm_model: "llama3.2".into(),
        llm_api_key: "ollama".into(),
        llm_enabled: false,
        llm_timeout_secs: 30,
        llm_max_tokens: 512,
        maint_max_docs: 50,
        maint_near_dup_threshold: 0.92,
        tool_surface: rag_mcp::mcp::ToolSurface::Full,
        http_bind: None,
            wiki_require_if_match: false,
    }
}

fn mock_embedder() -> Arc<dyn EmbeddingProvider> {
    Arc::new(MockEmbedder::new(DIMS))
}

/// Seed a small fixture corpus: unique raws, an exact-hash twin pair, a wiki page,
/// and an unresolved stub so analyze has real signals.
async fn seed_fixture_corpus(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
) -> FixtureIds {
    let now = Utc::now();
    let mock = MockEmbedder::new(DIMS);

    // Unique raw document.
    let raw_body = "Fixture notes on local Ollama maintenance and DuckDB vacuum.";
    let raw = Document {
        id: "doc-raw-unique".into(),
        uri: "raw://fixture/unique".into(),
        title: "Unique Fixture Notes".into(),
        content: raw_body.into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        layer: "raw".into(),
        kind: "document".into(),
        content_hash: Some(content_hash(raw_body)),
        wing: Some("research".into()),
        room: Some("rag".into()),
        ..Default::default()
    };
    store.upsert_document(&raw).expect("upsert unique raw");
    insert_chunk(store, &mock, &raw.id, raw_body, 0).await;

    // Exact-duplicate pair (same content_hash, different ids/uris).
    let twin_body = "identical twin content for exact hash dedupe signal";
    let twin_hash = content_hash(twin_body);
    for (id, uri, title) in [
        ("doc-twin-a", "raw://fixture/twin-a", "Twin A"),
        ("doc-twin-b", "raw://fixture/twin-b", "Twin B"),
    ] {
        let d = Document {
            id: id.into(),
            uri: uri.into(),
            title: title.into(),
            content: twin_body.into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            layer: "raw".into(),
            kind: "document".into(),
            content_hash: Some(twin_hash.clone()),
            wing: Some("research".into()),
            room: Some("dups".into()),
            ..Default::default()
        };
        store.upsert_document(&d).expect("upsert twin");
        insert_chunk(store, &mock, id, twin_body, 0).await;
    }

    // Wiki page (orphan-ish: no edges until graph rebuild).
    let wiki_body = "Compiled summary of fixture corpus. See [[Missing Page]].";
    let wiki = Document {
        id: "doc-wiki-summary".into(),
        uri: "wiki://fixture-summary".into(),
        title: "Fixture Summary".into(),
        content: wiki_body.into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        layer: "wiki".into(),
        kind: "source_summary".into(),
        content_hash: Some(content_hash(wiki_body)),
        wing: Some("wiki".into()),
        room: Some("catalog".into()),
        ..Default::default()
    };
    store.upsert_document(&wiki).expect("upsert wiki");
    insert_chunk(store, &mock, &wiki.id, wiki_body, 0).await;

    // Unresolved stub referenced by the wiki body text (graph stub only).
    store
        .upsert_graph_node(&GraphNode {
            id: "stub-missing-page".into(),
            kind: "stub".into(),
            label: "Missing Page".into(),
            document_id: None,
            uri: None,
            resolved: false,
            metadata_json: "{}".into(),
        })
        .expect("upsert stub");

    // Align embedding manifest with mock config so doctor.embed_ok stays true.
    let _ = store
        .write_embedding_manifest_from_config(&test_config(store.path().to_path_buf()))
        .expect("embedding manifest");

    // Silence unused embedder warning if only MockEmbedder is used above.
    let _ = embedder.dimensions();

    FixtureIds {
        raw_unique: raw.id,
        twin_a: "doc-twin-a".into(),
        twin_b: "doc-twin-b".into(),
        wiki: wiki.id,
        stub: "stub-missing-page".into(),
    }
}

struct FixtureIds {
    raw_unique: String,
    twin_a: String,
    twin_b: String,
    wiki: String,
    stub: String,
}

async fn insert_chunk(
    store: &Store,
    mock: &MockEmbedder,
    document_id: &str,
    content: &str,
    chunk_index: i32,
) {
    let emb = mock
        .embed(&[content.to_string()])
        .await
        .expect("embed")
        .into_iter()
        .next()
        .expect("one vector");
    assert_eq!(emb.len(), DIMS);
    store
        .insert_chunks(&[Chunk {
            id: format!("{document_id}-c{chunk_index}"),
            document_id: document_id.into(),
            chunk_index,
            content: content.into(),
            embedding: emb,
            char_start: 0,
            char_end: content.len() as i32,
            metadata_json: "{}".into(),
        }])
        .expect("insert_chunks");
}

/// Full offline loop on a fixture corpus: analyze → heuristic plan → dry_run apply
/// → compress L0 → vacuum. Uses mock embedder only (no network).
#[tokio::test]
async fn maintain_loop_analyze_plan_dry_run_compress_vacuum() {
    let (_dir, store) = open_temp_store();
    let config = test_config(store.path().to_path_buf());
    let embedder = mock_embedder();
    let ids = seed_fixture_corpus(&store, &embedder).await;

    // --- 1. analyze_corpus (deterministic, no LLM) ---
    let mut analyze_opts = AnalyzeOptions::from_config(&config);
    analyze_opts.log_ops = true;
    analyze_opts.include_near_dups = false;
    let analysis = analyze_corpus(&store, &config, &analyze_opts).expect("analyze_corpus");

    assert!(
        analysis.counts.documents >= 4,
        "fixture should have >=4 docs, got {}",
        analysis.counts.documents
    );
    assert!(
        analysis.counts.chunks >= 4,
        "fixture should have chunked docs, got {}",
        analysis.counts.chunks
    );
    assert!(
        analysis.doctor.schema_ok,
        "schema should be current: {:?}",
        analysis.doctor
    );
    assert!(
        analysis.doctor.embed_ok,
        "manifest should match mock config: {:?}",
        analysis.embed_mismatch
    );
    assert_eq!(
        analysis.exact_duplicates.len(),
        1,
        "expected one exact-hash twin group: {:?}",
        analysis.exact_duplicates
    );
    assert_eq!(analysis.exact_duplicates[0].count, 2);
    assert!(
        analysis
            .exact_duplicates[0]
            .document_ids
            .iter()
            .any(|id| id == &ids.twin_a),
        "twin group should include twin-a"
    );
    assert_eq!(
        analysis.unresolved_stubs.len(),
        1,
        "expected unresolved stub: {:?}",
        analysis.unresolved_stubs
    );
    assert_eq!(analysis.unresolved_stubs[0].node_id, ids.stub);
    assert!(
        analysis.issue_count > 0,
        "fixture should surface issues for the planner"
    );
    assert!(
        analysis.issues.iter().any(|i| i.code == "exact_dup"),
        "issues should include exact_dup: {:?}",
        analysis.issues
    );

    let ops_after_analyze = store.list_ops_log(50).expect("ops_log");
    assert!(
        ops_after_analyze.iter().any(|o| o.op == "analyze_corpus"),
        "analyze_corpus should append ops_log"
    );

    // --- 2. plan_maintenance heuristic (no network / no ChatClient) ---
    let mut plan_opts = PlanOptions::from_config(&config);
    plan_opts.force_heuristic = true;
    plan_opts.log_ops = false;
    let plan = plan_maintenance(&analysis, &config, None, &plan_opts)
        .await
        .expect("plan_maintenance");

    assert_eq!(plan.source, PlanSource::Heuristic);
    assert!(!plan.llm_enabled, "LLM must stay disabled (offline CI)");
    assert!(plan.model.is_none());
    assert!(
        plan.action_count >= 1,
        "heuristic should propose at least one action"
    );
    assert!(
        plan.rejected.is_empty(),
        "heuristic actions should all pass whitelist: {:?}",
        plan.rejected
    );

    let action_names: Vec<&str> = plan.actions.iter().map(|a| a.action.as_str()).collect();
    assert!(
        action_names.contains(&"merge_exact_dup"),
        "plan should propose merge_exact_dup for twins: {action_names:?}"
    );
    assert!(
        action_names.contains(&"resolve_stub") || action_names.contains(&"vacuum"),
        "plan should include stub resolve and/or vacuum: {action_names:?}"
    );
    for a in &plan.actions {
        assert!(
            rag_mcp::is_allowed_action(a.action.as_str()),
            "non-whitelist action slipped through: {}",
            a.action
        );
    }

    // Direct heuristic_plan parity check (same report, no async).
    let heuristic = heuristic_plan(&analysis, plan_opts.max_actions);
    assert_eq!(heuristic.source, PlanSource::Heuristic);
    assert!(
        heuristic
            .actions
            .iter()
            .any(|a| a.action == MaintenanceAction::MergeExactDup),
        "heuristic_plan should map exact dups"
    );

    log_plan(&store, &plan).expect("log_plan");
    let ops_after_plan = store.list_ops_log(50).expect("ops_log after plan");
    assert!(
        ops_after_plan.iter().any(|o| o.op == "plan_maintenance"),
        "log_plan should append plan_maintenance"
    );

    // Snapshot doc meta before dry_run apply so we can prove no mutation.
    let twin_a_before = store
        .get_document(&ids.twin_a)
        .expect("get twin_a")
        .expect("twin_a exists");
    let twin_b_before = store
        .get_document(&ids.twin_b)
        .expect("get twin_b")
        .expect("twin_b exists");
    let docs_before_apply = store.list_documents().expect("list docs").len();

    // --- 3. apply_maintenance_plan dry_run (default safety) ---
    let apply_opts = ApplyPlanOptions {
        dry_run: true,
        max_docs: None,
        agent: Some("maintain-test".into()),
    };
    assert!(apply_opts.dry_run, "dry_run must default/remain true");

    let apply_report = apply_maintenance_plan(
        &store,
        &embedder,
        &config,
        None,
        plan.actions.clone(),
        &apply_opts,
    )
    .await
    .expect("apply_maintenance_plan dry_run");

    assert!(apply_report.dry_run);
    assert_eq!(apply_report.total_actions, plan.actions.len());
    assert!(
        apply_report.ops_log_id.is_some(),
        "apply summary must hit ops_log"
    );
    // Dry-run must not delete / merge twins.
    assert_eq!(
        store.list_documents().expect("list after dry_run").len(),
        docs_before_apply,
        "dry_run must not change document count"
    );
    let twin_a_after = store
        .get_document(&ids.twin_a)
        .expect("get twin_a after")
        .expect("twin_a still present after dry_run");
    let twin_b_after = store
        .get_document(&ids.twin_b)
        .expect("get twin_b after")
        .expect("twin_b still present after dry_run");
    assert_eq!(twin_a_before.status, twin_a_after.status);
    assert_eq!(twin_b_before.status, twin_b_after.status);
    assert_eq!(twin_a_before.content_hash, twin_a_after.content_hash);

    // Every non-error outcome for applied steps should be dry-run preview language.
    for o in &apply_report.applied {
        assert_eq!(o.outcome, ActionOutcomeKind::Applied);
        assert!(
            o.message.starts_with("would_apply:") || o.message.contains("noop"),
            "dry_run applied messages should preview: {}",
            o.message
        );
    }

    let ops_after_apply = store.list_ops_log(100).expect("ops after apply");
    assert!(
        ops_after_apply
            .iter()
            .any(|o| o.op == "apply_maintenance_plan_dry_run"),
        "expected apply_maintenance_plan_dry_run in ops_log, got: {:?}",
        ops_after_apply.iter().map(|o| &o.op).collect::<Vec<_>>()
    );

    // --- 4. maintain_compress level 0 (checkpoint + FTS; dry_run then apply) ---
    let mut compress_opts = CompressOptions::from_config(&config);
    compress_opts.level = 0;
    compress_opts.dry_run = true;
    compress_opts.log_ops = true;

    let compress_dry: MaintainCompressReport =
        maintain_compress(&store, &config, &compress_opts).expect("compress L0 dry_run");
    assert_eq!(compress_dry.level, 0);
    assert!(compress_dry.dry_run);
    assert_eq!(compress_dry.docs_removed, 0);
    assert!(
        compress_dry.actions.iter().any(|a| a == "checkpoint"),
        "L0 actions should include checkpoint: {:?}",
        compress_dry.actions
    );
    assert!(
        compress_dry.actions.iter().any(|a| a == "reindex_fts"),
        "L0 actions should include reindex_fts: {:?}",
        compress_dry.actions
    );
    let l0_dry = compress_dry.l0.as_ref().expect("l0 report");
    assert!(l0_dry.dry_run);
    assert!(!l0_dry.checkpointed);

    compress_opts.dry_run = false;
    let compress_applied =
        maintain_compress(&store, &config, &compress_opts).expect("compress L0 apply");
    assert!(!compress_applied.dry_run);
    assert_eq!(compress_applied.docs_removed, 0);
    let l0_applied = compress_applied.l0.as_ref().expect("l0 applied");
    assert!(l0_applied.checkpointed);
    assert!(!l0_applied.dry_run);
    assert!(
        compress_applied.ops_log_id.is_some(),
        "compress should write ops_log"
    );

    // Fixture docs still present after L0 (L0 does not merge/delete).
    assert!(store.get_document(&ids.raw_unique).unwrap().is_some());
    assert!(store.get_document(&ids.wiki).unwrap().is_some());
    assert_eq!(
        store.list_documents().expect("list after L0").len(),
        docs_before_apply
    );

    // --- 5. vacuum_store ---
    let vacuum = store.vacuum_store().expect("vacuum_store");
    assert!(vacuum.checkpointed);
    assert_eq!(vacuum.db_path, store.path().display().to_string());
    // Size fields are best-effort; when present they should be finite.
    if let (Some(b), Some(a)) = (vacuum.bytes_before, vacuum.bytes_after) {
        assert!(b > 0 || a > 0 || vacuum.bytes_delta == Some(0));
    }

    let ops_final = store.list_ops_log(200).expect("final ops_log");
    let ops: Vec<&str> = ops_final.iter().map(|o| o.op.as_str()).collect();
    assert!(
        ops.contains(&"analyze_corpus"),
        "ops_log missing analyze_corpus: {ops:?}"
    );
    assert!(
        ops.contains(&"plan_maintenance"),
        "ops_log missing plan_maintenance: {ops:?}"
    );
    assert!(
        ops.contains(&"apply_maintenance_plan_dry_run"),
        "ops_log missing apply dry_run: {ops:?}"
    );
    assert!(
        ops.iter()
            .any(|o| *o == "maintain_compress" || *o == "maintain_compress_dry_run"),
        "ops_log missing maintain_compress: {ops:?}"
    );
    assert!(
        ops.contains(&"vacuum_store"),
        "ops_log missing vacuum_store: {ops:?}"
    );
}

/// Whitelist safety: unknown action names never reach apply as Applied mutations.
#[tokio::test]
async fn maintain_whitelist_rejects_unknown_action_string() {
    use rag_mcp::{action_from_parts, is_allowed_action, parse_and_filter_plan};

    assert!(!is_allowed_action("drop_table"));
    assert!(!is_allowed_action("shell"));
    assert!(is_allowed_action("vacuum"));
    assert!(is_allowed_action("merge_exact_dup"));

    let bad = action_from_parts("rm_rf", None, None, serde_json::json!({}));
    assert!(bad.is_err(), "unknown action must error");

    let (accepted, rejected, _) = parse_and_filter_plan(
        r#"{"actions":[
            {"action":"vacuum","params":{}},
            {"action":"exec_shell","params":{"cmd":"id"}},
            {"action":"merge_exact_dup","target_id":"keep","params":{"document_ids":["keep","drop"]}}
        ]}"#,
    )
    .expect("parse");
    assert_eq!(accepted.len(), 2);
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].action.as_deref(), Some("exec_shell"));
}

/// Empty / healthy corpus: analyze → heuristic noop → dry_run apply → L0 compress → vacuum.
#[tokio::test]
async fn maintain_loop_empty_corpus_is_safe() {
    let (_dir, store) = open_temp_store();
    let config = test_config(store.path().to_path_buf());
    let embedder = mock_embedder();
    let _ = store
        .write_embedding_manifest_from_config(&config)
        .expect("manifest");

    let mut analyze_opts = AnalyzeOptions::from_config(&config);
    analyze_opts.log_ops = true;
    let analysis = analyze_corpus(&store, &config, &analyze_opts).expect("analyze empty");
    assert_eq!(analysis.counts.documents, 0);
    assert_eq!(analysis.exact_duplicates.len(), 0);

    let plan_opts = PlanOptions {
        force_heuristic: true,
        log_ops: false,
        ..PlanOptions::default()
    };
    let plan = plan_maintenance(&analysis, &config, None, &plan_opts)
        .await
        .expect("plan empty");
    assert_eq!(plan.source, PlanSource::Heuristic);
    assert_eq!(plan.actions.len(), 1);
    assert_eq!(plan.actions[0].action, MaintenanceAction::Noop);

    let apply = apply_maintenance_plan(
        &store,
        &embedder,
        &config,
        None,
        plan.actions,
        &ApplyPlanOptions {
            dry_run: true,
            max_docs: None,
            agent: None,
        },
    )
    .await
    .expect("apply empty dry_run");
    assert!(apply.dry_run);
    assert_eq!(apply.errors.len(), 0);
    assert_eq!(apply.applied.len(), 1);

    let mut copts = CompressOptions::from_config(&config);
    copts.level = 0;
    copts.dry_run = false;
    let cr = maintain_compress(&store, &config, &copts).expect("L0 empty");
    assert!(cr.l0.as_ref().unwrap().checkpointed);

    let vac = store.vacuum_store().expect("vacuum empty");
    assert!(vac.checkpointed);
}
