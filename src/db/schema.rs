//! DuckDB DDL and schema migration.

use duckdb::Connection;

use crate::error::{AppError, Result};

/// Current schema version written to `schema_version` after a successful migrate.
pub const SCHEMA_VERSION: i32 = 26;

/// Durable node registry and replication journal. Payloads are intentionally
/// opaque here: transport persists them before higher layers apply/rebuild.
pub const CREATE_SYNC_NODES: &str = r#"
CREATE TABLE IF NOT EXISTS sync_nodes (
  node_id VARCHAR PRIMARY KEY,
  hostname VARCHAR NOT NULL,
  role VARCHAR NOT NULL DEFAULT 'replica',
  last_seen_at TIMESTAMP NOT NULL,
  last_push_seq BIGINT NOT NULL DEFAULT 0,
  pull_cursor BIGINT NOT NULL DEFAULT 0,
  last_error VARCHAR
)
"#;

pub const CREATE_SYNC_CHANGES: &str = r#"
CREATE TABLE IF NOT EXISTS sync_changes (
  primary_seq BIGINT PRIMARY KEY,
  origin_node VARCHAR NOT NULL,
  origin_seq BIGINT NOT NULL,
  entity_kind VARCHAR NOT NULL,
  entity_id VARCHAR NOT NULL,
  operation VARCHAR NOT NULL,
  payload_json VARCHAR NOT NULL,
  content_hash VARCHAR,
  created_at TIMESTAMP NOT NULL,
  UNIQUE(origin_node, origin_seq)
)
"#;

pub const CREATE_IDX_SYNC_CHANGES_ENTITY: &str =
    "CREATE INDEX IF NOT EXISTS idx_sync_changes_entity ON sync_changes(entity_kind, entity_id)";

pub const CREATE_SYNC_OUTBOX: &str = r#"
CREATE TABLE IF NOT EXISTS sync_outbox (
  local_seq BIGINT PRIMARY KEY,
  entity_kind VARCHAR NOT NULL,
  entity_id VARCHAR NOT NULL,
  operation VARCHAR NOT NULL,
  payload_json VARCHAR NOT NULL,
  content_hash VARCHAR,
  created_at TIMESTAMP NOT NULL,
  sent_at TIMESTAMP,
  primary_seq BIGINT
)
"#;

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
  embedding FLOAT[] NOT NULL,
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

/// Index graph edges by owner (§1.5): the rebuild delete predicate filters on it.
pub const CREATE_IDX_GRAPH_EDGES_ORIGIN: &str =
    "CREATE INDEX IF NOT EXISTS idx_graph_edges_origin ON graph_edges(origin)";

/// Index graph edges by relation type (§1.5).
pub const CREATE_IDX_GRAPH_EDGES_REL: &str =
    "CREATE INDEX IF NOT EXISTS idx_graph_edges_rel ON graph_edges(rel_type)";

/// Index graph nodes by label.
pub const CREATE_IDX_GRAPH_NODES_LABEL: &str =
    "CREATE INDEX IF NOT EXISTS idx_graph_nodes_label ON graph_nodes(label)";

/// Index graph nodes by the single normalization key (§3).
pub const CREATE_IDX_GRAPH_NODES_LABEL_KEY: &str =
    "CREATE INDEX IF NOT EXISTS idx_graph_nodes_label_key ON graph_nodes(label_key)";

/// Index graph nodes by document id.
pub const CREATE_IDX_GRAPH_NODES_DOCUMENT_ID: &str =
    "CREATE INDEX IF NOT EXISTS idx_graph_nodes_document_id ON graph_nodes(document_id)";

/// Index graph nodes by URI for document-node reuse and wikilink resolution.
pub const CREATE_IDX_GRAPH_NODES_URI: &str =
    "CREATE INDEX IF NOT EXISTS idx_graph_nodes_uri ON graph_nodes(uri)";

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

/// Filesystem source preflight state for incremental tree synchronization.
pub const CREATE_SOURCE_MANIFEST: &str = r#"
CREATE TABLE IF NOT EXISTS source_manifest (
  canonical_path VARCHAR PRIMARY KEY,
  canonical_root VARCHAR NOT NULL,
  size_bytes BIGINT NOT NULL,
  mtime_ns BIGINT NOT NULL,
  content_hash VARCHAR NOT NULL,
  document_id VARCHAR NOT NULL,
  last_seen TIMESTAMP NOT NULL,
  last_synced TIMESTAMP NOT NULL
)
"#;

