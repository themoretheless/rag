//! FTS overwrite under an abrupt process exit, on a composed store: the WAL
//! must replay without losing commits of the store and of the
//! code-intelligence tables. Moved here from `db::fts::tests` when the store
//! became its own crate (it needs `code_index`, `gate`, `calibration` and
//! `analysis_tasks`, which sit above the store).

use chrono::Utc;

use crate::db::{
    ensure_fts, fts_generation_state, fts_status, load_fts_extension, reindex, search_bm25,
    FtsBackend, FtsGenerationState, LexFilters,
};
use crate::models::{Chunk, Document};
use crate::testkit::open_test_store;
use crate::Store;

fn seed_docs(store: &Store) {
    let now = Utc::now();
    let docs = [
        (
            "d1",
            "file://cats.md",
            "Cats",
            "projects",
            "alpha",
            "raw",
            "document",
        ),
        (
            "d2",
            "file://dogs.md",
            "Dogs",
            "projects",
            "beta",
            "raw",
            "document",
        ),
        (
            "d3",
            "wiki://felines",
            "Felines",
            "archive",
            "misc",
            "wiki",
            "wiki",
        ),
    ];
    for (id, uri, title, wing, room, layer, kind) in docs {
        store
            .documents()
            .upsert_document(&Document {
                id: id.into(),
                uri: uri.into(),
                title: title.into(),
                content: title.into(),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                wing: Some(wing.into()),
                room: Some(room.into()),
                layer: layer.into(),
                kind: kind.into(),
                ..Default::default()
            })
            .unwrap();
    }

    let chunks = [
        (
            "c1",
            "d1",
            0,
            "The domestic cat is a small carnivorous mammal.",
        ),
        ("c2", "d1", 1, "Cats hunt mice and birds at night."),
        (
            "c3",
            "d2",
            0,
            "Dogs are domesticated descendants of the wolf.",
        ),
        (
            "c4",
            "d3",
            0,
            "Feline wiki: big cats and domestic cats overview.",
        ),
    ];
    let mut batch = Vec::new();
    for (id, doc, idx, content) in chunks {
        batch.push(Chunk {
            id: id.into(),
            document_id: doc.into(),
            chunk_index: idx,
            content: content.into(),
            embedding: vec![0.0; 4],
            char_start: 0,
            char_end: content.len() as i32,
            metadata_json: "{}".into(),
        });
    }
    store.chunks().insert_chunks(&batch).unwrap();
}

fn lifecycle(store: &Store) -> FtsGenerationState {
    let conn = store.lock().unwrap();
    fts_generation_state(&conn).unwrap()
}

#[test]
fn wal_replay_crash_child() {
    let Some(path) = std::env::var_os("RAG_TEST_FTS_CRASH_DB") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let store = open_test_store(&path).unwrap();
    let extension_available;
    {
        let conn = store.lock().unwrap();
        extension_available = load_fts_extension(&conn);
        let state = ensure_fts(&conn).unwrap();
        if extension_available {
            assert_eq!(state.backend, FtsBackend::DuckDbBm25);
        }
    }
    // Match production startup: the existing FTS schema is already checkpointed.
    // Replacing only WAL-created objects does not reproduce the catalog bug.
    store.maintenance().checkpoint().unwrap();
    seed_docs(&store);
    store
        .documents()
        .upsert_document(&Document {
            id: "wal-code".into(),
            uri: "file:///fixture/main.rs".into(),
            source_file: Some("/fixture/main.rs".into()),
            content: "fn preserved() {}".into(),
            ..Default::default()
        })
        .unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    crate::analysis_tasks::AnalysisTasks::new(&store)
        .analysis_task_set(
            "wal",
            &crate::analysis_tasks::TaskSet {
                name: Some("preserved".into()),
                ..Default::default()
            },
        )
        .unwrap();
    crate::analysis_tasks::AnalysisTasks::new(&store)
        .analysis_note(
            "wal",
            "fixture",
            &crate::analysis_tasks::Note {
                text: "committed before crash".into(),
                ..Default::default()
            },
        )
        .unwrap();
    crate::gate::Gate::new(&store)
        .analysis_gate(
            "wal",
            &crate::gate::Payload {
                hook_event_name: "SessionStart".into(),
                ..Default::default()
            },
            crate::gate::Mode::Warn,
            &[],
        )
        .unwrap();
    crate::calibration::Calibrations::new(&store)
        .calibrate_confidence(&Default::default())
        .unwrap();
    crate::db::fts::test_hooks::FTS_CRASH_ARMED.store(true, std::sync::atomic::Ordering::SeqCst);
    let backend;
    let (tables, functions) = {
        let conn = store.lock().unwrap();
        backend = reindex(&conn).unwrap().backend.as_str().to_string();
        if extension_available {
            assert_eq!(backend, "duckdb_bm25");
        }
        let mut stmt=conn.prepare("SELECT table_name FROM information_schema.tables WHERE table_schema='fts_main_chunks' ORDER BY table_name").unwrap();
        let tables = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<duckdb::Result<Vec<_>>>()
            .unwrap();
        let mut stmt=conn.prepare("SELECT function_name,function_type FROM duckdb_functions() WHERE schema_name='fts_main_chunks' ORDER BY function_name").unwrap();
        let functions = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<duckdb::Result<Vec<_>>>()
            .unwrap();
        (tables, functions)
    };
    std::fs::write(path.with_extension("ready.tmp"),serde_json::to_string(&serde_json::json!({"phase":"complete","backend":backend,"extension_available":extension_available,"tables":tables,"functions":functions})).unwrap()).unwrap();
    std::fs::rename(
        path.with_extension("ready.tmp"),
        path.with_extension("ready.json"),
    )
    .unwrap();
    // The parent kills this live process. Neither Rust drops nor C++ atexit
    // handlers may checkpoint away the WAL under test.
    loop {
        std::thread::park();
    }
}

