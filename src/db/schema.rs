//! DuckDB DDL and schema migration.

use duckdb::Connection;

use crate::error::{AppError, Result};

/// Current schema version written to `schema_version` after a successful migrate.
pub const SCHEMA_VERSION: i32 = 8;

/// Create `documents` table if missing (base v1 columns).
pub const CREATE_DOCUMENTS: &str = r#"
CREATE TABLE IF NOT EXISTS documents (
  id VARCHAR PRIMARY KEY,
  uri VARCHAR NOT NULL,
  title VARCHAR NOT NULL,
  content TEXT NOT NULL,
  metadata_json VARCHAR NOT NULL DEFAULT '{}',
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
)
"#;

/// Create `chunks` table if missing (base v1 columns).
pub const CREATE_CHUNKS: &str = r#"
CREATE TABLE IF NOT EXISTS chunks (
  id VARCHAR PRIMARY KEY,
  document_id VARCHAR NOT NULL,
  chunk_index INTEGER NOT NULL,
  content TEXT NOT NULL,
  embedding_json VARCHAR NOT NULL,
  char_start INTEGER NOT NULL,
  char_end INTEGER NOT NULL,
  created_at TIMESTAMP NOT NULL
)
"#;

/// Index chunks by parent document.
pub const CREATE_IDX_CHUNKS_DOCUMENT_ID: &str =
    "CREATE INDEX IF NOT EXISTS idx_chunks_document_id ON chunks(document_id)";

/// Create `graph_nodes` table if missing.
pub const CREATE_GRAPH_NODES: &str = r#"
CREATE TABLE IF NOT EXISTS graph_nodes (
  id VARCHAR PRIMARY KEY,
  kind VARCHAR NOT NULL,
  label VARCHAR NOT NULL,
  document_id VARCHAR,
  uri VARCHAR,
  resolved BOOLEAN NOT NULL DEFAULT true,
  metadata_json VARCHAR NOT NULL DEFAULT '{}',
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
)
"#;

/// Create `graph_edges` table if missing.
pub const CREATE_GRAPH_EDGES: &str = r#"
CREATE TABLE IF NOT EXISTS graph_edges (
  id VARCHAR PRIMARY KEY,
  source_id VARCHAR NOT NULL,
  target_id VARCHAR NOT NULL,
  rel_type VARCHAR NOT NULL,
  weight DOUBLE NOT NULL DEFAULT 1.0,
  context VARCHAR,
  created_at TIMESTAMP NOT NULL
)
"#;

/// Index graph edges by source node.
pub const CREATE_IDX_GRAPH_EDGES_SOURCE: &str =
    "CREATE INDEX IF NOT EXISTS idx_graph_edges_source ON graph_edges(source_id)";

/// Index graph edges by target node.
pub const CREATE_IDX_GRAPH_EDGES_TARGET: &str =
    "CREATE INDEX IF NOT EXISTS idx_graph_edges_target ON graph_edges(target_id)";

/// Index graph nodes by label.
pub const CREATE_IDX_GRAPH_NODES_LABEL: &str =
    "CREATE INDEX IF NOT EXISTS idx_graph_nodes_label ON graph_nodes(label)";

/// Index graph nodes by document id.
pub const CREATE_IDX_GRAPH_NODES_DOCUMENT_ID: &str =
    "CREATE INDEX IF NOT EXISTS idx_graph_nodes_document_id ON graph_nodes(document_id)";

/// Append-only operations / wiki timeline log.
pub const CREATE_OPS_LOG: &str = r#"
CREATE TABLE IF NOT EXISTS ops_log (
  id VARCHAR PRIMARY KEY,
  seq BIGINT,
  ts TIMESTAMP NOT NULL,
  op VARCHAR NOT NULL,
  prefix VARCHAR,
  message VARCHAR DEFAULT '',
  entity_id VARCHAR,
  entity_kind VARCHAR,
  payload_json VARCHAR NOT NULL DEFAULT '{}',
  agent_name VARCHAR
)
"#;

/// Wiki index catalog (index.md analogue).
pub const CREATE_WIKI_INDEX: &str = r#"
CREATE TABLE IF NOT EXISTS wiki_index (
  id VARCHAR PRIMARY KEY,
  slug VARCHAR,
  title VARCHAR,
  label VARCHAR,
  kind VARCHAR DEFAULT 'wiki',
  summary VARCHAR,
  category VARCHAR,
  document_id VARCHAR,
  page_id VARCHAR,
  updated_at TIMESTAMP NOT NULL
)
"#;

