//! Schema definitions and idempotent catalog initialization for the `lin` database.
//!
//! Declares all collections and secondary indexes for `rag-mcp` (`SCHEMA_VERSION = 25`),
//! assigning `text` strictly to FTS-searchable text fields (`documents.title`,
//! `documents.content`, `chunks.content`), `vec` to binary embeddings (`chunks.embedding`),
//! and `var` / `i64` / `f64` / `bool` to structured metadata and indexable keys.

use chrono::Utc;

use crate::db::lin_engine::{cell_i64_opt, cell_opt_string, cell_str, LinReader, LinWriter, RecBuilder};
use crate::error::Result;

/// Monotonic schema version recorded in `schema_version` and `meta`.
pub const SCHEMA_VERSION: i32 = 25;

pub const PAYLOAD_KV_COLLECTIONS: &[&str] = &[
    "search_feedback",
    "knowledge_views",
    "wiki_proposals",
    "eval_runs",
    "background_jobs",
    "eval_label_queue",
    "eval_traces",
    "eval_prompts",
];

/// Idempotently declare every `rag-mcp` collection, secondary index, and schema version
/// inside the target `lin` database.
pub fn migrate(w: &mut impl LinWriter) -> Result<()> {
    // 1. Core Sync Tables
    w.ensure_col(
        "sync_nodes",
        false,
        &[
            ("id", "id"),
            ("node_id", "var"),
            ("hostname", "var"),
            ("role", "var"),
            ("last_seen_at", "var"),
            ("last_push_seq", "i64"),
            ("pull_cursor", "i64"),
            ("last_error", "var"),
        ],
    )?;
    w.ensure_col(
        "sync_changes",
        false,
        &[
            ("id", "id"),
            ("primary_seq", "i64"),
            ("origin_node", "var"),
            ("origin_seq", "i64"),
            ("entity_kind", "var"),
            ("entity_id", "var"),
            ("operation", "var"),
            ("payload_json", "var"),
            ("content_hash", "var"),
            ("created_at", "var"),
        ],
    )?;
    w.ensure_col(
        "sync_outbox",
        false,
        &[
            ("id", "id"),
            ("local_seq", "i64"),
            ("entity_kind", "var"),
            ("entity_id", "var"),
            ("operation", "var"),
            ("payload_json", "var"),
            ("content_hash", "var"),
            ("created_at", "var"),
            ("sent_at", "var"),
            ("primary_seq", "i64"),
        ],
    )?;

    // 2. Documents & Revisions
    w.ensure_col(
        "documents",
        false,
        &[
            ("id", "id"),
            ("uri", "var"),
            ("title", "text"),
            ("content", "text"),
            ("metadata_json", "var"),
            ("created_at", "var"),
            ("updated_at", "var"),
            ("content_hash", "var"),
            ("wing", "var"),
            ("room", "var"),
            ("source_file", "var"),
            ("layer", "var"),
            ("kind", "var"),
            ("pinned", "bool"),
            ("boost", "f64"),
            ("status", "var"),
            ("revision", "i64"),
        ],
    )?;
    w.ensure_col(
        "document_revisions",
        false,
        &[
            ("id", "id"),
            ("document_id", "var"),
            ("revision", "i64"),
            ("uri", "var"),
            ("title", "var"),
            ("content", "var"),
            ("metadata_json", "var"),
            ("content_hash", "var"),
            ("wing", "var"),
            ("room", "var"),
            ("source_file", "var"),
            ("layer", "var"),
            ("kind", "var"),
            ("status", "var"),
            ("pinned", "bool"),
            ("boost", "f64"),
            ("created_at", "var"),
            ("updated_at", "var"),
            ("superseded_at", "var"),
        ],
    )?;

    // 3. Chunks & Change Log
    // Note: `snippet: var` is kept non-empty ("1") when an embedding is staged so `lin`'s
    // `maybe_embed_rows` invokes `BridgeEmbedder::embed_rows` and writes native `Cell::Vec`!
    w.ensure_col(
        "chunks",
        false,
        &[
            ("id", "id"),
            ("document_id", "var"),
            ("chunk_index", "i64"),
            ("content", "text"),
            ("snippet", "var"),
            ("vec", "vec"),
            ("embedding", "var"),
            ("embedding_json", "var"),
            ("char_start", "i64"),
            ("char_end", "i64"),
            ("created_at", "var"),
            ("content_hash", "var"),
            ("metadata_json", "var"),
            ("parent_chunk_id", "var"),
            ("embedding_digest", "i64"),
        ],
    )?;
    w.ensure_col(
        "chunk_change_log",
        false,
        &[
            ("id", "id"),
            ("generation", "i64"),
            ("document_id", "var"),
        ],
    )?;

    // 4. Graph Nodes & Edges
    w.ensure_col(
        "graph_nodes",
        false,
        &[
            ("id", "id"),
            ("kind", "var"),
            ("label", "var"),
            ("label_key", "var"),
            ("document_id", "var"),
            ("uri", "var"),
            ("resolved", "bool"),
            ("metadata_json", "var"),
            ("created_at", "var"),
            ("updated_at", "var"),
        ],
    )?;
    w.ensure_col(
        "graph_edges",
        false,
        &[
            ("id", "id"),
            ("source_id", "var"),
            ("target_id", "var"),
            ("rel_type", "var"),
            ("weight", "f64"),
            ("context", "var"),
            ("created_at", "var"),
            ("edge_origin", "var"),
            ("origin", "var"),
            ("alias", "var"),
            ("heading", "var"),
            ("chunk_id", "var"),
            ("char_start", "i64"),
            ("char_end", "i64"),
            ("occurrence", "i64"),
        ],
    )?;

    // 5. Ops Log, Wiki Index, Manifests, Re-embed Progress
    w.ensure_col(
        "ops_log",
        false,
        &[
            ("id", "id"),
            ("seq", "i64"),
            ("ts", "var"),
            ("op", "var"),
            ("prefix", "var"),
            ("message", "var"),
            ("entity_id", "var"),
            ("entity_kind", "var"),
            ("payload_json", "var"),
            ("agent_name", "var"),
        ],
    )?;
    w.ensure_col(
        "wiki_index",
        false,
        &[
            ("id", "id"),
            ("slug", "var"),
            ("title", "var"),
            ("label", "var"),
            ("kind", "var"),
            ("summary", "var"),
            ("category", "var"),
            ("document_id", "var"),
            ("page_id", "var"),
            ("updated_at", "var"),
        ],
    )?;
    w.ensure_col(
        "embedding_manifest",
        false,
        &[
            ("id", "id"),
            ("provider", "var"),
            ("model", "var"),
            ("dims", "i64"),
            ("base_url", "var"),
            ("content_fingerprint", "var"),
            ("updated_at", "var"),
        ],
    )?;
    w.ensure_col(
        "source_manifest",
        false,
        &[
            ("id", "id"),
            ("canonical_path", "var"),
            ("canonical_root", "var"),
            ("size_bytes", "i64"),
            ("mtime_ns", "i64"),
            ("content_hash", "var"),
            ("document_id", "var"),
            ("last_seen", "var"),
            ("last_synced", "var"),
        ],
    )?;
    w.ensure_col(
        "reembed_progress",
        false,
        &[
            ("id", "id"),
            ("target_fingerprint", "var"),
            ("document_id", "var"),
            ("chunks_reembedded", "i64"),
            ("completed_at", "var"),
        ],
    )?;

    // 6. Temporal Knowledge Graph (`kg_facts`)
    w.ensure_col(
        "kg_facts",
        false,
        &[
            ("id", "id"),
            ("subject", "var"),
            ("predicate", "var"),
            ("object", "var"),
            ("valid_from", "var"),
            ("valid_to", "var"),
            ("status", "var"),
            ("superseded_by", "var"),
            ("source_document_id", "var"),
            ("source", "var"),
            ("confidence", "f64"),
            ("metadata_json", "var"),
            ("created_at", "var"),
            ("updated_at", "var"),
            ("invalidated_at", "var"),
        ],
    )?;

    // 7. Schema Version & Meta
    w.ensure_col(
        "schema_version",
        false,
        &[
            ("id", "id"),
            ("version", "i64"),
            ("applied_at", "var"),
            ("note", "var"),
        ],
    )?;
    w.ensure_col(
        "meta",
        false,
        &[("id", "id"), ("key", "var"), ("value", "var")],
    )?;

    // 8. Collections
    w.ensure_col(
        "collections",
        false,
        &[
            ("id", "id"),
            ("name", "var"),
            ("description", "var"),
            ("metadata_json", "var"),
            ("created_at", "var"),
            ("updated_at", "var"),
        ],
    )?;
    w.ensure_col(
        "collection_entries",
        false,
        &[
            ("id", "id"),
            ("collection_id", "var"),
            ("document_id", "var"),
            ("position", "i64"),
            ("parent_document_id", "var"),
        ],
    )?;
    w.ensure_col(
        "collection_dependencies",
        false,
        &[
            ("id", "id"),
            ("collection_id", "var"),
            ("document_id", "var"),
            ("depends_on_document_id", "var"),
        ],
    )?;

    // 9. Payload KV Tables
    for &tbl in PAYLOAD_KV_COLLECTIONS {
        w.ensure_col(tbl, false, &[("id", "id"), ("payload", "var")])?;
    }

    // 10. Code Index Tables
    w.ensure_col(
        "code_index_documents",
        false,
        &[
            ("id", "id"),
            ("document_id", "var"),
            ("payload", "var"),
        ],
    )?;
    w.ensure_col(
        "code_index_source_blobs",
        false,
        &[
            ("id", "id"),
            ("content_hash", "var"),
            ("content", "var"),
        ],
    )?;
    w.ensure_col(
        "code_index_revisions",
        false,
        &[
            ("id", "id"),
            ("rev_id", "i64"),
            ("fingerprint", "var"),
            ("created_at", "var"),
            ("graph_json", "var"),
        ],
    )?;
    w.ensure_col(
        "code_index_revision_files",
        false,
        &[
            ("id", "id"),
            ("index_revision", "i64"),
            ("document_id", "var"),
            ("path", "var"),
            ("project", "var"),
            ("document_revision", "i64"),
            ("content_hash", "var"),
            ("analysis_json", "var"),
        ],
    )?;
    w.ensure_col(
        "code_index_runs",
        false,
        &[
            ("id", "id"),
            ("seq", "i64"),
            ("run_id", "var"),
            ("index_revision", "i64"),
            ("revision_created", "bool"),
            ("recorded_at", "var"),
            ("metrics_json", "var"),
        ],
    )?;
    w.ensure_col(
        "code_index_drift",
        false,
        &[
            ("id", "id"),
            ("event_id", "var"),
            ("seq", "i64"),
            ("index_revision", "i64"),
            ("document_id", "var"),
            ("path", "var"),
            ("project", "var"),
            ("kind", "var"),
            ("source_kind", "var"),
            ("indexed_hash", "var"),
            ("observed_hash", "var"),
            ("first_seen", "var"),
            ("last_seen", "var"),
            ("acknowledged_at", "var"),
            ("acknowledgement", "var"),
        ],
    )?;

    // 11. Analysis Tasks, Gate, Calibration, External DB Snapshots
    w.ensure_col(
        "analysis_tasks",
        false,
        &[
            ("id", "id"),
            ("session_id", "var"),
            ("name", "var"),
            ("generation", "i64"),
            ("updated_ms", "i64"),
            ("expires_ms", "i64"),
        ],
    )?;
    w.ensure_col(
        "analysis_notes",
        false,
        &[
            ("id", "id"),
            ("seq", "i64"),
            ("note_id", "var"),
            ("session_id", "var"),
            ("task", "var"),
            ("agent", "var"),
            ("path", "var"),
            ("tags_json", "var"),
            ("body", "var"),
            ("index_rev", "i64"),
            ("created_ms", "i64"),
        ],
    )?;
    w.ensure_col(
        "analysis_calls",
        false,
        &[
            ("id", "id"),
            ("seq", "i64"),
            ("session_id", "var"),
            ("task", "var"),
            ("tool", "var"),
            ("started_ms", "i64"),
            ("finished_ms", "i64"),
            ("ok", "bool"),
            ("evidence_count", "i64"),
            ("index_rev", "i64"),
        ],
    )?;
    w.ensure_col(
        "analysis_gate_decisions",
        false,
        &[
            ("id", "id"),
            ("seq", "i64"),
            ("decision_id", "var"),
            ("session_id", "var"),
            ("task", "var"),
            ("index_rev", "i64"),
            ("created_ms", "i64"),
            ("mode", "var"),
            ("kind", "var"),
            ("verdict", "var"),
            ("rule", "var"),
            ("facts_json", "var"),
        ],
    )?;
    w.ensure_col(
        "analysis_calibrations",
        false,
        &[
            ("id", "id"),
            ("seq", "i64"),
            ("calibration_id", "var"),
            ("created_ms", "i64"),
            ("kind", "var"),
            ("sample_size", "i64"),
            ("index_rev", "i64"),
            ("report_json", "var"),
        ],
    )?;
    w.ensure_col(
        "external_db_snapshots",
        false,
        &[
            ("id", "id"),
            ("connection_alias", "var"),
            ("captured_ms", "i64"),
            ("payload", "var"),
            ("seq", "i64"),
        ],
    )?;

    // 12. Secondary Indexes for O(log N) Seeks
    w.ensure_index("documents", false, &["uri"])?;
    w.ensure_index("documents", false, &["content_hash"])?;
    w.ensure_index("documents", false, &["wing", "room"])?;
    w.ensure_index("documents", false, &["layer"])?;
    w.ensure_index("documents", false, &["kind"])?;
    w.ensure_index("documents", false, &["status"])?;
    w.ensure_index("documents", false, &["source_file"])?;

    w.ensure_index("chunks", false, &["document_id"])?;
    w.ensure_index("chunks", false, &["parent_chunk_id"])?;
    w.ensure_index("chunk_change_log", false, &["generation"])?;

    w.ensure_index("graph_nodes", false, &["label"])?;
    w.ensure_index("graph_nodes", false, &["label_key"])?;
    w.ensure_index("graph_nodes", false, &["document_id"])?;
    w.ensure_index("graph_nodes", false, &["uri"])?;
    w.ensure_index("graph_nodes", false, &["kind"])?;

    w.ensure_index("graph_edges", false, &["source_id"])?;
    w.ensure_index("graph_edges", false, &["target_id"])?;
    w.ensure_index("graph_edges", false, &["origin"])?;
    w.ensure_index("graph_edges", false, &["rel_type"])?;

    w.ensure_index("source_manifest", false, &["canonical_root"])?;
    w.ensure_index("source_manifest", false, &["document_id"])?;

    w.ensure_index("reembed_progress", false, &["target_fingerprint"])?;
    w.ensure_index("reembed_progress", false, &["document_id"])?;

    w.ensure_index("kg_facts", false, &["subject"])?;
    w.ensure_index("kg_facts", false, &["predicate"])?;
    w.ensure_index("kg_facts", false, &["status"])?;
    w.ensure_index("kg_facts", false, &["subject", "predicate", "object"])?;

    w.ensure_index("document_revisions", false, &["document_id"])?;
    w.ensure_index("collection_entries", false, &["collection_id"])?;
    w.ensure_index("collection_entries", false, &["document_id"])?;
    w.ensure_index("collection_dependencies", false, &["collection_id"])?;
    w.ensure_index("collection_dependencies", false, &["document_id"])?;
    w.ensure_index("sync_changes", false, &["origin_node", "origin_seq"])?;
    w.ensure_index("code_index_revision_files", false, &["index_revision"])?;

    // 13. Data Backfills (run only when upgrading from < SCHEMA_VERSION; skipped in O(1) on current stores!)
    let prior_version = current_schema_version(w)?.unwrap_or(0);
    if prior_version < SCHEMA_VERSION {
        backfill_graph_node_label_keys(w)?;
        backfill_graph_edge_origins(w)?;
        backfill_chunk_parent_ids(w)?;
        record_schema_version(w)?;
    }
    Ok(())
}