pub const CREATE_IDX_SOURCE_MANIFEST_ROOT: &str =
    "CREATE INDEX IF NOT EXISTS idx_source_manifest_root ON source_manifest(canonical_root)";

pub const CREATE_IDX_SOURCE_MANIFEST_DOCUMENT: &str =
    "CREATE INDEX IF NOT EXISTS idx_source_manifest_document ON source_manifest(document_id)";

/// Resumable embedding-migration progress: one row per document whose chunks
/// were already re-embedded under a given target identity fingerprint. Rows
/// are cleared when the migration completes and the manifest flips.
pub const CREATE_REEMBED_PROGRESS: &str = r#"
CREATE TABLE IF NOT EXISTS reembed_progress (
  target_fingerprint VARCHAR NOT NULL,
  document_id VARCHAR NOT NULL,
  chunks_reembedded BIGINT NOT NULL,
  completed_at TIMESTAMP NOT NULL,
  PRIMARY KEY (target_fingerprint, document_id)
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

/// Which documents' chunks changed at each chunk generation. A NULL
/// `document_id` means "unknown / any": readers then fall back to a full
/// listing. Rows older than a window are pruned (see `db::fts`).
pub const CREATE_CHUNK_CHANGE_LOG: &str = r#"
CREATE TABLE IF NOT EXISTS chunk_change_log (
  generation BIGINT NOT NULL,
  document_id VARCHAR
)
"#;

pub const CREATE_IDX_CHUNK_CHANGE_LOG_GENERATION: &str =
    "CREATE INDEX IF NOT EXISTS idx_chunk_change_log_generation ON chunk_change_log(generation)";

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
pub const CREATE_IDX_DOCUMENTS_URI: &str =
    "CREATE INDEX IF NOT EXISTS idx_documents_uri ON documents(uri)";

pub const CREATE_IDX_DOCUMENTS_CONTENT_HASH: &str =
    "CREATE INDEX IF NOT EXISTS idx_documents_content_hash ON documents(content_hash)";

pub const CREATE_IDX_DOCUMENTS_WING_ROOM: &str =
    "CREATE INDEX IF NOT EXISTS idx_documents_wing_room ON documents(wing, room)";

pub const CREATE_IDX_DOCUMENTS_LAYER: &str =
    "CREATE INDEX IF NOT EXISTS idx_documents_layer ON documents(layer)";

/// Diary activity (`/v1/agents`, polled) filters on `kind = 'diary'`.
pub const CREATE_IDX_DOCUMENTS_KIND: &str =
    "CREATE INDEX IF NOT EXISTS idx_documents_kind ON documents(kind)";

pub const CREATE_IDX_DOCUMENTS_STATUS: &str =
    "CREATE INDEX IF NOT EXISTS idx_documents_status ON documents(status)";

pub const CREATE_IDX_DOCUMENTS_PINNED: &str =
    "CREATE INDEX IF NOT EXISTS idx_documents_pinned ON documents(pinned)";

/// Tables owned by a layer above the store (code intelligence: analysis
/// tasks, gate, calibration, external DB snapshots). The owning module
/// provides the DDL; the composition root registers it, and the owning
/// repository makes sure its store has it ([`Store::ensure_schema_extensions`]).
///
/// [`Store::ensure_schema_extensions`]: crate::db::Store::ensure_schema_extensions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaExtension {
    /// Stable name; a second registration under the same name is ignored.
    pub name: &'static str,
    /// Idempotent DDL (`CREATE ... IF NOT EXISTS`).
    pub ddl: &'static str,
}

static SCHEMA_EXTENSIONS: std::sync::RwLock<Vec<SchemaExtension>> =
    std::sync::RwLock::new(Vec::new());

/// Register `extension` for every store opened afterwards in this process.
/// Idempotent by name.
pub fn register_schema_extension(extension: SchemaExtension) {
    let mut registered = SCHEMA_EXTENSIONS.write().unwrap_or_else(|e| e.into_inner());
    if registered.iter().any(|e| e.name == extension.name) {
        return;
    }
    registered.push(extension);
}

