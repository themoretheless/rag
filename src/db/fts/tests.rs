use super::*;

/// dict (term, df), docs (name, len), stats (num_docs, avgdl), postings.
type Bm25IndexView = (
    Vec<(String, i64)>,
    Vec<(String, i64)>,
    (i64, f64),
    Vec<(String, i64)>,
);

/// Index contents that determine BM25 scores, independent of id numbering.
fn bm25_index_view(conn: &Connection) -> Bm25IndexView {
    let pairs = |sql: &str| -> Vec<(String, i64)> {
        let mut stmt = conn.prepare(sql).unwrap();
        crate::db::rows::query_vec(&mut stmt, [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
    };
    let dict = pairs("SELECT term, df FROM fts_main_chunks.dict ORDER BY term");
    let docs = pairs("SELECT name, len FROM fts_main_chunks.docs ORDER BY name");
    let stats = conn
        .query_row(
            "SELECT num_docs, avgdl FROM fts_main_chunks.stats",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let postings = pairs(
        "SELECT d.name || ':' || dict.term, COUNT(*) FROM fts_main_chunks.terms t \
             JOIN fts_main_chunks.docs d ON d.docid = t.docid \
             JOIN fts_main_chunks.dict dict ON dict.termid = t.termid \
             GROUP BY 1 ORDER BY 1",
    );
    (dict, docs, stats, postings)
}

fn bm25_scores(conn: &Connection, query: &str) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT id, CAST(ROUND(fts_main_chunks.match_bm25(id, ?), 9) AS VARCHAR) AS s \
                 FROM chunks WHERE s IS NOT NULL ORDER BY id",
        )
        .unwrap();
    crate::db::rows::query_vec(&mut stmt, [query], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
}

#[test]
fn forced_rebuild_flag_bypasses_the_incremental_patch_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("fts-forced.duckdb")).unwrap();
    let conn = store.lock().unwrap();
    if !load_fts_extension(&conn) {
        eprintln!("skipping: DuckDB FTS extension unavailable");
        return;
    }
    conn.execute_batch(
        "INSERT INTO documents (id, uri, title, content, metadata_json, created_at, \
               updated_at, layer, kind, status) \
             VALUES ('d', 'uri://d', 't', 'b', '{}', now(), now(), 'raw', 'document', 'active'); \
             INSERT INTO chunks (id, document_id, chunk_index, content, embedding_json, \
               char_start, char_end, created_at, metadata_json, embedding_digest) \
             SELECT 'c' || lpad(i::VARCHAR, 3, '0'), 'd', i, 'alpha beta ' || i, '[0.1]', \
               0, 1, now(), '{}', i FROM range(20) t(i);",
    )
    .unwrap();
    reindex(&conn).unwrap();
    // Same id, different text: invisible to an id diff.
    conn.execute_batch(
        "DELETE FROM chunks WHERE id = 'c005'; \
             INSERT INTO chunks (id, document_id, chunk_index, content, embedding_json, \
               char_start, char_end, created_at, metadata_json, embedding_digest) \
             VALUES ('c005', 'd', 5, 'zebra quokka', '[0.1]', 0, 1, now(), '{}', 5);",
    )
    .unwrap();
    mark_fts_needs_rebuild(&conn).unwrap();
    refresh_fts_if_stale(&conn).unwrap();
    let hits: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks \
                 WHERE fts_main_chunks.match_bm25(id, 'quokka') IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(hits, 1, "the rebuilt index sees the replaced text");
    assert_eq!(
        read_meta(&conn, META_FORCE_REBUILD).unwrap().as_deref(),
        Some("0")
    );
}