/// Embedding provider fingerprint for the corpus.
pub const CREATE_EMBEDDING_MANIFEST: &str = r#"
CREATE TABLE IF NOT EXISTS embedding_manifest (
  id VARCHAR PRIMARY KEY,
  provider VARCHAR NOT NULL,
  model VARCHAR NOT NULL,
  dims INTEGER NOT NULL,
  base_url VARCHAR,
  content_fingerprint VARCHAR,
  updated_at TIMESTAMP NOT NULL
)
"#;

/// Temporal knowledge-graph facts (MemPalace / Graphiti-lite).
///
/// Open-ended facts leave `valid_to` NULL and `status = 'active'`.
/// Validity windows are half-open `[valid_from, valid_to)`.
pub const CREATE_KG_FACTS: &str = r#"
CREATE TABLE IF NOT EXISTS kg_facts (
  id VARCHAR PRIMARY KEY,
  subject VARCHAR NOT NULL,
  predicate VARCHAR NOT NULL,
  object VARCHAR NOT NULL,
  valid_from TIMESTAMP,
  valid_to TIMESTAMP,
  status VARCHAR NOT NULL DEFAULT 'active',
  superseded_by VARCHAR,
  source_document_id VARCHAR,
  source VARCHAR,
  confidence DOUBLE,
  metadata_json VARCHAR NOT NULL DEFAULT '{}',
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL,
  invalidated_at TIMESTAMP
)
"#;

/// Index kg_facts by subject.
pub const CREATE_IDX_KG_FACTS_SUBJECT: &str =
    "CREATE INDEX IF NOT EXISTS idx_kg_facts_subject ON kg_facts(subject)";

/// Index kg_facts by predicate.
pub const CREATE_IDX_KG_FACTS_PREDICATE: &str =
    "CREATE INDEX IF NOT EXISTS idx_kg_facts_predicate ON kg_facts(predicate)";

/// Index kg_facts by valid_to (open/closed window queries).
pub const CREATE_IDX_KG_FACTS_VALID_TO: &str =
    "CREATE INDEX IF NOT EXISTS idx_kg_facts_valid_to ON kg_facts(valid_to)";

/// Index kg_facts by lifecycle status.
pub const CREATE_IDX_KG_FACTS_STATUS: &str =
    "CREATE INDEX IF NOT EXISTS idx_kg_facts_status ON kg_facts(status)";

/// Index kg_facts by (subject, predicate, object) for SPO lookups.
pub const CREATE_IDX_KG_FACTS_SPO: &str =
    "CREATE INDEX IF NOT EXISTS idx_kg_facts_spo ON kg_facts(subject, predicate, object)";

/// Migration version ledger.
pub const CREATE_SCHEMA_VERSION: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version (
  version INTEGER NOT NULL,
  applied_at TIMESTAMP NOT NULL,
  note VARCHAR
)
"#;

/// Key-value meta store (includes `schema_version` mirror and future flags).
pub const CREATE_META: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
  key VARCHAR PRIMARY KEY,
  value VARCHAR NOT NULL
)
"#;

/// Named, durable reading lists / outlines.
pub const CREATE_COLLECTIONS: &str = r#"
CREATE TABLE IF NOT EXISTS collections (
  id VARCHAR PRIMARY KEY,
  name VARCHAR NOT NULL,
  description VARCHAR,
  metadata_json VARCHAR NOT NULL DEFAULT '{}',
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
)
"#;

/// Ordered document membership. `parent_document_id` supplies outline nesting.
pub const CREATE_COLLECTION_ENTRIES: &str = r#"
CREATE TABLE IF NOT EXISTS collection_entries (
  collection_id VARCHAR NOT NULL,
  document_id VARCHAR NOT NULL,
  position INTEGER NOT NULL,
  parent_document_id VARCHAR,
  PRIMARY KEY (collection_id, document_id)
)
"#;

/// Optional prerequisite links scoped to a collection.
pub const CREATE_COLLECTION_DEPENDENCIES: &str = r#"
CREATE TABLE IF NOT EXISTS collection_dependencies (
  collection_id VARCHAR NOT NULL,
  document_id VARCHAR NOT NULL,
  depends_on_document_id VARCHAR NOT NULL,
  PRIMARY KEY (collection_id, document_id, depends_on_document_id)
)
"#;

pub const CREATE_IDX_COLLECTION_ENTRIES_ORDER: &str =
    "CREATE INDEX IF NOT EXISTS idx_collection_entries_order ON collection_entries(collection_id, position)";

pub const CREATE_IDX_COLLECTION_DEPENDENCIES_DOCUMENT: &str =
    "CREATE INDEX IF NOT EXISTS idx_collection_dependencies_document ON collection_dependencies(collection_id, document_id)";