/// Extensions registered so far, in registration order.
pub(crate) fn registered_schema_extensions() -> Vec<SchemaExtension> {
    SCHEMA_EXTENSIONS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Apply schema DDL. Safe to call repeatedly.
///
/// 1. Creates base v1 tables (`documents`, `chunks`, `graph_*`) if missing.
/// 2. Best-effort `ALTER TABLE ... ADD COLUMN` for P0/P1 columns on existing DBs.
/// 3. Creates P0 tables (`ops_log`, `wiki_index`, `embedding_manifest`, `schema_version`, `meta`).
/// 4. Creates MemPalace parity tables (`kg_facts`) and organize columns (`pinned`, `boost`, `status`).
/// 5. Creates the schema-v9 filesystem source manifest.
/// 6. Records [`SCHEMA_VERSION`].
pub fn migrate(conn: &Connection) -> Result<()> {
    migrate_with(conn, &registered_schema_extensions()).map(|_| ())
}

/// What [`migrate_with`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MigrateOutcome {
    /// The core schema DDL ran (the stored version differed).
    pub core_ddl: bool,
    /// Schema extensions whose DDL ran (new, or changed since recorded).
    pub extension_ddl: usize,
}

/// [`migrate`] with an explicit list of schema extensions (applied first).
///
/// The core DDL runs only when the stored schema version differs from
/// [`SCHEMA_VERSION`]; an extension's DDL runs only when its digest is not
/// recorded in `meta` yet. A store at the current version opens with reads
/// only.
pub(crate) fn migrate_with(
    conn: &Connection,
    extensions: &[SchemaExtension],
) -> Result<MigrateOutcome> {
    let current = stored_schema_version(conn);
    let mut outcome = MigrateOutcome {
        core_ddl: false,
        extension_ddl: 0,
    };
    let mut pending_extensions = Vec::new();
    for extension in extensions {
        let digest = blake3::hash(extension.ddl.as_bytes()).to_hex().to_string();
        let recorded = match current {
            Some(_) => read_meta(conn, &extension_meta_key(extension.name))?,
            None => None,
        };
        if recorded.as_deref() != Some(digest.as_str()) {
            conn.execute_batch(extension.ddl)?;
            outcome.extension_ddl += 1;
            pending_extensions.push((extension.name, digest));
        }
    }
    if current != Some(SCHEMA_VERSION) {
        migrate_core(conn)?;
        outcome.core_ddl = true;
    }
    for (name, digest) in pending_extensions {
        upsert_meta(conn, &extension_meta_key(name), &digest)?;
    }
    Ok(outcome)
}

fn extension_meta_key(name: &str) -> String {
    format!("schema_extension:{name}")
}

/// The stored schema version, or `None` when there is no ledger yet.
fn stored_schema_version(conn: &Connection) -> Option<i32> {
    current_schema_version(conn).ok().flatten()
}

