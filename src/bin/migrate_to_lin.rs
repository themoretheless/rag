//! Full corpus & state migrator from legacy `.duckdb` files into durable `.lin` directories.
//!
//! Usage:
//! ```bash
//! cargo run --release --bin migrate_to_lin -- \
//!   --src /path/to/rag.duckdb \
//!   --dst /path/to/rag.lin \
//!   --overwrite
//! ```
//!
//! Opens `--src` strictly read-only via `duckdb`, discovers all present tables and columns,
//! streams all 35 `rag-mcp` collections into `lin` (converting `chunks.embedding_json` into
//! native binary `Cell::Vec(Arc<[f32]>)` without storing bulky JSON text), runs `schema::migrate`
//! backfills and `fts::ensure_fts`, materializes a Zstd-compressed `.lin` directory snapshot,
//! and verifies 1:1 row counts + read-only reopen + vector dimensions.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use duckdb::{AccessMode, Config as DuckConfig, Connection};
use rag_mcp::db::Store;
use rag_mcp::db::fts::{self, LexFilters};
use rag_mcp::db::lin_engine::{
    LinReader, LinWriter, RecBuilder, cell_vec_f32, composite_id_2, composite_id_3,
};
use rag_mcp::db::schema::{self, PAYLOAD_KV_COLLECTIONS};
use rag_mcp::db::search;

const ROW_BATCH_SIZE: usize = 512;
const CHUNK_BATCH_SIZE: usize = 256;

struct CliArgs {
    src: PathBuf,
    dst: PathBuf,
    overwrite: bool,
    backup_only: bool,
    vacuum_only: bool,
}

fn parse_args() -> Result<CliArgs> {
    let mut src: Option<PathBuf> = None;
    let mut dst: Option<PathBuf> = None;
    let mut overwrite = false;
    let mut backup_only = false;
    let mut vacuum_only = false;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--src" => {
                let val = it.next().ok_or_else(|| anyhow!("--src requires a path"))?;
                src = Some(PathBuf::from(val));
            }
            "--dst" => {
                let val = it.next().ok_or_else(|| anyhow!("--dst requires a path"))?;
                dst = Some(PathBuf::from(val));
            }
            "--overwrite" => {
                overwrite = true;
            }
            "--backup-only" => {
                backup_only = true;
            }
            "--vacuum" | "--vacuum-only" => {
                vacuum_only = true;
            }
            "-h" | "--help" => {
                println!(
                    "Usage: migrate_to_lin --src <input> [--dst <output.lin>] [--overwrite] [--backup-only] [--vacuum]"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    let src = src.ok_or_else(|| anyhow!("missing required --src <input>"))?;
    let dst = if vacuum_only {
        dst.unwrap_or_else(|| src.clone())
    } else {
        dst.ok_or_else(|| anyhow!("missing required --dst <output.lin>"))?
    };
    Ok(CliArgs {
        src,
        dst,
        overwrite,
        backup_only,
        vacuum_only,
    })
}

fn main() -> Result<()> {
    rag_mcp::bootstrap::init_tracing("info");
    let args = parse_args()?;
    if args.vacuum_only {
        let t0 = Instant::now();
        println!(
            "==> Running DiskEngineV2 LSM compaction (IVF-SQ8 upgrade) + MmapBm25Index v3 on '{}'...",
            args.src.display()
        );
        let store = Store::open(&args.src).map_err(|e| anyhow!("Store::open failed: {e}"))?;
        let report = store
            .vacuum_store()
            .map_err(|e| anyhow!("vacuum_store failed: {e}"))?;
        let bm25_persisted = fts::persist_bm25_disk_cache(&store, "porter")
            .map_err(|e| anyhow!("persist_bm25_disk_cache failed: {e}"))?;
        let stats = store.stats().map_err(|e| anyhow!("stats failed: {e}"))?;
        drop(store);

        let ro = Store::open_read_only(&args.src)
            .map_err(|e| anyhow!("Store::open_read_only failed: {e}"))?;
        let ro_stats = ro.stats().map_err(|e| anyhow!("ro.stats failed: {e}"))?;
        assert_eq!(stats, ro_stats);

        let bm25_mmap_bytes = dir_or_file_bytes(&args.src.join("bm25_porter.v3.mmap"));
        println!(
            "==> Vacuum & v2 Phase-2 upgrade complete in {:.2}s!\n    Size before: {} -> Size after: {} | bm25_porter.v3.mmap: {} (persisted={})\n    Verified stats: docs={} chunks={} nodes={} edges={}",
            t0.elapsed().as_secs_f64(),
            format_mib(report.bytes_before.unwrap_or(0)),
            format_mib(report.bytes_after.unwrap_or(0)),
            format_mib(bm25_mmap_bytes),
            bm25_persisted,
            stats.0,
            stats.1,
            stats.2,
            stats.3,
        );
        return Ok(());
    }
    if args.backup_only {
        let t0 = Instant::now();
        println!(
            "==> Creating verified portable backup from '{}' -> '{}'...",
            args.src.display(),
            args.dst.display()
        );
        let store = Store::open(&args.src).map_err(|e| anyhow!("Store::open failed: {e}"))?;
        let report = store
            .backup_database(&args.dst, false, args.overwrite)
            .map_err(|e| anyhow!("backup_database failed: {e}"))?;
        if !report.success {
            bail!("backup_database reported failure: {:?}", report.errors);
        }
        println!(
            "==> Verified backup complete in {:.2}s! Size: {} | sha256={}",
            t0.elapsed().as_secs_f64(),
            format_mib(report.bytes.unwrap_or(0)),
            report.sha256.as_deref().unwrap_or("n/a"),
        );
        return Ok(());
    }
    if rag_mcp::db::lin_engine::is_legacy_lin_v1_dir(&args.src)
        || rag_mcp::db::lin_engine::is_lin_portable_backup_file(&args.src)
    {
        return run_lin_v1_to_v2_upgrade(&args);
    }
    run_migration(&args)
}

fn run_lin_v1_to_v2_upgrade(args: &CliArgs) -> Result<()> {
    let t0 = Instant::now();
    if !args.src.exists() {
        bail!("source lin path does not exist: {}", args.src.display());
    }
    if args.dst.exists() {
        if !args.overwrite {
            bail!(
                "destination already exists: {} (pass --overwrite to replace)",
                args.dst.display()
            );
        }
        if args.dst.is_dir() {
            std::fs::remove_dir_all(&args.dst)?;
        } else {
            std::fs::remove_file(&args.dst)?;
        }
    }
    println!(
        "==> Upgrading legacy lin v1 '{}' -> DiskEngineV2 '{}' (65,536 rows/segment + mmap indexes)...",
        args.src.display(),
        args.dst.display()
    );
    let eng = lin::disk_v2::DiskEngineV2::migrate_from_legacy(
        &args.src,
        &args.dst,
        65_536,
        &rag_mcp::db::lin_engine::rag_default_indexed_fields(),
    )
    .map_err(|e| anyhow!("migrate_from_legacy failed: {e}"))?;
    drop(eng);

    println!("==> Reopening DiskEngineV2 Store & persisting BM25 disk cache...");
    let store = Store::open(&args.dst).map_err(|e| anyhow!("Store::open failed: {e}"))?;
    store
        .checkpoint()
        .map_err(|e| anyhow!("checkpoint failed: {e}"))?;
    let persisted = fts::persist_bm25_disk_cache(&store, "porter")
        .map_err(|e| anyhow!("persist_bm25_disk_cache failed: {e}"))?;
    let stats = store.stats().map_err(|e| anyhow!("stats failed: {e}"))?;
    drop(store);

    let ro = Store::open_read_only(&args.dst)
        .map_err(|e| anyhow!("Store::open_read_only failed: {e}"))?;
    let ro_stats = ro.stats().map_err(|e| anyhow!("ro.stats failed: {e}"))?;
    assert_eq!(stats, ro_stats);

    let dst_bytes = dir_or_file_bytes(&args.dst);
    println!(
        "==> Upgrade complete in {:.2}s! Size: {} | docs={} chunks={} nodes={} edges={} | bm25_disk={}",
        t0.elapsed().as_secs_f64(),
        format_mib(dst_bytes),
        stats.0,
        stats.1,
        stats.2,
        stats.3,
        persisted,
    );
    Ok(())
}

type SchemaMap = HashMap<String, HashSet<String>>;

fn discover_duckdb_schema(conn: &Connection) -> Result<SchemaMap> {
    let mut stmt = conn.prepare(
        "SELECT table_name, column_name \
         FROM information_schema.columns \
         WHERE table_schema = 'main'",
    )?;
    let mut rows = stmt.query([])?;
    let mut map: SchemaMap = HashMap::new();
    while let Some(row) = rows.next()? {
        let tbl: String = row.get(0)?;
        let col: String = row.get(1)?;
        map.entry(tbl).or_default().insert(col);
    }
    Ok(map)
}

fn sql_col(cols: &HashSet<String>, name: &str) -> String {
    if cols.contains(name) {
        format!("\"{name}\"")
    } else {
        format!("NULL AS \"{name}\"")
    }
}

fn sql_ts_col(cols: &HashSet<String>, name: &str) -> String {
    if cols.contains(name) {
        format!("CAST(\"{name}\" AS VARCHAR)")
    } else {
        format!("NULL AS \"{name}\"")
    }
}

fn embedding_digest_str(json: &str) -> i64 {
    let hash = blake3::hash(json.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash.as_bytes()[..8]);
    i64::from_le_bytes(bytes)
}

fn normalize_ts_opt(raw: Option<String>) -> Option<String> {
    raw.map(|s| {
        let trimmed = s.trim();
        if let Some(dt) = rag_mcp::util::parse_db_timestamp(trimmed) {
            rag_mcp::util::format_db_timestamp(dt)
        } else {
            trimmed.to_string()
        }
    })
    .filter(|s| !s.is_empty())
}

fn normalize_ts_or_now(raw: Option<String>) -> String {
    normalize_ts_opt(raw).unwrap_or_else(|| rag_mcp::util::format_db_timestamp(chrono::Utc::now()))
}

fn dir_or_file_bytes(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if meta.is_dir() {
        let mut total = 0u64;
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                total = total.saturating_add(dir_or_file_bytes(&entry.path()));
            }
        }
        return total;
    }
    0
}

