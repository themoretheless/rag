
use super::*;
use crate::diagnostics::DoctorReport;
use crate::maintain::analyze::{
    AnalysisIssue, ArchiveCandidate, CorpusCounts, DuplicateGroup, EmbedMismatchInfo,
    NearDuplicatePair, SizeStats, StaleWikiPage,
};

/// In-process LLM double: no HTTP server, fixed model output.
struct ScriptedLlm(&'static str);

#[async_trait::async_trait]
impl crate::llm::ChatCompleter for ScriptedLlm {
    async fn complete(&self, _: &[crate::llm::ChatMessage]) -> Result<String> {
        Ok(self.0.to_string())
    }
    async fn complete_with_max_tokens(
        &self,
        messages: &[crate::llm::ChatMessage],
        _: u32,
    ) -> Result<String> {
        self.complete(messages).await
    }
    fn model(&self) -> &str {
        "scripted"
    }
}

#[tokio::test]
async fn llm_planner_keeps_whitelisted_actions_and_rejects_the_rest() {
    let llm = ScriptedLlm(
            "```json\n{\"actions\": [{\"action\": \"vacuum\", \"reason\": \"wal\"}, {\"action\": \"drop_table\"}], \"notes\": \"n\"}\n```",
        );
    let config = Config {
        llm_enabled: true,
        ..Config::for_tests()
    };
    let plan = plan_maintenance(
        &empty_report(),
        &config,
        Some(&llm),
        &PlanOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(plan.model.as_deref(), Some("scripted"));
    assert_eq!(plan.actions.len(), 1);
    assert_eq!(plan.actions[0].action, MaintenanceAction::Vacuum);
    assert_eq!(plan.rejected.len(), 1);
}

fn empty_report() -> AnalysisReport {
    AnalysisReport {
        generated_at: Utc::now(),
        db_path: ":memory:".into(),
        db_size_bytes: None,
        counts: CorpusCounts::default(),
        size: SizeStats::default(),
        doctor: DoctorReport {
            backend: "duckdb".into(),
            storage_capabilities: crate::storage::duckdb_capability_names(),
            schema_version: 1,
            expected_schema_version: 1,
            schema_ok: true,
            fts_ready: true,
            document_count: 0,
            chunk_count: 0,
            node_count: 0,
            edge_count: 0,
            embed_dims: 8,
            manifest_dims: None,
            embed_ok: true,
            ready_for_search: false,
            ingest_roots_configured: false,
            db_path: ":memory:".into(),
            wal_bytes: 0,
            wal_warn_bytes: 256 * 1024 * 1024,
            wal_too_large: false,
            documents_without_chunks: 0,
            orphan_chunks: 0,
            orphan_document_nodes: 0,
            orphan_edges: 0,
            unscoped_documents: 0,
            relational_integrity_ok: true,
            repair_hint: None,
            ok: true,
        },
        fts_ready: true,
        embed_mismatch: None,
        exact_duplicates: vec![],
        near_duplicates: vec![],
        orphan_nodes: vec![],
        orphan_wiki_pages: vec![],
        unresolved_stubs: vec![],
        aging_stubs: vec![],
        stale_wiki: vec![],
        archive_candidates: vec![],
        issues: vec![],
        issue_count: 0,
    }
}

#[test]
fn whitelist_accepts_known_ops() {
    assert!(is_allowed_action("refile"));
    assert!(is_allowed_action("vacuum"));
    assert!(is_allowed_action("merge_exact_dup"));
    assert!(is_allowed_action("consolidate_propose"));
    assert!(is_allowed_action("resolve_stub"));
    assert!(is_allowed_action("rebuild_index"));
    assert!(is_allowed_action("reindex_fts"));
    assert!(!is_allowed_action("drop_table"));
    assert!(!is_allowed_action(""));
    assert!(!is_allowed_action("Refile"));
}

#[test]
fn enum_roundtrip_snake_case() {
    for &a in MaintenanceAction::ALL {
        let s = serde_json::to_string(&a).unwrap();
        assert_eq!(s, format!("\"{}\"", a.as_str()));
        let back: MaintenanceAction = serde_json::from_str(&s).unwrap();
        assert_eq!(back, a);
    }
    assert_eq!(MaintenanceAction::ALL.len(), ALLOWED_ACTIONS.len());
    for (i, name) in ALLOWED_ACTIONS.iter().enumerate() {
        assert_eq!(MaintenanceAction::ALL[i].as_str(), *name);
    }
}

#[test]
fn validate_rejects_unknown() {
    let err = action_from_parts("rm -rf", None, None, serde_json::json!({})).unwrap_err();
    assert!(err.to_string().contains("whitelist"));
}

#[test]
fn validate_rejects_non_object_params_for_merge_and_consolidate() {
    for action in ["merge_exact_dup", "consolidate"] {
        for params in [serde_json::json!([1, 2]), serde_json::json!("x")] {
            let err = action_from_parts(action, None, Some("doc-1".into()), params).unwrap_err();
            assert!(err.to_string().contains("object"), "{action}: {err}");
        }
    }
}

#[test]
fn validate_plan_all_or_nothing() {
    let plan = vec![
        MaintenancePlanItem {
            action: MaintenanceAction::Vacuum,
            reason: Some("L0".into()),
            target_id: None,
            params: serde_json::json!({}),
        },
        MaintenancePlanItem {
            action: MaintenanceAction::Pin,
            reason: None,
            target_id: None,
            params: serde_json::json!({}),
        },
    ];
    assert!(validate_plan(plan).is_err());
}

#[test]
fn filter_plan_keeps_valid_drops_invalid() {
    let (ok, bad) = filter_raw_plan(vec![
        RawPlanItem {
            action: "vacuum".into(),
            reason: None,
            target_id: None,
            params: serde_json::json!({}),
        },
        RawPlanItem {
            action: "drop_table".into(),
            reason: None,
            target_id: None,
            params: serde_json::json!({}),
        },
    ]);
    assert_eq!(ok.len(), 1);
    assert_eq!(ok[0].action, MaintenanceAction::Vacuum);
    assert_eq!(bad.len(), 1);
}

#[test]
fn parse_plan_envelope_and_array() {
    let raw = r#"{"actions":[{"action":"reindex_fts","reason":"fts","params":{}}],"notes":"n"}"#;
    let (acts, notes) = parse_plan_json(raw).unwrap();
    assert_eq!(acts.len(), 1);
    assert_eq!(acts[0]["action"], "reindex_fts");
    assert_eq!(notes.as_deref(), Some("n"));

    let arr = r#"[{"action":"vacuum","params":{}}]"#;
    let (acts, notes) = parse_plan_json(arr).unwrap();
    assert_eq!(acts[0]["action"], "vacuum");
    assert!(notes.is_none());
}

#[test]
fn parse_plan_fenced_json() {
    let raw = "```json\n{\"actions\":[{\"action\":\"noop\",\"params\":{}}]}\n```";
    let (acts, _) = parse_plan_json(raw).unwrap();
    assert_eq!(acts[0]["action"], "noop");
    let (filtered, rejected, _) = parse_and_filter_plan(raw).unwrap();
    assert_eq!(filtered[0].action, MaintenanceAction::Noop);
    assert!(rejected.is_empty());
}

#[test]
fn heuristic_healthy_is_noop() {
    let plan = heuristic_plan(&empty_report(), 10);
    assert_eq!(plan.source, PlanSource::Heuristic);
    assert_eq!(plan.actions.len(), 1);
    assert_eq!(plan.actions[0].action, MaintenanceAction::Noop);
}

#[test]
fn heuristic_maps_dups_stale_embed_fts() {
    let mut report = empty_report();
    report.counts.chunks = 5;
    report.fts_ready = false;
    report.embed_mismatch = Some(EmbedMismatchInfo {
        config_dims: 8,
        config_provider: "mock".into(),
        config_model: "mock".into(),
        manifest_dims: Some(16),
        manifest_provider: Some("mock".into()),
        manifest_model: Some("old".into()),
        message: "dims mismatch".into(),
    });
    report.exact_duplicates = vec![DuplicateGroup {
        content_hash: "abc".into(),
        document_ids: vec!["d1".into(), "d2".into()],
        titles: vec!["A".into(), "B".into()],
        count: 2,
    }];
    report.stale_wiki = vec![StaleWikiPage {
        wiki_document_id: "w1".into(),
        wiki_title: "Wiki".into(),
        wiki_updated_at: Utc::now(),
        raw_document_id: "r1".into(),
        raw_title: "Raw".into(),
        raw_updated_at: Some(Utc::now()),
        link_kind: "graph_related".into(),
    }];
    report.near_duplicates = vec![NearDuplicatePair {
        document_id_a: "n1".into(),
        document_id_b: "n2".into(),
        title_a: "N1".into(),
        title_b: "N2".into(),
        cosine: 0.99,
    }];
    report.archive_candidates = vec![ArchiveCandidate {
        document_id: "a1".into(),
        title: "Old".into(),
        layer: "raw".into(),
        boost: 0.2,
        updated_at: Utc::now(),
        reason: "low boost".into(),
    }];
    report.issues = vec![AnalysisIssue {
        code: "exact_dup".into(),
        severity: crate::models::IssueSeverity::Warn,
        message: "dup".into(),
        entity_id: Some("d1".into()),
    }];
    report.issue_count = 6;

    let plan = heuristic_plan(&report, 30);
    let names: Vec<&str> = plan.actions.iter().map(|a| a.action.as_str()).collect();
    assert!(names.contains(&"reembed_all"), "{names:?}");
    assert!(names.contains(&"reindex_fts"), "{names:?}");
    assert!(names.contains(&"merge_exact_dup"), "{names:?}");
    assert!(names.contains(&"refresh_stale_wiki"), "{names:?}");
    assert!(names.contains(&"merge_near_dup"), "{names:?}");
    assert!(names.contains(&"archive"), "{names:?}");
    assert!(names.contains(&"rebuild_index"), "{names:?}");
    assert!(names.contains(&"vacuum"), "{names:?}");
    assert!(plan.rejected.is_empty(), "{:?}", plan.rejected);
    for a in &plan.actions {
        assert!(is_allowed_action(a.action.as_str()));
    }
}

#[test]
fn heuristic_respects_max_actions() {
    let mut report = empty_report();
    for i in 0..20 {
        report.archive_candidates.push(ArchiveCandidate {
            document_id: format!("a{i}"),
            title: format!("T{i}"),
            layer: "raw".into(),
            boost: 0.1,
            updated_at: Utc::now(),
            reason: "old".into(),
        });
    }
    report.issue_count = 20;
    let plan = heuristic_plan(&report, 5);
    assert!(plan.action_count <= 5);
}

#[test]
fn json_schema_is_object() {
    let s = maintenance_plan_json_schema();
    assert!(s.is_object());
    let s2 = maintenance_actions_json_schema();
    assert!(s2.is_object());
}

#[test]
fn validate_named_variants_from_task() {
    let plan = validate_plan(vec![
        MaintenancePlanItem {
            action: MaintenanceAction::Refile,
            reason: None,
            target_id: Some("d1".into()),
            params: serde_json::json!({"wing": "w"}),
        },
        MaintenancePlanItem {
            action: MaintenanceAction::Archive,
            reason: None,
            target_id: Some("d2".into()),
            params: empty_object(),
        },
        MaintenancePlanItem {
            action: MaintenanceAction::Pin,
            reason: None,
            target_id: Some("d3".into()),
            params: empty_object(),
        },
        MaintenancePlanItem {
            action: MaintenanceAction::MergeExactDup,
            reason: None,
            target_id: Some("keep".into()),
            params: serde_json::json!({"document_ids": ["keep", "drop"]}),
        },
        MaintenancePlanItem {
            action: MaintenanceAction::RebuildIndex,
            reason: None,
            target_id: None,
            params: empty_object(),
        },
        MaintenancePlanItem {
            action: MaintenanceAction::Vacuum,
            reason: None,
            target_id: None,
            params: empty_object(),
        },
        MaintenancePlanItem {
            action: MaintenanceAction::ReindexFts,
            reason: None,
            target_id: None,
            params: empty_object(),
        },
        MaintenancePlanItem {
            action: MaintenanceAction::ConsolidatePropose,
            reason: None,
            target_id: None,
            params: serde_json::json!({"source_ids": ["a", "b"]}),
        },
        MaintenancePlanItem {
            action: MaintenanceAction::ResolveStub,
            reason: None,
            target_id: Some("stub".into()),
            params: serde_json::json!({"target_document_id": "doc"}),
        },
    ])
    .unwrap();
    assert_eq!(plan.len(), 9);
}

#[tokio::test]
async fn plan_maintenance_heuristic_when_llm_disabled() {
    let cfg = Config {
        db_path: std::path::PathBuf::from("./x.duckdb"),
        embedding_model: "mock".into(),
        embedding_dims: 8,
        llm_enabled: false,
        llm_timeout_secs: 30,
        llm_max_tokens: 512,
        ..Config::for_tests()
    };
    let report = empty_report();
    let opts = PlanOptions {
        max_actions: 10,
        force_heuristic: false,
        log_ops: false,
    };
    let plan = plan_maintenance(&report, &cfg, None, &opts).await.unwrap();
    assert_eq!(plan.source, PlanSource::Heuristic);
    assert!(!plan.llm_enabled);
    assert_eq!(plan.actions[0].action, MaintenanceAction::Noop);
}