fn read_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    match conn.query_row("SELECT value FROM meta WHERE key = ?", [key], |row| {
        row.get::<_, String>(0)
    }) {
        Ok(value) => Ok(Some(value)),
        Err(duckdb::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The full, idempotent core schema DDL plus backfills; records
/// [`SCHEMA_VERSION`] last, so an interrupted run repeats on the next open.
fn migrate_core(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS code_index_documents (
        document_id VARCHAR PRIMARY KEY, payload VARCHAR NOT NULL
    )",
    )?;
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS code_index_source_blobs (content_hash VARCHAR PRIMARY KEY, content VARCHAR NOT NULL);
        CREATE TABLE IF NOT EXISTS code_index_revisions (
            id BIGINT PRIMARY KEY, fingerprint VARCHAR NOT NULL,
            created_at TIMESTAMP NOT NULL DEFAULT current_timestamp, graph_json VARCHAR NOT NULL
        );
        CREATE TABLE IF NOT EXISTS code_index_revision_files (
            index_revision BIGINT NOT NULL, document_id VARCHAR NOT NULL, path VARCHAR NOT NULL,
            project VARCHAR, document_revision BIGINT NOT NULL, content_hash VARCHAR NOT NULL,
            analysis_json VARCHAR, PRIMARY KEY(index_revision, document_id)
        );
        CREATE INDEX IF NOT EXISTS idx_code_revision_files_project ON code_index_revision_files(index_revision,project);
        CREATE TABLE IF NOT EXISTS code_index_runs (
            seq BIGINT PRIMARY KEY, run_id VARCHAR UNIQUE NOT NULL,
            index_revision BIGINT NOT NULL, revision_created BOOLEAN NOT NULL,
            recorded_at TIMESTAMP NOT NULL DEFAULT current_timestamp, metrics_json VARCHAR NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_code_index_runs_revision ON code_index_runs(index_revision,seq);

    "#)?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS code_index_drift (
            event_id VARCHAR PRIMARY KEY, seq BIGINT UNIQUE NOT NULL,
            index_revision BIGINT NOT NULL, document_id VARCHAR NOT NULL, path VARCHAR NOT NULL,
            project VARCHAR, kind VARCHAR NOT NULL, source_kind VARCHAR NOT NULL,
            indexed_hash VARCHAR, observed_hash VARCHAR,
            first_seen TIMESTAMP NOT NULL DEFAULT current_timestamp,
            last_seen TIMESTAMP NOT NULL DEFAULT current_timestamp,
            acknowledged_at TIMESTAMP, acknowledgement VARCHAR
        );
        CREATE INDEX IF NOT EXISTS idx_code_drift_seq ON code_index_drift(seq);
    "#,
    )?;
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
            CREATE_IDX_GRAPH_NODES_URI,
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
    // Parent/child hierarchy (schema v17): leaf chunks point at their section
    // leader (Markdown) or at the first window of their code item. The column
    // mirrors the `parent_chunk_id` key inside `metadata_json`; writers derive
    // it at insert time and this backfill covers rows ingested before v17.
    // Rows without the metadata key keep NULL — lazy backfill on re-ingest.
    add_column_best_effort(conn, "chunks", "parent_chunk_id", "VARCHAR")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_chunks_parent_chunk_id ON chunks(parent_chunk_id)",
    )?;
    backfill_chunk_parent_ids(conn)?;
    // 64-bit digest of the stored embedding, maintained by every chunk writer,
    // so a vector-snapshot refresh can tell changed vectors apart without
    // reading the vector. Rows written before the column existed are
    // backfilled once.
    add_column_best_effort(conn, "chunks", "embedding_digest", "BIGINT")?;
    let legacy_embedding = column_exists(conn, "chunks", "embedding_json")?;
    let digest_source = if legacy_embedding {
        "embedding_json"
    } else {
        "embedding"
    };
    conn.execute_batch(&format!(
        "UPDATE chunks SET embedding_digest = CAST(hash({digest_source}) >> 1 AS BIGINT) \
         WHERE embedding_digest IS NULL"
    ))?;
    // Schema v26: embeddings are a native FLOAT[] column instead of JSON text.
    // Databases from before v26 gain the column (nullable: DuckDB cannot add
    // NOT NULL to an indexed table), are backfilled in batches and lose the
    // JSON column.
    if legacy_embedding {
        add_column_best_effort(conn, "chunks", "embedding", "FLOAT[]")?;
        backfill_chunk_embeddings(conn, EMBEDDING_BACKFILL_BATCH, None)?;
        drop_legacy_embedding_column(conn)?;
    }

    // NULL identifies pre-v11 edges whose author was never persisted. New writes
    // explicitly distinguish extracted edges from durable user-authored links.
    add_column_best_effort(conn, "graph_edges", "edge_origin", "VARCHAR")?;

    // GRAPH_DESIGN §3/§1.5: one normalization key per node, used for case- and
    // width-insensitive matching. Derivation lives in `graph::normalize` because
    // DuckDB has no NFKC; existing rows are backfilled in
    // [`backfill_graph_node_label_keys`].
    add_column_best_effort(conn, "graph_nodes", "label_key", "VARCHAR")?;
    conn.execute_batch(CREATE_IDX_GRAPH_NODES_LABEL_KEY)?;

    // GRAPH_DESIGN §1.6: `origin` decides which edges a rebuild may delete.
    // Added nullable and backfilled in [`backfill_graph_edge_origins`]: the
    // literal §1.5 `DEFAULT 'extract'` would retroactively claim every
    // pre-existing user-authored edge as derived and let rebuild delete it.
    add_column_best_effort(conn, "graph_edges", "origin", "VARCHAR")?;
    conn.execute_batch(&[CREATE_IDX_GRAPH_EDGES_ORIGIN, CREATE_IDX_GRAPH_EDGES_REL].join(";\n"))?;

    // GRAPH_DESIGN §4.1/§6.1: where an extracted edge came from. These are only
    // ever written for `origin = 'extract'` rows; explicit edges keep them NULL,
    // because a hand-made `link_nodes` edge has no span in any document.
    add_column_best_effort(conn, "graph_edges", "alias", "VARCHAR")?;
    add_column_best_effort(conn, "graph_edges", "heading", "VARCHAR")?;
    add_column_best_effort(conn, "graph_edges", "chunk_id", "VARCHAR")?;
    add_column_best_effort(conn, "graph_edges", "char_start", "BIGINT")?;
    add_column_best_effort(conn, "graph_edges", "char_end", "BIGINT")?;
    add_column_best_effort(conn, "graph_edges", "occurrence", "BIGINT")?;

    // Indexes that depend on document columns (after columns exist).
    conn.execute_batch(
        &[
            CREATE_IDX_DOCUMENTS_URI,
            CREATE_IDX_DOCUMENTS_CONTENT_HASH,
            CREATE_IDX_DOCUMENTS_WING_ROOM,
            CREATE_IDX_DOCUMENTS_LAYER,
            CREATE_IDX_DOCUMENTS_KIND,
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
            CREATE_SOURCE_MANIFEST,
            CREATE_IDX_SOURCE_MANIFEST_ROOT,
            CREATE_IDX_SOURCE_MANIFEST_DOCUMENT,
            CREATE_REEMBED_PROGRESS,
            CREATE_SCHEMA_VERSION,
            CREATE_META,
            CREATE_CHUNK_CHANGE_LOG,
            CREATE_IDX_CHUNK_CHANGE_LOG_GENERATION,
            "CREATE TABLE IF NOT EXISTS search_feedback (id VARCHAR PRIMARY KEY, payload VARCHAR NOT NULL)",
            "CREATE TABLE IF NOT EXISTS knowledge_views (id VARCHAR PRIMARY KEY, payload VARCHAR NOT NULL)",
            "CREATE TABLE IF NOT EXISTS wiki_proposals (id VARCHAR PRIMARY KEY, payload VARCHAR NOT NULL)",
            "CREATE TABLE IF NOT EXISTS eval_runs (id VARCHAR PRIMARY KEY, payload VARCHAR NOT NULL)",
            "CREATE TABLE IF NOT EXISTS background_jobs (id VARCHAR PRIMARY KEY, payload VARCHAR NOT NULL)",
            "CREATE TABLE IF NOT EXISTS eval_label_queue (id VARCHAR PRIMARY KEY, payload VARCHAR NOT NULL)",
            "CREATE TABLE IF NOT EXISTS eval_traces (id VARCHAR PRIMARY KEY, payload VARCHAR NOT NULL)",
            "CREATE TABLE IF NOT EXISTS eval_prompts (id VARCHAR PRIMARY KEY, payload VARCHAR NOT NULL)",
            CREATE_COLLECTIONS,
            CREATE_COLLECTION_ENTRIES,
            CREATE_COLLECTION_DEPENDENCIES,
            CREATE_IDX_COLLECTION_ENTRIES_ORDER,
            CREATE_IDX_COLLECTION_DEPENDENCIES_DOCUMENT,
            CREATE_DOCUMENT_REVISIONS,
            CREATE_IDX_DOCUMENT_REVISIONS_DOCUMENT,
            CREATE_SYNC_NODES,
            CREATE_SYNC_CHANGES,
            CREATE_IDX_SYNC_CHANGES_ENTITY,
            CREATE_SYNC_OUTBOX,
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

    backfill_graph_node_label_keys(conn)?;
    backfill_graph_edge_origins(conn)?;

    record_schema_version(conn)?;
    Ok(())
}

/// Derive `graph_nodes.label_key` for rows written before the column existed.
///
/// The loop is guarded by a missing-key count so an upgraded database pays it
/// once and every later open costs one aggregate. Rows keep `label` untouched:
/// `label_key` is the match key, `label` stays the display form (§1.2).
pub fn backfill_graph_node_label_keys(conn: &Connection) -> Result<u64> {
    const COUNT_MISSING: &str = "SELECT COUNT(*) FROM graph_nodes WHERE label_key IS NULL";
    const PENDING: &str =
        "SELECT id, label FROM graph_nodes WHERE label_key IS NULL ORDER BY id ASC";

    let missing: i64 = conn.query_row(COUNT_MISSING, [], |row| row.get(0))?;
    if missing == 0 {
        return Ok(0);
    }

    let pending: Vec<(String, String)> = {
        let mut stmt = conn.prepare(PENDING)?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<duckdb::Result<Vec<_>>>()?
    };

    let mut updated = 0u64;
    {
        let mut stmt = conn.prepare("UPDATE graph_nodes SET label_key = ? WHERE id = ?")?;
        for (id, label) in &pending {
            let key = rag_mcp_markup::normalize::label_key(label);
            if key.is_empty() {
                continue;
            }
            updated += stmt.execute(duckdb::params![key, id])? as u64;
        }
    }
    Ok(updated)
}

/// Map the pre-§1.6 `edge_origin` vocabulary onto the authoritative `origin`.
///
/// `derived` → `extract`, `manual` → `explicit`. Rows predating both columns
/// have no recorded author, so only the ones whose relation is extraction-owned
/// (`wikilink` / `tagged` / `mentions`, §1.6) plus the pipeline's structural
/// membership edges are claimed as `extract`; every other unknown row falls back
/// to `explicit`, which is the side that rebuild never deletes. Set-based rather
/// than row-looped because the live corpus has hundreds of thousands of edges.
///
/// Membership edges need their own branch: `related` is an explicit relation
/// (§1.6), yet `graph::resolve::append_structural_edges` emits thousands of them
/// per corpus with a structural `context`. Classified by name alone they would be
/// stamped `explicit` and escape every later rebuild, duplicating placement edges
/// on each re-ingest.
pub fn backfill_graph_edge_origins(conn: &Connection) -> Result<u64> {
    let extract_owned =
        crate::db::graph::sql_string_list(crate::models::EXTRACT_OWNED_REL_TYPES.iter().copied());
    let structural_contexts =
        crate::db::graph::sql_string_list(crate::models::STRUCTURAL_EDGE_CONTEXTS.iter().copied());
    let membership = format!("rel_type = 'related' AND context IN ({structural_contexts})");
    let mut done = 0u64;

    let missing: i64 = conn.query_row(
        "SELECT COUNT(*) FROM graph_edges WHERE origin IS NULL",
        [],
        |row| row.get(0),
    )?;
    if missing > 0 {
        done += conn.execute(
            &format!(
                "UPDATE graph_edges SET origin = 'extract' \
                 WHERE origin IS NULL \
                   AND (edge_origin = 'derived' \
                        OR (edge_origin IS NULL AND rel_type IN ({extract_owned})) \
                        OR ({membership}))"
            ),
            [],
        )? as u64;
        done += conn.execute(
            &format!(
                "UPDATE graph_edges SET origin = 'explicit' \
                 WHERE origin IS NULL \
                   AND (edge_origin = 'manual' \
                        OR (edge_origin IS NULL AND rel_type NOT IN ({extract_owned}))) \
                   AND NOT ({membership})"
            ),
            [],
        )? as u64;
        // Any vocabulary we have never seen stays on the non-deleted side.
        done += conn.execute(
            "UPDATE graph_edges SET origin = 'explicit' WHERE origin IS NULL",
            [],
        )? as u64;
    }

    // Repair for databases that already ran the rel_type-only version of this
    // backfill: those membership rows are `explicit` and therefore invisible to
    // §1.6's delete predicate. Guarded so a healthy store pays one count.
    let mislabeled: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM graph_edges WHERE origin = 'explicit' AND {membership}"),
        [],
        |row| row.get(0),
    )?;
    if mislabeled > 0 {
        done += conn.execute(
            &format!("UPDATE graph_edges SET origin = 'extract' WHERE origin = 'explicit' AND {membership}"),
            [],
        )? as u64;
    }
    Ok(done)
}

