//! Schema v26: `chunks.embedding_json` (JSON text) becomes `chunks.embedding`
//! (`FLOAT[]`). These tests build a database with the pre-v26 chunk DDL,
//! open it through [`Store::open`] and check that the migrated store answers
//! exactly like one ingested natively.

use super::*;
use crate::db::search::{search, SearchQuery};
use crate::db::Store;
use crate::models::SearchMode;
use crate::models::{Chunk, Document};
use chrono::{DateTime, Utc};

/// The pre-v26 base tables plus the chunk columns and index a v25 database
/// had (`metadata_json`, `parent_chunk_id`, `embedding_digest`). The index on
/// `parent_chunk_id` sits after `embedding_json`, which is what makes the
/// column drop need the drop-and-recreate dance.
const LEGACY_DDL: &str = r#"
CREATE TABLE documents (
  id VARCHAR PRIMARY KEY,
  uri VARCHAR NOT NULL,
  title VARCHAR NOT NULL,
  content TEXT NOT NULL,
  metadata_json VARCHAR NOT NULL DEFAULT '{}',
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);
CREATE TABLE chunks (
  id VARCHAR PRIMARY KEY,
  document_id VARCHAR NOT NULL,
  chunk_index INTEGER NOT NULL,
  content TEXT NOT NULL,
  embedding_json VARCHAR NOT NULL,
  char_start INTEGER NOT NULL,
  char_end INTEGER NOT NULL,
  created_at TIMESTAMP NOT NULL,
  content_hash VARCHAR,
  metadata_json VARCHAR DEFAULT '{}',
  parent_chunk_id VARCHAR,
  embedding_digest BIGINT
);
CREATE INDEX idx_chunks_document_id ON chunks(document_id);
CREATE INDEX idx_chunks_parent_chunk_id ON chunks(parent_chunk_id);
"#;

const DOCS: usize = 6;
/// One distinct word per chunk, so a lexical query can hit exactly one.
const WORDS: [&str; 18] = [
    "apple", "birch", "cedar", "delta", "ember", "fjord", "grove", "harbor", "iris", "juniper",
    "kelp", "lotus", "maple", "nectar", "orchid", "pine", "quartz", "raven",
];
const CHUNKS_PER_DOC: usize = 3;

fn fixed_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z")
        .unwrap()
        .with_timezone(&Utc)
}

/// Vectors with awkward values (subnormal, negative zero, extremes, values
/// whose shortest decimal form needs an exponent) so an inexact conversion
/// would show.
fn vector(seed: usize) -> Vec<f32> {
    let awkward = [
        1e-7_f32,
        -0.0,
        0.1,
        3.402_823_5e38,
        -1.175_494_4e-38,
        1e-45,
        0.333_333_34,
        12_345.678,
    ];
    (0..8)
        .map(|i| {
            if (seed + i).is_multiple_of(3) {
                awkward[(seed + i) % awkward.len()]
            } else {
                ((seed * 31 + i * 7) % 97) as f32 / 97.0 - 0.5
            }
        })
        .collect()
}

