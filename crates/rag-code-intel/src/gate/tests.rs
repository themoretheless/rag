use super::*;
use crate::{analysis_tasks::TaskSet, models::Document};
struct Fixture {
    _root: tempfile::TempDir,
    _db: tempfile::TempDir,
    root: PathBuf,
    path: PathBuf,
    store: Store,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("main.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let db = tempfile::tempdir().unwrap();
        let store = crate::testkit::open_test_store(&db.path().join("gate.duckdb")).unwrap();
        store
            .documents()
            .upsert_document(&Document {
                id: "file".into(),
                uri: format!("file://{}", path.display()),
                source_file: Some(path.display().to_string()),
                content: "fn main() {}\n".into(),
                ..Default::default()
            })
            .unwrap();
        store.lock().unwrap().execute("INSERT INTO source_manifest VALUES (?,?,13,0,?,'file',current_timestamp,current_timestamp)",params![path.display().to_string(),root.display().to_string(),content_hash("fn main() {}\n")]).unwrap();
        crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap();
        crate::analysis_tasks::AnalysisTasks::new(&store)
            .analysis_task_set(
                "s",
                &TaskSet {
                    name: Some("audit".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        Self {
            _root: dir,
            _db: db,
            root,
            path,
            store,
        }
    }
    fn call(&self, mode: Mode) -> Value {
        crate::gate::Gate::new(&self.store)
            .analysis_gate(
                "s",
                &Payload {
                    tool_name: "Bash".into(),
                    tool_input: json!({"command":"rg SECRET_COMMAND_INPUT main.rs"}),
                    cwd: self.root.display().to_string(),
                    ..Default::default()
                },
                mode,
                std::slice::from_ref(&self.root),
            )
            .unwrap()
    }
    fn use_index(&self, ok: bool, records: Option<i64>) {
        let id = crate::analysis_tasks::AnalysisTasks::new(&self.store)
            .analysis_call_start("s", "symbol")
            .unwrap();
        crate::analysis_tasks::AnalysisTasks::new(&self.store)
            .analysis_call_finish(id, ok, records)
            .unwrap();
    }
}
#[test]
fn shell_ast_ignores_strings_comments_and_recognizes_real_commands() {
    for (cmd, kind) in [
        ("echo 'rg foo'", "other"),
        ("printf grep", "other"),
        ("git status", "other"),
        ("git grep foo", "file_search"),
        ("git -C /repo grep foo", "file_search"),
        ("Get-ChildItem -Recurse -Filter *.rs", "file_search"),
        ("# rg foo\ncargo test", "other"),
        ("ls README.md", "other"),
        ("rg foo | sort", "file_search"),
        ("grep x file", "file_search"),
        ("env FOO=bar rg x", "file_search"),
        ("bash -c 'rg foo'", "file_search"),
        ("ls -laR", "file_search"),
        ("echo $(rg foo)", "file_search"),
        ("$PROGRAM foo", "unknown_shell"),
        ("sudo -u someone rg x", "unknown_shell"),
    ] {
        let c = classify(&Payload {
            tool_name: "Bash".into(),
            tool_input: json!({"command":cmd}),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(c.kind, kind, "{cmd}");
    }
    let c = classify(&Payload {
        tool_name: "Read".into(),
        tool_input: json!({"file_path":"main.rs"}),
        ..Default::default()
    })
    .unwrap();
    assert!(c.is_code);
    assert_eq!(
        classify(&Payload {
            tool_name: "ragraf_fake".into(),
            ..Default::default()
        })
        .unwrap()
        .kind,
        "other"
    );
}
#[test]
fn real_completed_nonempty_calls_required_and_ask_escalates_per_task() {
    let f = Fixture::new();
    let r = f.call(Mode::Warn);
    assert_eq!(r["verdict"], "allow");
    assert_eq!(r["facts"]["fresh"], true);
    assert_eq!(r["rule"], "index_unused");
    let fake = Payload {
        tool_name: "ragraf_symbol".into(),
        ..Default::default()
    };
    crate::gate::Gate::new(&f.store)
        .analysis_gate("s", &fake, Mode::Warn, std::slice::from_ref(&f.root))
        .unwrap();
    f.use_index(false, Some(10));
    f.use_index(true, Some(0));
    crate::analysis_tasks::AnalysisTasks::new(&f.store)
        .analysis_call_start("s", "symbol")
        .unwrap();
    assert_eq!(f.call(Mode::Ask)["verdict"], "ask");
    assert_eq!(f.call(Mode::Ask)["verdict"], "deny");
    crate::analysis_tasks::AnalysisTasks::new(&f.store)
        .analysis_task_set(
            "s",
            &TaskSet {
                name: Some("new".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(f.call(Mode::Ask)["verdict"], "ask");
    f.use_index(true, Some(1));
    let r = f.call(Mode::Ask);
    assert_eq!(r["verdict"], "allow");
    assert_eq!(r["rule"], "index_used");
    let stats = crate::analysis_tasks::AnalysisTasks::new(&f.store)
        .analysis_task_stats("s", &Default::default())
        .unwrap();
    assert_eq!(stats["gate"]["decisions"], 2);
    assert_eq!(stats["gate"]["counters"]["ask"], 1);
    let text: String = f
        .store
        .lock()
        .unwrap()
        .query_row(
            "SELECT facts_json FROM analysis_gate_decisions ORDER BY seq DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!text.contains("SECRET_COMMAND_INPUT"));
    crate::gate::Gate::new(&f.store)
        .analysis_gate(
            "s",
            &Payload {
                hook_event_name: "SessionStart".into(),
                ..Default::default()
            },
            Mode::Warn,
            std::slice::from_ref(&f.root),
        )
        .unwrap();
    let reset = f.call(Mode::Ask);
    assert_eq!(reset["verdict"], "ask");
    assert_eq!(reset["facts"]["index_used"], false);
}
#[test]
fn changed_bytes_even_same_length_and_old_revision_never_qualify() {
    let f = Fixture::new();
    f.use_index(true, Some(1));
    assert_eq!(f.call(Mode::Deny)["verdict"], "allow");
    std::fs::write(&f.path, "fn next() {}\n").unwrap();
    let r = f.call(Mode::Deny);
    assert_eq!(r["verdict"], "deny");
    assert_eq!(r["rule"], "stale_index");
    let mut doc = f.store.documents().get_document("file").unwrap().unwrap();
    doc.content = "fn next() {}\n".into();
    f.store.documents().upsert_document(&doc).unwrap();
    crate::code_index::CodeIndex::new(&f.store)
        .index_code()
        .unwrap();
    let r = f.call(Mode::Deny);
    assert_eq!(r["facts"]["fresh"], true);
    assert_eq!(r["rule"], "index_unused");
    assert_eq!(r["verdict"], "deny");
}
#[test]
fn bom_sources_use_decoded_checksums_but_charge_original_byte_budget() {
    let f = Fixture::new();
    let bytes = [
        vec![0xff, 0xfe],
        "fn main() {}\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect(),
    ]
    .concat();
    std::fs::write(&f.path, &bytes).unwrap();
    f.use_index(true, Some(1));
    let result = f.call(Mode::Deny);
    assert_eq!(result["facts"]["fresh"], true, "{result}");
    assert_eq!(result["verdict"], "allow");
    assert_eq!(result["facts"]["checked_bytes"], bytes.len());
    assert_eq!(
        result["facts"]["checked_bytes_basis"],
        "original_file_bytes"
    );
    std::fs::write(&f.path, [0xff, 0xfe, 0]).unwrap();
    let result = f.call(Mode::Deny);
    assert_eq!(result["rule"], "freshness_unknown");
    assert!(result["facts"]["fresh"].is_null());
}

#[test]
fn unavailable_sources_do_not_claim_fresh_and_reads_remain_allowed() {
    let f = Fixture::new();
    std::fs::write(&f.path, vec![b'a'; 4 * 1024 * 1024 + 1]).unwrap();
    let r = f.call(Mode::Deny);
    assert!(r["facts"]["fresh"].is_null());
    assert_eq!(r["rule"], "freshness_unknown");
    assert_eq!(r["verdict"], "deny");
    let p = Payload {
        tool_name: "Read".into(),
        cwd: f.root.display().to_string(),
        tool_input: json!({"path":"main.rs"}),
        ..Default::default()
    };
    let r = crate::gate::Gate::new(&f.store)
        .analysis_gate("s", &p, Mode::Deny, std::slice::from_ref(&f.root))
        .unwrap();
    assert_eq!(r["verdict"], "allow");
    let r = crate::gate::Gate::new(&f.store)
        .analysis_gate("s", &p, Mode::Off, &[])
        .unwrap();
    assert_eq!(r["verdict"], "off");
    assert_eq!(r["facts"]["checked_bytes"], 0);
}
#[test]
fn sessions_time_windows_and_missing_files_are_explicit() {
    let f = Fixture::new();
    f.use_index(true, Some(1));
    f.store
        .lock()
        .unwrap()
        .execute("UPDATE analysis_calls SET finished_ms=0", [])
        .unwrap();
    assert_eq!(f.call(Mode::Deny)["rule"], "index_unused");
    std::fs::remove_file(&f.path).unwrap();
    assert_eq!(f.call(Mode::Deny)["facts"]["fresh"], false);
    let r = crate::gate::Gate::new(&f.store)
        .analysis_gate(
            "different",
            &Payload {
                tool_name: "Bash".into(),
                cwd: f.root.display().to_string(),
                tool_input: json!({"command":"rg x"}),
                ..Default::default()
            },
            Mode::Ask,
            std::slice::from_ref(&f.root),
        )
        .unwrap();
    assert_eq!(r["verdict"], "ask");
}
#[test]
fn oversized_manifest_is_unknown_after_bounded_live_reads() {
    let f = Fixture::new();
    for i in 0..MAX_FILES {
        let path = f.root.join(format!("{i:03}.txt"));
        std::fs::write(&path, "small").unwrap();
        f.store
            .documents()
            .upsert_document(&Document {
                id: format!("extra-{i}"),
                uri: format!("file://{}", path.display()),
                source_file: Some(path.display().to_string()),
                content: "small".into(),
                ..Default::default()
            })
            .unwrap();
    }
    crate::code_index::CodeIndex::new(&f.store)
        .index_code()
        .unwrap();
    let r = f.call(Mode::Deny);
    assert!(r["facts"]["fresh"].is_null(), "{r}");
    assert_eq!(r["facts"]["checked_files"], MAX_FILES);
    assert_eq!(r["verdict"], "deny");
    assert!(r["facts"]["freshness_reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "LIVE_FILE_BUDGET_EXCEEDED"));
}

#[test]
fn hook_contract_does_not_execute_or_rewrite_calls_and_input_is_bounded() {
    let d = Decision {
        verdict: "deny".into(),
        rule: "test".into(),
        reason: "denied".into(),
        context: "context".into(),
        facts: json!({}),
    };
    assert_eq!(d.hook()["exit_code"], 2);
    assert_eq!(d.hook()["stderr"], "denied");
    assert_eq!(d.hook()["stdout"], "");
    let ask = Decision {
        verdict: "ask".into(),
        ..d
    };
    let hook = ask.hook();
    let out: Value = serde_json::from_str(hook["stdout"].as_str().unwrap()).unwrap();
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "ask");
    assert!(out["hookSpecificOutput"].get("updatedInput").is_none());
    assert!(Mode::parse("silent_fallback").is_err());
    assert!(parse_payload(&json!({})).is_err());
    assert!(parse_payload(&json!({"tool_name":"Bash","tool_input":"x".repeat(65536)})).is_err());
}