/// Mirror `metadata_json.parent_chunk_id` into the `parent_chunk_id` column
/// for rows written before schema v17. Set-based, guarded by a missing count
/// so healthy stores pay one aggregate per open. Rows whose metadata carries
/// no parent key stay NULL (lazy backfill on the next re-ingest).
pub fn backfill_chunk_parent_ids(conn: &Connection) -> Result<u64> {
    let missing: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chunks \
         WHERE parent_chunk_id IS NULL AND metadata_json LIKE '%parent_chunk_id%'",
        [],
        |row| row.get(0),
    )?;
    if missing == 0 {
        return Ok(0);
    }
    let updated = conn.execute(
        "UPDATE chunks SET parent_chunk_id = \
           NULLIF(json_extract_string(TRY_CAST(metadata_json AS JSON), '$.parent_chunk_id'), '') \
         WHERE parent_chunk_id IS NULL AND metadata_json LIKE '%parent_chunk_id%'",
        [],
    )? as u64;
    Ok(updated)
}

/// Rows per transaction when [`backfill_chunk_embeddings`] converts JSON
/// embeddings.
pub(crate) const EMBEDDING_BACKFILL_BATCH: usize = 5_000;

/// Whether `table` has `column`.
fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM duckdb_columns() \
         WHERE table_name = ? AND column_name = ? AND schema_name = current_schema()",
        [table, column],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Fill `chunks.embedding` from `embedding_json` for rows that have none, one