fn fixture() -> Vec<(Document, Vec<Chunk>)> {
    (0..DOCS)
        .map(|d| {
            let id = format!("doc-{d}");
            let chunks: Vec<Chunk> = (0..CHUNKS_PER_DOC)
                .map(|c| {
                    let content = format!(
                        "document {d} chunk {c} alpha beta {}",
                        WORDS[d * CHUNKS_PER_DOC + c]
                    );
                    let metadata_json = if c == 0 {
                        r#"{"heading_path":["Top"],"section":"Top"}"#.to_owned()
                    } else {
                        format!(r#"{{"parent_chunk_id":"{id}-c0"}}"#)
                    };
                    Chunk {
                        id: format!("{id}-c{c}"),
                        document_id: id.clone(),
                        chunk_index: c as i32,
                        char_start: (c * 10) as i32,
                        char_end: (c * 10 + 9) as i32,
                        content,
                        embedding: vector(d * CHUNKS_PER_DOC + c),
                        metadata_json,
                    }
                })
                .collect();
            let document = Document {
                id: id.clone(),
                uri: format!("legacy://{d}"),
                title: format!("Legacy document {d}"),
                content: chunks
                    .iter()
                    .map(|chunk| chunk.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                metadata_json: "{}".into(),
                created_at: fixed_time(),
                updated_at: fixed_time(),
                ..Default::default()
            };
            (document, chunks)
        })
        .collect()
}

/// Write the fixture with the pre-v26 DDL, embeddings as JSON text the way
/// pre-v26 writers stored them.
fn write_legacy(path: &std::path::Path) {
    let conn = duckdb::Connection::open(path).unwrap();
    conn.execute_batch(LEGACY_DDL).unwrap();
    for (document, chunks) in fixture() {
        conn.execute(
            "INSERT INTO documents (id, uri, title, content, metadata_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
            duckdb::params![
                document.id,
                document.uri,
                document.title,
                document.content,
                document.metadata_json,
                "2026-01-02 03:04:05",
                "2026-01-02 03:04:05",
            ],
        )
        .unwrap();
        for chunk in chunks {
            let json = serde_json::to_string(&chunk.embedding).unwrap();
            let parent = serde_json::from_str::<serde_json::Value>(&chunk.metadata_json)
                .unwrap()
                .get("parent_chunk_id")
                .and_then(|value| value.as_str().map(str::to_owned));
            conn.execute(
                "INSERT INTO chunks (id, document_id, chunk_index, content, embedding_json, \
                 char_start, char_end, created_at, metadata_json, parent_chunk_id, embedding_digest) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, ?, ?, ?)",
                duckdb::params![
                    chunk.id,
                    chunk.document_id,
                    chunk.chunk_index,
                    chunk.content,
                    json,
                    chunk.char_start,
                    chunk.char_end,
                    chunk.metadata_json,
                    parent,
                    crate::db::store::embedding_digest(&json),
                ],
            )
            .unwrap();
        }
    }
}

fn write_native(path: &std::path::Path) -> Store {
    let store = Store::open(path).unwrap();
    for (document, chunks) in fixture() {
        store.documents().upsert_document(&document).unwrap();
        store
            .chunks()
            .replace_chunks_for_document(&document.id, &chunks)
            .unwrap();
    }
    store
}

fn has_column(conn: &duckdb::Connection, column: &str) -> bool {
    column_exists(conn, "chunks", column).unwrap()
}

/// Search results reduced to what a caller sees, scores compared bitwise.
fn answers(store: &Store) -> Vec<Vec<(String, u32, String)>> {
    let queries = [
        (SearchMode::Vec, None, Some(vector(4))),
        (SearchMode::Vec, None, Some(vector(11))),
        (SearchMode::Lex, Some("alpha kelp"), None),
        // A lexical term held by one chunk: BM25 ties would make the fused
        // order depend on physical row order, which no caller relies on.
        (SearchMode::Hybrid, Some("kelp"), Some(vector(7))),
    ];
    queries
        .into_iter()
        .map(|(mode, text, embedding)| {
            search(
                store,
                &SearchQuery {
                    mode,
                    top_k: 8,
                    query_text: text.map(str::to_owned),
                    query_embedding: embedding,
                    ..SearchQuery::default()
                },
            )
            .unwrap()
            .into_iter()
            .map(|hit| (hit.chunk_id, hit.score.to_bits(), hit.content))
            .collect()
        })
        .collect()
}

type BundleView = Vec<(String, String, String, Vec<(String, i32, Vec<u32>)>)>;

/// The exported bundle's documents and chunks, embeddings as bit patterns.
fn bundle_view(store: &Store) -> BundleView {
    store
        .embedding_manifest()
        .set_embedding_manifest(&crate::models::EmbeddingManifest {
            id: "default".into(),
            provider: "mock".into(),
            model: "mock".into(),
            dims: 8,
            base_url: Some("mock://local".into()),
            content_fingerprint: Some(crate::db::store::embedding_identity_fingerprint(
                "mock",
                "mock",
                8,
                "mock://local",
            )),
            updated_at: fixed_time(),
        })
        .unwrap();
    store
        .recovery()
        .recovery_bundle()
        .unwrap()
        .documents
        .into_iter()
        .map(|entry| {
            (
                entry.document.id,
                entry.document.title,
                entry.document.content,
                entry
                    .chunks
                    .into_iter()
                    .map(|chunk| {
                        (
                            chunk.id,
                            chunk.chunk_index,
                            chunk.embedding.iter().map(|v| v.to_bits()).collect(),
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

fn expected_bundle() -> BundleView {
    fixture()
        .into_iter()
        .map(|(document, chunks)| {
            (
                document.id,
                document.title,
                document.content,
                chunks
                    .into_iter()
                    .map(|chunk| {
                        (
                            chunk.id,
                            chunk.chunk_index,
                            chunk.embedding.iter().map(|v| v.to_bits()).collect(),
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn legacy_json_embeddings_migrate_to_float_arrays_with_identical_answers() {
    let dir = tempfile::tempdir().unwrap();
    let legacy_path = dir.path().join("legacy.duckdb");
    write_legacy(&legacy_path);

    let migrated = Store::open(&legacy_path).unwrap();
    {
        let conn = migrated.lock().unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), Some(SCHEMA_VERSION));
        assert!(!has_column(&conn, "embedding_json"));
        assert!(has_column(&conn, "embedding"));
        for index in ["idx_chunks_document_id", "idx_chunks_parent_chunk_id"] {
            let present: i64 = conn
                .query_row(
                    "SELECT count(*) FROM duckdb_indexes() WHERE index_name = ?",
                    [index],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(present, 1, "{index} is re-created after the column drop");
        }
    }
    let native = write_native(&dir.path().join("native.duckdb"));

    // Every vector survives the text -> FLOAT[] conversion bit for bit.
    let mut migrated_chunks = migrated.chunks().all_chunks_with_embeddings().unwrap();
    let mut native_chunks = native.chunks().all_chunks_with_embeddings().unwrap();
    migrated_chunks.sort_by(|a, b| a.id.cmp(&b.id));
    native_chunks.sort_by(|a, b| a.id.cmp(&b.id));
    let bits = |chunks: &[Chunk]| {
        chunks
            .iter()
            .map(|chunk| {
                (
                    chunk.id.clone(),
                    chunk
                        .embedding
                        .iter()
                        .map(|v| v.to_bits())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(bits(&migrated_chunks), bits(&native_chunks));

    migrated.maintenance().ensure_fts("porter").unwrap();
    native.maintenance().ensure_fts("porter").unwrap();
    assert_eq!(answers(&migrated), answers(&native));
    assert_eq!(bundle_view(&migrated), expected_bundle());
    assert_eq!(bundle_view(&native), expected_bundle());
}

#[test]
fn second_open_runs_no_ddl() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("twice.duckdb");
    drop(Store::open(&path).unwrap());
    let store = Store::open(&path).unwrap();
    let conn = store.lock().unwrap();
    let outcome = migrate_with(&conn, &registered_schema_extensions()).unwrap();
    assert_eq!(
        outcome,
        MigrateOutcome {
            core_ddl: false,
            extension_ddl: 0,
        }
    );
}

#[test]
fn interrupted_embedding_backfill_resumes_on_next_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("interrupted.duckdb");
    write_legacy(&path);
    {
        // The first open got as far as one committed batch, then stopped.
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute_batch("ALTER TABLE chunks ADD COLUMN embedding FLOAT[]")
            .unwrap();
        assert_eq!(backfill_chunk_embeddings(&conn, 4, Some(1)).unwrap(), 4);
        let pending: i64 = conn
            .query_row(
                "SELECT count(*) FROM chunks WHERE embedding IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending as usize, DOCS * CHUNKS_PER_DOC - 4);
        assert!(
            current_schema_version(&conn).is_err()
                || current_schema_version(&conn).unwrap().is_none()
        );
    }
    let resumed = Store::open(&path).unwrap();
    {
        let conn = resumed.lock().unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), Some(SCHEMA_VERSION));
        assert!(!has_column(&conn, "embedding_json"));
    }
    assert_eq!(bundle_view(&resumed), expected_bundle());
    let native = write_native(&dir.path().join("native.duckdb"));
    resumed.maintenance().ensure_fts("porter").unwrap();
    native.maintenance().ensure_fts("porter").unwrap();
    assert_eq!(answers(&resumed), answers(&native));
}