#[test]
fn incremental_bm25_update_matches_a_full_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("fts-incremental.duckdb")).unwrap();
    let conn = store.lock().unwrap();
    if !load_fts_extension(&conn) {
        eprintln!("skipping: DuckDB FTS extension unavailable");
        return;
    }
    conn.execute_batch(
        "INSERT INTO documents (id, uri, title, content, metadata_json, created_at, \
               updated_at, layer, kind, status) \
             SELECT 'd' || i, 'uri://' || i, 't', 'b', '{}', now(), now(), 'raw', \
               'document', 'active' FROM range(8) t(i); \
             INSERT INTO chunks (id, document_id, chunk_index, content, embedding_json, \
               char_start, char_end, created_at, metadata_json, embedding_digest) \
             SELECT 'c' || lpad(i::VARCHAR, 3, '0'), 'd' || (i % 8), i, \
               'The runner ' || i || ' was running over ' || (i % 7) || ' hills; café-Résumé \
               jumping jumps word' || (i % 11) || ' and the end', \
               '[0.1]', 0, 1, now(), '{}', i FROM range(60) t(i);",
    )
    .unwrap();
    reindex(&conn).unwrap();
    let docid_of = |conn: &Connection, name: &str| -> i64 {
        conn.query_row(
            "SELECT docid FROM fts_main_chunks.docs WHERE name = ?",
            [name],
            |row| row.get(0),
        )
        .unwrap()
    };
    let untouched_docid = docid_of(&conn, "c050");

    // A write burst: remove some chunks, add new ones (incl. empty and
    // stopword-only text and brand-new terms).
    conn.execute_batch(
            "DELETE FROM chunks WHERE id IN ('c003', 'c017', 'c044'); \
             INSERT INTO chunks (id, document_id, chunk_index, content, embedding_json, \
               char_start, char_end, created_at, metadata_json, embedding_digest) VALUES \
             ('n1', 'd1', 100, 'Zebras galloping across savannas, runners running', '[0.1]', 0, 1, now(), '{}', 1), \
             ('n2', 'd2', 101, '', '[0.1]', 0, 1, now(), '{}', 2), \
             ('n3', 'd3', 102, 'the and of to', '[0.1]', 0, 1, now(), '{}', 3), \
             ('n4', 'd4', 103, 'hills hills hills 42 café', '[0.1]', 0, 1, now(), '{}', 4);",
        )
        .unwrap();
    mark_fts_dirty(&conn).unwrap();
    let state = refresh_fts_if_stale(&conn).unwrap();
    assert_eq!(state.backend, FtsBackend::DuckDbBm25);
    assert!(!fts_generation_state(&conn).unwrap().dirty);
    // Patched in place: a full rebuild would renumber after the deletions.
    assert_eq!(docid_of(&conn, "c050"), untouched_docid);
    let incremental = bm25_index_view(&conn);
    let queries = [
        "running hills",
        "zebra savanna",
        "cafe resume",
        "word3 jumping",
        "42",
    ];
    let incremental_scores: Vec<_> = queries.iter().map(|q| bm25_scores(&conn, q)).collect();

    reindex(&conn).unwrap();
    let rebuilt = bm25_index_view(&conn);
    let rebuilt_scores: Vec<_> = queries.iter().map(|q| bm25_scores(&conn, q)).collect();

    assert_eq!(
        incremental.0, rebuilt.0,
        "dict terms and document frequencies"
    );
    assert_eq!(incremental.1, rebuilt.1, "documents and lengths");
    assert_eq!(incremental.2 .0, rebuilt.2 .0, "num_docs");
    assert!((incremental.2 .1 - rebuilt.2 .1).abs() < 1e-9, "avgdl");
    assert_eq!(incremental.3, rebuilt.3, "postings");
    assert_eq!(incremental_scores, rebuilt_scores, "BM25 scores");
}

#[test]
fn lexical_and_vector_legs_share_one_trimmed_scope() {
    let filters = LexFilters {
        wing: Some(" rag ".into()),
        room: Some("   ".into()),
        document_id: Some(" doc-1".into()),
        ..LexFilters::default()
    };
    let (lexical_sql, lexical_bind) = filters.scope_where_clause("c.document_id");
    let (vector_sql, vector_bind) = filters.scope_where_clause("d.id");
    assert_eq!(lexical_bind, vec!["doc-1".to_string(), "rag".to_string()]);
    assert_eq!(lexical_bind, vector_bind);
    assert!(lexical_sql.contains("c.document_id = ?") && vector_sql.contains("d.id = ?"));
    assert!(
        !lexical_sql.contains("d.room"),
        "blank scope values are ignored"
    );
}
use crate::db::schema;
use crate::models::{Chunk, Document};
use chrono::Utc;
use tempfile::TempDir;

fn open_store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fts.duckdb");
    let store = Store::open(&path).expect("open");
    (dir, store)
}

fn state(chunks: u64, index: Option<u64>, dirty: bool) -> FtsGenerationState {
    FtsGenerationState {
        chunks_generation: chunks,
        index_generation: index,
        dirty,
        rebuild_count: 0,
    }
}

#[test]
fn stale_threshold_policy() {
    // Clean index never reindexes, whatever the threshold.
    assert!(!fts_stale_needs_reindex(&state(10, Some(10), false), 1));
    // Disabled threshold never reindexes.
    assert!(!fts_stale_needs_reindex(&state(100, Some(0), true), 0));
    // Gap below threshold: not yet.
    assert!(!fts_stale_needs_reindex(&state(70, Some(10), true), 64));
    // Gap at threshold: reindex.
    assert!(fts_stale_needs_reindex(&state(74, Some(10), true), 64));
    // Dirty with no completed index generation always qualifies.
    assert!(fts_stale_needs_reindex(&state(1, None, true), 64));
    // Saturating gap arithmetic: index ahead of chunks is gap zero.
    assert_eq!(fts_generation_gap(&state(5, Some(9), false)), Some(0));
    assert_eq!(fts_generation_gap(&state(5, None, true)), None);
}

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
    let store = Store::open(&path).unwrap();
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
    FTS_CRASH_ARMED.store(true, std::sync::atomic::Ordering::SeqCst);
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
                "db::fts::tests::wal_replay_crash_child",
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
        let store = Store::open(&path)
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
                .prepare("SELECT embedding_json FROM chunks ORDER BY id")
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