/// Immutable snapshots captured before each document update.
pub const CREATE_DOCUMENT_REVISIONS: &str = r#"
CREATE TABLE IF NOT EXISTS document_revisions (
  document_id VARCHAR NOT NULL,
  revision BIGINT NOT NULL,
  uri VARCHAR NOT NULL,
  title VARCHAR NOT NULL,
  content TEXT NOT NULL,
  metadata_json VARCHAR NOT NULL,
  content_hash VARCHAR,
  wing VARCHAR,
  room VARCHAR,
  source_file VARCHAR,
  layer VARCHAR,
  kind VARCHAR,
  status VARCHAR,
  pinned BOOLEAN,
  boost DOUBLE,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL,
  superseded_at TIMESTAMP NOT NULL,
  PRIMARY KEY (document_id, revision)
)
"#;

pub const CREATE_IDX_DOCUMENT_REVISIONS_DOCUMENT: &str =
    "CREATE INDEX IF NOT EXISTS idx_document_revisions_document ON document_revisions(document_id, revision)";

/// Optional index helpers for document scope / dedupe / organize columns.
pub const CREATE_IDX_DOCUMENTS_CONTENT_HASH: &str =
    "CREATE INDEX IF NOT EXISTS idx_documents_content_hash ON documents(content_hash)";

pub const CREATE_IDX_DOCUMENTS_WING_ROOM: &str =
    "CREATE INDEX IF NOT EXISTS idx_documents_wing_room ON documents(wing, room)";

pub const CREATE_IDX_DOCUMENTS_LAYER: &str =
    "CREATE INDEX IF NOT EXISTS idx_documents_layer ON documents(layer)";

pub const CREATE_IDX_DOCUMENTS_STATUS: &str =
    "CREATE INDEX IF NOT EXISTS idx_documents_status ON documents(status)";

pub const CREATE_IDX_DOCUMENTS_PINNED: &str =
    "CREATE INDEX IF NOT EXISTS idx_documents_pinned ON documents(pinned)";

