//! Abrupt process exit (`SIGKILL`) + `lin` WAL replay test on a composed store:
//! committed writes across documents, chunks, `code_index`, `analysis_tasks`,
//! `gate`, `calibration`, and `search_bm25` must survive ungraceful termination.

use chrono::Utc;

use crate::Store;
use crate::db::{
    FtsBackend, FtsGenerationState, LexFilters, ensure_fts, fts_generation_state, fts_status,
    reindex, search_bm25,
};
use crate::models::{Chunk, Document, DocumentLayer};
use crate::testkit::open_test_store;

fn seed_docs(store: &Store) {
    let now = Utc::now();
    let docs = [
        (
            "d1",
            "file://cats.md",
            "Cats",
            "projects",
            "alpha",
            DocumentLayer::Raw,
            "document",
        ),
        (
            "d2",
            "file://dogs.md",
            "Dogs",
            "projects",
            "beta",
            DocumentLayer::Raw,
            "document",
        ),
        (
            "d3",
            "wiki://felines",
            "Felines",
            "archive",
            "misc",
            DocumentLayer::Wiki,
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
                layer,
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
    let r = store.read().unwrap();
    fts_generation_state(&*r).unwrap()
}

#[test]
fn wal_replay_crash_child() {
    let Some(path) = std::env::var_os("RAG_TEST_FTS_CRASH_DB") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let store = open_test_store(&path).unwrap();
    {
        let mut w = store.write().unwrap();
        let state = ensure_fts(&mut *w).unwrap();
        assert_eq!(state.backend, FtsBackend::DuckDbBm25);
    }
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
    let backend = {
        let mut w = store.write().unwrap();
        reindex(&mut *w).unwrap().backend.as_str().to_string()
    };
    let chunks_gen = lifecycle(&store).chunks_generation;
    std::fs::write(
        path.with_extension("ready.tmp"),
        serde_json::to_string(&serde_json::json!({
            "phase": "complete",
            "backend": backend,
            "chunks_generation": chunks_gen,
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::rename(
        path.with_extension("ready.tmp"),
        path.with_extension("ready.json"),
    )
    .unwrap();
    // Parent kills this live process via SIGKILL.
    loop {
        std::thread::park();
    }
}

#[test]
fn fts_overwrite_wal_replays_after_abrupt_process_exit_without_losing_commits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crash.lin");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "storage::wal_replay_tests::wal_replay_crash_child",
            "--nocapture",
        ])
        .env("RAG_TEST_FTS_CRASH_DB", &path)
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
    if let Some(expected) = checkpoint["chunks_generation"].as_u64() {
        assert_eq!(lifecycle(&store).chunks_generation, expected);
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
    let mut chunk_count = 0usize;
    for doc_id in ["d1", "d2", "d3"] {
        for chunk in store.chunks().list_chunks_for_document(doc_id).unwrap() {
            assert_eq!(chunk.embedding, vec![0.0; 4]);
            chunk_count += 1;
        }
    }
    assert_eq!(chunk_count, 4);
    assert_eq!(
        fts_status(&*store.read().unwrap())
            .unwrap()
            .unwrap()
            .backend,
        FtsBackend::DuckDbBm25
    );
}