/// transaction per `batch` rows, stopping after `max_batches` when given (a
/// test seam for an interrupted run). Each committed batch stays done, so a
/// run cut short resumes where it stopped. Returns the rows converted.
pub(crate) fn backfill_chunk_embeddings(
    conn: &Connection,
    batch: usize,
    max_batches: Option<usize>,
) -> Result<u64> {
    let mut converted = 0u64;
    let mut batches = 0usize;
    loop {
        if max_batches.is_some_and(|max| batches >= max) {
            break;
        }
        conn.execute_batch("BEGIN TRANSACTION")?;
        let updated = conn.execute(
            "UPDATE chunks SET embedding = CAST(embedding_json AS FLOAT[]) \
             WHERE id IN (SELECT id FROM chunks WHERE embedding IS NULL ORDER BY id LIMIT ?)",
            [batch as i64],
        );
        let updated = match updated {
            Ok(updated) => {
                conn.execute_batch("COMMIT")?;
                updated
            }
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(AppError::db(format!(
                    "migrate chunks.embedding backfill: {error}"
                )));
            }
        };
        batches += 1;
        converted += updated as u64;
        if updated < batch {
            break;
        }
    }
    Ok(converted)
}

/// Drop `chunks.embedding_json` once every row has a native embedding.
///
/// DuckDB refuses to drop a column while an index covers a later column, and
/// the drop does not see indexes dropped earlier in the same transaction, so
/// the chunk indexes are dropped, the column dropped and the indexes
/// re-created as separate statements. A crash in between leaves the column
/// or missing indexes; both are repaired by the next open, which repeats the
/// core migration because the version is recorded last.
fn drop_legacy_embedding_column(conn: &Connection) -> Result<()> {
    let missing: i64 = conn.query_row(
        "SELECT count(*) FROM chunks WHERE embedding IS NULL",
        [],
        |row| row.get(0),
    )?;
    if missing > 0 {
        return Err(AppError::db(format!(
            "migrate chunks.embedding: {missing} rows still lack a converted embedding"
        )));
    }
    let mut stmt = conn.prepare(
        "SELECT index_name, sql FROM duckdb_indexes() \
         WHERE table_name = 'chunks' AND schema_name = current_schema() AND sql IS NOT NULL",
    )?;
    let indexes = crate::db::rows::query_vec(&mut stmt, [], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    drop(stmt);
    for (name, _) in &indexes {
        conn.execute_batch(&format!("DROP INDEX IF EXISTS \"{}\"", name.replace('"', "\"\"")))?;
    }
    conn.execute_batch("ALTER TABLE chunks DROP COLUMN embedding_json")?;
    for (_, sql) in &indexes {
        let sql = sql.trim().trim_end_matches(';');
        let sql = match sql.strip_prefix("CREATE INDEX ") {
            Some(rest) if !rest.starts_with("IF NOT EXISTS") => {
                format!("CREATE INDEX IF NOT EXISTS {rest}")
            }
            _ => sql.to_owned(),
        };
        conn.execute_batch(&sql)?;
    }
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
    let sql_if =
        format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS {column} {type_and_default}");
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
    let current = match conn.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
        row.get::<_, Option<i32>>(0)
    }) {
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
            "Native FLOAT[] chunk embeddings replace embedding_json; schema v26"
        ],
    )?;

    upsert_meta(conn, "schema_version", &SCHEMA_VERSION.to_string())?;
    Ok(())
}

pub(crate) fn upsert_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
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
    let v: Option<i32> = conn.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
        row.get::<_, Option<i32>>(0)
    })?;
    Ok(v)
}

#[cfg(test)]
mod embedding_migration_tests;
#[cfg(test)]
mod tests;