/// Apply schema DDL. Safe to call repeatedly.
///
/// 1. Creates base v1 tables (`documents`, `chunks`, `graph_*`) if missing.
/// 2. Best-effort `ALTER TABLE ... ADD COLUMN` for P0/P1 columns on existing DBs.
/// 3. Creates P0 tables (`ops_log`, `wiki_index`, `embedding_manifest`, `schema_version`, `meta`).
/// 4. Creates MemPalace parity tables (`kg_facts`) and organize columns (`pinned`, `boost`, `status`).
/// 5. Records [`SCHEMA_VERSION`].
pub fn migrate(conn: &Connection) -> Result<()> {
    // Base tables (CREATE IF NOT EXISTS is idempotent).
    conn.execute_batch(
        &[
            CREATE_DOCUMENTS,
            CREATE_CHUNKS,
            CREATE_IDX_CHUNKS_DOCUMENT_ID,
            CREATE_GRAPH_NODES,
            CREATE_GRAPH_EDGES,
            CREATE_IDX_GRAPH_EDGES_SOURCE,
            CREATE_IDX_GRAPH_EDGES_TARGET,
            CREATE_IDX_GRAPH_NODES_LABEL,
            CREATE_IDX_GRAPH_NODES_DOCUMENT_ID,
        ]
        .join(";\n"),
    )?;

    // P0 additive columns on existing (and fresh) databases.
    // documents: content_hash, wing, room, source_file, layer, kind
    add_column_best_effort(conn, "documents", "content_hash", "VARCHAR")?;
    add_column_best_effort(conn, "documents", "wing", "VARCHAR")?;
    add_column_best_effort(conn, "documents", "room", "VARCHAR")?;
    add_column_best_effort(conn, "documents", "source_file", "VARCHAR")?;
    add_column_best_effort(conn, "documents", "layer", "VARCHAR DEFAULT 'raw'")?;
    add_column_best_effort(conn, "documents", "kind", "VARCHAR DEFAULT 'document'")?;

    // MemPalace / ORGANIZE parity: pin, rank boost, lifecycle status.
    add_column_best_effort(conn, "documents", "pinned", "BOOLEAN DEFAULT false")?;
    add_column_best_effort(conn, "documents", "boost", "DOUBLE DEFAULT 1.0")?;
    add_column_best_effort(conn, "documents", "status", "VARCHAR DEFAULT 'active'")?;

    // Optimistic concurrency for multi-agent writers (schema v5).
    add_column_best_effort(conn, "documents", "revision", "BIGINT DEFAULT 1")?;

    // chunks: optional content_hash for embed cache / dedupe
    add_column_best_effort(conn, "chunks", "content_hash", "VARCHAR")?;
    // Additive section metadata; old rows read as an empty object.
    add_column_best_effort(conn, "chunks", "metadata_json", "VARCHAR DEFAULT '{}'")?;

    // Indexes that depend on document columns (after columns exist).
    conn.execute_batch(
        &[
            CREATE_IDX_DOCUMENTS_CONTENT_HASH,
            CREATE_IDX_DOCUMENTS_WING_ROOM,
            CREATE_IDX_DOCUMENTS_LAYER,
            CREATE_IDX_DOCUMENTS_STATUS,
            CREATE_IDX_DOCUMENTS_PINNED,
        ]
        .join(";\n"),
    )?;

    // P0 tables + MemPalace temporal KG.
    conn.execute_batch(
        &[
            CREATE_OPS_LOG,
            CREATE_WIKI_INDEX,
            CREATE_EMBEDDING_MANIFEST,
            CREATE_SCHEMA_VERSION,
            CREATE_META,
            CREATE_COLLECTIONS,
            CREATE_COLLECTION_ENTRIES,
            CREATE_COLLECTION_DEPENDENCIES,
            CREATE_IDX_COLLECTION_ENTRIES_ORDER,
            CREATE_IDX_COLLECTION_DEPENDENCIES_DOCUMENT,
            CREATE_DOCUMENT_REVISIONS,
            CREATE_IDX_DOCUMENT_REVISIONS_DOCUMENT,
            CREATE_KG_FACTS,
            CREATE_IDX_KG_FACTS_SUBJECT,
            CREATE_IDX_KG_FACTS_PREDICATE,
            CREATE_IDX_KG_FACTS_VALID_TO,
            CREATE_IDX_KG_FACTS_STATUS,
            CREATE_IDX_KG_FACTS_SPO,
        ]
        .join(";\n"),
    )?;

    // embedding_manifest additive columns (upgrade older singleton shape).
    add_column_best_effort(conn, "embedding_manifest", "base_url", "VARCHAR")?;
    add_column_best_effort(conn, "embedding_manifest", "content_fingerprint", "VARCHAR")?;
    add_column_best_effort(conn, "embedding_manifest", "updated_at", "TIMESTAMP")?;

    // ops_log / wiki_index richer columns (Karpathy wiki layer, schema v3).
    add_column_best_effort(conn, "ops_log", "seq", "BIGINT")?;
    add_column_best_effort(conn, "ops_log", "prefix", "VARCHAR")?;
    add_column_best_effort(conn, "ops_log", "message", "VARCHAR DEFAULT ''")?;
    add_column_best_effort(conn, "ops_log", "entity_id", "VARCHAR")?;
    add_column_best_effort(conn, "ops_log", "entity_kind", "VARCHAR")?;
    add_column_best_effort(conn, "ops_log", "agent_name", "VARCHAR")?;
    add_column_best_effort(conn, "wiki_index", "slug", "VARCHAR")?;
    add_column_best_effort(conn, "wiki_index", "title", "VARCHAR")?;
    add_column_best_effort(conn, "wiki_index", "kind", "VARCHAR DEFAULT 'wiki'")?;
    add_column_best_effort(conn, "wiki_index", "page_id", "VARCHAR")?;

    // kg_facts additive columns for older DBs that may have a partial table shape.
    add_column_best_effort(conn, "kg_facts", "valid_from", "TIMESTAMP")?;
    add_column_best_effort(conn, "kg_facts", "valid_to", "TIMESTAMP")?;
    add_column_best_effort(conn, "kg_facts", "status", "VARCHAR DEFAULT 'active'")?;
    add_column_best_effort(conn, "kg_facts", "superseded_by", "VARCHAR")?;
    add_column_best_effort(conn, "kg_facts", "source_document_id", "VARCHAR")?;
    add_column_best_effort(conn, "kg_facts", "source", "VARCHAR")?;
    add_column_best_effort(conn, "kg_facts", "confidence", "DOUBLE")?;
    add_column_best_effort(conn, "kg_facts", "metadata_json", "VARCHAR DEFAULT '{}'")?;
    add_column_best_effort(conn, "kg_facts", "created_at", "TIMESTAMP")?;
    add_column_best_effort(conn, "kg_facts", "updated_at", "TIMESTAMP")?;
    add_column_best_effort(conn, "kg_facts", "invalidated_at", "TIMESTAMP")?;

    // Indexes that depend on additive kg columns (safe re-run).
    conn.execute_batch(
        &[
            CREATE_IDX_KG_FACTS_SUBJECT,
            CREATE_IDX_KG_FACTS_PREDICATE,
            CREATE_IDX_KG_FACTS_VALID_TO,
            CREATE_IDX_KG_FACTS_STATUS,
            CREATE_IDX_KG_FACTS_SPO,
        ]
        .join(";\n"),
    )?;

    record_schema_version(conn)?;
    Ok(())
}

