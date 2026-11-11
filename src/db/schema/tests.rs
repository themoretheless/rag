use super::*;
use duckdb::Connection;

fn open_mem() -> Connection {
    Connection::open_in_memory().expect("in-memory duckdb")
}

fn column_names(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("DESCRIBE {table}"))
        .expect("describe");
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query");
    rows.map(|r| r.expect("row")).collect()
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = ?",
            duckdb::params![name],
            |row| row.get(0),
        )
        .unwrap_or(0);
    n > 0
}

fn index_exists(conn: &Connection, name: &str) -> bool {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM duckdb_indexes() WHERE index_name = ?",
            duckdb::params![name],
            |row| row.get(0),
        )
        .unwrap_or(0);
    n > 0
}

#[test]
fn migrate_fresh_creates_p0_columns_and_tables() {
    let conn = open_mem();
    migrate(&conn).expect("migrate");

    let docs = column_names(&conn, "documents");
    for col in [
        "content_hash",
        "wing",
        "room",
        "source_file",
        "layer",
        "kind",
        "pinned",
        "boost",
        "status",
    ] {
        assert!(docs.iter().any(|c| c == col), "missing documents.{col}");
    }

    let chunks = column_names(&conn, "chunks");
    assert!(
        chunks.iter().any(|c| c == "content_hash"),
        "missing chunks.content_hash"
    );
    assert!(
        chunks.iter().any(|c| c == "parent_chunk_id"),
        "missing chunks.parent_chunk_id"
    );
    assert!(index_exists(&conn, "idx_chunks_parent_chunk_id"));

    for t in [
        "ops_log",
        "wiki_index",
        "embedding_manifest",
        "source_manifest",
        "schema_version",
        "meta",
        "graph_nodes",
        "graph_edges",
        "kg_facts",
        "eval_prompts",
    ] {
        assert!(table_exists(&conn, t), "missing table {t}");
    }
    assert!(index_exists(&conn, "idx_graph_nodes_uri"));
    assert!(index_exists(&conn, "idx_documents_uri"));

    let source_manifest = column_names(&conn, "source_manifest");
    for col in [
        "canonical_path",
        "canonical_root",
        "size_bytes",
        "mtime_ns",
        "content_hash",
        "document_id",
        "last_seen",
        "last_synced",
    ] {
        assert!(
            source_manifest.iter().any(|candidate| candidate == col),
            "missing source_manifest.{col}"
        );
    }

    let kg = column_names(&conn, "kg_facts");
    for col in [
        "id",
        "subject",
        "predicate",
        "object",
        "valid_from",
        "valid_to",
        "status",
        "superseded_by",
        "source_document_id",
        "source",
        "confidence",
        "created_at",
        "updated_at",
        "invalidated_at",
        "metadata_json",
    ] {
        assert!(kg.iter().any(|c| c == col), "missing kg_facts.{col}");
    }

    let ver = current_schema_version(&conn).expect("version");
    assert_eq!(ver, Some(SCHEMA_VERSION));
}

#[test]
fn migrate_is_idempotent() {
    let conn = open_mem();
    migrate(&conn).expect("migrate 1");
    migrate(&conn).expect("migrate 2");
    migrate(&conn).expect("migrate 3");

    let ver = current_schema_version(&conn).expect("version");
    assert_eq!(ver, Some(SCHEMA_VERSION));

    // schema_version should not grow unboundedly on re-run at same version
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_version WHERE version = ?",
            duckdb::params![SCHEMA_VERSION],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(n, 1);
}

/// A database that already ran the rel_type-only version of the origin
/// backfill keeps its pipeline membership rows as `explicit`, so the
/// `origin IS NULL` pass never revisits them. §1.6's delete predicate only
/// reaches them once the contextual repair pass re-stamps them.
#[test]
fn origin_backfill_repairs_explicit_membership_rows_from_an_older_migration() {
    let conn = open_mem();
    migrate(&conn).expect("migrate");
    conn.execute(
            "INSERT INTO graph_edges (id, source_id, target_id, rel_type, weight, context, created_at, origin) VALUES \
             ('edge-membership', 'doc-node', 'project-node', 'related', 0.5, 'project membership', CURRENT_TIMESTAMP, 'explicit'), \
             ('edge-hand-made', 'doc-node', 'stub-node', 'related', 1.0, NULL, CURRENT_TIMESTAMP, 'explicit'), \
             ('edge-untouched', 'doc-node', 'tag-node', 'wikilink', 1.0, NULL, CURRENT_TIMESTAMP, 'extract')",
            [],
        )
        .expect("insert already-stamped edges");

    let repaired = backfill_graph_edge_origins(&conn).expect("repair pass");
    assert_eq!(
        repaired, 1,
        "only the mislabeled membership row changes owner"
    );
    assert_eq!(origin_of(&conn, "edge-membership"), "extract");
    assert_eq!(
        origin_of(&conn, "edge-hand-made"),
        "explicit",
        "a hand-made `related` edge keeps its owner"
    );
    assert_eq!(origin_of(&conn, "edge-untouched"), "extract");
}