#[test]
fn fts_overwrite_wal_replays_after_abrupt_process_exit_without_losing_commits() {
    for phase in [
        "dirty",
        "leaf",
        "leaves",
        "schema",
        "built",
        "published",
        "complete",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash.duckdb");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::wal_replay_tests::wal_replay_crash_child",
                "--nocapture",
            ])
            .env("RAG_TEST_FTS_CRASH_DB", &path)
            .env("RAG_TEST_FTS_CRASH_PHASE", phase)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !path.with_extension("ready.json").exists() {
            if child.try_wait().unwrap().is_some() {
                let out = child.wait_with_output().unwrap();
                panic!(
                    "crash child exited before readiness: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("crash child readiness timeout");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.kill().unwrap();
        let _ = child.wait_with_output().unwrap();
        let catalog = std::fs::read_to_string(path.with_extension("ready.json")).unwrap();
        let checkpoint: serde_json::Value = serde_json::from_str(&catalog).unwrap();
        let store = open_test_store(&path)
            .unwrap_or_else(|e| panic!("WAL replay failed: {e}; catalog={catalog}"));
        if checkpoint["extension_available"] == true {
            assert_eq!(checkpoint["phase"], phase);
            if let Some(expected) = checkpoint["chunks_generation"].as_u64() {
                assert_eq!(lifecycle(&store).chunks_generation, expected);
            }
            if !matches!(phase, "published" | "complete") {
                assert!(lifecycle(&store).dirty);
            }
        }
        assert_eq!(
            store
                .documents()
                .get_document("wal-code")
                .unwrap()
                .unwrap()
                .content,
            "fn preserved() {}"
        );
        assert_eq!(
            crate::analysis_tasks::AnalysisTasks::new(&store)
                .analysis_notes("wal", &Default::default())
                .unwrap()["total_matching"],
            1
        );
        assert_eq!(
            crate::analysis_tasks::AnalysisTasks::new(&store)
                .analysis_task_stats("wal", &Default::default())
                .unwrap()["gate"]["decisions"],
            1
        );
        assert_eq!(
            crate::calibration::Calibrations::new(&store)
                .calibration_reports(&Default::default())
                .unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::code_index::CodeIndex::new(&store)
                .code_snapshot()
                .unwrap()
                .symbols[0]
                .name,
            "preserved"
        );
        let hits = search_bm25(&store, "cat", 10, &LexFilters::default()).unwrap();
        assert!(!hits.is_empty());
        {
            let conn = store.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT CAST(embedding AS VARCHAR) FROM chunks ORDER BY id")
                .unwrap();
            let values = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<duckdb::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(values.len(), 4);
            for value in values {
                assert_eq!(
                    serde_json::from_str::<Vec<f32>>(&value).unwrap(),
                    vec![0.0; 4]
                );
            }
            if checkpoint["extension_available"] == true {
                let direct: Option<f64> = conn
                    .query_row(
                        "SELECT MAX(fts_main_chunks.match_bm25(id,'cat')) FROM chunks",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert!(
                    direct.is_some_and(|score| score > 0.0),
                    "physical BM25 must work, not just its fallback"
                );
                assert_eq!(
                    fts_status(&conn).unwrap().unwrap().backend,
                    FtsBackend::DuckDbBm25
                );
            }
        }
        eprintln!("crash recovery exercised {catalog}");
        if checkpoint["extension_available"] == false {
            eprintln!("FTS unavailable; only fallback replay exercised");
            break;
        }
    }
}