fn format_mib(bytes: u64) -> String {
    let mib = bytes as f64 / (1024.0 * 1024.0);
    if mib >= 1024.0 {
        format!("{:.2} GiB", mib / 1024.0)
    } else {
        format!("{mib:.2} MiB")
    }
}

fn count_duck_table(conn: &Connection, table: &str) -> Result<usize> {
    let sql = format!("SELECT COUNT(*) FROM \"{table}\"");
    let count: i64 = conn.query_row(&sql, [], |r| r.get(0))?;
    Ok(count.max(0) as usize)
}

struct TableReport {
    src_rows: usize,
    dst_rows: usize,
    exact_1_to_1: bool,
}

#[allow(clippy::too_many_lines)]
fn run_migration(args: &CliArgs) -> Result<()> {
    let t0 = Instant::now();
    if !args.src.exists() {
        bail!("source DuckDB file does not exist: {}", args.src.display());
    }
    if args.dst.exists() {
        if !args.overwrite {
            bail!(
                "destination already exists: {} (pass --overwrite to replace)",
                args.dst.display()
            );
        }
        if args.dst.is_dir() {
            std::fs::remove_dir_all(&args.dst).with_context(|| {
                format!("failed to remove existing directory {}", args.dst.display())
            })?;
        } else {
            std::fs::remove_file(&args.dst).with_context(|| {
                format!("failed to remove existing file {}", args.dst.display())
            })?;
        }
    }

    let src_bytes = dir_or_file_bytes(&args.src);
    println!(
        "==> Opening DuckDB source (read-only): {} ({})",
        args.src.display(),
        format_mib(src_bytes)
    );
    let duck_cfg = DuckConfig::default().access_mode(AccessMode::ReadOnly)?;
    let conn = Connection::open_with_flags(&args.src, duck_cfg)
        .with_context(|| format!("failed to open DuckDB read-only: {}", args.src.display()))?;

    let schema_map = discover_duckdb_schema(&conn)?;
    println!(
        "    Discovered {} tables in source DuckDB",
        schema_map.len()
    );

    let mem_store = Store::open_in_memory().context("failed to initialize in-memory lin Store")?;
    let mut reports: BTreeMap<String, TableReport> = BTreeMap::new();
    let mut migrated_vectors = 0usize;
    let mut detected_dims: Option<usize> = None;

    {
        let mut w = mem_store
            .write()
            .map_err(|e| anyhow!("lin write lock: {e}"))?;

        // 1. sync_nodes
        if let Some(cols) = schema_map.get("sync_nodes") {
            let src_rows = count_duck_table(&conn, "sync_nodes")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {} FROM \"sync_nodes\"",
                sql_col(cols, "node_id"),
                sql_col(cols, "hostname"),
                sql_col(cols, "role"),
                sql_ts_col(cols, "last_seen_at"),
                sql_col(cols, "last_push_seq"),
                sql_col(cols, "pull_cursor"),
                sql_col(cols, "last_error"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let node_id: String = r.get(0)?;
                let hostname: Option<String> = r.get(1)?;
                let role: Option<String> = r.get(2)?;
                let last_seen_at: Option<String> = r.get(3)?;
                let last_push_seq: Option<i64> = r.get(4)?;
                let pull_cursor: Option<i64> = r.get(5)?;
                let last_error: Option<String> = r.get(6)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", &node_id)
                        .str("node_id", &node_id)
                        .str("hostname", hostname.unwrap_or_default())
                        .str("role", role.unwrap_or_else(|| "replica".into()))
                        .str("last_seen_at", normalize_ts_or_now(last_seen_at))
                        .i64("last_push_seq", last_push_seq.unwrap_or(0))
                        .i64("pull_cursor", pull_cursor.unwrap_or(0))
                        .opt_string("last_error", &last_error)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("sync_nodes", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("sync_nodes", batch)?;
            }
            reports.insert(
                "sync_nodes".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 2. sync_changes
        if let Some(cols) = schema_map.get("sync_changes") {
            let src_rows = count_duck_table(&conn, "sync_changes")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"sync_changes\"",
                sql_col(cols, "primary_seq"),
                sql_col(cols, "origin_node"),
                sql_col(cols, "origin_seq"),
                sql_col(cols, "entity_kind"),
                sql_col(cols, "entity_id"),
                sql_col(cols, "operation"),
                sql_col(cols, "payload_json"),
                sql_col(cols, "content_hash"),
                sql_ts_col(cols, "created_at"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let primary_seq: i64 = r.get(0)?;
                let origin_node: String = r.get(1)?;
                let origin_seq: i64 = r.get(2)?;
                let entity_kind: String = r.get(3)?;
                let entity_id: String = r.get(4)?;
                let operation: String = r.get(5)?;
                let payload_json: String = r.get(6)?;
                let content_hash: Option<String> = r.get(7)?;
                let created_at: Option<String> = r.get(8)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", primary_seq.to_string())
                        .i64("primary_seq", primary_seq)
                        .str("origin_node", origin_node)
                        .i64("origin_seq", origin_seq)
                        .str("entity_kind", entity_kind)
                        .str("entity_id", entity_id)
                        .str("operation", operation)
                        .str("payload_json", payload_json)
                        .opt_string("content_hash", &content_hash)
                        .str("created_at", normalize_ts_or_now(created_at))
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("sync_changes", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("sync_changes", batch)?;
            }
            reports.insert(
                "sync_changes".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 3. sync_outbox
        if let Some(cols) = schema_map.get("sync_outbox") {
            let src_rows = count_duck_table(&conn, "sync_outbox")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"sync_outbox\"",
                sql_col(cols, "local_seq"),
                sql_col(cols, "entity_kind"),
                sql_col(cols, "entity_id"),
                sql_col(cols, "operation"),
                sql_col(cols, "payload_json"),
                sql_col(cols, "content_hash"),
                sql_ts_col(cols, "created_at"),
                sql_ts_col(cols, "sent_at"),
                sql_col(cols, "primary_seq"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let local_seq: i64 = r.get(0)?;
                let entity_kind: String = r.get(1)?;
                let entity_id: String = r.get(2)?;
                let operation: String = r.get(3)?;
                let payload_json: String = r.get(4)?;
                let content_hash: Option<String> = r.get(5)?;
                let created_at: Option<String> = r.get(6)?;
                let sent_at: Option<String> = normalize_ts_opt(r.get(7)?);
                let primary_seq: Option<i64> = r.get(8)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", local_seq.to_string())
                        .i64("local_seq", local_seq)
                        .str("entity_kind", entity_kind)
                        .str("entity_id", entity_id)
                        .str("operation", operation)
                        .str("payload_json", payload_json)
                        .opt_string("content_hash", &content_hash)
                        .str("created_at", normalize_ts_or_now(created_at))
                        .opt_string("sent_at", &sent_at)
                        .opt_i64("primary_seq", primary_seq)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("sync_outbox", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("sync_outbox", batch)?;
            }
            reports.insert(
                "sync_outbox".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 4. documents
        if let Some(cols) = schema_map.get("documents") {
            let src_rows = count_duck_table(&conn, "documents")?;
            let t_doc = Instant::now();
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"documents\"",
                sql_col(cols, "id"),
                sql_col(cols, "uri"),
                sql_col(cols, "title"),
                sql_col(cols, "content"),
                sql_col(cols, "metadata_json"),
                sql_ts_col(cols, "created_at"),
                sql_ts_col(cols, "updated_at"),
                sql_col(cols, "content_hash"),
                sql_col(cols, "wing"),
                sql_col(cols, "room"),
                sql_col(cols, "source_file"),
                sql_col(cols, "layer"),
                sql_col(cols, "kind"),
                sql_col(cols, "pinned"),
                sql_col(cols, "boost"),
                sql_col(cols, "status"),
                sql_col(cols, "revision"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                let uri: String = r.get(1)?;
                let title: Option<String> = r.get(2)?;
                let content: Option<String> = r.get(3)?;
                let metadata_json: Option<String> = r.get(4)?;
                let created_at: Option<String> = r.get(5)?;
                let updated_at: Option<String> = r.get(6)?;
                let content_hash: Option<String> = r.get(7)?;
                let wing: Option<String> = r.get(8)?;
                let room: Option<String> = r.get(9)?;
                let source_file: Option<String> = r.get(10)?;
                let layer: Option<String> = r.get(11)?;
                let kind: Option<String> = r.get(12)?;
                let pinned: Option<bool> = r.get(13)?;
                let boost: Option<f64> = r.get(14)?;
                let status: Option<String> = r.get(15)?;
                let revision: Option<i64> = r.get(16)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .str("uri", uri)
                        .str("title", title.unwrap_or_default())
                        .str("content", content.unwrap_or_default())
                        .str(
                            "metadata_json",
                            metadata_json.unwrap_or_else(|| "{}".into()),
                        )
                        .str("created_at", normalize_ts_or_now(created_at))
                        .str("updated_at", normalize_ts_or_now(updated_at))
                        .opt_string("content_hash", &content_hash)
                        .opt_string("wing", &wing)
                        .opt_string("room", &room)
                        .opt_string("source_file", &source_file)
                        .str("layer", layer.unwrap_or_else(|| "raw".into()))
                        .str("kind", kind.unwrap_or_else(|| "document".into()))
                        .bool("pinned", pinned.unwrap_or(false))
                        .f64("boost", boost.unwrap_or(1.0))
                        .str("status", status.unwrap_or_else(|| "active".into()))
                        .i64("revision", revision.unwrap_or(1).max(1))
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("documents", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("documents", batch)?;
            }
            println!(
                "    Migrated documents: {src_rows} rows in {:.2}s",
                t_doc.elapsed().as_secs_f64()
            );
            reports.insert(
                "documents".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 5. document_revisions
        if let Some(cols) = schema_map.get("document_revisions") {
            let src_rows = count_duck_table(&conn, "document_revisions")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"document_revisions\"",
                sql_col(cols, "document_id"),
                sql_col(cols, "revision"),
                sql_col(cols, "uri"),
                sql_col(cols, "title"),
                sql_col(cols, "content"),
                sql_col(cols, "metadata_json"),
                sql_col(cols, "content_hash"),
                sql_col(cols, "wing"),
                sql_col(cols, "room"),
                sql_col(cols, "source_file"),
                sql_col(cols, "layer"),
                sql_col(cols, "kind"),
                sql_col(cols, "status"),
                sql_col(cols, "pinned"),
                sql_col(cols, "boost"),
                sql_ts_col(cols, "created_at"),
                sql_ts_col(cols, "updated_at"),
                sql_ts_col(cols, "superseded_at"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let document_id: String = r.get(0)?;
                let revision: i64 = r.get(1)?;
                let uri: String = r.get(2)?;
                let title: Option<String> = r.get(3)?;
                let content: Option<String> = r.get(4)?;
                let metadata_json: Option<String> = r.get(5)?;
                let content_hash: Option<String> = r.get(6)?;
                let wing: Option<String> = r.get(7)?;
                let room: Option<String> = r.get(8)?;
                let source_file: Option<String> = r.get(9)?;
                let layer: Option<String> = r.get(10)?;
                let kind: Option<String> = r.get(11)?;
                let status: Option<String> = r.get(12)?;
                let pinned: Option<bool> = r.get(13)?;
                let boost: Option<f64> = r.get(14)?;
                let created_at: Option<String> = r.get(15)?;
                let updated_at: Option<String> = r.get(16)?;
                let superseded_at: Option<String> = r.get(17)?;
                let id = composite_id_2(&document_id, &revision.to_string());
                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .str("document_id", document_id)
                        .i64("revision", revision)
                        .str("uri", uri)
                        .str("title", title.unwrap_or_default())
                        .str("content", content.unwrap_or_default())
                        .str(
                            "metadata_json",
                            metadata_json.unwrap_or_else(|| "{}".into()),
                        )
                        .opt_string("content_hash", &content_hash)
                        .opt_string("wing", &wing)
                        .opt_string("room", &room)
                        .opt_string("source_file", &source_file)
                        .opt_string("layer", &layer)
                        .opt_string("kind", &kind)
                        .opt_string("status", &status)
                        .bool("pinned", pinned.unwrap_or(false))
                        .f64("boost", boost.unwrap_or(1.0))
                        .str("created_at", normalize_ts_or_now(created_at))
                        .str("updated_at", normalize_ts_or_now(updated_at))
                        .str("superseded_at", normalize_ts_or_now(superseded_at))
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("document_revisions", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("document_revisions", batch)?;
            }
            reports.insert(
                "document_revisions".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 6. chunks (JSON embeddings -> native binary Cell::Vec(Arc<[f32]>))
        if let Some(cols) = schema_map.get("chunks") {
            let src_rows = count_duck_table(&conn, "chunks")?;
            let t_chunks = Instant::now();
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"chunks\"",
                sql_col(cols, "id"),
                sql_col(cols, "document_id"),
                sql_col(cols, "chunk_index"),
                sql_col(cols, "content"),
                sql_col(cols, "embedding_json"),
                sql_col(cols, "char_start"),
                sql_col(cols, "char_end"),
                sql_ts_col(cols, "created_at"),
                sql_col(cols, "content_hash"),
                sql_col(cols, "metadata_json"),
                sql_col(cols, "parent_chunk_id"),
                sql_col(cols, "embedding_digest"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let bridge = w.bridge_embedder().clone();
            let mut batch = Vec::with_capacity(CHUNK_BATCH_SIZE);
            let mut processed = 0usize;
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                let document_id: String = r.get(1)?;
                let chunk_index: i64 = r.get(2)?;
                let content: Option<String> = r.get(3)?;
                let embedding_json: Option<String> = r.get(4)?;
                let char_start: i64 = r.get(5)?;
                let char_end: i64 = r.get(6)?;
                let created_at: Option<String> = r.get(7)?;
                let content_hash: Option<String> = r.get(8)?;
                let metadata_json: Option<String> = r.get(9)?;
                let parent_chunk_id: Option<String> = r.get(10)?;
                let existing_digest: Option<i64> = r.get(11)?;

                let json_str = embedding_json.as_deref().unwrap_or("[]");
                let vec_f32: Vec<f32> = serde_json::from_str(json_str)
                    .with_context(|| format!("failed to parse embedding_json for chunk id={id}"))?;
                let has_vec = !vec_f32.is_empty();
                if has_vec {
                    if detected_dims.is_none() {
                        detected_dims = Some(vec_f32.len());
                    }
                    bridge.stage(id.clone(), Arc::from(vec_f32.into_boxed_slice()));
                    migrated_vectors += 1;
                }
                let digest = existing_digest.unwrap_or_else(|| embedding_digest_str(json_str));

                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .str("document_id", document_id)
                        .i64("chunk_index", chunk_index)
                        .str("content", content.unwrap_or_default())
                        .opt_str("snippet", has_vec.then_some("1"))
                        .i64("char_start", char_start)
                        .i64("char_end", char_end)
                        .str("created_at", normalize_ts_or_now(created_at))
                        .opt_string("content_hash", &content_hash)
                        .str(
                            "metadata_json",
                            metadata_json.unwrap_or_else(|| "{}".into()),
                        )
                        .opt_string("parent_chunk_id", &parent_chunk_id)
                        .i64("embedding_digest", digest)
                        .build(),
                );
                processed += 1;
                if batch.len() >= CHUNK_BATCH_SIZE {
                    w.insert_records("chunks", std::mem::take(&mut batch))?;
                }
                if processed % 100_000 == 0 {
                    println!(
                        "      ... chunks progress: {processed}/{src_rows} ({:.1}s)",
                        t_chunks.elapsed().as_secs_f64()
                    );
                }
            }
            if !batch.is_empty() {
                w.insert_records("chunks", batch)?;
            }
            println!(
                "    Migrated chunks: {src_rows} rows ({migrated_vectors} binary vectors, dims={:?}) in {:.2}s",
                detected_dims,
                t_chunks.elapsed().as_secs_f64()
            );
            reports.insert(
                "chunks".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 7. chunk_change_log
        if let Some(cols) = schema_map.get("chunk_change_log") {
            let src_rows = count_duck_table(&conn, "chunk_change_log")?;
            let sql = format!(
                "SELECT {}, {} FROM \"chunk_change_log\"",
                sql_col(cols, "generation"),
                sql_col(cols, "document_id"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            let mut idx = 0usize;
            while let Some(r) = rows.next()? {
                let generation: i64 = r.get(0)?;
                let document_id: Option<String> = r.get(1)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", format!("mig:{idx}:{generation}"))
                        .i64("generation", generation)
                        .opt_string("document_id", &document_id)
                        .build(),
                );
                idx += 1;
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("chunk_change_log", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("chunk_change_log", batch)?;
            }
            reports.insert(
                "chunk_change_log".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 8. graph_nodes
        if let Some(cols) = schema_map.get("graph_nodes") {
            let src_rows = count_duck_table(&conn, "graph_nodes")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"graph_nodes\"",
                sql_col(cols, "id"),
                sql_col(cols, "kind"),
                sql_col(cols, "label"),
                sql_col(cols, "label_key"),
                sql_col(cols, "document_id"),
                sql_col(cols, "uri"),
                sql_col(cols, "resolved"),
                sql_col(cols, "metadata_json"),
                sql_ts_col(cols, "created_at"),
                sql_ts_col(cols, "updated_at"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                let kind: String = r.get(1)?;
                let label: String = r.get(2)?;
                let label_key: Option<String> = r.get(3)?;
                let document_id: Option<String> = r.get(4)?;
                let uri: Option<String> = r.get(5)?;
                let resolved: Option<bool> = r.get(6)?;
                let metadata_json: Option<String> = r.get(7)?;
                let created_at: Option<String> = r.get(8)?;
                let updated_at: Option<String> = r.get(9)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .str("kind", kind)
                        .str("label", label)
                        .opt_string("label_key", &label_key)
                        .opt_string("document_id", &document_id)
                        .opt_string("uri", &uri)
                        .bool("resolved", resolved.unwrap_or(true))
                        .str(
                            "metadata_json",
                            metadata_json.unwrap_or_else(|| "{}".into()),
                        )
                        .str("created_at", normalize_ts_or_now(created_at))
                        .str("updated_at", normalize_ts_or_now(updated_at))
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("graph_nodes", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("graph_nodes", batch)?;
            }
            reports.insert(
                "graph_nodes".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 9. graph_edges
        if let Some(cols) = schema_map.get("graph_edges") {
            let src_rows = count_duck_table(&conn, "graph_edges")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"graph_edges\"",
                sql_col(cols, "id"),
                sql_col(cols, "source_id"),
                sql_col(cols, "target_id"),
                sql_col(cols, "rel_type"),
                sql_col(cols, "weight"),
                sql_col(cols, "context"),
                sql_ts_col(cols, "created_at"),
                sql_col(cols, "edge_origin"),
                sql_col(cols, "origin"),
                sql_col(cols, "alias"),
                sql_col(cols, "heading"),
                sql_col(cols, "chunk_id"),
                sql_col(cols, "char_start"),
                sql_col(cols, "char_end"),
                sql_col(cols, "occurrence"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                let source_id: String = r.get(1)?;
                let target_id: String = r.get(2)?;
                let rel_type: String = r.get(3)?;
                let weight: Option<f64> = r.get(4)?;
                let context: Option<String> = r.get(5)?;
                let created_at: Option<String> = r.get(6)?;
                let edge_origin: Option<String> = r.get(7)?;
                let origin: Option<String> = r.get(8)?;
                let alias: Option<String> = r.get(9)?;
                let heading: Option<String> = r.get(10)?;
                let chunk_id: Option<String> = r.get(11)?;
                let char_start: Option<i64> = r.get(12)?;
                let char_end: Option<i64> = r.get(13)?;
                let occurrence: Option<i64> = r.get(14)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .str("source_id", source_id)
                        .str("target_id", target_id)
                        .str("rel_type", rel_type)
                        .f64("weight", weight.unwrap_or(1.0))
                        .opt_string("context", &context)
                        .str("created_at", normalize_ts_or_now(created_at))
                        .opt_string("edge_origin", &edge_origin)
                        .opt_string("origin", &origin)
                        .opt_string("alias", &alias)
                        .opt_string("heading", &heading)
                        .opt_string("chunk_id", &chunk_id)
                        .opt_i64("char_start", char_start)
                        .opt_i64("char_end", char_end)
                        .opt_i64("occurrence", occurrence)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("graph_edges", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("graph_edges", batch)?;
            }
            reports.insert(
                "graph_edges".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 10. ops_log
        if let Some(cols) = schema_map.get("ops_log") {
            let src_rows = count_duck_table(&conn, "ops_log")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"ops_log\"",
                sql_col(cols, "id"),
                sql_col(cols, "seq"),
                sql_ts_col(cols, "ts"),
                sql_col(cols, "op"),
                sql_col(cols, "prefix"),
                sql_col(cols, "message"),
                sql_col(cols, "entity_id"),
                sql_col(cols, "entity_kind"),
                sql_col(cols, "payload_json"),
                sql_col(cols, "agent_name"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            let mut idx = 0usize;
            while let Some(r) = rows.next()? {
                let id_opt: Option<String> = r.get(0)?;
                let seq: Option<i64> = r.get(1)?;
                let ts: Option<String> = r.get(2)?;
                let op: Option<String> = r.get(3)?;
                let prefix: Option<String> = r.get(4)?;
                let message: Option<String> = r.get(5)?;
                let entity_id: Option<String> = r.get(6)?;
                let entity_kind: Option<String> = r.get(7)?;
                let payload_json: Option<String> = r.get(8)?;
                let agent_name: Option<String> = r.get(9)?;
                let id = id_opt
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("seq:{}:{idx}", seq.unwrap_or(0)));
                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .opt_i64("seq", seq)
                        .str("ts", normalize_ts_or_now(ts))
                        .str("op", op.unwrap_or_default())
                        .opt_string("prefix", &prefix)
                        .str("message", message.unwrap_or_default())
                        .opt_string("entity_id", &entity_id)
                        .opt_string("entity_kind", &entity_kind)
                        .str("payload_json", payload_json.unwrap_or_else(|| "{}".into()))
                        .opt_string("agent_name", &agent_name)
                        .build(),
                );
                idx += 1;
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("ops_log", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("ops_log", batch)?;
            }
            reports.insert(
                "ops_log".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 11. wiki_index
        if let Some(cols) = schema_map.get("wiki_index") {
            let src_rows = count_duck_table(&conn, "wiki_index")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"wiki_index\"",
                sql_col(cols, "id"),
                sql_col(cols, "slug"),
                sql_col(cols, "title"),
                sql_col(cols, "label"),
                sql_col(cols, "kind"),
                sql_col(cols, "summary"),
                sql_col(cols, "category"),
                sql_col(cols, "document_id"),
                sql_col(cols, "page_id"),
                sql_ts_col(cols, "updated_at"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                let slug: Option<String> = r.get(1)?;
                let title: Option<String> = r.get(2)?;
                let label: Option<String> = r.get(3)?;
                let kind: Option<String> = r.get(4)?;
                let summary: Option<String> = r.get(5)?;
                let category: Option<String> = r.get(6)?;
                let document_id: Option<String> = r.get(7)?;
                let page_id: Option<String> = r.get(8)?;
                let updated_at: Option<String> = r.get(9)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .opt_string("slug", &slug)
                        .opt_string("title", &title)
                        .opt_string("label", &label)
                        .str("kind", kind.unwrap_or_else(|| "wiki".into()))
                        .opt_string("summary", &summary)
                        .opt_string("category", &category)
                        .opt_string("document_id", &document_id)
                        .opt_string("page_id", &page_id)
                        .str("updated_at", normalize_ts_or_now(updated_at))
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("wiki_index", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("wiki_index", batch)?;
            }
            reports.insert(
                "wiki_index".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 12. embedding_manifest
        if let Some(cols) = schema_map.get("embedding_manifest") {
            let src_rows = count_duck_table(&conn, "embedding_manifest")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {} FROM \"embedding_manifest\"",
                sql_col(cols, "id"),
                sql_col(cols, "provider"),
                sql_col(cols, "model"),
                sql_col(cols, "dims"),
                sql_col(cols, "base_url"),
                sql_col(cols, "content_fingerprint"),
                sql_ts_col(cols, "updated_at"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                let provider: String = r.get(1)?;
                let model: String = r.get(2)?;
                let dims: i64 = r.get(3)?;
                let base_url: Option<String> = r.get(4)?;
                let content_fingerprint: Option<String> = r.get(5)?;
                let updated_at: Option<String> = r.get(6)?;
                w.upsert_by_id(
                    "embedding_manifest",
                    &id,
                    RecBuilder::new()
                        .str("id", &id)
                        .str("provider", provider)
                        .str("model", model)
                        .i64("dims", dims)
                        .opt_string("base_url", &base_url)
                        .opt_string("content_fingerprint", &content_fingerprint)
                        .str("updated_at", normalize_ts_or_now(updated_at))
                        .build(),
                )?;
            }
            reports.insert(
                "embedding_manifest".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 13. source_manifest
        if let Some(cols) = schema_map.get("source_manifest") {
            let src_rows = count_duck_table(&conn, "source_manifest")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {} FROM \"source_manifest\"",
                sql_col(cols, "canonical_path"),
                sql_col(cols, "canonical_root"),
                sql_col(cols, "size_bytes"),
                sql_col(cols, "mtime_ns"),
                sql_col(cols, "content_hash"),
                sql_col(cols, "document_id"),
                sql_ts_col(cols, "last_seen"),
                sql_ts_col(cols, "last_synced"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let canonical_path: String = r.get(0)?;
                let canonical_root: String = r.get(1)?;
                let size_bytes: i64 = r.get(2)?;
                let mtime_ns: i64 = r.get(3)?;
                let content_hash: String = r.get(4)?;
                let document_id: String = r.get(5)?;
                let last_seen: Option<String> = r.get(6)?;
                let last_synced: Option<String> = r.get(7)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", &canonical_path)
                        .str("canonical_path", &canonical_path)
                        .str("canonical_root", canonical_root)
                        .i64("size_bytes", size_bytes)
                        .i64("mtime_ns", mtime_ns)
                        .str("content_hash", content_hash)
                        .str("document_id", document_id)
                        .str("last_seen", normalize_ts_or_now(last_seen))
                        .str("last_synced", normalize_ts_or_now(last_synced))
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("source_manifest", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("source_manifest", batch)?;
            }
            reports.insert(
                "source_manifest".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 14. reembed_progress
        if let Some(cols) = schema_map.get("reembed_progress") {
            let src_rows = count_duck_table(&conn, "reembed_progress")?;
            let sql = format!(
                "SELECT {}, {}, {}, {} FROM \"reembed_progress\"",
                sql_col(cols, "target_fingerprint"),
                sql_col(cols, "document_id"),
                sql_col(cols, "chunks_reembedded"),
                sql_ts_col(cols, "completed_at"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let target_fingerprint: String = r.get(0)?;
                let document_id: String = r.get(1)?;
                let chunks_reembedded: i64 = r.get(2)?;
                let completed_at: Option<String> = r.get(3)?;
                let id = composite_id_2(&target_fingerprint, &document_id);
                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .str("target_fingerprint", target_fingerprint)
                        .str("document_id", document_id)
                        .i64("chunks_reembedded", chunks_reembedded)
                        .str("completed_at", normalize_ts_or_now(completed_at))
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("reembed_progress", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("reembed_progress", batch)?;
            }
            reports.insert(
                "reembed_progress".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 15. kg_facts
        if let Some(cols) = schema_map.get("kg_facts") {
            let src_rows = count_duck_table(&conn, "kg_facts")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"kg_facts\"",
                sql_col(cols, "id"),
                sql_col(cols, "subject"),
                sql_col(cols, "predicate"),
                sql_col(cols, "object"),
                sql_ts_col(cols, "valid_from"),
                sql_ts_col(cols, "valid_to"),
                sql_col(cols, "status"),
                sql_col(cols, "superseded_by"),
                sql_col(cols, "source_document_id"),
                sql_col(cols, "source"),
                sql_col(cols, "confidence"),
                sql_col(cols, "metadata_json"),
                sql_ts_col(cols, "created_at"),
                sql_ts_col(cols, "updated_at"),
                sql_ts_col(cols, "invalidated_at"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                let subject: String = r.get(1)?;
                let predicate: String = r.get(2)?;
                let object: String = r.get(3)?;
                let valid_from: Option<String> = normalize_ts_opt(r.get(4)?);
                let valid_to: Option<String> = normalize_ts_opt(r.get(5)?);
                let status: Option<String> = r.get(6)?;
                let superseded_by: Option<String> = r.get(7)?;
                let source_document_id: Option<String> = r.get(8)?;
                let source: Option<String> = r.get(9)?;
                let confidence: Option<f64> = r.get(10)?;
                let metadata_json: Option<String> = r.get(11)?;
                let created_at: Option<String> = r.get(12)?;
                let updated_at: Option<String> = r.get(13)?;
                let invalidated_at: Option<String> = normalize_ts_opt(r.get(14)?);
                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .str("subject", subject)
                        .str("predicate", predicate)
                        .str("object", object)
                        .opt_string("valid_from", &valid_from)
                        .opt_string("valid_to", &valid_to)
                        .str("status", status.unwrap_or_else(|| "active".into()))
                        .opt_string("superseded_by", &superseded_by)
                        .opt_string("source_document_id", &source_document_id)
                        .opt_string("source", &source)
                        .opt_f64("confidence", confidence)
                        .str(
                            "metadata_json",
                            metadata_json.unwrap_or_else(|| "{}".into()),
                        )
                        .str("created_at", normalize_ts_or_now(created_at))
                        .str("updated_at", normalize_ts_or_now(updated_at))
                        .opt_string("invalidated_at", &invalidated_at)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("kg_facts", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("kg_facts", batch)?;
            }
            reports.insert(
                "kg_facts".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 16. schema_version (upserted by v{version}; Store::open_in_memory also ensures v25)
        if let Some(cols) = schema_map.get("schema_version") {
            let src_rows = count_duck_table(&conn, "schema_version")?;
            let sql = format!(
                "SELECT {}, {}, {} FROM \"schema_version\"",
                sql_col(cols, "version"),
                sql_ts_col(cols, "applied_at"),
                sql_col(cols, "note"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let version: i64 = r.get(0)?;
                let applied_at: Option<String> = r.get(1)?;
                let note: Option<String> = r.get(2)?;
                let id = format!("v{version}");
                w.upsert_by_id(
                    "schema_version",
                    &id,
                    RecBuilder::new()
                        .str("id", &id)
                        .i64("version", version)
                        .str("applied_at", normalize_ts_or_now(applied_at))
                        .opt_string("note", &note)
                        .build(),
                )?;
            }
            reports.insert(
                "schema_version".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: false,
                },
            );
        }

        // 17. meta (upserted by key)
        if let Some(cols) = schema_map.get("meta") {
            let src_rows = count_duck_table(&conn, "meta")?;
            let sql = format!(
                "SELECT {}, {} FROM \"meta\"",
                sql_col(cols, "key"),
                sql_col(cols, "value"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let key: String = r.get(0)?;
                let value: String = r.get(1)?;
                schema::upsert_meta(&mut *w, &key, &value)?;
            }
            reports.insert(
                "meta".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: false,
                },
            );
        }

        // 18. collections
        if let Some(cols) = schema_map.get("collections") {
            let src_rows = count_duck_table(&conn, "collections")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {} FROM \"collections\"",
                sql_col(cols, "id"),
                sql_col(cols, "name"),
                sql_col(cols, "description"),
                sql_col(cols, "metadata_json"),
                sql_ts_col(cols, "created_at"),
                sql_ts_col(cols, "updated_at"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                let name: String = r.get(1)?;
                let description: Option<String> = r.get(2)?;
                let metadata_json: Option<String> = r.get(3)?;
                let created_at: Option<String> = r.get(4)?;
                let updated_at: Option<String> = r.get(5)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .str("name", name)
                        .opt_string("description", &description)
                        .str(
                            "metadata_json",
                            metadata_json.unwrap_or_else(|| "{}".into()),
                        )
                        .str("created_at", normalize_ts_or_now(created_at))
                        .str("updated_at", normalize_ts_or_now(updated_at))
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("collections", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("collections", batch)?;
            }
            reports.insert(
                "collections".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 19. collection_entries
        if let Some(cols) = schema_map.get("collection_entries") {
            let src_rows = count_duck_table(&conn, "collection_entries")?;
            let sql = format!(
                "SELECT {}, {}, {}, {} FROM \"collection_entries\"",
                sql_col(cols, "collection_id"),
                sql_col(cols, "document_id"),
                sql_col(cols, "position"),
                sql_col(cols, "parent_document_id"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let collection_id: String = r.get(0)?;
                let document_id: String = r.get(1)?;
                let position: i64 = r.get(2)?;
                let parent_document_id: Option<String> = r.get(3)?;
                let id = composite_id_2(&collection_id, &document_id);
                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .str("collection_id", collection_id)
                        .str("document_id", document_id)
                        .i64("position", position)
                        .opt_string("parent_document_id", &parent_document_id)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("collection_entries", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("collection_entries", batch)?;
            }
            reports.insert(
                "collection_entries".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 20. collection_dependencies
        if let Some(cols) = schema_map.get("collection_dependencies") {
            let src_rows = count_duck_table(&conn, "collection_dependencies")?;
            let sql = format!(
                "SELECT {}, {}, {} FROM \"collection_dependencies\"",
                sql_col(cols, "collection_id"),
                sql_col(cols, "document_id"),
                sql_col(cols, "depends_on_document_id"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let collection_id: String = r.get(0)?;
                let document_id: String = r.get(1)?;
                let depends_on: String = r.get(2)?;
                let id = composite_id_3(&collection_id, &document_id, &depends_on);
                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .str("collection_id", collection_id)
                        .str("document_id", document_id)
                        .str("depends_on_document_id", depends_on)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("collection_dependencies", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("collection_dependencies", batch)?;
            }
            reports.insert(
                "collection_dependencies".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 21. Payload KV Tables (8 collections)
        for &tbl in PAYLOAD_KV_COLLECTIONS {
            if let Some(cols) = schema_map.get(tbl) {
                let src_rows = count_duck_table(&conn, tbl)?;
                let sql = format!(
                    "SELECT {}, {} FROM \"{tbl}\"",
                    sql_col(cols, "id"),
                    sql_col(cols, "payload"),
                );
                let mut stmt = conn.prepare(&sql)?;
                let mut rows = stmt.query([])?;
                let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
                while let Some(r) = rows.next()? {
                    let id: String = r.get(0)?;
                    let payload: String = r.get(1)?;
                    batch.push(
                        RecBuilder::new()
                            .str("id", id)
                            .str("payload", payload)
                            .build(),
                    );
                    if batch.len() >= ROW_BATCH_SIZE {
                        w.insert_records(tbl, std::mem::take(&mut batch))?;
                    }
                }
                if !batch.is_empty() {
                    w.insert_records(tbl, batch)?;
                }
                reports.insert(
                    tbl.into(),
                    TableReport {
                        src_rows,
                        dst_rows: 0,
                        exact_1_to_1: true,
                    },
                );
            }
        }

        // 22. code_index_documents
        if let Some(cols) = schema_map.get("code_index_documents") {
            let src_rows = count_duck_table(&conn, "code_index_documents")?;
            let sql = format!(
                "SELECT {}, {} FROM \"code_index_documents\"",
                sql_col(cols, "document_id"),
                sql_col(cols, "payload"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let document_id: String = r.get(0)?;
                let payload: String = r.get(1)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", &document_id)
                        .str("document_id", &document_id)
                        .str("payload", payload)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("code_index_documents", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("code_index_documents", batch)?;
            }
            reports.insert(
                "code_index_documents".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 23. code_index_source_blobs
        if let Some(cols) = schema_map.get("code_index_source_blobs") {
            let src_rows = count_duck_table(&conn, "code_index_source_blobs")?;
            let sql = format!(
                "SELECT {}, {} FROM \"code_index_source_blobs\"",
                sql_col(cols, "content_hash"),
                sql_col(cols, "content"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let content_hash: String = r.get(0)?;
                let content: String = r.get(1)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", &content_hash)
                        .str("content_hash", &content_hash)
                        .str("content", content)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("code_index_source_blobs", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("code_index_source_blobs", batch)?;
            }
            reports.insert(
                "code_index_source_blobs".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 24. code_index_revisions (DuckDB `id BIGINT` -> lin `id = rev.to_string(), rev_id = rev`)
        if let Some(cols) = schema_map.get("code_index_revisions") {
            let src_rows = count_duck_table(&conn, "code_index_revisions")?;
            let sql = format!(
                "SELECT {}, {}, {}, {} FROM \"code_index_revisions\"",
                sql_col(cols, "id"),
                sql_col(cols, "fingerprint"),
                sql_ts_col(cols, "created_at"),
                sql_col(cols, "graph_json"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let rev_id: i64 = r.get(0)?;
                let fingerprint: String = r.get(1)?;
                let created_at: Option<String> = r.get(2)?;
                let graph_json: String = r.get(3)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", rev_id.to_string())
                        .i64("rev_id", rev_id)
                        .str("fingerprint", fingerprint)
                        .str("created_at", normalize_ts_or_now(created_at))
                        .str("graph_json", graph_json)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("code_index_revisions", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("code_index_revisions", batch)?;
            }
            reports.insert(
                "code_index_revisions".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 25. code_index_revision_files
        if let Some(cols) = schema_map.get("code_index_revision_files") {
            let src_rows = count_duck_table(&conn, "code_index_revision_files")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {} FROM \"code_index_revision_files\"",
                sql_col(cols, "index_revision"),
                sql_col(cols, "document_id"),
                sql_col(cols, "path"),
                sql_col(cols, "project"),
                sql_col(cols, "document_revision"),
                sql_col(cols, "content_hash"),
                sql_col(cols, "analysis_json"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let index_revision: i64 = r.get(0)?;
                let document_id: String = r.get(1)?;
                let path: String = r.get(2)?;
                let project: Option<String> = r.get(3)?;
                let document_revision: i64 = r.get(4)?;
                let content_hash: String = r.get(5)?;
                let analysis_json: Option<String> = r.get(6)?;
                let id = composite_id_2(&index_revision.to_string(), &document_id);
                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .i64("index_revision", index_revision)
                        .str("document_id", document_id)
                        .str("path", path)
                        .opt_string("project", &project)
                        .i64("document_revision", document_revision)
                        .str("content_hash", content_hash)
                        .opt_string("analysis_json", &analysis_json)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("code_index_revision_files", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("code_index_revision_files", batch)?;
            }
            reports.insert(
                "code_index_revision_files".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 26. code_index_runs
        if let Some(cols) = schema_map.get("code_index_runs") {
            let src_rows = count_duck_table(&conn, "code_index_runs")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {} FROM \"code_index_runs\"",
                sql_col(cols, "seq"),
                sql_col(cols, "run_id"),
                sql_col(cols, "index_revision"),
                sql_col(cols, "revision_created"),
                sql_ts_col(cols, "recorded_at"),
                sql_col(cols, "metrics_json"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let seq: i64 = r.get(0)?;
                let run_id: String = r.get(1)?;
                let index_revision: i64 = r.get(2)?;
                let revision_created: bool = r.get(3)?;
                let recorded_at: Option<String> = r.get(4)?;
                let metrics_json: String = r.get(5)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", seq.to_string())
                        .i64("seq", seq)
                        .str("run_id", run_id)
                        .i64("index_revision", index_revision)
                        .bool("revision_created", revision_created)
                        .str("recorded_at", normalize_ts_or_now(recorded_at))
                        .str("metrics_json", metrics_json)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("code_index_runs", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("code_index_runs", batch)?;
            }
            reports.insert(
                "code_index_runs".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 27. code_index_drift
        if let Some(cols) = schema_map.get("code_index_drift") {
            let src_rows = count_duck_table(&conn, "code_index_drift")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"code_index_drift\"",
                sql_col(cols, "event_id"),
                sql_col(cols, "seq"),
                sql_col(cols, "index_revision"),
                sql_col(cols, "document_id"),
                sql_col(cols, "path"),
                sql_col(cols, "project"),
                sql_col(cols, "kind"),
                sql_col(cols, "source_kind"),
                sql_col(cols, "indexed_hash"),
                sql_col(cols, "observed_hash"),
                sql_ts_col(cols, "first_seen"),
                sql_ts_col(cols, "last_seen"),
                sql_ts_col(cols, "acknowledged_at"),
                sql_col(cols, "acknowledgement"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let event_id: String = r.get(0)?;
                let seq: i64 = r.get(1)?;
                let index_revision: i64 = r.get(2)?;
                let document_id: String = r.get(3)?;
                let path: String = r.get(4)?;
                let project: Option<String> = r.get(5)?;
                let kind: String = r.get(6)?;
                let source_kind: String = r.get(7)?;
                let indexed_hash: Option<String> = r.get(8)?;
                let observed_hash: Option<String> = r.get(9)?;
                let first_seen: Option<String> = r.get(10)?;
                let last_seen: Option<String> = r.get(11)?;
                let acknowledged_at: Option<String> = normalize_ts_opt(r.get(12)?);
                let acknowledgement: Option<String> = r.get(13)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", &event_id)
                        .str("event_id", &event_id)
                        .i64("seq", seq)
                        .i64("index_revision", index_revision)
                        .str("document_id", document_id)
                        .str("path", path)
                        .opt_string("project", &project)
                        .str("kind", kind)
                        .str("source_kind", source_kind)
                        .opt_string("indexed_hash", &indexed_hash)
                        .opt_string("observed_hash", &observed_hash)
                        .str("first_seen", normalize_ts_or_now(first_seen))
                        .str("last_seen", normalize_ts_or_now(last_seen))
                        .opt_string("acknowledged_at", &acknowledged_at)
                        .opt_string("acknowledgement", &acknowledgement)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("code_index_drift", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("code_index_drift", batch)?;
            }
            reports.insert(
                "code_index_drift".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 28. analysis_tasks
        if let Some(cols) = schema_map.get("analysis_tasks") {
            let src_rows = count_duck_table(&conn, "analysis_tasks")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {} FROM \"analysis_tasks\"",
                sql_col(cols, "session_id"),
                sql_col(cols, "name"),
                sql_col(cols, "generation"),
                sql_col(cols, "updated_ms"),
                sql_col(cols, "expires_ms"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let session_id: String = r.get(0)?;
                let name: String = r.get(1)?;
                let generation: i64 = r.get(2)?;
                let updated_ms: i64 = r.get(3)?;
                let expires_ms: i64 = r.get(4)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", &session_id)
                        .str("session_id", &session_id)
                        .str("name", name)
                        .i64("generation", generation)
                        .i64("updated_ms", updated_ms)
                        .i64("expires_ms", expires_ms)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("analysis_tasks", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("analysis_tasks", batch)?;
            }
            reports.insert(
                "analysis_tasks".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 29. analysis_notes
        if let Some(cols) = schema_map.get("analysis_notes") {
            let src_rows = count_duck_table(&conn, "analysis_notes")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"analysis_notes\"",
                sql_col(cols, "seq"),
                sql_col(cols, "id"),
                sql_col(cols, "session_id"),
                sql_col(cols, "task"),
                sql_col(cols, "agent"),
                sql_col(cols, "path"),
                sql_col(cols, "tags_json"),
                sql_col(cols, "body"),
                sql_col(cols, "index_rev"),
                sql_col(cols, "created_ms"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let seq: i64 = r.get(0)?;
                let note_id: String = r.get(1)?;
                let session_id: String = r.get(2)?;
                let task: String = r.get(3)?;
                let agent: String = r.get(4)?;
                let path: String = r.get(5)?;
                let tags_json: String = r.get(6)?;
                let body: String = r.get(7)?;
                let index_rev: Option<i64> = r.get(8)?;
                let created_ms: i64 = r.get(9)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", seq.to_string())
                        .i64("seq", seq)
                        .str("note_id", note_id)
                        .str("session_id", session_id)
                        .str("task", task)
                        .str("agent", agent)
                        .str("path", path)
                        .str("tags_json", tags_json)
                        .str("body", body)
                        .opt_i64("index_rev", index_rev)
                        .i64("created_ms", created_ms)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("analysis_notes", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("analysis_notes", batch)?;
            }
            reports.insert(
                "analysis_notes".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 30. analysis_calls
        if let Some(cols) = schema_map.get("analysis_calls") {
            let src_rows = count_duck_table(&conn, "analysis_calls")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"analysis_calls\"",
                sql_col(cols, "seq"),
                sql_col(cols, "session_id"),
                sql_col(cols, "task"),
                sql_col(cols, "tool"),
                sql_col(cols, "started_ms"),
                sql_col(cols, "finished_ms"),
                sql_col(cols, "ok"),
                sql_col(cols, "evidence_count"),
                sql_col(cols, "index_rev"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let seq: i64 = r.get(0)?;
                let session_id: String = r.get(1)?;
                let task: String = r.get(2)?;
                let tool: String = r.get(3)?;
                let started_ms: i64 = r.get(4)?;
                let finished_ms: Option<i64> = r.get(5)?;
                let ok: Option<bool> = r.get(6)?;
                let evidence_count: Option<i64> = r.get(7)?;
                let index_rev: Option<i64> = r.get(8)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", seq.to_string())
                        .i64("seq", seq)
                        .str("session_id", session_id)
                        .str("task", task)
                        .str("tool", tool)
                        .i64("started_ms", started_ms)
                        .opt_i64("finished_ms", finished_ms)
                        .opt_bool("ok", ok)
                        .opt_i64("evidence_count", evidence_count)
                        .opt_i64("index_rev", index_rev)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("analysis_calls", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("analysis_calls", batch)?;
            }
            reports.insert(
                "analysis_calls".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 31. analysis_gate_decisions
        if let Some(cols) = schema_map.get("analysis_gate_decisions") {
            let src_rows = count_duck_table(&conn, "analysis_gate_decisions")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {} FROM \"analysis_gate_decisions\"",
                sql_col(cols, "seq"),
                sql_col(cols, "id"),
                sql_col(cols, "session_id"),
                sql_col(cols, "task"),
                sql_col(cols, "index_rev"),
                sql_col(cols, "created_ms"),
                sql_col(cols, "mode"),
                sql_col(cols, "kind"),
                sql_col(cols, "verdict"),
                sql_col(cols, "rule"),
                sql_col(cols, "facts_json"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let seq: i64 = r.get(0)?;
                let decision_id: String = r.get(1)?;
                let session_id: String = r.get(2)?;
                let task: String = r.get(3)?;
                let index_rev: Option<i64> = r.get(4)?;
                let created_ms: i64 = r.get(5)?;
                let mode: String = r.get(6)?;
                let kind: String = r.get(7)?;
                let verdict: String = r.get(8)?;
                let rule: String = r.get(9)?;
                let facts_json: String = r.get(10)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", seq.to_string())
                        .i64("seq", seq)
                        .str("decision_id", decision_id)
                        .str("session_id", session_id)
                        .str("task", task)
                        .opt_i64("index_rev", index_rev)
                        .i64("created_ms", created_ms)
                        .str("mode", mode)
                        .str("kind", kind)
                        .str("verdict", verdict)
                        .str("rule", rule)
                        .str("facts_json", facts_json)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("analysis_gate_decisions", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("analysis_gate_decisions", batch)?;
            }
            reports.insert(
                "analysis_gate_decisions".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 32. analysis_calibrations
        if let Some(cols) = schema_map.get("analysis_calibrations") {
            let src_rows = count_duck_table(&conn, "analysis_calibrations")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {}, {}, {} FROM \"analysis_calibrations\"",
                sql_col(cols, "seq"),
                sql_col(cols, "id"),
                sql_col(cols, "created_ms"),
                sql_col(cols, "kind"),
                sql_col(cols, "sample_size"),
                sql_col(cols, "index_rev"),
                sql_col(cols, "report_json"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            while let Some(r) = rows.next()? {
                let seq: i64 = r.get(0)?;
                let calibration_id: String = r.get(1)?;
                let created_ms: i64 = r.get(2)?;
                let kind: String = r.get(3)?;
                let sample_size: i64 = r.get(4)?;
                let index_rev: Option<i64> = r.get(5)?;
                let report_json: String = r.get(6)?;
                batch.push(
                    RecBuilder::new()
                        .str("id", seq.to_string())
                        .i64("seq", seq)
                        .str("calibration_id", calibration_id)
                        .i64("created_ms", created_ms)
                        .str("kind", kind)
                        .i64("sample_size", sample_size)
                        .opt_i64("index_rev", index_rev)
                        .str("report_json", report_json)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("analysis_calibrations", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("analysis_calibrations", batch)?;
            }
            reports.insert(
                "analysis_calibrations".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        // 33. external_db_snapshots
        if let Some(cols) = schema_map.get("external_db_snapshots") {
            let src_rows = count_duck_table(&conn, "external_db_snapshots")?;
            let sql = format!(
                "SELECT {}, {}, {}, {}, {} FROM \"external_db_snapshots\"",
                sql_col(cols, "id"),
                sql_col(cols, "connection_alias"),
                sql_col(cols, "captured_ms"),
                sql_col(cols, "payload"),
                sql_col(cols, "seq"),
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
            let mut fallback_seq = 1_i64;
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                let connection_alias: String = r.get(1)?;
                let captured_ms: i64 = r.get(2)?;
                let payload: String = r.get(3)?;
                let seq: Option<i64> = r.get(4)?;
                let seq_val = seq.unwrap_or_else(|| {
                    let s = fallback_seq;
                    fallback_seq += 1;
                    s
                });
                batch.push(
                    RecBuilder::new()
                        .str("id", id)
                        .str("connection_alias", connection_alias)
                        .i64("captured_ms", captured_ms)
                        .str("payload", payload)
                        .i64("seq", seq_val)
                        .build(),
                );
                if batch.len() >= ROW_BATCH_SIZE {
                    w.insert_records("external_db_snapshots", std::mem::take(&mut batch))?;
                }
            }
            if !batch.is_empty() {
                w.insert_records("external_db_snapshots", batch)?;
            }
            reports.insert(
                "external_db_snapshots".into(),
                TableReport {
                    src_rows,
                    dst_rows: 0,
                    exact_1_to_1: true,
                },
            );
        }

        println!("==> Running schema v25 backfills & FTS readiness metadata...");
        schema::migrate(&mut *w).map_err(|e| anyhow!("schema::migrate failed: {e}"))?;
        fts::ensure_fts(&mut *w).map_err(|e| anyhow!("fts::ensure_fts failed: {e}"))?;

        println!(
            "==> Materializing Zstd-compressed durable lin directory: {}",
            args.dst.display()
        );
        let t_snap = Instant::now();
        w.materialize_durable_dir(&args.dst)
            .map_err(|e| anyhow!("materialize_durable_dir failed: {e}"))?;
        println!(
            "    Snapshot written in {:.2}s",
            t_snap.elapsed().as_secs_f64()
        );
    }

    drop(mem_store);
    drop(conn);

    // Reopen durable Store (read-write + read-only) to verify persistence, catalog hash,
    // 1:1 row counts, binary vector integrity, and lexical/vector search readiness.
    println!("==> Verifying durable reopen & 1:1 row counts...");
    let verified = Store::open(&args.dst)
        .map_err(|e| anyhow!("failed to reopen migrated Store read-write: {e}"))?;
    {
        let r = verified
            .read()
            .map_err(|e| anyhow!("verified.read() failed: {e}"))?;
        for (tbl, rep) in &mut reports {
            rep.dst_rows = r
                .count_all(tbl)
                .map_err(|e| anyhow!("count_all({tbl}) failed: {e}"))?;
            if rep.exact_1_to_1 && rep.src_rows != rep.dst_rows {
                bail!(
                    "row count mismatch on collection '{tbl}': duckdb={} vs lin={}",
                    rep.src_rows,
                    rep.dst_rows
                );
            }
        }

        // Sample first non-empty vector from `chunks` and verify native Cell::Vec round-trip.
        let mut verified_sample_vec: Option<usize> = None;
        r.scan_each("chunks", |row| {
            let v = cell_vec_f32(row, "embedding");
            if !v.is_empty() {
                verified_sample_vec = Some(v.len());
                return Ok(false);
            }
            Ok(true)
        })
        .map_err(|e| anyhow!("chunks vector scan failed: {e}"))?;
        if migrated_vectors > 0 {
            let got_dims = verified_sample_vec.ok_or_else(|| {
                anyhow!("expected {migrated_vectors} vectors in chunks, found 0 after reopen")
            })?;
            if let Some(exp) = detected_dims
                && got_dims != exp
            {
                bail!("vector dimension mismatch after reopen: expected {exp}, got {got_dims}");
            }
        }
    }

    // Smoke-test BM25 lexical search + vector cosine search if chunks exist.
    let (doc_count, chunk_count) = {
        let r = verified
            .read()
            .map_err(|e| anyhow!("verified.read() failed: {e}"))?;
        (
            r.count_all("documents").unwrap_or(0),
            r.count_all("chunks").unwrap_or(0),
        )
    };
    if chunk_count > 0 {
        let bm25_hits = fts::search_bm25(
            &verified,
            "rust the function data",
            5,
            &LexFilters::default(),
        )
        .map_err(|e| anyhow!("BM25 smoke query failed: {e}"))?;
        let vec_hits = if let Some(dims) = detected_dims {
            let mut probe = vec![0.0_f32; dims];
            probe[0] = 1.0;
            search::search_chunks(&verified, &probe, 5, None)
                .map_err(|e| anyhow!("vector search smoke query failed: {e}"))?
                .len()
        } else {
            0
        };
        println!(
            "    Smoke queries OK: docs={doc_count}, chunks={chunk_count}, bm25_hits={}, vec_hits={vec_hits}",
            bm25_hits.len()
        );
    }
    drop(verified);

    // Also verify concurrent read-only reopen (`Store::open_read_only`).
    let ro = Store::open_read_only(&args.dst)
        .map_err(|e| anyhow!("Store::open_read_only verification failed: {e}"))?;
    let ro_docs = ro
        .read()
        .map_err(|e| anyhow!("ro.read() failed: {e}"))?
        .count_all("documents")
        .map_err(|e| anyhow!("ro.count_all(documents) failed: {e}"))?;
    if ro_docs != doc_count {
        bail!("open_read_only doc count mismatch: {ro_docs} != {doc_count}");
    }
    drop(ro);

    let dst_bytes = dir_or_file_bytes(&args.dst);
    println!(
        "\n=== Migration Summary: {} -> {} ===",
        args.src.display(),
        args.dst.display()
    );
    for (tbl, rep) in &reports {
        if rep.src_rows > 0 || rep.dst_rows > 0 {
            println!(
                "  {:28} {:>9} -> {:>9} rows",
                tbl, rep.src_rows, rep.dst_rows
            );
        }
    }
    let ratio = if src_bytes > 0 {
        (dst_bytes as f64 / src_bytes as f64) * 100.0
    } else {
        100.0
    };
    println!(
        "  Disk footprint: {} -> {} ({ratio:.1}% of DuckDB size)",
        format_mib(src_bytes),
        format_mib(dst_bytes)
    );
    println!("  Total elapsed:  {:.2}s\n", t0.elapsed().as_secs_f64());
    Ok(())
}