fn origin_of(conn: &Connection, id: &str) -> String {
    conn.query_row(
        "SELECT origin FROM graph_edges WHERE id = ?",
        duckdb::params![id],
        |row| row.get::<_, String>(0),
    )
    .expect("read origin")
}

#[test]
fn migrate_upgrades_legacy_documents_table() {
    let conn = open_mem();
    // Simulate a pre-P0 database: base tables only, no P0 columns/tables.
    conn.execute_batch(
        &[
            CREATE_DOCUMENTS,
            CREATE_CHUNKS,
            CREATE_IDX_CHUNKS_DOCUMENT_ID,
            CREATE_GRAPH_NODES,
            CREATE_GRAPH_EDGES,
        ]
        .join(";\n"),
    )
    .expect("legacy create");

    conn.execute(
        r#"
            INSERT INTO documents (id, uri, title, content, metadata_json, created_at, updated_at)
            VALUES ('d1', 'u1', 't', 'c', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            "#,
        [],
    )
    .expect("insert legacy doc");
    conn.execute(
        r#"
            INSERT INTO documents (id, uri, title, content, metadata_json, created_at, updated_at)
            VALUES ('d2', 'u1', 't2', 'c2', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            "#,
        [],
    )
    .expect("insert legacy duplicate-uri doc");

    migrate(&conn).expect("upgrade migrate");

    let docs = column_names(&conn, "documents");
    assert!(docs.iter().any(|c| c == "layer"));
    assert!(docs.iter().any(|c| c == "kind"));
    assert!(docs.iter().any(|c| c == "content_hash"));
    assert!(docs.iter().any(|c| c == "pinned"));
    assert!(docs.iter().any(|c| c == "boost"));
    assert!(docs.iter().any(|c| c == "status"));

    // Existing row still readable; new columns defaulted.
    type PlacementColumns = (
        Option<String>,
        Option<String>,
        Option<bool>,
        Option<f64>,
        Option<String>,
    );
    let (layer, kind, pinned, boost, status): PlacementColumns = conn
        .query_row(
            "SELECT layer, kind, pinned, boost, status FROM documents WHERE id = 'd1'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .expect("select");
    assert_eq!(layer.as_deref(), Some("raw"));
    assert_eq!(kind.as_deref(), Some("document"));
    assert_eq!(pinned, Some(false));
    assert_eq!(boost, Some(1.0));
    assert_eq!(status.as_deref(), Some("active"));

    assert!(table_exists(&conn, "ops_log"));
    assert!(table_exists(&conn, "wiki_index"));
    assert!(table_exists(&conn, "embedding_manifest"));
    assert!(table_exists(&conn, "kg_facts"));
    assert!(index_exists(&conn, "idx_graph_nodes_uri"));
    assert!(index_exists(&conn, "idx_documents_uri"));
    let duplicate_uri_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM documents WHERE uri = 'u1'",
            [],
            |row| row.get(0),
        )
        .expect("count duplicate legacy uris");
    assert_eq!(duplicate_uri_count, 2);
}

#[test]
fn migrate_kg_facts_roundtrip_insert() {
    let conn = open_mem();
    migrate(&conn).expect("migrate");

    conn.execute(
        r#"
            INSERT INTO kg_facts (
              id, subject, predicate, object, valid_from, valid_to,
              status, source_document_id, source, confidence,
              created_at, updated_at, invalidated_at, metadata_json
            ) VALUES (
              'f1', 'Alice', 'works_at', 'Acme',
              TIMESTAMP '2020-01-01', NULL,
              'active', 'doc:1', 'doc:1', 1.0,
              CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, NULL, '{}'
            )
            "#,
        [],
    )
    .expect("insert kg fact");

    let (subject, predicate, object, valid_to): (
            String,
            String,
            String,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT subject, predicate, object, CAST(valid_to AS VARCHAR) FROM kg_facts WHERE id = 'f1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("select kg");
    assert_eq!(subject, "Alice");
    assert_eq!(predicate, "works_at");
    assert_eq!(object, "Acme");
    assert!(valid_to.is_none());
}

/// Pre-v17 rows carry the parent link only inside `metadata_json`; the
/// migrate backfill mirrors it into the column, and re-running migrate
/// leaves healthy rows untouched.
#[test]
fn chunk_parent_backfill_mirrors_metadata_and_stays_lazy() {
    let conn = open_mem();
    migrate(&conn).expect("migrate");
    conn.execute_batch(
        r#"
            INSERT INTO chunks (id, document_id, chunk_index, content, embedding,
                                char_start, char_end, created_at, metadata_json)
            VALUES
              ('parent', 'd', 0, 'p', '[]', 0, 1, CURRENT_TIMESTAMP, '{}'),
              ('leaf', 'd', 1, 'l', '[]', 1, 2, CURRENT_TIMESTAMP,
               '{"parent_chunk_id":"parent","section":"S"}'),
              ('plain', 'd', 2, 'x', '[]', 2, 3, CURRENT_TIMESTAMP, '{}')
            "#,
    )
    .expect("insert pre-v17 shaped chunks");

    let updated = backfill_chunk_parent_ids(&conn).expect("backfill");
    assert_eq!(updated, 1, "only the metadata-linked row is mirrored");
    let linked: Option<String> = conn
        .query_row(
            "SELECT parent_chunk_id FROM chunks WHERE id = 'leaf'",
            [],
            |row| row.get(0),
        )
        .expect("read leaf");
    assert_eq!(linked.as_deref(), Some("parent"));
    let plain: Option<String> = conn
        .query_row(
            "SELECT parent_chunk_id FROM chunks WHERE id = 'plain'",
            [],
            |row| row.get(0),
        )
        .expect("read plain");
    assert_eq!(plain, None, "rows without the key stay NULL (lazy)");
    // Idempotent: nothing left to mirror.
    assert_eq!(backfill_chunk_parent_ids(&conn).expect("re-run"), 0);
    migrate(&conn).expect("migrate again");
}

/// Open time of a current-schema store (docs/refactor/BASELINE.md, Stage 5
/// review). Ignored: run in release with `RAG_TIMING_DOCS` documents × 4
/// chunks plus 4 graph nodes and edges per document.
#[test]
#[ignore = "timing baseline; run explicitly"]
fn timing_open_current_store() {
    let docs: usize = std::env::var("RAG_TIMING_DOCS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2000);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("open-timing.duckdb");
    {
        let store = crate::db::Store::open(&path).unwrap();
        let conn = store.lock().unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO documents (id, uri, title, content, metadata_json, created_at, updated_at) \
             SELECT 'd' || i, 'uri://' || i, 'title ' || i, 'body ' || i, '{{}}', now(), now() \
             FROM range({docs}) t(i);
             INSERT INTO chunks (id, document_id, chunk_index, content, embedding, char_start, \
               char_end, created_at, metadata_json, parent_chunk_id, embedding_digest) \
             SELECT 'd' || (i // 4) || '-' || (i % 4), 'd' || (i // 4), i % 4, 'chunk ' || i, \
               list_transform(range(32), x -> CAST(random() AS FLOAT)), 0, 1, now(), \
               CASE WHEN i % 4 = 0 THEN '{{}}' ELSE '{{\"parent_chunk_id\":\"d' || (i // 4) || '-0\"}}' END, \
               CASE WHEN i % 4 = 0 THEN NULL ELSE 'd' || (i // 4) || '-0' END, i \
             FROM range({chunks}) t(i);
             INSERT INTO graph_nodes (id, kind, label, label_key, created_at, updated_at) \
             SELECT 'n' || i, 'doc', 'Label ' || i, 'label ' || i, now(), now() FROM range({chunks}) t(i);
             INSERT INTO graph_edges (id, source_id, target_id, rel_type, created_at, origin, edge_origin) \
             SELECT 'e' || i, 'n' || i, 'n' || ((i + 1) % {chunks}), 'wikilink', now(), 'extract', 'derived' \
             FROM range({chunks}) t(i);",
            chunks = docs * 4
        ))
        .unwrap();
    }
    let mut samples = Vec::new();
    for _ in 0..7 {
        let started = std::time::Instant::now();
        drop(crate::db::Store::open(&path).unwrap());
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "TIMING open_current_store docs={docs} median_ms={:.1} min_ms={:.1} max_ms={:.1}",
        samples[samples.len() / 2],
        samples[0],
        samples[samples.len() - 1]
    );
}

/// Repairs that used to run on every open still run on every open of a
/// current-schema store, and a healthy store opens without core DDL.
#[test]
fn open_repairs_run_on_a_current_schema_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repairs.duckdb");
    {
        let store = crate::db::Store::open(&path).unwrap();
        let conn = store.lock().unwrap();
        let membership = crate::models::STRUCTURAL_EDGE_CONTEXTS[0];
        conn.execute_batch(&format!(
            "INSERT INTO chunks (id, document_id, chunk_index, content, embedding, char_start, \
               char_end, created_at, metadata_json, parent_chunk_id, embedding_digest) \
             VALUES ('c1', 'd1', 1, 'x', [1.0, 0.0], 0, 1, now(), \
               '{{\"parent_chunk_id\":\"c0\"}}', NULL, NULL);
             INSERT INTO graph_nodes (id, kind, label, label_key, created_at, updated_at) \
             VALUES ('n1', 'doc', 'Some Label', NULL, now(), now());
             INSERT INTO graph_edges (id, source_id, target_id, rel_type, context, created_at, origin) \
             VALUES ('e1', 'n1', 'n1', 'related', '{membership}', now(), 'explicit'),
                    ('e2', 'n1', 'n1', 'wikilink', NULL, now(), NULL);"
        ))
        .unwrap();
    }
    let store = crate::db::Store::open(&path).unwrap();
    let conn = store.lock().unwrap();
    let parent: Option<String> = conn
        .query_row("SELECT parent_chunk_id FROM chunks WHERE id = 'c1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(parent.as_deref(), Some("c0"));
    let digest: Option<i64> = conn
        .query_row("SELECT embedding_digest FROM chunks WHERE id = 'c1'", [], |r| r.get(0))
        .unwrap();
    assert!(digest.is_some());
    let key: Option<String> = conn
        .query_row("SELECT label_key FROM graph_nodes WHERE id = 'n1'", [], |r| r.get(0))
        .unwrap();
    assert!(key.is_some());
    let origins: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare("SELECT id, origin FROM graph_edges ORDER BY id")
            .unwrap();
        crate::db::rows::query_vec(&mut stmt, [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    };
    assert_eq!(
        origins,
        vec![
            ("e1".to_owned(), "extract".to_owned()),
            ("e2".to_owned(), "extract".to_owned())
        ]
    );
    assert_eq!(
        migrate_with(&conn, &registered_schema_extensions()).unwrap(),
        MigrateOutcome {
            core_ddl: false,
            extension_ddl: 0,
        }
    );
}

#[test]
fn stored_schema_version_is_none_without_a_ledger_and_reads_it_otherwise() {
    let conn = open_mem();
    assert_eq!(stored_schema_version(&conn).unwrap(), None);
    migrate(&conn).unwrap();
    assert_eq!(stored_schema_version(&conn).unwrap(), Some(SCHEMA_VERSION));
    conn.execute_batch("DROP TABLE schema_version; CREATE TABLE schema_version (version VARCHAR); INSERT INTO schema_version VALUES ('x')")
        .unwrap();
    assert!(stored_schema_version(&conn).is_err(), "a broken ledger is an error, not 'no ledger'");
}

#[test]
fn a_newer_schema_is_refused_with_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("newer.duckdb");
    {
        let conn = Connection::open(&path).unwrap();
        migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO schema_version (version, applied_at, note) VALUES (?, CURRENT_TIMESTAMP, 'future')",
            [SCHEMA_VERSION + 1],
        )
        .unwrap();
    }
    let expected = format!(
        "database schema v{} is newer than this build supports (v{SCHEMA_VERSION})",
        SCHEMA_VERSION + 1
    );
    let error = crate::db::Store::open(&path).err().expect("refused").to_string();
    assert!(error.contains(&expected), "{error}");
    let error = crate::db::Store::open_read_only(&path).err().expect("refused").to_string();
    assert!(error.contains(&expected), "{error}");
}