/// Derive `graph_nodes.label_key` for rows written before the column existed.
pub fn backfill_graph_node_label_keys(w: &mut impl LinWriter) -> Result<u64> {
    let mut pending = Vec::new();
    w.scan_each("graph_nodes", |row| {
        if cell_str(row, "label_key").is_none()
            && let (Some(id), Some(label)) = (cell_str(row, "id"), cell_str(row, "label"))
        {
            let key = crate::graph::normalize::label_key(label);
            if !key.is_empty() {
                pending.push((id.to_string(), key));
            }
        }
        Ok(true)
    })?;
    let mut updated = 0u64;
    for (id, key) in pending {
        updated += w.update_where(
            "graph_nodes",
            lin::query::pred::eq("id", id.as_str()),
            RecBuilder::new().str("label_key", key).build(),
        )? as u64;
    }
    Ok(updated)
}

/// Map legacy `edge_origin` / unlabeled rows onto authoritative `origin` (`extract` | `explicit`).
pub fn backfill_graph_edge_origins(w: &mut impl LinWriter) -> Result<u64> {
    let mut updates: Vec<(String, &'static str)> = Vec::new();
    w.scan_each("graph_edges", |row| {
        let Some(id) = cell_str(row, "id") else {
            return Ok(true);
        };
        let rel_type = cell_str(row, "rel_type").unwrap_or("");
        let context = cell_str(row, "context").unwrap_or("");
        let edge_origin = cell_str(row, "edge_origin");
        let origin = cell_str(row, "origin");

        let is_membership = rel_type == "related"
            && crate::models::STRUCTURAL_EDGE_CONTEXTS.contains(&context);
        let is_extract_owned = crate::models::EXTRACT_OWNED_REL_TYPES.contains(&rel_type);

        match origin {
            None => {
                let target = if edge_origin == Some("derived")
                    || (edge_origin.is_none() && is_extract_owned)
                    || is_membership
                {
                    "extract"
                } else {
                    "explicit"
                };
                updates.push((id.to_string(), target));
            }
            Some("explicit") if is_membership => {
                updates.push((id.to_string(), "extract"));
            }
            _ => {}
        }
        Ok(true)
    })?;

    let mut done = 0u64;
    for (id, target) in updates {
        done += w.update_where(
            "graph_edges",
            lin::query::pred::eq("id", id.as_str()),
            RecBuilder::new().str("origin", target).build(),
        )? as u64;
    }
    Ok(done)
}

/// Mirror `metadata_json.parent_chunk_id` into `chunks.parent_chunk_id` when missing.
pub fn backfill_chunk_parent_ids(w: &mut impl LinWriter) -> Result<u64> {
    let mut pending = Vec::new();
    w.scan_each("chunks", |row| {
        if cell_str(row, "parent_chunk_id").is_none()
            && let Some(meta) = cell_str(row, "metadata_json")
            && meta.contains("parent_chunk_id")
            && let Ok(val) = serde_json::from_str::<serde_json::Value>(meta)
            && let Some(parent) = val.get("parent_chunk_id").and_then(|v| v.as_str())
            && !parent.is_empty()
            && let Some(id) = cell_str(row, "id")
        {
            pending.push((id.to_string(), parent.to_string()));
        }
        Ok(true)
    })?;

    let mut updated = 0u64;
    for (id, parent) in pending {
        updated += w.update_where(
            "chunks",
            lin::query::pred::eq("id", id.as_str()),
            RecBuilder::new().str("parent_chunk_id", parent).build(),
        )? as u64;
    }
    Ok(updated)
}

pub fn upsert_meta(w: &mut impl LinWriter, key: &str, value: &str) -> Result<()> {
    w.upsert_by_id(
        "meta",
        key,
        RecBuilder::new()
            .str("id", key)
            .str("key", key)
            .str("value", value)
            .build(),
    )
}

pub fn get_meta(r: &impl LinReader, key: &str) -> Result<Option<String>> {
    if !r.has_collection("meta") {
        return Ok(None);
    }
    if let Some(row) = r.get_by_id("meta", key)? {
        return Ok(cell_opt_string(&row, "value"));
    }
    let rows = r.find_by_eq("meta", "key", key)?;
    Ok(rows.first().and_then(|row| cell_opt_string(row, "value")))
}

pub fn delete_meta(w: &mut impl LinWriter, key: &str) -> Result<()> {
    let _ = w.delete_by_id("meta", key)?;
    let _ = w.delete_where("meta", lin::query::pred::eq("key", key))?;
    Ok(())
}

fn record_schema_version(w: &mut impl LinWriter) -> Result<()> {
    let current = current_schema_version(w)?;
    if current.is_some_and(|v| v >= SCHEMA_VERSION) {
        upsert_meta(w, "schema_version", &SCHEMA_VERSION.to_string())?;
        return Ok(());
    }
    let id = format!("v{SCHEMA_VERSION}");
    w.upsert_by_id(
        "schema_version",
        &id,
        RecBuilder::new()
            .str("id", &id)
            .i64("version", i64::from(SCHEMA_VERSION))
            .str("applied_at", Utc::now().to_rfc3339())
            .str(
                "note",
                "Migrated storage engine to lin v0.5.0; schema v25",
            )
            .build(),
    )?;
    upsert_meta(w, "schema_version", &SCHEMA_VERSION.to_string())
}

/// Read the highest applied schema version, if any.
pub fn current_schema_version(r: &impl LinReader) -> Result<Option<i32>> {
    if !r.has_collection("schema_version") {
        return Ok(None);
    }
    let mut max_ver: Option<i32> = None;
    r.scan_each("schema_version", |row| {
        if let Some(v) = cell_i64_opt(row, "version") {
            let v32 = v as i32;
            max_ver = Some(max_ver.map_or(v32, |cur| cur.max(v32)));
        }
        Ok(true)
    })?;
    if max_ver.is_none()
        && let Some(meta_v) = get_meta(r, "schema_version")?
        && let Ok(parsed) = meta_v.parse::<i32>()
    {
        return Ok(Some(parsed));
    }
    Ok(max_ver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::lin_engine::{write_lock, LinBackend};
    use std::sync::RwLock;

    fn open_mem() -> RwLock<LinBackend> {
        RwLock::new(LinBackend::open_memory())
    }

    #[test]
    fn migrate_fresh_creates_collections_and_version() {
        let backend = open_mem();
        let mut conn = write_lock(&backend).expect("write lock");
        migrate(&mut conn).expect("migrate");

        for t in [
            "documents",
            "chunks",
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
            "code_index_documents",
            "analysis_tasks",
            "external_db_snapshots",
        ] {
            assert!(conn.has_collection(t), "missing collection {t}");
        }

        let ver = current_schema_version(&conn).expect("version");
        assert_eq!(ver, Some(SCHEMA_VERSION));
    }

    #[test]
    fn migrate_is_idempotent() {
        let backend = open_mem();
        let mut conn = write_lock(&backend).expect("write lock");
        migrate(&mut conn).expect("migrate 1");
        migrate(&mut conn).expect("migrate 2");
        migrate(&mut conn).expect("migrate 3");

        assert_eq!(
            current_schema_version(&conn).expect("version"),
            Some(SCHEMA_VERSION)
        );
        assert_eq!(conn.count_rows("schema_version", None).unwrap(), 1);
    }

    #[test]
    fn origin_backfill_repairs_explicit_membership_rows_from_an_older_migration() {
        let backend = open_mem();
        let mut conn = write_lock(&backend).expect("write lock");
        migrate(&mut conn).expect("migrate");

        let now = Utc::now().to_rfc3339();
        conn.insert_one(
            "graph_edges",
            RecBuilder::new()
                .str("id", "edge-membership")
                .str("source_id", "doc-node")
                .str("target_id", "project-node")
                .str("rel_type", "related")
                .f64("weight", 0.5)
                .str("context", "project membership")
                .str("created_at", &now)
                .str("origin", "explicit")
                .build(),
        )
        .unwrap();
        conn.insert_one(
            "graph_edges",
            RecBuilder::new()
                .str("id", "edge-hand-made")
                .str("source_id", "doc-node")
                .str("target_id", "stub-node")
                .str("rel_type", "related")
                .f64("weight", 1.0)
                .str("created_at", &now)
                .str("origin", "explicit")
                .build(),
        )
        .unwrap();
        conn.insert_one(
            "graph_edges",
            RecBuilder::new()
                .str("id", "edge-untouched")
                .str("source_id", "doc-node")
                .str("target_id", "tag-node")
                .str("rel_type", "wikilink")
                .f64("weight", 1.0)
                .str("created_at", &now)
                .str("origin", "extract")
                .build(),
        )
        .unwrap();

        let repaired = backfill_graph_edge_origins(&mut conn).expect("repair pass");
        assert_eq!(repaired, 1);

        let origin_of = |id: &str| -> String {
            let row = conn.get_by_id("graph_edges", id).unwrap().unwrap();
            cell_str(&row, "origin").unwrap().to_string()
        };
        assert_eq!(origin_of("edge-membership"), "extract");
        assert_eq!(origin_of("edge-hand-made"), "explicit");
        assert_eq!(origin_of("edge-untouched"), "extract");
    }

    #[test]
    fn chunk_parent_backfill_mirrors_metadata_and_stays_lazy() {
        let backend = open_mem();
        let mut conn = write_lock(&backend).expect("write lock");
        migrate(&mut conn).expect("migrate");

        let now = Utc::now().to_rfc3339();
        for (id, idx, meta) in [
            ("parent", 0, "{}"),
            ("leaf", 1, r#"{"parent_chunk_id":"parent","section":"S"}"#),
            ("plain", 2, "{}"),
        ] {
            conn.insert_one(
                "chunks",
                RecBuilder::new()
                    .str("id", id)
                    .str("document_id", "d")
                    .i64("chunk_index", idx)
                    .str("content", "x")
                    .str("embedding_json", "[]")
                    .i64("char_start", 0)
                    .i64("char_end", 1)
                    .str("created_at", &now)
                    .str("metadata_json", meta)
                    .build(),
            )
            .unwrap();
        }

        assert_eq!(backfill_chunk_parent_ids(&mut conn).unwrap(), 1);
        let leaf = conn.get_by_id("chunks", "leaf").unwrap().unwrap();
        assert_eq!(cell_str(&leaf, "parent_chunk_id"), Some("parent"));
        let plain = conn.get_by_id("chunks", "plain").unwrap().unwrap();
        assert_eq!(cell_str(&plain, "parent_chunk_id"), None);
        assert_eq!(backfill_chunk_parent_ids(&mut conn).unwrap(), 0);
    }
}