#[test]
fn tokenize_splits_and_lowercases() {
    assert_eq!(
        tokenize("Hello, WORLD! cat-nap 42"),
        vec!["hello", "world", "cat", "nap", "42"]
    );
}

#[test]
fn term_frequency_ranks_relevant_higher() {
    let q = tokenize("cat mammal");
    let a = term_frequency_score(&q, "The domestic cat is a small carnivorous mammal.");
    let b = term_frequency_score(&q, "Dogs are domesticated descendants of the wolf.");
    assert!(a > b, "expected cat/mammal doc higher: {a} vs {b}");
    assert!((a - 2.0).abs() < 1e-6); // cat + mammal
    assert_eq!(b, 0.0);
}

#[test]
fn ensure_and_reindex_are_idempotent() {
    let conn = Connection::open_in_memory().unwrap();
    schema::migrate(&conn).unwrap();
    // empty chunks table is fine for index create
    let s1 = ensure_fts(&conn).expect("ensure");
    let s2 = reindex(&conn).expect("reindex");
    assert_eq!(s1.backend, s2.backend);
    assert_eq!(s1.stemmer, s2.stemmer);
    let status = fts_status(&conn).unwrap().expect("status");
    assert_eq!(status.backend, s1.backend);
    match s1.backend {
        FtsBackend::DuckDbBm25 => assert!(fts_index_present(&conn)),
        FtsBackend::TermFrequency => {
            // schema may be absent under fallback
        }
    }
}

#[test]
fn clean_warm_search_does_not_rebuild_fts() {
    let (_dir, store) = open_store();
    seed_docs(&store);
    {
        let conn = store.lock().unwrap();
        ensure_fts(&conn).unwrap();
    }
    let initialized = lifecycle(&store);
    assert!(!initialized.dirty);
    assert!(initialized.rebuild_count > 0);

    for _ in 0..2 {
        let hits = search_bm25(&store, "cat", 10, &LexFilters::default()).unwrap();
        assert!(!hits.is_empty());
    }

    let warm = lifecycle(&store);
    assert_eq!(warm.chunks_generation, initialized.chunks_generation);
    assert_eq!(warm.index_generation, initialized.index_generation);
    assert_eq!(
        warm.rebuild_count, initialized.rebuild_count,
        "a clean warm search must not execute create_fts_index"
    );
}

#[test]
fn reopened_clean_index_searches_without_rebuild() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fts-reopen.duckdb");
    let (before, backend) = {
        let store = Store::open(&path).unwrap();
        seed_docs(&store);
        let backend = {
            let conn = store.lock().unwrap();
            ensure_fts(&conn).unwrap().backend
        };
        (lifecycle(&store), backend)
    };

    let reopened = Store::open(&path).unwrap();
    if backend == FtsBackend::DuckDbBm25 {
        let conn = reopened.lock().unwrap();
        let direct = search_duckdb_bm25(&conn, "cat", 10, &LexFilters::default()).unwrap();
        assert!(!direct.is_empty());
    }
    let hits = search_bm25(&reopened, "cat", 10, &LexFilters::default()).unwrap();
    assert!(!hits.is_empty());
    let after = lifecycle(&reopened);
    assert_eq!(after.rebuild_count, before.rebuild_count);
    assert!(!after.dirty);
}

#[test]
fn dirty_term_frequency_backend_advances_without_fts_ddl_retry() {
    let conn = Connection::open_in_memory().unwrap();
    schema::migrate(&conn).unwrap();
    let fallback = FtsState {
        backend: FtsBackend::TermFrequency,
        stemmer: DEFAULT_STEMMER.into(),
    };
    publish_refreshed_state(&conn, &fallback, 0).unwrap();
    mark_fts_dirty(&conn).unwrap();
    let before = fts_generation_state(&conn).unwrap();
    assert!(before.dirty);
    assert!(!fts_index_present(&conn));

    let refreshed = refresh_fts_if_stale(&conn).unwrap();
    assert_eq!(refreshed.backend, FtsBackend::TermFrequency);
    let after = fts_generation_state(&conn).unwrap();
    assert!(!after.dirty);
    assert_eq!(after.rebuild_count, before.rebuild_count + 1);
    assert!(
        !fts_index_present(&conn),
        "dirty TF refresh must not retry create_fts_index"
    );
}