/// Best-effort `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`.
///
/// Ignores "already exists" / duplicate-column failures so older DuckDB builds
/// and re-runs remain safe. Other errors propagate.
fn add_column_best_effort(
    conn: &Connection,
    table: &str,
    column: &str,
    type_and_default: &str,
) -> Result<()> {
    // Prefer IF NOT EXISTS when the engine supports it.
    let sql_if = format!(
        "ALTER TABLE {table} ADD COLUMN IF NOT EXISTS {column} {type_and_default}"
    );
    match conn.execute_batch(&sql_if) {
        Ok(()) => return Ok(()),
        Err(e) => {
            let msg = e.to_string().to_lowercase();
            if is_benign_add_column_error(&msg) {
                return Ok(());
            }
            // Fall through: try without IF NOT EXISTS (older parsers).
            if msg.contains("syntax") || msg.contains("parser") {
                // continue below
            } else {
                // Unknown error on IF NOT EXISTS path — still try plain ADD.
            }
        }
    }

    let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {type_and_default}");
    match conn.execute_batch(&sql) {
        Ok(()) => Ok(()),
        Err(e) => {
            let msg = e.to_string().to_lowercase();
            if is_benign_add_column_error(&msg) {
                Ok(())
            } else {
                Err(AppError::db(format!(
                    "migrate add column {table}.{column}: {e}"
                )))
            }
        }
    }
}

fn is_benign_add_column_error(msg: &str) -> bool {
    msg.contains("already exists")
        || msg.contains("duplicate column")
        || msg.contains("already present")
}

/// Insert current [`SCHEMA_VERSION`] if not already recorded at that version,
/// and mirror it into `meta`.
fn record_schema_version(conn: &Connection) -> Result<()> {
    let current = match conn.query_row(
        "SELECT MAX(version) FROM schema_version",
        [],
        |row| row.get::<_, Option<i32>>(0),
    ) {
        Ok(v) => v,
        Err(duckdb::Error::QueryReturnedNoRows) => None,
        Err(e) => return Err(e.into()),
    };

    if current.map(|v| v >= SCHEMA_VERSION).unwrap_or(false) {
        // Still refresh meta mirror.
        upsert_meta(conn, "schema_version", &SCHEMA_VERSION.to_string())?;
        return Ok(());
    }

    conn.execute(
        r#"
        INSERT INTO schema_version (version, applied_at, note)
        VALUES (?, CURRENT_TIMESTAMP, ?)
        "#,
        duckdb::params![
            SCHEMA_VERSION,
            "Document revision snapshots before mutation; schema v8"
        ],
    )?;

    upsert_meta(conn, "schema_version", &SCHEMA_VERSION.to_string())?;
    Ok(())
}

fn upsert_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        r#"
        INSERT OR REPLACE INTO meta (key, value)
        VALUES (?, ?)
        "#,
        duckdb::params![key, value],
    )?;
    Ok(())
}

/// Read the highest applied schema version, if any.
pub fn current_schema_version(conn: &Connection) -> Result<Option<i32>> {
    let v: Option<i32> = conn.query_row(
        "SELECT MAX(version) FROM schema_version",
        [],
        |row| row.get::<_, Option<i32>>(0),
    )?;
    Ok(v)
}

#[cfg(test)]
mod tests {
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

        for t in [
            "ops_log",
            "wiki_index",
            "embedding_manifest",
            "schema_version",
            "meta",
            "graph_nodes",
            "graph_edges",
            "kg_facts",
        ] {
            assert!(table_exists(&conn, t), "missing table {t}");
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

        migrate(&conn).expect("upgrade migrate");

        let docs = column_names(&conn, "documents");
        assert!(docs.iter().any(|c| c == "layer"));
        assert!(docs.iter().any(|c| c == "kind"));
        assert!(docs.iter().any(|c| c == "content_hash"));
        assert!(docs.iter().any(|c| c == "pinned"));
        assert!(docs.iter().any(|c| c == "boost"));
        assert!(docs.iter().any(|c| c == "status"));

        // Existing row still readable; new columns defaulted.
        let (layer, kind, pinned, boost, status): (
            Option<String>,
            Option<String>,
            Option<bool>,
            Option<f64>,
            Option<String>,
        ) = conn
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
}