#[test]
fn mutation_marks_dirty_and_controlled_refresh_restores_freshness() {
    let (_dir, store) = open_store();
    seed_docs(&store);
    {
        let conn = store.lock().unwrap();
        ensure_fts(&conn).unwrap();
    }
    let before = lifecycle(&store);

    let content = "A quokka is a small marsupial native to Australia.";
    store
        .chunks()
        .insert_chunks(&[Chunk {
            id: "c5".into(),
            document_id: "d1".into(),
            chunk_index: 2,
            content: content.into(),
            embedding: vec![0.0; 4],
            char_start: 0,
            char_end: content.len() as i32,
            metadata_json: "{}".into(),
        }])
        .unwrap();

    let dirty = lifecycle(&store);
    assert!(dirty.dirty);
    assert_eq!(dirty.chunks_generation, before.chunks_generation + 1);
    assert_eq!(dirty.index_generation, before.index_generation);

    {
        let conn = store.lock().unwrap();
        refresh_fts_if_stale(&conn).unwrap();
    }
    let refreshed = lifecycle(&store);
    assert!(!refreshed.dirty);
    assert_eq!(
        refreshed.index_generation,
        Some(refreshed.chunks_generation)
    );
    assert_eq!(refreshed.rebuild_count, before.rebuild_count + 1);

    let hits = search_bm25(&store, "quokka", 10, &LexFilters::default()).unwrap();
    assert!(hits.iter().any(|hit| hit.chunk_id == "c5"));
    assert_eq!(
        lifecycle(&store).rebuild_count,
        refreshed.rebuild_count,
        "search after controlled refresh must stay warm"
    );
}

#[test]
fn concurrent_stale_searches_singleflight_one_refresh() {
    let (_dir, store) = open_store();
    seed_docs(&store);
    {
        let conn = store.lock().unwrap();
        ensure_fts(&conn).unwrap();
        mark_fts_dirty(&conn).unwrap();
    }
    let before = lifecycle(&store);
    assert!(before.dirty);

    let workers = (0..4)
        .map(|_| {
            let store = store.clone();
            std::thread::spawn(move || {
                search_bm25(&store, "cat", 10, &LexFilters::default()).unwrap()
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        assert!(!worker.join().unwrap().is_empty());
    }

    let after = lifecycle(&store);
    assert!(!after.dirty);
    assert_eq!(after.rebuild_count, before.rebuild_count + 1);
}

#[test]
fn search_bm25_finds_keyword_and_respects_top_k() {
    let (_dir, store) = open_store();
    seed_docs(&store);

    {
        let conn = store.lock().unwrap();
        ensure_fts(&conn).unwrap();
        reindex(&conn).unwrap();
    }

    let hits = search_bm25(&store, "cat mammal", 10, &LexFilters::default()).unwrap();
    assert!(!hits.is_empty(), "expected at least one lex hit");
    assert!(
        hits.iter()
            .any(|h| h.chunk_id == "c1" || h.content.to_lowercase().contains("cat")),
        "expected cat-related chunk, got {:?}",
        hits.iter().map(|h| &h.chunk_id).collect::<Vec<_>>()
    );
    for h in &hits {
        assert!(h.score_lex.is_some());
        assert!(h.score > 0.0);
        assert!(h.snippet.is_some());
    }

    let limited = search_bm25(&store, "cat", 1, &LexFilters::default()).unwrap();
    assert!(limited.len() <= 1);
}

#[test]
fn search_bm25_filters_document_and_wing() {
    let (_dir, store) = open_store();
    seed_docs(&store);
    {
        let conn = store.lock().unwrap();
        ensure_fts(&conn).unwrap();
        reindex(&conn).unwrap();
    }

    let by_doc = search_bm25(&store, "cats", 10, &LexFilters::document_id("d1")).unwrap();
    assert!(!by_doc.is_empty());
    assert!(by_doc.iter().all(|h| h.document_id == "d1"));

    let by_wing = search_bm25(
        &store,
        "cats",
        10,
        &LexFilters {
            wing: Some("archive".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(by_wing.iter().all(|h| h.document_id == "d3"));
}

#[test]
fn empty_query_or_top_k_zero_returns_empty() {
    let (_dir, store) = open_store();
    seed_docs(&store);
    assert!(search_bm25(&store, "  ", 5, &LexFilters::default())
        .unwrap()
        .is_empty());
    assert!(search_bm25(&store, "cat", 0, &LexFilters::default())
        .unwrap()
        .is_empty());
}

#[test]
fn stemmer_none_ensure_succeeds() {
    let conn = Connection::open_in_memory().unwrap();
    schema::migrate(&conn).unwrap();
    let state = ensure_fts_with_stemmer(&conn, "none").unwrap();
    assert_eq!(state.stemmer, "none");
}
