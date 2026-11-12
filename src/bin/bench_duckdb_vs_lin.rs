//! Comprehensive 3-Way Deep-Dive Benchmark Suite:
//! - SQLite 3 (`rusqlite` bundled, WAL + `synchronous=NORMAL`, B-Tree indexes + `FTS5` `porter unicode61` + binary `BLOB` `f32` vectors)
//! - DuckDB (`v1.10505`, columnar MVCC + JSON embeddings + DuckDB `fts` `match_bm25`)
//! - `lin` Storage Engine v2 (`DiskEngineV2` `mmap` segments + `TextDict` `MmapDictCol` + `IVF-SQ8/f32` SIMD + `MmapBm25Index` v3 + 16-shard L1 `HotTierCache` + `:memory:` mode)
//!
//! Evaluated head-to-head on the real 82,934-document / 779,432-chunk / 108,881-node / 240,619-edge production corpus.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use duckdb::{AccessMode, Config as DuckConfig, Connection as DuckConnection, params as dparams};
use rag_mcp::db::DocumentCatalogFilter;
use rag_mcp::db::Store;
use rag_mcp::db::fts::{self, LexFilters};
use rag_mcp::db::lin_engine::{LinReader, cell_vec_f32};
use rag_mcp::db::search::{self, SearchQuery};
use rag_mcp::models::{Document, GraphFilter, SearchMode};
use rusqlite::{Connection as SqliteConnection, OpenFlags as SqliteFlags, params as sparams};

const DUCKDB_SNAPSHOT_PATH: &str =
    "/Users/themoretheless/.local/share/rag-mcp/backups/rag-auto-20261111T163409.835Z.duckdb";
const DUCKDB_LIVE_PATH: &str = "/Users/themoretheless/.local/share/rag-mcp/rag.duckdb";
const LIN_LIVE_PATH: &str = "/Users/themoretheless/.local/share/rag-mcp/rag.lin";
const BACKUPS_DIR: &str = "/Users/themoretheless/.local/share/rag-mcp/backups";
const SQLITE_BENCH_PATH: &str = "/tmp/rag_bench_sqlite3_corpus.db";

/// RAII cleanup guard ensuring temporary SQLite corpus mirror + WAL/SHM sidecars are removed on exit.
struct SqliteCleanupGuard {
    path: PathBuf,
    keep: bool,
}

impl Drop for SqliteCleanupGuard {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let _ = std::fs::remove_file(&self.path);
        let wal = PathBuf::from(format!("{}-wal", self.path.display()));
        let shm = PathBuf::from(format!("{}-shm", self.path.display()));
        let _ = std::fs::remove_file(wal);
        let _ = std::fs::remove_file(shm);
    }
}

fn latest_lin_backup_path() -> Option<PathBuf> {
    let entries = std::fs::read_dir(BACKUPS_DIR).ok()?;
    let mut candidates: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "lin") && p.is_file())
        .collect();
    candidates.sort();
    candidates.pop()
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

fn sqlite_total_bytes(path: &Path) -> u64 {
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    let shm = PathBuf::from(format!("{}-shm", path.display()));
    dir_or_file_bytes(path) + dir_or_file_bytes(&wal) + dir_or_file_bytes(&shm)
}

#[derive(Default)]
struct LinDirBreakdown {
    total_bytes: u64,
    vf32_bytes: u64,
    vsq8_bytes: u64,
    vivf_bytes: u64,
    text_col_bytes: u64,
    dict_col_bytes: u64,
    dict_col_count: usize,
    scalar_col_bytes: u64,
    pk_idx_bytes: u64,
    sec_idx_bytes: u64,
    bm25_mmap_bytes: u64,
    manifest_wal_tomb_bytes: u64,
    segment_count: usize,
    vivf_count: usize,
}

fn inspect_lin_dir(root: &Path) -> LinDirBreakdown {
    let mut b = LinDirBreakdown {
        total_bytes: dir_or_file_bytes(root),
        bm25_mmap_bytes: dir_or_file_bytes(&root.join("bm25_porter.v3.mmap")),
        manifest_wal_tomb_bytes: dir_or_file_bytes(&root.join("manifest.v2.json"))
            + dir_or_file_bytes(&root.join("wal.v2.log"))
            + dir_or_file_bytes(&root.join("tombstones.v2.bin")),
        ..LinDirBreakdown::default()
    };
    let segs_dir = root.join("segments");
    if let Ok(segs) = std::fs::read_dir(&segs_dir) {
        for s in segs.flatten() {
            let sp = s.path();
            if !sp.is_dir() {
                continue;
            }
            b.segment_count += 1;
            if let Ok(files) = std::fs::read_dir(&sp) {
                for f in files.flatten() {
                    let fp = f.path();
                    let sz = dir_or_file_bytes(&fp);
                    let name = fp.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.ends_with(".vf32") || name.ends_with(".vnorm") {
                        b.vf32_bytes += sz;
                    } else if name.ends_with(".vsq8") {
                        b.vsq8_bytes += sz;
                    } else if name.ends_with(".vivf") {
                        b.vivf_bytes += sz;
                        b.vivf_count += 1;
                    } else if name.ends_with(".dstr")
                        || name.ends_with(".doff")
                        || name.ends_with(".dord")
                    {
                        b.dict_col_bytes += sz;
                        if name.ends_with(".dord") {
                            b.dict_col_count += 1;
                        }
                    } else if name.ends_with(".bdat")
                        || name.ends_with(".boff")
                        || name.ends_with(".loc")
                    {
                        b.text_col_bytes += sz;
                    } else if name.ends_with(".val") || name.ends_with(".null") {
                        b.scalar_col_bytes += sz;
                    } else if name == "pk_id.ht" {
                        b.pk_idx_bytes += sz;
                    } else if name.starts_with("sec_") && name.ends_with(".ht") {
                        b.sec_idx_bytes += sz;
                    }
                }
            }
        }
    }
    b
}

fn format_gib(bytes: u64) -> String {
    format!("{:.2} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn format_mib(bytes: u64) -> String {
    format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
}

fn format_mib_signed(delta_bytes: i64) -> String {
    format!("{:+.2} MiB", delta_bytes as f64 / (1024.0 * 1024.0))
}

fn current_rss_bytes() -> u64 {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let pid = Pid::from_u32(std::process::id());
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_memory(),
    );
    sys.process(pid).map(|p| p.memory()).unwrap_or(0)
}

fn clear_lin_hot_cache(store: &Store) {
    if let Ok(r) = store.read()
        && let Some(eng) = r.disk_v2()
    {
        eng.clear_hot_cache();
    }
}

fn f32_slice_to_le_bytes(v: &[f32], buf: &mut Vec<u8>) {
    buf.clear();
    buf.reserve(v.len() * 4);
    for &x in v {
        buf.extend_from_slice(&x.to_le_bytes());
    }
}

fn le_bytes_to_f32_vec(bytes: &[u8]) -> Vec<f32> {
    let mut out = Vec::with_capacity(bytes.len() / 4);
    for &chunk in bytes.as_chunks::<4>().0 {
        out.push(f32::from_le_bytes(chunk));
    }
    out
}

fn sanitize_fts5_match_query(q: &str) -> String {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t.to_ascii_lowercase()))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Build an exact 1:1 SQLite 3 mirror of the live `rag.lin` 82,934-doc / 779,432-chunk corpus
/// (with compact binary `BLOB` `f32` embeddings, B-Tree secondary indexes, `FTS5` `porter unicode61`, and `ANALYZE`).
fn build_sqlite_corpus_mirror(lin_store: &Store, sqlite_path: &Path) -> Result<f64> {
    let _ = std::fs::remove_file(sqlite_path);
    let _ = std::fs::remove_file(format!("{}-wal", sqlite_path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", sqlite_path.display()));

    let t0 = Instant::now();
    let mut conn = SqliteConnection::open(sqlite_path)?;
    conn.execute_batch(
        "PRAGMA page_size = 4096;
         PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         PRAGMA temp_store = MEMORY;
         PRAGMA cache_size = -524288;
         CREATE TABLE documents (
             id TEXT PRIMARY KEY,
             uri TEXT NOT NULL,
             title TEXT NOT NULL,
             content TEXT NOT NULL,
             metadata_json TEXT NOT NULL,
             content_hash TEXT,
             wing TEXT,
             room TEXT,
             source_file TEXT,
             layer TEXT NOT NULL,
             kind TEXT NOT NULL,
             status TEXT NOT NULL,
             pinned INTEGER NOT NULL,
             boost REAL NOT NULL,
             revision INTEGER NOT NULL,
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL
         );
         CREATE TABLE chunks (
             id TEXT PRIMARY KEY,
             document_id TEXT NOT NULL,
             chunk_index INTEGER NOT NULL,
             content TEXT NOT NULL,
             embedding BLOB NOT NULL,
             char_start INTEGER NOT NULL,
             char_end INTEGER NOT NULL,
             metadata_json TEXT NOT NULL
         );
         CREATE VIRTUAL TABLE chunks_fts USING fts5(
             chunk_id UNINDEXED,
             document_id UNINDEXED,
             content,
             tokenize = 'porter unicode61'
         );
         CREATE TABLE graph_nodes (
             id TEXT PRIMARY KEY,
             kind TEXT NOT NULL,
             label TEXT NOT NULL,
             document_id TEXT,
             uri TEXT,
             resolved INTEGER NOT NULL,
             metadata_json TEXT NOT NULL
         );
         CREATE TABLE graph_edges (
             id TEXT PRIMARY KEY,
             source_id TEXT NOT NULL,
             target_id TEXT NOT NULL,
             rel_type TEXT NOT NULL,
             weight REAL NOT NULL,
             origin TEXT,
             context TEXT
         );",
    )?;

    let r = lin_store.read()?;
    {
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO documents VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            )?;
            r.scan_each("documents", |row| {
                use rag_mcp::db::lin_engine::{cell_bool, cell_f64, cell_i64, cell_str};
                stmt.execute(sparams![
                    cell_str(row, "id").unwrap_or(""),
                    cell_str(row, "uri").unwrap_or(""),
                    cell_str(row, "title").unwrap_or(""),
                    cell_str(row, "content").unwrap_or(""),
                    cell_str(row, "metadata_json").unwrap_or("{}"),
                    cell_str(row, "content_hash"),
                    cell_str(row, "wing"),
                    cell_str(row, "room"),
                    cell_str(row, "source_file"),
                    cell_str(row, "layer").unwrap_or("raw"),
                    cell_str(row, "kind").unwrap_or("document"),
                    cell_str(row, "status").unwrap_or("active"),
                    i64::from(cell_bool(row, "pinned", false)),
                    cell_f64(row, "boost", 1.0),
                    cell_i64(row, "revision", 1),
                    cell_str(row, "created_at").unwrap_or(""),
                    cell_str(row, "updated_at").unwrap_or(""),
                ])
                .map_err(|e| rag_mcp::error::AppError::db(e.to_string()))?;
                Ok(true)
            })?;
        }
        {
            let mut cstmt =
                tx.prepare("INSERT INTO chunks VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")?;
            let mut fstmt = tx.prepare(
                "INSERT INTO chunks_fts (chunk_id, document_id, content) VALUES (?1, ?2, ?3)",
            )?;
            let mut vbuf = Vec::with_capacity(768 * 4);
            r.scan_each("chunks", |row| {
                use rag_mcp::db::lin_engine::{cell_i64, cell_str};
                let cid = cell_str(row, "id").unwrap_or("");
                let did = cell_str(row, "document_id").unwrap_or("");
                let body = cell_str(row, "content").unwrap_or("");
                let vec = cell_vec_f32(row, "embedding");
                f32_slice_to_le_bytes(&vec, &mut vbuf);
                cstmt
                    .execute(sparams![
                        cid,
                        did,
                        cell_i64(row, "chunk_index", 0),
                        body,
                        &vbuf,
                        cell_i64(row, "char_start", 0),
                        cell_i64(row, "char_end", 0),
                        cell_str(row, "metadata_json").unwrap_or("{}"),
                    ])
                    .map_err(|e| rag_mcp::error::AppError::db(e.to_string()))?;
                fstmt
                    .execute(sparams![cid, did, body])
                    .map_err(|e| rag_mcp::error::AppError::db(e.to_string()))?;
                Ok(true)
            })?;
        }
        {
            let mut nstmt =
                tx.prepare("INSERT INTO graph_nodes VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
            r.scan_each("graph_nodes", |row| {
                use rag_mcp::db::lin_engine::{cell_bool, cell_str};
                nstmt
                    .execute(sparams![
                        cell_str(row, "id").unwrap_or(""),
                        cell_str(row, "kind").unwrap_or("stub"),
                        cell_str(row, "label").unwrap_or(""),
                        cell_str(row, "document_id"),
                        cell_str(row, "uri"),
                        i64::from(cell_bool(row, "resolved", false)),
                        cell_str(row, "metadata_json").unwrap_or("{}"),
                    ])
                    .map_err(|e| rag_mcp::error::AppError::db(e.to_string()))?;
                Ok(true)
            })?;
        }
        {
            let mut estmt =
                tx.prepare("INSERT INTO graph_edges VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
            r.scan_each("graph_edges", |row| {
                use rag_mcp::db::lin_engine::{cell_f64, cell_str};
                estmt
                    .execute(sparams![
                        cell_str(row, "id").unwrap_or(""),
                        cell_str(row, "source_id").unwrap_or(""),
                        cell_str(row, "target_id").unwrap_or(""),
                        cell_str(row, "rel_type").unwrap_or(""),
                        cell_f64(row, "weight", 1.0),
                        cell_str(row, "origin"),
                        cell_str(row, "context"),
                    ])
                    .map_err(|e| rag_mcp::error::AppError::db(e.to_string()))?;
                Ok(true)
            })?;
        }
        tx.commit()?;
    }

    conn.execute_batch(
        "CREATE INDEX idx_docs_uri ON documents(uri);
         CREATE INDEX idx_docs_wing_updated ON documents(wing, pinned DESC, updated_at DESC, id ASC);
         CREATE INDEX idx_docs_wing_room ON documents(wing, room);
         CREATE INDEX idx_chunks_doc_idx ON chunks(document_id, chunk_index);
         CREATE INDEX idx_nodes_doc ON graph_nodes(document_id);
         CREATE INDEX idx_edges_src ON graph_edges(source_id);
         CREATE INDEX idx_edges_tgt ON graph_edges(target_id);
         CREATE INDEX idx_edges_rel ON graph_edges(rel_type);
         INSERT INTO chunks_fts(chunks_fts) VALUES('optimize');
         ANALYZE;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA wal_checkpoint(TRUNCATE);",
    )?;
    Ok(t0.elapsed().as_secs_f64())
}

fn open_sqlite_bench_ro(path: &Path) -> Result<SqliteConnection> {
    let conn = SqliteConnection::open_with_flags(
        path,
        SqliteFlags::SQLITE_OPEN_READ_ONLY | SqliteFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.execute_batch(
        "PRAGMA temp_store = MEMORY;
         PRAGMA cache_size = -262144;
         PRAGMA mmap_size = 4294967296;",
    )?;
    Ok(conn)
}

#[derive(Clone, Copy)]
struct LatStats {
    min_us: f64,
    mean_us: f64,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    max_us: f64,
    ops_per_sec: f64,
}

fn compute_stats(samples: &mut [Duration], total: Duration) -> LatStats {
    samples.sort_unstable();
    let n = samples.len().max(1);
    let sum_us: f64 = samples.iter().map(|d| d.as_secs_f64() * 1e6).sum();
    let min_us = samples[0].as_secs_f64() * 1e6;
    let mean_us = sum_us / n as f64;
    let p50_us = samples[n / 2].as_secs_f64() * 1e6;
    let p95_idx = ((n as f64 * 0.95) as usize).min(n - 1);
    let p99_idx = ((n as f64 * 0.99) as usize).min(n - 1);
    let p95_us = samples[p95_idx].as_secs_f64() * 1e6;
    let p99_us = samples[p99_idx].as_secs_f64() * 1e6;
    let max_us = samples[n - 1].as_secs_f64() * 1e6;
    let ops_per_sec = n as f64 / total.as_secs_f64().max(1e-9);
    LatStats {
        min_us,
        mean_us,
        p50_us,
        p95_us,
        p99_us,
        max_us,
        ops_per_sec,
    }
}

fn print_us_row(label: &str, st: LatStats, vs_sqlite_us: Option<f64>, vs_duck_us: Option<f64>) {
    let mut suffix = String::new();
    if let Some(sq) = vs_sqlite_us {
        suffix.push_str(&format!("  [{:.1}x vs SQLite]", sq / st.mean_us.max(0.01)));
    }
    if let Some(dk) = vs_duck_us {
        suffix.push_str(&format!("  [{:.1}x vs DuckDB]", dk / st.mean_us.max(0.01)));
    }
    println!(
        "    {label:<30} min {:>7.1} | p50 {:>7.1} | mean {:>7.1} | p95 {:>7.1} | p99 {:>7.1} | max {:>8.1} µs | {:>9.0} ops/s{suffix}",
        st.min_us, st.p50_us, st.mean_us, st.p95_us, st.p99_us, st.max_us, st.ops_per_sec,
    );
}

fn print_ms_row(label: &str, st: LatStats, vs_sqlite_us: Option<f64>, vs_duck_us: Option<f64>) {
    let mut suffix = String::new();
    if let Some(sq) = vs_sqlite_us {
        suffix.push_str(&format!("  [{:.1}x vs SQLite]", sq / st.mean_us.max(0.01)));
    }
    if let Some(dk) = vs_duck_us {
        suffix.push_str(&format!("  [{:.1}x vs DuckDB]", dk / st.mean_us.max(0.01)));
    }
    println!(
        "    {label:<30} min {:>6.2} | p50 {:>6.2} | mean {:>6.2} | p95 {:>6.2} | p99 {:>6.2} | max {:>6.2} ms | {:>8.1} q/s{suffix}",
        st.min_us / 1000.0,
        st.p50_us / 1000.0,
        st.mean_us / 1000.0,
        st.p95_us / 1000.0,
        st.p99_us / 1000.0,
        st.max_us / 1000.0,
        st.ops_per_sec,
    );
}

#[allow(clippy::too_many_lines)]
fn main() -> Result<()> {
    let keep_sqlite = std::env::args().any(|a| a == "--keep-sqlite");
    let sqlite_path = PathBuf::from(SQLITE_BENCH_PATH);
    let _sqlite_guard = SqliteCleanupGuard {
        path: sqlite_path.clone(),
        keep: keep_sqlite,
    };

    println!(
        "=========================================================================================================="
    );
    println!(
        "  RAG-MCP 3-WAY BENCHMARK SUITE: SQLite 3 (WAL+FTS5+BLOB) vs DuckDB (v1.10505) vs lin Storage Engine v2"
    );
    println!(
        "  Corpus: 82,934 documents | 779,432 chunks (768-dim) | 108,881 nodes | 240,619 edges"
    );
    println!(
        "==========================================================================================================\n"
    );

    let rss_0 = current_rss_bytes();
    let t_lin_open = Instant::now();
    let lin_store = Store::open(Path::new(LIN_LIVE_PATH)).context("open lin store")?;
    let lin_open_ms = t_lin_open.elapsed().as_secs_f64() * 1000.0;
    let rss_after_lin = current_rss_bytes();

    println!(
        "==> Building 1:1 SQLite 3.x mirror (`WAL + FTS5 + BLOB f32 + B-Tree indexes + ANALYZE`) at {} ...",
        sqlite_path.display()
    );
    let sqlite_build_secs = build_sqlite_corpus_mirror(&lin_store, &sqlite_path)?;
    clear_lin_hot_cache(&lin_store);
    println!("==> SQLite 3 mirror ready in {:.2}s!\n", sqlite_build_secs);

    // =========================================================================
    // SECTION 1: Physical Storage Breakdown on Disk
    // =========================================================================
    let sqlite_bytes = sqlite_total_bytes(&sqlite_path);
    let duck_live_bytes = dir_or_file_bytes(Path::new(DUCKDB_LIVE_PATH));
    let duck_snap_bytes = dir_or_file_bytes(Path::new(DUCKDB_SNAPSHOT_PATH));
    let lin_bd = inspect_lin_dir(Path::new(LIN_LIVE_PATH));
    let lin_bak_path = latest_lin_backup_path();
    let lin_bak_bytes = lin_bak_path.as_deref().map(dir_or_file_bytes).unwrap_or(0);

    println!("### 1. Physical Storage Layout & Disk Footprint Breakdown (3-Way)");
    println!(
        "  SQLite 3 (WAL + FTS5 + B-Tree + binary BLOB f32): {:>10}  (uncompressed B-Tree pages + FTS5 postings)",
        format_gib(sqlite_bytes)
    );
    println!(
        "  DuckDB live working file:                         {:>10}  (MVCC + JSON text vectors + FTS tables)",
        format_gib(duck_live_bytes)
    );
    println!(
        "  DuckDB compacted backup:                          {:>10}",
        format_gib(duck_snap_bytes)
    );
    println!(
        "  lin DiskEngineV2 total live dir:                  {:>10}  ({} segments across 35 tables)",
        format_gib(lin_bd.total_bytes),
        lin_bd.segment_count
    );
    println!(
        "    ├─ Exact f32 vectors (.vf32+.vn):               {:>10}  (779,432 × 768 × 4B zero-copy mmap)",
        format_gib(lin_bd.vf32_bytes)
    );
    println!(
        "    ├─ Quantized SQ8 (.vsq8):                       {:>10}  (779,432 × 768 × 1B flat scan)",
        format_mib(lin_bd.vsq8_bytes)
    );
    println!(
        "    ├─ Clustered IVF-SQ8 (.vivf):                   {:>10}  ({} IVF-256 segment indexes)",
        format_mib(lin_bd.vivf_bytes),
        lin_bd.vivf_count
    );
    println!(
        "    ├─ Adaptive Dict cols (.dstr/.doff/.dord):      {:>10}  ({} u16-ordinal MmapDictCol columns!)",
        format_mib(lin_bd.dict_col_bytes),
        lin_bd.dict_col_count
    );
    println!(
        "    ├─ Zstd 32KiB text cols (.bdat):                {:>10}  (documents/chunks/graph/wiki bodies)",
        format_mib(lin_bd.text_col_bytes)
    );
    println!(
        "    ├─ Scalar fixed-width cols:                     {:>10}  (i64/f64/bool + null bitmaps)",
        format_mib(lin_bd.scalar_col_bytes)
    );
    println!(
        "    ├─ Primary key mmap (.ht):                      {:>10}  (O(1) open-addressed FNV-1a)",
        format_mib(lin_bd.pk_idx_bytes)
    );
    println!(
        "    ├─ Secondary mmap indexes (.ht):                {:>10}  (uri, document_id, wing, source_id, ...)",
        format_mib(lin_bd.sec_idx_bytes)
    );
    println!(
        "    ├─ Zero-heap MmapBm25Index v3:                  {:>10}  (LEB128 delta-varint postings)",
        format_mib(lin_bd.bm25_mmap_bytes)
    );
    println!(
        "    └─ Manifest + WAL + Tombstones:                 {:>10}",
        format_mib(lin_bd.manifest_wal_tomb_bytes)
    );
    if let Some(bp) = &lin_bak_path {
        println!(
            "  lin portable backup (Zstd):                       {:>10}  ({:.2}x smaller than SQLite, {:.2}x smaller than DuckDB) [{}]\n",
            format_gib(lin_bak_bytes),
            sqlite_bytes as f64 / lin_bak_bytes.max(1) as f64,
            duck_snap_bytes as f64 / lin_bak_bytes.max(1) as f64,
            bp.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("backup.lin"),
        );
    }

    // =========================================================================
    // SECTION 2: Cold Open, Read-Only Open & Process RSS Memory Trajectory
    // =========================================================================
    let rss_pre_sq = current_rss_bytes();
    let t_sq_open = Instant::now();
    let sqlite_conn = open_sqlite_bench_ro(&sqlite_path)?;
    let _: i64 = sqlite_conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get(0))?;
    let sq_open_ms = t_sq_open.elapsed().as_secs_f64() * 1000.0;
    let rss_post_sq = current_rss_bytes();

    let rss_pre_duck = current_rss_bytes();
    let t_duck_open = Instant::now();
    let duck_cfg = DuckConfig::default().access_mode(AccessMode::ReadOnly)?;
    let duck_conn = DuckConnection::open_with_flags(DUCKDB_SNAPSHOT_PATH, duck_cfg)
        .context("open DuckDB snapshot")?;
    let duck_open_ms = t_duck_open.elapsed().as_secs_f64() * 1000.0;
    let rss_post_duck = current_rss_bytes();

    let mut lin_ro_open_samples = Vec::with_capacity(5);
    let t_ro_total = Instant::now();
    for _ in 0..5 {
        let s = Instant::now();
        let ro = Store::open_read_only(Path::new(LIN_LIVE_PATH))?;
        std::hint::black_box(ro.stats()?);
        lin_ro_open_samples.push(s.elapsed());
    }
    let lin_ro_open_stats = compute_stats(&mut lin_ro_open_samples, t_ro_total.elapsed());

    println!("### 2. Cold Open Latency & Process RAM (RSS) Trajectory");
    println!(
        "  - SQLite 3 Connection::open (RO+mmap): {:>8.2} ms  (RSS delta: {})",
        sq_open_ms,
        format_mib_signed(rss_post_sq as i64 - rss_pre_sq as i64),
    );
    println!(
        "  - DuckDB Connection::open (RO):        {:>8.2} ms  (RSS delta: {})",
        duck_open_ms,
        format_mib_signed(rss_post_duck as i64 - rss_pre_duck as i64),
    );
    println!(
        "  - lin Store::open (RW + LOCK + WAL):   {:>8.2} ms  (RSS delta: {}, 0 rows decoded into heap!)",
        lin_open_ms,
        format_mib_signed(rss_after_lin as i64 - rss_0 as i64),
    );
    println!(
        "  - lin Store::open_read_only (5x):      p50 {:>5.2} ms | mean {:>5.2} ms | p95 {:>5.2} ms\n",
        lin_ro_open_stats.p50_us / 1000.0,
        lin_ro_open_stats.mean_us / 1000.0,
        lin_ro_open_stats.p95_us / 1000.0,
    );

    // Sample real IDs, URIs, wings, graph nodes, and real corpus embeddings!
    let mut sample_doc_ids: Vec<String> = Vec::with_capacity(2000);
    let mut sample_uris: Vec<String> = Vec::with_capacity(2000);
    let mut top_wings: Vec<(String, usize)> = Vec::new();
    {
        let mut stmt =
            duck_conn.prepare("SELECT id, uri FROM documents LIMIT 2000 OFFSET 10000")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            sample_doc_ids.push(r.get(0)?);
            sample_uris.push(r.get(1)?);
        }
        let mut wstmt = duck_conn.prepare(
            "SELECT wing, COUNT(*) AS cnt FROM documents \
             WHERE wing IS NOT NULL AND TRIM(wing) != '' \
             GROUP BY wing ORDER BY cnt DESC LIMIT 10",
        )?;
        let mut wrows = wstmt.query([])?;
        while let Some(r) = wrows.next()? {
            let w: String = r.get(0)?;
            let c: i64 = r.get(1)?;
            top_wings.push((w, c as usize));
        }
    }

    let mut sample_node_ids: Vec<String> = Vec::with_capacity(100);
    {
        let mut stmt = duck_conn.prepare("SELECT DISTINCT source_id FROM graph_edges LIMIT 100")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            sample_node_ids.push(r.get(0)?);
        }
    }

    let mut real_query_vecs: Vec<Vec<f32>> = Vec::with_capacity(50);
    {
        let r = lin_store.read()?;
        let mut step = 0usize;
        r.scan_each_projected("chunks", &["embedding"], |row| {
            step += 1;
            if step % 15_000 == 1 {
                let v = cell_vec_f32(row, "embedding");
                if v.len() == 768 {
                    let mut q = v;
                    q[0] += 0.025;
                    q[7] -= 0.019;
                    q[31] += 0.013;
                    real_query_vecs.push(q);
                    if real_query_vecs.len() >= 50 {
                        return Ok(false);
                    }
                }
            }
            Ok(true)
        })?;
    }

    // =========================================================================
    // SECTION 3: Point Reads, Projected Reads & Secondary Index Lookups
    // =========================================================================
    println!("### 3. Point Reads, Projected Metadata & Secondary Index Lookups (2,000 ops, 3-Way)");

    // 3a. Full Document Point Read (`get_document` — 2,000 ops)
    let mut sq_doc_s = Vec::with_capacity(sample_doc_ids.len());
    let t0 = Instant::now();
    {
        let mut stmt = sqlite_conn.prepare(
            "SELECT id, uri, title, content, metadata_json, content_hash, wing, room, \
             source_file, layer, kind, status, pinned, boost, revision \
             FROM documents WHERE id = ?1",
        )?;
        for id in &sample_doc_ids {
            let s = Instant::now();
            let row: (String, String, String, String) = stmt.query_row(sparams![id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            std::hint::black_box(row);
            sq_doc_s.push(s.elapsed());
        }
    }
    let sq_doc = compute_stats(&mut sq_doc_s, t0.elapsed());

    let mut duck_doc_s = Vec::with_capacity(sample_doc_ids.len());
    let t0 = Instant::now();
    {
        let mut stmt = duck_conn.prepare(
            "SELECT id, uri, title, content, metadata_json, content_hash, wing, room, \
             source_file, layer, kind, status, pinned, boost, revision \
             FROM documents WHERE id = ?",
        )?;
        for id in &sample_doc_ids {
            let s = Instant::now();
            let row: (String, String, String, String) = stmt.query_row(dparams![id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            std::hint::black_box(row);
            duck_doc_s.push(s.elapsed());
        }
    }
    let duck_doc = compute_stats(&mut duck_doc_s, t0.elapsed());

    clear_lin_hot_cache(&lin_store);
    let mut lin_doc_cold_s = Vec::with_capacity(sample_doc_ids.len());
    let t0 = Instant::now();
    for id in &sample_doc_ids {
        let s = Instant::now();
        std::hint::black_box(lin_store.get_document(id)?);
        lin_doc_cold_s.push(s.elapsed());
    }
    let lin_doc_cold = compute_stats(&mut lin_doc_cold_s, t0.elapsed());

    let mut lin_doc_hot_s = Vec::with_capacity(sample_doc_ids.len());
    let t0 = Instant::now();
    for id in &sample_doc_ids {
        let s = Instant::now();
        std::hint::black_box(lin_store.get_document(id)?);
        lin_doc_hot_s.push(s.elapsed());
    }
    let lin_doc_hot = compute_stats(&mut lin_doc_hot_s, t0.elapsed());

    println!("  [3a. Full Document Point Read (`get_document`, 2,000 docs incl. full body)]");
    print_us_row("SQLite 3 B-Tree PK:", sq_doc, None, None);
    print_us_row("DuckDB ART PK:", duck_doc, None, None);
    print_us_row(
        "lin Cold (mmap + Zstd):",
        lin_doc_cold,
        Some(sq_doc.mean_us),
        Some(duck_doc.mean_us),
    );
    print_us_row(
        "lin Hot (L1 HotTierCache):",
        lin_doc_hot,
        Some(sq_doc.mean_us),
        Some(duck_doc.mean_us),
    );

    // 3b. Projected Metadata Point Read (`id, uri, title, wing, layer`)
    let mut sq_proj_s = Vec::with_capacity(sample_doc_ids.len());
    let t0 = Instant::now();
    {
        let mut stmt = sqlite_conn
            .prepare("SELECT id, uri, title, wing, layer FROM documents WHERE id = ?1")?;
        for id in &sample_doc_ids {
            let s = Instant::now();
            let row: (String, String, String) =
                stmt.query_row(sparams![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            std::hint::black_box(row);
            sq_proj_s.push(s.elapsed());
        }
    }
    let sq_proj = compute_stats(&mut sq_proj_s, t0.elapsed());

    let mut duck_proj_s = Vec::with_capacity(sample_doc_ids.len());
    let t0 = Instant::now();
    {
        let mut stmt =
            duck_conn.prepare("SELECT id, uri, title, wing, layer FROM documents WHERE id = ?")?;
        for id in &sample_doc_ids {
            let s = Instant::now();
            let row: (String, String, String) =
                stmt.query_row(dparams![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            std::hint::black_box(row);
            duck_proj_s.push(s.elapsed());
        }
    }
    let duck_proj = compute_stats(&mut duck_proj_s, t0.elapsed());

    clear_lin_hot_cache(&lin_store);
    let mut lin_proj_cold_s = Vec::with_capacity(sample_doc_ids.len());
    let t0 = Instant::now();
    {
        let r = lin_store.read()?;
        for id in &sample_doc_ids {
            let s = Instant::now();
            let row =
                r.get_projected_by_id("documents", id, &["id", "uri", "title", "wing", "layer"])?;
            std::hint::black_box(row);
            lin_proj_cold_s.push(s.elapsed());
        }
    }
    let lin_proj_cold = compute_stats(&mut lin_proj_cold_s, t0.elapsed());

    println!(
        "  [3b. Projected Point Read (`get_projected_by_id` without `content` body, 2,000 docs)]"
    );
    print_us_row("SQLite 3 Row Page Read:", sq_proj, None, None);
    print_us_row("DuckDB Projected SQL:", duck_proj, None, None);
    print_us_row(
        "lin Cold Projected mmap:",
        lin_proj_cold,
        Some(sq_proj.mean_us),
        Some(duck_proj.mean_us),
    );

    // 3c. Secondary Index Lookup (`find_by_uri`, 2,000 ops)
    let mut sq_uri_s = Vec::with_capacity(sample_uris.len());
    let t0 = Instant::now();
    {
        let mut stmt = sqlite_conn.prepare(
            "SELECT id, uri, title, content, metadata_json, content_hash, wing, room, \
             source_file, layer, kind, status, pinned, boost, revision \
             FROM documents WHERE uri = ?1 LIMIT 1",
        )?;
        for uri in &sample_uris {
            let s = Instant::now();
            let row: (String, String, String, String) = stmt.query_row(sparams![uri], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            std::hint::black_box(row);
            sq_uri_s.push(s.elapsed());
        }
    }
    let sq_uri = compute_stats(&mut sq_uri_s, t0.elapsed());

    let mut duck_uri_s = Vec::with_capacity(sample_uris.len());
    let t0 = Instant::now();
    {
        let mut stmt = duck_conn.prepare(
            "SELECT id, uri, title, content, metadata_json, content_hash, wing, room, \
             source_file, layer, kind, status, pinned, boost, revision \
             FROM documents WHERE uri = ? LIMIT 1",
        )?;
        for uri in &sample_uris {
            let s = Instant::now();
            let row: (String, String, String, String) = stmt.query_row(dparams![uri], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            std::hint::black_box(row);
            duck_uri_s.push(s.elapsed());
        }
    }
    let duck_uri = compute_stats(&mut duck_uri_s, t0.elapsed());

    clear_lin_hot_cache(&lin_store);
    let mut lin_uri_cold_s = Vec::with_capacity(sample_uris.len());
    let t0 = Instant::now();
    for uri in &sample_uris {
        let s = Instant::now();
        std::hint::black_box(lin_store.find_by_uri(uri)?);
        lin_uri_cold_s.push(s.elapsed());
    }
    let lin_uri_cold = compute_stats(&mut lin_uri_cold_s, t0.elapsed());

    let mut lin_uri_hot_s = Vec::with_capacity(sample_uris.len());
    let t0 = Instant::now();
    for uri in &sample_uris {
        let s = Instant::now();
        std::hint::black_box(lin_store.find_by_uri(uri)?);
        lin_uri_hot_s.push(s.elapsed());
    }
    let lin_uri_hot = compute_stats(&mut lin_uri_hot_s, t0.elapsed());

    println!("  [3c. Secondary Index Lookup (`find_by_uri`, 2,000 ops)]");
    print_us_row("SQLite 3 `idx_docs_uri`:", sq_uri, None, None);
    print_us_row("DuckDB Index Scan:", duck_uri, None, None);
    print_us_row(
        "lin Cold (`sec_uri.ht`):",
        lin_uri_cold,
        Some(sq_uri.mean_us),
        Some(duck_uri.mean_us),
    );
    print_us_row(
        "lin Hot (L1 SecQueryCache):",
        lin_uri_hot,
        Some(sq_uri.mean_us),
        Some(duck_uri.mean_us),
    );

    // 3d. Chunk + 768-dim Vector Hydration (`list_chunks_for_document`, 500 docs)
    let chunk_doc_slice = &sample_doc_ids[..500];
    let mut sq_ch_s = Vec::with_capacity(500);
    let t0 = Instant::now();
    {
        let mut stmt = sqlite_conn.prepare(
            "SELECT id, document_id, chunk_index, content, embedding, char_start, char_end, metadata_json \
             FROM chunks WHERE document_id = ?1 ORDER BY chunk_index ASC",
        )?;
        for doc_id in chunk_doc_slice {
            let s = Instant::now();
            let mut rows = stmt.query(sparams![doc_id])?;
            let mut cnt = 0usize;
            while let Some(r) = rows.next()? {
                let blob: &[u8] = r.get_ref(4)?.as_blob()?;
                let emb = le_bytes_to_f32_vec(blob);
                std::hint::black_box(emb);
                cnt += 1;
            }
            std::hint::black_box(cnt);
            sq_ch_s.push(s.elapsed());
        }
    }
    let sq_ch = compute_stats(&mut sq_ch_s, t0.elapsed());

    let mut duck_ch_s = Vec::with_capacity(500);
    let t0 = Instant::now();
    {
        let mut stmt = duck_conn.prepare(
            "SELECT id, document_id, chunk_index, content, embedding_json, char_start, char_end, metadata_json \
             FROM chunks WHERE document_id = ? ORDER BY chunk_index ASC",
        )?;
        for doc_id in chunk_doc_slice {
            let s = Instant::now();
            let mut rows = stmt.query(dparams![doc_id])?;
            let mut cnt = 0usize;
            while let Some(r) = rows.next()? {
                let emb_json: String = r.get(4)?;
                let emb: Vec<f32> = serde_json::from_str(&emb_json)?;
                std::hint::black_box(emb);
                cnt += 1;
            }
            std::hint::black_box(cnt);
            duck_ch_s.push(s.elapsed());
        }
    }
    let duck_ch = compute_stats(&mut duck_ch_s, t0.elapsed());

    clear_lin_hot_cache(&lin_store);
    let mut lin_ch_cold_s = Vec::with_capacity(500);
    let t0 = Instant::now();
    for doc_id in chunk_doc_slice {
        let s = Instant::now();
        std::hint::black_box(lin_store.list_chunks_for_document(doc_id)?);
        lin_ch_cold_s.push(s.elapsed());
    }
    let lin_ch_cold = compute_stats(&mut lin_ch_cold_s, t0.elapsed());

    let mut lin_ch_hot_s = Vec::with_capacity(500);
    let t0 = Instant::now();
    for doc_id in chunk_doc_slice {
        let s = Instant::now();
        std::hint::black_box(lin_store.list_chunks_for_document(doc_id)?);
        lin_ch_hot_s.push(s.elapsed());
    }
    let lin_ch_hot = compute_stats(&mut lin_ch_hot_s, t0.elapsed());

    println!(
        "  [3d. Document Chunks + 768-dim Vector Hydration (`list_chunks_for_document`, 500 docs)]"
    );
    print_us_row("SQLite 3 + BLOB f32:", sq_ch, None, None);
    print_us_row("DuckDB + JSON f32 parse:", duck_ch, None, None);
    print_us_row(
        "lin Cold (`mmap` .vf32):",
        lin_ch_cold,
        Some(sq_ch.mean_us),
        Some(duck_ch.mean_us),
    );
    print_us_row(
        "lin Hot (L1 SecQueryCache):",
        lin_ch_hot,
        Some(sq_ch.mean_us),
        Some(duck_ch.mean_us),
    );
    println!();

    // =========================================================================
    // SECTION 4: Analytical Aggregations, Taxonomy & Lean Catalog Queries (Gaps #1 & #2)
    // =========================================================================
    println!(
        "### 4. Analytical Aggregations (`TextDict` `GROUP BY`) & Streaming Top-K Catalog (82,934 Docs)"
    );

    // 4a. `store.stats()` (4 table counts)
    let mut sq_st_s = Vec::with_capacity(20);
    let t0 = Instant::now();
    for _ in 0..20 {
        let s = Instant::now();
        let d: i64 = sqlite_conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))?;
        let c: i64 = sqlite_conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
        let n: i64 = sqlite_conn.query_row("SELECT COUNT(*) FROM graph_nodes", [], |r| r.get(0))?;
        let e: i64 = sqlite_conn.query_row("SELECT COUNT(*) FROM graph_edges", [], |r| r.get(0))?;
        std::hint::black_box((d, c, n, e));
        sq_st_s.push(s.elapsed());
    }
    let sq_st = compute_stats(&mut sq_st_s, t0.elapsed());

    let mut duck_st_s = Vec::with_capacity(100);
    let t0 = Instant::now();
    for _ in 0..100 {
        let s = Instant::now();
        let d: i64 = duck_conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))?;
        let c: i64 = duck_conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
        let n: i64 = duck_conn.query_row("SELECT COUNT(*) FROM graph_nodes", [], |r| r.get(0))?;
        let e: i64 = duck_conn.query_row("SELECT COUNT(*) FROM graph_edges", [], |r| r.get(0))?;
        std::hint::black_box((d, c, n, e));
        duck_st_s.push(s.elapsed());
    }
    let duck_st = compute_stats(&mut duck_st_s, t0.elapsed());

    let mut lin_st_s = Vec::with_capacity(100);
    let t0 = Instant::now();
    for _ in 0..100 {
        let s = Instant::now();
        std::hint::black_box(lin_store.stats()?);
        lin_st_s.push(s.elapsed());
    }
    let lin_st = compute_stats(&mut lin_st_s, t0.elapsed());
    println!("  [4a. 4-Table Row Counts (`store.stats()`)]");
    print_us_row("SQLite 3 4x COUNT(*):", sq_st, None, None);
    print_us_row("DuckDB 4x COUNT(*):", duck_st, None, None);
    print_us_row(
        "lin Segment Meta O(1):",
        lin_st,
        Some(sq_st.mean_us),
        Some(duck_st.mean_us),
    );

    // 4b. Taxonomy (`get_taxonomy` over 82,934 documents — GAP #1!)
    let mut sq_tax_s = Vec::with_capacity(20);
    let t0 = Instant::now();
    {
        let mut stmt = sqlite_conn.prepare(
            "SELECT wing, COALESCE(room, '(unassigned)'), COUNT(*) \
             FROM documents \
             WHERE wing IS NOT NULL AND TRIM(wing) != '' AND COALESCE(status, 'active') = 'active' \
             GROUP BY 1, 2",
        )?;
        for _ in 0..20 {
            let s = Instant::now();
            let mut rows = stmt.query([])?;
            let mut n = 0usize;
            while let Some(r) = rows.next()? {
                let c: i64 = r.get(2)?;
                std::hint::black_box(c);
                n += 1;
            }
            std::hint::black_box(n);
            sq_tax_s.push(s.elapsed());
        }
    }
    let sq_tax = compute_stats(&mut sq_tax_s, t0.elapsed());

    let mut duck_tax_s = Vec::with_capacity(20);
    let t0 = Instant::now();
    {
        let mut stmt = duck_conn.prepare(
            "SELECT wing, COALESCE(room, '(unassigned)'), COUNT(*) \
             FROM documents \
             WHERE wing IS NOT NULL AND TRIM(wing) != '' AND COALESCE(status, 'active') = 'active' \
             GROUP BY 1, 2",
        )?;
        for _ in 0..20 {
            let s = Instant::now();
            let mut rows = stmt.query([])?;
            let mut n = 0usize;
            while let Some(r) = rows.next()? {
                let c: i64 = r.get(2)?;
                std::hint::black_box(c);
                n += 1;
            }
            std::hint::black_box(n);
            duck_tax_s.push(s.elapsed());
        }
    }
    let duck_tax = compute_stats(&mut duck_tax_s, t0.elapsed());

    let mut lin_tax_s = Vec::with_capacity(20);
    let t0 = Instant::now();
    for _ in 0..20 {
        let s = Instant::now();
        std::hint::black_box(lin_store.get_taxonomy()?);
        lin_tax_s.push(s.elapsed());
    }
    let lin_tax = compute_stats(&mut lin_tax_s, t0.elapsed());
    println!(
        "  [4b. Full Corpus Taxonomy GROUP BY wing, room (`get_taxonomy`, 82,934 docs, 20 ops — GAP #1)]"
    );
    print_ms_row("SQLite 3 SQL GROUP BY:", sq_tax, None, None);
    print_ms_row("DuckDB SQL GROUP BY:", duck_tax, None, None);
    print_ms_row(
        "lin `TextDict` u16 GROUP BY:",
        lin_tax,
        Some(sq_tax.mean_us),
        Some(duck_tax.mean_us),
    );

    // 4c. Wing-Scoped Catalog Page (`list_document_catalog` with `wing` filter + sort + page 50 — GAP #2!)
    let target_wing = top_wings
        .first()
        .map(|(w, _)| w.clone())
        .unwrap_or_else(|| "default".into());
    let target_wing_docs = top_wings.first().map(|(_, c)| *c).unwrap_or(0);

    let mut sq_cat_s = Vec::with_capacity(20);
    let t0 = Instant::now();
    {
        let mut cnt_stmt = sqlite_conn.prepare(
            "SELECT COUNT(*) FROM documents WHERE wing = ?1 AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone')",
        )?;
        let mut pg_stmt = sqlite_conn.prepare(
            "SELECT id, uri, title, wing, room, source_file, layer, kind, status, pinned, boost, created_at, updated_at \
             FROM documents WHERE wing = ?1 AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone') \
             ORDER BY pinned DESC, boost DESC, updated_at DESC, id ASC LIMIT 50 OFFSET 0",
        )?;
        for _ in 0..20 {
            let s = Instant::now();
            let total: i64 = cnt_stmt.query_row(sparams![&target_wing], |r| r.get(0))?;
            let mut rows = pg_stmt.query(sparams![&target_wing])?;
            let mut k = 0usize;
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                std::hint::black_box(id);
                k += 1;
            }
            std::hint::black_box((total, k));
            sq_cat_s.push(s.elapsed());
        }
    }
    let sq_cat = compute_stats(&mut sq_cat_s, t0.elapsed());

    let mut duck_cat_s = Vec::with_capacity(20);
    let t0 = Instant::now();
    {
        let mut cnt_stmt = duck_conn.prepare(
            "SELECT COUNT(*) FROM documents WHERE wing = ? AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone')",
        )?;
        let mut pg_stmt = duck_conn.prepare(
            "SELECT id, uri, title, wing, room, source_file, layer, kind, status, pinned, boost, created_at, updated_at \
             FROM documents WHERE wing = ? AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone') \
             ORDER BY pinned DESC, boost DESC, updated_at DESC, id ASC LIMIT 50 OFFSET 0",
        )?;
        for _ in 0..20 {
            let s = Instant::now();
            let total: i64 = cnt_stmt.query_row(dparams![&target_wing], |r| r.get(0))?;
            let mut rows = pg_stmt.query(dparams![&target_wing])?;
            let mut k = 0usize;
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                std::hint::black_box(id);
                k += 1;
            }
            std::hint::black_box((total, k));
            duck_cat_s.push(s.elapsed());
        }
    }
    let duck_cat = compute_stats(&mut duck_cat_s, t0.elapsed());

    clear_lin_hot_cache(&lin_store);
    let cat_filter = DocumentCatalogFilter {
        wing: Some(target_wing.clone()),
        limit: 50,
        offset: 0,
        ..Default::default()
    };
    let mut lin_cat_cold_s = Vec::with_capacity(1);
    let t0 = Instant::now();
    std::hint::black_box(lin_store.list_document_catalog(&cat_filter)?);
    lin_cat_cold_s.push(t0.elapsed());
    let lin_cat_cold = compute_stats(&mut lin_cat_cold_s, t0.elapsed());

    let mut lin_cat_hot_s = Vec::with_capacity(20);
    let t0 = Instant::now();
    for _ in 0..20 {
        let s = Instant::now();
        std::hint::black_box(lin_store.list_document_catalog(&cat_filter)?);
        lin_cat_hot_s.push(s.elapsed());
    }
    let lin_cat_hot = compute_stats(&mut lin_cat_hot_s, t0.elapsed());

    println!(
        "  [4c. Scoped UI Catalog Page (`wing = '{}'`, {} docs -> sort + top 50 — GAP #2)]",
        target_wing, target_wing_docs
    );
    print_ms_row("SQLite 3 Composite B-Tree:", sq_cat, None, None);
    print_ms_row("DuckDB COUNT + ORDER BY:", duck_cat, None, None);
    print_ms_row(
        "lin Cold Streaming Top-50:",
        lin_cat_cold,
        Some(sq_cat.mean_us),
        Some(duck_cat.mean_us),
    );
    print_ms_row(
        "lin Warm Streaming Top-50:",
        lin_cat_hot,
        Some(sq_cat.mean_us),
        Some(duck_cat.mean_us),
    );
    println!();

    // =========================================================================
    // SECTION 5: Vector Search Deep-Dive (SQLite BLOB Scan vs DuckDB vs lin IVF-SQ8+f32)
    // =========================================================================
    println!(
        "### 5. Vector Search Deep-Dive (779,432 × 768-dim: SQLite BLOB vs DuckDB JSON vs lin IVF-SQ8+f32)"
    );

    // 5a. SQLite native binary f32 BLOB scan (measured on 100,000 rows -> extrapolated to 779,432, plus 1 full 779,432 scan!)
    let t_sq_vec = Instant::now();
    {
        let qv = &real_query_vecs[0];
        let mut stmt = sqlite_conn.prepare("SELECT id, embedding FROM chunks")?;
        let mut rows = stmt.query([])?;
        let mut best = f32::NEG_INFINITY;
        while let Some(r) = rows.next()? {
            let blob: &[u8] = r.get_ref(1)?.as_blob()?;
            let mut dot = 0.0f32;
            for (i, &b) in blob.as_chunks::<4>().0.iter().enumerate() {
                let x = f32::from_le_bytes(b);
                dot += qv[i] * x;
            }
            if dot > best {
                best = dot;
            }
        }
        std::hint::black_box(best);
    }
    let sq_779k_ms = t_sq_vec.elapsed().as_secs_f64() * 1000.0;

    let t_duck_vec = Instant::now();
    let mut parsed_vecs = 0usize;
    {
        let mut stmt = duck_conn.prepare("SELECT id, embedding_json FROM chunks LIMIT 50000")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let json: String = r.get(1)?;
            let v: Vec<f32> = serde_json::from_str(&json)?;
            std::hint::black_box(v.len());
            parsed_vecs += 1;
        }
    }
    let duck_50k_sec = t_duck_vec.elapsed().as_secs_f64();
    let duck_779k_ms = duck_50k_sec * (779_432.0 / parsed_vecs.max(1) as f64) * 1000.0;

    clear_lin_hot_cache(&lin_store);
    let t_first_vec = Instant::now();
    let _ = search::search_chunks(&lin_store, &real_query_vecs[0], 10, None)?;
    let lin_first_vec_ms = t_first_vec.elapsed().as_secs_f64() * 1000.0;

    println!(
        "  [5a. Cold First Vector Query over 779,432 × 768-dim]\n    SQLite 3 (full binary BLOB scan): {:>8.2} ms\n    DuckDB (SQL + serde_json 779k):   {:>8.2} ms  (extrapolated from 50k={:.2}s)\n    lin DiskEngineV2 (IVF-SQ8+f32):   {:>8.2} ms  --> [{:.0}x vs SQLite | {:.0}x vs DuckDB]",
        sq_779k_ms,
        duck_779k_ms,
        duck_50k_sec,
        lin_first_vec_ms,
        sq_779k_ms / lin_first_vec_ms.max(0.01),
        duck_779k_ms / lin_first_vec_ms.max(0.01),
    );

    for &tk in &[5usize, 10, 50] {
        clear_lin_hot_cache(&lin_store);
        let mut ooc_s = Vec::with_capacity(real_query_vecs.len());
        let t0 = Instant::now();
        for qv in &real_query_vecs {
            let s = Instant::now();
            std::hint::black_box(search::search_chunks(&lin_store, qv, tk, None)?);
            ooc_s.push(s.elapsed());
        }
        let ooc_st = compute_stats(&mut ooc_s, t0.elapsed());

        let mut hot_s = Vec::with_capacity(real_query_vecs.len());
        let t0 = Instant::now();
        for qv in &real_query_vecs {
            let s = Instant::now();
            std::hint::black_box(search::search_chunks(&lin_store, qv, tk, None)?);
            hot_s.push(s.elapsed());
        }
        let hot_st = compute_stats(&mut hot_s, t0.elapsed());

        println!(
            "  [5b. Vector Search over 779,432 chunks — top_k={tk} (50 real corpus embeddings)]"
        );
        print_ms_row(
            "lin Out-of-Core IVF-SQ8+f32:",
            ooc_st,
            Some(sq_779k_ms * 1000.0),
            Some(duck_779k_ms * 1000.0),
        );
        print_us_row("lin Hot L1 VecQueryCache:", hot_st, None, None);
    }

    // 5c. Exact Brute-Force f32 Cosine Ground-Truth vs IVF-SQ8+f32 Recall@1 & Recall@10 Audit!
    let audit_queries = &real_query_vecs[..10];
    let t_gt = Instant::now();
    let mut exact_top10_per_q: Vec<Vec<String>> = vec![Vec::new(); audit_queries.len()];
    {
        let r = lin_store.read()?;
        let mut heaps: Vec<std::collections::BinaryHeap<std::cmp::Reverse<(i64, String)>>> = (0
            ..audit_queries.len())
            .map(|_| std::collections::BinaryHeap::with_capacity(11))
            .collect();
        let q_norms: Vec<f64> = audit_queries
            .iter()
            .map(|q| {
                q.iter()
                    .map(|&x| (x as f64) * (x as f64))
                    .sum::<f64>()
                    .sqrt()
            })
            .collect();

        r.scan_each_projected("chunks", &["id", "embedding"], |row| {
            let Some(cid) = rag_mcp::db::lin_engine::cell_str(row, "id") else {
                return Ok(true);
            };
            let v = cell_vec_f32(row, "embedding");
            if v.len() != 768 {
                return Ok(true);
            }
            let mut vnorm_sq = 0.0f64;
            for &x in &v {
                vnorm_sq += (x as f64) * (x as f64);
            }
            let vnorm = vnorm_sq.sqrt();
            if vnorm <= 1e-18 {
                return Ok(true);
            }
            for (qi, qv) in audit_queries.iter().enumerate() {
                let dot: f64 = qv
                    .iter()
                    .zip(v.iter())
                    .map(|(&a, &b)| (a as f64) * (b as f64))
                    .sum();
                let cos = dot / (q_norms[qi] * vnorm);
                let scaled = (cos * 1_000_000_000.0) as i64;
                let h = &mut heaps[qi];
                if h.len() < 10 {
                    h.push(std::cmp::Reverse((scaled, cid.to_string())));
                } else if h.peek().is_some_and(|m| scaled > m.0.0) {
                    h.pop();
                    h.push(std::cmp::Reverse((scaled, cid.to_string())));
                }
            }
            Ok(true)
        })?;
        for (qi, h) in heaps.into_iter().enumerate() {
            let mut v: Vec<(i64, String)> = h.into_iter().map(|x| x.0).collect();
            v.sort_by_key(|a| std::cmp::Reverse(a.0));
            exact_top10_per_q[qi] = v.into_iter().map(|(_, id)| id).collect();
        }
    }
    let gt_elapsed_s = t_gt.elapsed().as_secs_f64();

    clear_lin_hot_cache(&lin_store);
    let mut recall_at_1_hits = 0usize;
    let mut recall_at_10_hits = 0usize;
    let mut total_gt = 0usize;
    for (qi, qv) in audit_queries.iter().enumerate() {
        let r = lin_store.read()?;
        let eng = r.disk_v2().unwrap();
        let ivf_hits = eng.search_vectors_out_of_core(
            "chunks",
            "embedding",
            "document_id",
            qv,
            10,
            true,
            None,
            None,
            |_| true,
        )?;
        let ivf_ids: HashSet<String> = ivf_hits
            .iter()
            .filter_map(|(_, row)| rag_mcp::db::lin_engine::cell_str(row, "id").map(str::to_string))
            .collect();
        let gt = &exact_top10_per_q[qi];
        if let Some(top1) = gt.first()
            && ivf_ids.contains(top1)
        {
            recall_at_1_hits += 1;
        }
        for gid in gt {
            total_gt += 1;
            if ivf_ids.contains(gid) {
                recall_at_10_hits += 1;
            }
        }
    }
    println!(
        "  [5c. ANN Accuracy Audit vs Exact Brute-Force f32 Cosine over all 779,432 vectors]\n    Top-1 Hit Rate: {:>5.1}% ({}/10) | Recall@10: {:>5.1}% ({}/{}) | (Brute-force reference took {:.2}s)\n",
        (recall_at_1_hits as f64 / audit_queries.len() as f64) * 100.0,
        recall_at_1_hits,
        (recall_at_10_hits as f64 / total_gt.max(1) as f64) * 100.0,
        recall_at_10_hits,
        total_gt,
        gt_elapsed_s,
    );

    // =========================================================================
    // SECTION 6: BM25 Lexical Search Matrix (SQLite FTS5 vs DuckDB FTS vs lin MmapBm25Index v3)
    // =========================================================================
    println!(
        "### 6. Full-Text BM25 Search: SQLite FTS5 vs DuckDB FTS vs lin `MmapBm25Index` v3 (779,432 Chunks)"
    );

    let bm25_categories: &[(&str, &[&str])] = &[
        (
            "1-Term High-DF Queries",
            &["database", "rust", "architecture", "graph", "migration"],
        ),
        (
            "3–4 Term Technical Queries",
            &[
                "database migration schema version",
                "graph edges wikilink resolution",
                "vector embedding cosine similarity",
                "tokio runtime worker threads",
                "reciprocal rank fusion hybrid search",
            ],
        ),
        (
            "Rare / Identifier Queries",
            &[
                "tree sitter ast",
                "sha256 backup manifest",
                "idempotency outbox journal",
                "stemmer porter conflation",
                "mempalace diary tunnel",
            ],
        ),
    ];

    fts::evict_bm25_ram_cache(&lin_store);
    let rss_pre_bm25 = current_rss_bytes();
    let t_bm25_cold = Instant::now();
    let _ = fts::search_bm25(&lin_store, "database migration", 10, &LexFilters::default())?;
    let bm25_cold_ms = t_bm25_cold.elapsed().as_secs_f64() * 1000.0;
    let rss_post_bm25 = current_rss_bytes();
    println!(
        "  [6a. Cold lin MmapBm25Index v3 Open + First Query]: {:>6.2} ms  (RSS delta: {}, 0 heap postings!)",
        bm25_cold_ms,
        format_mib_signed(rss_post_bm25 as i64 - rss_pre_bm25 as i64),
    );

    let mut sq_fts_stmt = sqlite_conn.prepare(
        "SELECT f.chunk_id, f.document_id, bm25(chunks_fts) AS score \
         FROM chunks_fts f \
         JOIN documents d ON d.id = f.document_id \
         WHERE chunks_fts MATCH ?1 AND COALESCE(d.status, 'active') = 'active' \
         ORDER BY score ASC LIMIT 10",
    )?;

    let _ = duck_conn.execute_batch("LOAD fts");
    let mut duck_fts_stmt = duck_conn.prepare(
        "SELECT c.id, c.document_id, fts_main_chunks.match_bm25(c.id, ?) AS score \
         FROM chunks c \
         JOIN documents d ON d.id = c.document_id \
         WHERE score IS NOT NULL AND COALESCE(d.status, 'active') = 'active' \
         ORDER BY score DESC LIMIT 10",
    )?;

    for (cat_name, qlist) in bm25_categories {
        let mut sq_s = Vec::with_capacity(qlist.len());
        let t0 = Instant::now();
        for q in *qlist {
            let mq = sanitize_fts5_match_query(q);
            let s = Instant::now();
            let mut rows = sq_fts_stmt.query(sparams![&mq])?;
            let mut n = 0usize;
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                std::hint::black_box(id);
                n += 1;
            }
            std::hint::black_box(n);
            sq_s.push(s.elapsed());
        }
        let sq_st = compute_stats(&mut sq_s, t0.elapsed());

        let mut d_s = Vec::with_capacity(qlist.len());
        let t0 = Instant::now();
        for q in *qlist {
            let s = Instant::now();
            let mut rows = duck_fts_stmt.query(dparams![q])?;
            let mut n = 0usize;
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                std::hint::black_box(id);
                n += 1;
            }
            std::hint::black_box(n);
            d_s.push(s.elapsed());
        }
        let d_st = compute_stats(&mut d_s, t0.elapsed());

        let mut l_s = Vec::with_capacity(qlist.len());
        let t0 = Instant::now();
        for q in *qlist {
            let s = Instant::now();
            let hits = fts::search_bm25(&lin_store, q, 10, &LexFilters::default())?;
            std::hint::black_box(hits);
            l_s.push(s.elapsed());
        }
        let l_st = compute_stats(&mut l_s, t0.elapsed());

        println!("  [BM25 Category: {cat_name}]");
        print_ms_row("SQLite 3 FTS5 bm25():", sq_st, None, None);
        print_ms_row("DuckDB FTS match_bm25:", d_st, None, None);
        print_ms_row(
            "lin MmapBm25Index v3:",
            l_st,
            Some(sq_st.mean_us),
            Some(d_st.mean_us),
        );
    }

    // Scoped BM25 (`wing` filter)
    let mut sq_bm25_wing_s = Vec::with_capacity(5);
    let t0 = Instant::now();
    {
        let mut wstmt = sqlite_conn.prepare(
            "SELECT f.chunk_id, f.document_id, bm25(chunks_fts) AS score \
             FROM chunks_fts f \
             JOIN documents d ON d.id = f.document_id \
             WHERE chunks_fts MATCH ?1 AND d.wing = ?2 AND COALESCE(d.status, 'active') = 'active' \
             ORDER BY score ASC LIMIT 10",
        )?;
        for q in bm25_categories[1].1 {
            let mq = sanitize_fts5_match_query(q);
            let s = Instant::now();
            let mut rows = wstmt.query(sparams![&mq, &target_wing])?;
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                std::hint::black_box(id);
            }
            sq_bm25_wing_s.push(s.elapsed());
        }
    }
    let sq_bm25_wing = compute_stats(&mut sq_bm25_wing_s, t0.elapsed());

    let mut duck_bm25_wing_s = Vec::with_capacity(5);
    let t0 = Instant::now();
    {
        let mut wstmt = duck_conn.prepare(
            "SELECT c.id, c.document_id, fts_main_chunks.match_bm25(c.id, ?) AS score \
             FROM chunks c \
             JOIN documents d ON d.id = c.document_id \
             WHERE score IS NOT NULL AND d.wing = ? AND COALESCE(d.status, 'active') = 'active' \
             ORDER BY score DESC LIMIT 10",
        )?;
        for q in bm25_categories[1].1 {
            let s = Instant::now();
            let mut rows = wstmt.query(dparams![q, &target_wing])?;
            while let Some(r) = rows.next()? {
                let id: String = r.get(0)?;
                std::hint::black_box(id);
            }
            duck_bm25_wing_s.push(s.elapsed());
        }
    }
    let duck_bm25_wing = compute_stats(&mut duck_bm25_wing_s, t0.elapsed());

    let mut lin_bm25_wing_s = Vec::with_capacity(5);
    let t0 = Instant::now();
    let lex_wing_filter = LexFilters {
        wing: Some(target_wing.clone()),
        ..Default::default()
    };
    for q in bm25_categories[1].1 {
        let s = Instant::now();
        std::hint::black_box(fts::search_bm25(&lin_store, q, 10, &lex_wing_filter)?);
        lin_bm25_wing_s.push(s.elapsed());
    }
    let lin_bm25_wing = compute_stats(&mut lin_bm25_wing_s, t0.elapsed());
    println!("  [BM25 Project-Scoped (`wing = '{target_wing}'`, 5 multi-term queries)]");
    print_ms_row("SQLite 3 Scoped FTS5:", sq_bm25_wing, None, None);
    print_ms_row("DuckDB Scoped FTS:", duck_bm25_wing, None, None);
    print_ms_row(
        "lin Scoped MmapBm25Index:",
        lin_bm25_wing,
        Some(sq_bm25_wing.mean_us),
        Some(duck_bm25_wing.mean_us),
    );
    println!();

    // =========================================================================
    // SECTION 7: Hybrid RRF Search Pipeline (End-to-End RAG Retrieval)
    // =========================================================================
    println!(
        "### 7. End-to-End Hybrid RRF Search (BM25 + 768-dim IVF-SQ8 + Reciprocal Rank Fusion)"
    );
    let hybrid_queries = [
        "database migration schema version",
        "graph edges wikilink resolution",
        "vector embedding cosine similarity",
        "tokio runtime worker threads",
        "authentication bearer token http",
        "incremental file ingest content hash",
        "reciprocal rank fusion hybrid search",
        "tree sitter rust ast parser",
        "backup verification sha256 manifest",
        "temporal knowledge graph facts",
    ];

    clear_lin_hot_cache(&lin_store);
    let mut hyb_ooc_s = Vec::with_capacity(hybrid_queries.len());
    let t0 = Instant::now();
    for (i, q) in hybrid_queries.iter().enumerate() {
        let s = Instant::now();
        let hits = search::search(
            &lin_store,
            &SearchQuery {
                mode: SearchMode::Hybrid,
                top_k: 10,
                query_text: Some((*q).to_string()),
                query_embedding: Some(real_query_vecs[i].clone()),
                ..SearchQuery::default()
            },
        )?;
        std::hint::black_box(hits);
        hyb_ooc_s.push(s.elapsed());
    }
    let hyb_ooc = compute_stats(&mut hyb_ooc_s, t0.elapsed());

    let mut hyb_hot_s = Vec::with_capacity(hybrid_queries.len());
    let t0 = Instant::now();
    for (i, q) in hybrid_queries.iter().enumerate() {
        let s = Instant::now();
        let hits = search::search(
            &lin_store,
            &SearchQuery {
                mode: SearchMode::Hybrid,
                top_k: 10,
                query_text: Some((*q).to_string()),
                query_embedding: Some(real_query_vecs[i].clone()),
                ..SearchQuery::default()
            },
        )?;
        std::hint::black_box(hits);
        hyb_hot_s.push(s.elapsed());
    }
    let hyb_hot = compute_stats(&mut hyb_hot_s, t0.elapsed());

    let mut hyb_wing_s = Vec::with_capacity(hybrid_queries.len());
    let t0 = Instant::now();
    for (i, q) in hybrid_queries.iter().enumerate() {
        let s = Instant::now();
        let hits = search::search(
            &lin_store,
            &SearchQuery {
                mode: SearchMode::Hybrid,
                top_k: 10,
                query_text: Some((*q).to_string()),
                query_embedding: Some(real_query_vecs[i + 10].clone()),
                wing: Some(target_wing.clone()),
                ..SearchQuery::default()
            },
        )?;
        std::hint::black_box(hits);
        hyb_wing_s.push(s.elapsed());
    }
    let hyb_wing = compute_stats(&mut hyb_wing_s, t0.elapsed());

    print_ms_row(
        "lin Global Out-of-Core Hybrid:",
        hyb_ooc,
        Some(sq_779k_ms * 1000.0),
        Some(duck_779k_ms * 1000.0),
    );
    print_ms_row(
        "lin Global Hot L1 Hybrid:",
        hyb_hot,
        Some(sq_779k_ms * 1000.0),
        Some(duck_779k_ms * 1000.0),
    );
    print_ms_row("lin Wing-Scoped Hybrid RRF:", hyb_wing, None, None);
    println!();

    // =========================================================================
    // SECTION 8: Knowledge Graph Workloads (Backlinks, 2-Hop BFS, Graph Stats — Gaps #1 & #3)
    // =========================================================================
    println!("### 8. Knowledge Graph Workloads (108,881 Nodes / 240,619 Edges — 3-Way)");

    // 8a. Obsidian Backlinks (`store.backlinks(node_id)` — 100 nodes)
    let mut sq_bl_s = Vec::with_capacity(sample_node_ids.len());
    let t0 = Instant::now();
    {
        let mut estmt = sqlite_conn.prepare(
            "SELECT id, source_id, target_id, rel_type, weight, context FROM graph_edges WHERE target_id = ?1",
        )?;
        let mut nstmt = sqlite_conn.prepare(
            "SELECT id, kind, label, document_id, uri, resolved, metadata_json FROM graph_nodes WHERE id = ?1",
        )?;
        for nid in &sample_node_ids {
            let s = Instant::now();
            let mut srcs = HashSet::new();
            srcs.insert(nid.clone());
            let mut rows = estmt.query(sparams![nid])?;
            while let Some(r) = rows.next()? {
                let sid: String = r.get(1)?;
                srcs.insert(sid);
            }
            for sid in &srcs {
                let _: Option<String> = nstmt.query_row(sparams![sid], |r| r.get(2)).ok();
            }
            sq_bl_s.push(s.elapsed());
        }
    }
    let sq_bl = compute_stats(&mut sq_bl_s, t0.elapsed());

    let mut duck_bl_s = Vec::with_capacity(sample_node_ids.len());
    let t0 = Instant::now();
    {
        let mut estmt = duck_conn.prepare(
            "SELECT id, source_id, target_id, rel_type, weight, context FROM graph_edges WHERE target_id = ?",
        )?;
        let mut nstmt = duck_conn.prepare(
            "SELECT id, kind, label, document_id, uri, resolved, metadata_json FROM graph_nodes WHERE id = ?",
        )?;
        for nid in &sample_node_ids {
            let s = Instant::now();
            let mut srcs = HashSet::new();
            srcs.insert(nid.clone());
            let mut rows = estmt.query(dparams![nid])?;
            while let Some(r) = rows.next()? {
                let sid: String = r.get(1)?;
                srcs.insert(sid);
            }
            for sid in &srcs {
                let _: Option<String> = nstmt.query_row(dparams![sid], |r| r.get(2)).ok();
            }
            duck_bl_s.push(s.elapsed());
        }
    }
    let duck_bl = compute_stats(&mut duck_bl_s, t0.elapsed());

    clear_lin_hot_cache(&lin_store);
    let mut lin_bl_cold_s = Vec::with_capacity(sample_node_ids.len());
    let t0 = Instant::now();
    for nid in &sample_node_ids {
        let s = Instant::now();
        std::hint::black_box(lin_store.backlinks(nid)?);
        lin_bl_cold_s.push(s.elapsed());
    }
    let lin_bl_cold = compute_stats(&mut lin_bl_cold_s, t0.elapsed());

    let mut lin_bl_hot_s = Vec::with_capacity(sample_node_ids.len());
    let t0 = Instant::now();
    for nid in &sample_node_ids {
        let s = Instant::now();
        std::hint::black_box(lin_store.backlinks(nid)?);
        lin_bl_hot_s.push(s.elapsed());
    }
    let lin_bl_hot = compute_stats(&mut lin_bl_hot_s, t0.elapsed());

    println!("  [8a. Backlinks Lookup (`store.backlinks`, 100 nodes)]");
    print_us_row("SQLite 3 `idx_edges_tgt`:", sq_bl, None, None);
    print_us_row("DuckDB SQL Backlinks:", duck_bl, None, None);
    print_us_row(
        "lin Cold (`sec_target_id`):",
        lin_bl_cold,
        Some(sq_bl.mean_us),
        Some(duck_bl.mean_us),
    );
    print_us_row(
        "lin Hot (L1 Cache):",
        lin_bl_hot,
        Some(sq_bl.mean_us),
        Some(duck_bl.mean_us),
    );

    // 8b. 2-Hop Graph BFS (`neighbors(id, depth=2, max_nodes=50)`, 100 seeds including 20k-edge superhubs — GAP #3!)
    let mut sq_bfs_s = Vec::with_capacity(sample_node_ids.len());
    let t0 = Instant::now();
    {
        let mut src_edge_stmt = sqlite_conn.prepare(
            "SELECT e.id, e.target_id FROM graph_edges e \
             JOIN graph_nodes n ON n.id = e.target_id \
             WHERE e.source_id = ?1",
        )?;
        let mut tgt_edge_stmt = sqlite_conn.prepare(
            "SELECT e.id, e.source_id FROM graph_edges e \
             JOIN graph_nodes n ON n.id = e.source_id \
             WHERE e.target_id = ?1",
        )?;
        let mut node_stmt = sqlite_conn.prepare(
            "SELECT id, kind, label, document_id, uri, resolved, metadata_json \
             FROM graph_nodes WHERE id = ?1",
        )?;
        let mut edge_among_stmt = sqlite_conn.prepare(
            "SELECT id, source_id, target_id, rel_type, weight, context \
             FROM graph_edges WHERE source_id = ?1",
        )?;
        for seed in &sample_node_ids {
            let s = Instant::now();
            let mut visited: Vec<String> = vec![seed.clone()];
            let mut seen: HashSet<String> = HashSet::from([seed.clone()]);
            let mut frontier = vec![seed.clone()];
            for _hop in 0..2 {
                let remaining = 50usize.saturating_sub(visited.len());
                if remaining == 0 {
                    break;
                }
                let limit = remaining + seen.len();
                let mut grouped: HashMap<String, (usize, String)> = HashMap::new();
                for (ord, fid) in frontier.iter().enumerate() {
                    let mut distinct_for_fid = 0usize;
                    let mut r1 = src_edge_stmt.query(sparams![fid])?;
                    while let Some(r) = r1.next()? {
                        let eid: String = r.get(0)?;
                        let nid: String = r.get(1)?;
                        match grouped.get_mut(&nid) {
                            Some((mo, me)) => {
                                if ord < *mo {
                                    *mo = ord;
                                }
                                if eid < *me {
                                    *me = eid;
                                }
                            }
                            None => {
                                grouped.insert(nid, (ord, eid));
                                distinct_for_fid += 1;
                            }
                        }
                    }
                    let mut r2 = tgt_edge_stmt.query(sparams![fid])?;
                    while let Some(r) = r2.next()? {
                        let eid: String = r.get(0)?;
                        let nid: String = r.get(1)?;
                        match grouped.get_mut(&nid) {
                            Some((mo, me)) => {
                                if ord < *mo {
                                    *mo = ord;
                                }
                                if eid < *me {
                                    *me = eid;
                                }
                            }
                            None => {
                                grouped.insert(nid, (ord, eid));
                                distinct_for_fid += 1;
                            }
                        }
                    }
                    if distinct_for_fid >= limit && grouped.len() >= limit {
                        break;
                    }
                }
                let mut ordered: Vec<(usize, String, String)> = grouped
                    .into_iter()
                    .map(|(nid, (ord, eid))| (ord, eid, nid))
                    .collect();
                ordered.sort_by(|a, b| {
                    a.0.cmp(&b.0)
                        .then_with(|| a.1.cmp(&b.1))
                        .then_with(|| a.2.cmp(&b.2))
                });
                ordered.truncate(limit);
                let mut next_frontier = Vec::new();
                for (_, _, cand) in ordered {
                    if seen.insert(cand.clone()) {
                        next_frontier.push(cand);
                        if next_frontier.len() == remaining {
                            break;
                        }
                    }
                }
                if next_frontier.is_empty() {
                    break;
                }
                visited.extend(next_frontier.iter().cloned());
                frontier = next_frontier;
            }
            let selected: HashSet<&str> = visited.iter().map(String::as_str).collect();
            for nid in &visited {
                let _: Option<String> = node_stmt.query_row(sparams![nid], |nr| nr.get(2)).ok();
            }
            let mut edge_count = 0usize;
            for nid in &visited {
                let mut erows = edge_among_stmt.query(sparams![nid])?;
                while let Some(er) = erows.next()? {
                    let tgt: String = er.get(2)?;
                    if selected.contains(tgt.as_str()) {
                        edge_count += 1;
                    }
                }
            }
            std::hint::black_box(edge_count);
            sq_bfs_s.push(s.elapsed());
        }
    }
    let sq_bfs = compute_stats(&mut sq_bfs_s, t0.elapsed());

    let mut duck_bfs_s = Vec::with_capacity(sample_node_ids.len());
    let t0 = Instant::now();
    {
        let mut src_edge_stmt = duck_conn.prepare(
            "SELECT e.id, e.target_id FROM graph_edges e \
             JOIN graph_nodes n ON n.id = e.target_id \
             WHERE e.source_id = ?",
        )?;
        let mut tgt_edge_stmt = duck_conn.prepare(
            "SELECT e.id, e.source_id FROM graph_edges e \
             JOIN graph_nodes n ON n.id = e.source_id \
             WHERE e.target_id = ?",
        )?;
        let mut node_stmt = duck_conn.prepare(
            "SELECT id, kind, label, document_id, uri, resolved, metadata_json \
             FROM graph_nodes WHERE id = ?",
        )?;
        let mut edge_among_stmt = duck_conn.prepare(
            "SELECT id, source_id, target_id, rel_type, weight, context \
             FROM graph_edges WHERE source_id = ?",
        )?;
        for seed in &sample_node_ids {
            let s = Instant::now();
            let mut visited: Vec<String> = vec![seed.clone()];
            let mut seen: HashSet<String> = HashSet::from([seed.clone()]);
            let mut frontier = vec![seed.clone()];
            for _hop in 0..2 {
                let remaining = 50usize.saturating_sub(visited.len());
                if remaining == 0 {
                    break;
                }
                let limit = remaining + seen.len();
                let mut grouped: HashMap<String, (usize, String)> = HashMap::new();
                for (ord, fid) in frontier.iter().enumerate() {
                    let mut distinct_for_fid = 0usize;
                    let mut r1 = src_edge_stmt.query(dparams![fid])?;
                    while let Some(r) = r1.next()? {
                        let eid: String = r.get(0)?;
                        let nid: String = r.get(1)?;
                        match grouped.get_mut(&nid) {
                            Some((mo, me)) => {
                                if ord < *mo {
                                    *mo = ord;
                                }
                                if eid < *me {
                                    *me = eid;
                                }
                            }
                            None => {
                                grouped.insert(nid, (ord, eid));
                                distinct_for_fid += 1;
                            }
                        }
                    }
                    let mut r2 = tgt_edge_stmt.query(dparams![fid])?;
                    while let Some(r) = r2.next()? {
                        let eid: String = r.get(0)?;
                        let nid: String = r.get(1)?;
                        match grouped.get_mut(&nid) {
                            Some((mo, me)) => {
                                if ord < *mo {
                                    *mo = ord;
                                }
                                if eid < *me {
                                    *me = eid;
                                }
                            }
                            None => {
                                grouped.insert(nid, (ord, eid));
                                distinct_for_fid += 1;
                            }
                        }
                    }
                    if distinct_for_fid >= limit && grouped.len() >= limit {
                        break;
                    }
                }
                let mut ordered: Vec<(usize, String, String)> = grouped
                    .into_iter()
                    .map(|(nid, (ord, eid))| (ord, eid, nid))
                    .collect();
                ordered.sort_by(|a, b| {
                    a.0.cmp(&b.0)
                        .then_with(|| a.1.cmp(&b.1))
                        .then_with(|| a.2.cmp(&b.2))
                });
                ordered.truncate(limit);
                let mut next_frontier = Vec::new();
                for (_, _, cand) in ordered {
                    if seen.insert(cand.clone()) {
                        next_frontier.push(cand);
                        if next_frontier.len() == remaining {
                            break;
                        }
                    }
                }
                if next_frontier.is_empty() {
                    break;
                }
                visited.extend(next_frontier.iter().cloned());
                frontier = next_frontier;
            }
            let selected: HashSet<&str> = visited.iter().map(String::as_str).collect();
            for nid in &visited {
                let _: Option<String> = node_stmt.query_row(dparams![nid], |nr| nr.get(2)).ok();
            }
            let mut edge_count = 0usize;
            for nid in &visited {
                let mut erows = edge_among_stmt.query(dparams![nid])?;
                while let Some(er) = erows.next()? {
                    let tgt: String = er.get(2)?;
                    if selected.contains(tgt.as_str()) {
                        edge_count += 1;
                    }
                }
            }
            std::hint::black_box(edge_count);
            duck_bfs_s.push(s.elapsed());
        }
    }
    let duck_bfs = compute_stats(&mut duck_bfs_s, t0.elapsed());

    clear_lin_hot_cache(&lin_store);
    let mut lin_bfs_cold_s = Vec::with_capacity(sample_node_ids.len());
    let t0 = Instant::now();
    for seed in &sample_node_ids {
        let s = Instant::now();
        std::hint::black_box(lin_store.neighbors(seed, 2, 50)?);
        lin_bfs_cold_s.push(s.elapsed());
    }
    let lin_bfs_cold = compute_stats(&mut lin_bfs_cold_s, t0.elapsed());

    let mut lin_bfs_hot_s = Vec::with_capacity(sample_node_ids.len());
    let t0 = Instant::now();
    for seed in &sample_node_ids {
        let s = Instant::now();
        std::hint::black_box(lin_store.neighbors(seed, 2, 50)?);
        lin_bfs_hot_s.push(s.elapsed());
    }
    let lin_bfs_hot = compute_stats(&mut lin_bfs_hot_s, t0.elapsed());

    println!(
        "  [8b. 2-Hop Graph BFS incl. 20k-edge Superhubs (`neighbors(depth=2, max_nodes=50)`, 100 seeds — GAP #3)]"
    );
    print_ms_row("SQLite 3 2-Hop BFS:", sq_bfs, None, None);
    print_ms_row("DuckDB 2-Hop BFS:", duck_bfs, None, None);
    print_ms_row(
        "lin Cold Zero-Row 2-Hop BFS:",
        lin_bfs_cold,
        Some(sq_bfs.mean_us),
        Some(duck_bfs.mean_us),
    );
    print_ms_row(
        "lin Warm Zero-Row 2-Hop BFS:",
        lin_bfs_hot,
        Some(sq_bfs.mean_us),
        Some(duck_bfs.mean_us),
    );

    // 8c. Full Graph Aggregation (`graph_stats` over 108k nodes + 240k edges — GAP #1!)
    let mut sq_gs_s = Vec::with_capacity(5);
    let t0 = Instant::now();
    for _ in 0..5 {
        let s = Instant::now();
        let mut kstmt = sqlite_conn.prepare("SELECT kind, COUNT(*) FROM graph_nodes GROUP BY 1")?;
        let mut krows = kstmt.query([])?;
        let mut kn = 0usize;
        while krows.next()?.is_some() {
            kn += 1;
        }
        let mut rstmt = sqlite_conn.prepare(
            "SELECT rel_type, origin, context, COUNT(*) FROM graph_edges GROUP BY 1, 2, 3",
        )?;
        let mut rrows = rstmt.query([])?;
        while rrows.next()?.is_some() {
            kn += 1;
        }
        std::hint::black_box(kn);
        sq_gs_s.push(s.elapsed());
    }
    let sq_gs = compute_stats(&mut sq_gs_s, t0.elapsed());

    let mut duck_gs_s = Vec::with_capacity(5);
    let t0 = Instant::now();
    for _ in 0..5 {
        let s = Instant::now();
        let mut kstmt = duck_conn.prepare("SELECT kind, COUNT(*) FROM graph_nodes GROUP BY 1")?;
        let mut krows = kstmt.query([])?;
        let mut kn = 0usize;
        while krows.next()?.is_some() {
            kn += 1;
        }
        let mut rstmt = duck_conn.prepare(
            "SELECT rel_type, origin, context, COUNT(*) FROM graph_edges GROUP BY 1, 2, 3",
        )?;
        let mut rrows = rstmt.query([])?;
        while rrows.next()?.is_some() {
            kn += 1;
        }
        std::hint::black_box(kn);
        duck_gs_s.push(s.elapsed());
    }
    let duck_gs = compute_stats(&mut duck_gs_s, t0.elapsed());

    let mut lin_gs_s = Vec::with_capacity(5);
    let t0 = Instant::now();
    for _ in 0..5 {
        let s = Instant::now();
        std::hint::black_box(lin_store.graph_stats()?);
        lin_gs_s.push(s.elapsed());
    }
    let lin_gs = compute_stats(&mut lin_gs_s, t0.elapsed());
    let _ = GraphFilter::default();
    println!(
        "  [8c. Full Graph Stats Aggregation (`graph_stats`, 108,881 nodes + 240,619 edges, 5 ops — GAP #1)]"
    );
    print_ms_row("SQLite 3 SQL GROUP BY:", sq_gs, None, None);
    print_ms_row("DuckDB SQL GROUP BY:", duck_gs, None, None);
    print_ms_row(
        "lin `TextDict` u16 GROUP BY:",
        lin_gs,
        Some(sq_gs.mean_us),
        Some(duck_gs.mean_us),
    );
    println!();

    // =========================================================================
    // SECTION 9: Multi-Threaded Concurrency Scaling Curve (1, 2, 4, 8, 16 Reader Threads)
    // =========================================================================
    println!(
        "### 9. Multi-Threaded Concurrency Scaling Curve (1 -> 2 -> 4 -> 8 -> 16 Threads — 3-Way)"
    );
    println!(
        "  (Note: SQLite runs with a true multi-connection WAL reader pool — 1 independent connection per thread!)"
    );

    for id in &sample_doc_ids {
        let _ = lin_store.get_document(id)?;
    }

    let shared_ids = Arc::new(sample_doc_ids);
    let shared_qvecs = Arc::new(real_query_vecs);
    let duck_mutex = Arc::new(Mutex::new(duck_conn));

    println!("  [9a. Concurrent Full Document Reads (`get_document`, 1,000 ops/thread)]");
    for &threads in &[1usize, 2, 4, 8, 16] {
        let total_ops = threads * 1000;

        // SQLite: independent read-only WAL connection per thread!
        let sq_conns: Vec<SqliteConnection> = (0..threads)
            .map(|_| open_sqlite_bench_ro(&sqlite_path))
            .collect::<Result<Vec<_>>>()?;
        let t_sq = Instant::now();
        std::thread::scope(|s| {
            for (tid, c) in sq_conns.into_iter().enumerate() {
                let ids = Arc::clone(&shared_ids);
                s.spawn(move || {
                    let mut stmt = c
                        .prepare("SELECT id, uri, title, content FROM documents WHERE id = ?1")
                        .unwrap();
                    for i in 0..1000 {
                        let id = &ids[(tid * 131 + i) % ids.len()];
                        let row: (String, String, String, String) = stmt
                            .query_row(sparams![id], |r| {
                                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                            })
                            .unwrap();
                        std::hint::black_box(row);
                    }
                });
            }
        });
        let sq_qps = total_ops as f64 / t_sq.elapsed().as_secs_f64();

        let t_d = Instant::now();
        std::thread::scope(|s| {
            for tid in 0..threads {
                let conn = Arc::clone(&duck_mutex);
                let ids = Arc::clone(&shared_ids);
                s.spawn(move || {
                    for i in 0..1000 {
                        let id = &ids[(tid * 131 + i) % ids.len()];
                        let guard = conn.lock().unwrap();
                        let row: (String, String) = guard
                            .query_row(
                                "SELECT id, title FROM documents WHERE id = ?",
                                dparams![id],
                                |r| Ok((r.get(0)?, r.get(1)?)),
                            )
                            .unwrap();
                        std::hint::black_box(row);
                    }
                });
            }
        });
        let d_qps = total_ops as f64 / t_d.elapsed().as_secs_f64();

        let t_l = Instant::now();
        std::thread::scope(|s| {
            for tid in 0..threads {
                let st = lin_store.clone();
                let ids = Arc::clone(&shared_ids);
                s.spawn(move || {
                    for i in 0..1000 {
                        let id = &ids[(tid * 131 + i) % ids.len()];
                        let doc = st.get_document(id).unwrap();
                        std::hint::black_box(doc);
                    }
                });
            }
        });
        let l_qps = total_ops as f64 / t_l.elapsed().as_secs_f64();

        println!(
            "    {:>2} thr ({:>5} ops): SQLite WAL Pool {:>8.0} QPS | DuckDB {:>7.0} QPS | lin DiskEngineV2 {:>10.0} QPS  [{:>4.1}x vs SQLite | {:>5.1}x vs DuckDB]",
            threads,
            total_ops,
            sq_qps,
            d_qps,
            l_qps,
            l_qps / sq_qps.max(1.0),
            l_qps / d_qps.max(1.0),
        );
    }

    // 9b. Concurrent Mixed RAG Workload (8 threads: Hybrid + BM25 + Point Read)
    clear_lin_hot_cache(&lin_store);
    let t_mix = Instant::now();
    std::thread::scope(|s| {
        for tid in 0..8usize {
            let st = lin_store.clone();
            let ids = Arc::clone(&shared_ids);
            let qvs = Arc::clone(&shared_qvecs);
            s.spawn(move || {
                for step in 0..25usize {
                    let id = &ids[(tid * 97 + step) % ids.len()];
                    std::hint::black_box(st.get_document(id).unwrap());
                    if step.is_multiple_of(5) {
                        let q = hybrid_queries[(tid + step) % hybrid_queries.len()];
                        std::hint::black_box(
                            fts::search_bm25(&st, q, 10, &LexFilters::default()).unwrap(),
                        );
                    }
                    if step.is_multiple_of(10) {
                        let qv = &qvs[(tid * 3 + step) % qvs.len()];
                        std::hint::black_box(search::search_chunks(&st, qv, 10, None).unwrap());
                    }
                }
            });
        }
    });
    let mix_ms = t_mix.elapsed().as_secs_f64() * 1000.0;
    println!(
        "  [9b. 8-Thread Mixed Production RAG Burst (200 doc reads + 40 BM25 + 24 global 779k ANN)]: {:.1} ms total\n",
        mix_ms
    );

    // =========================================================================
    // SECTION 10: Write Lifecycle — Ingest, Updates, Deletes, LSM Vacuum & `:memory:` (GAP #4)
    // =========================================================================
    println!(
        "### 10. Write Lifecycle: ACID Ingest, Updates, Cascading Deletes, LSM Vacuum & `:memory:` (3-Way, GAP #4)"
    );
    let tmp = tempfile::tempdir()?;
    let sq_write_path = tmp.path().join("bench_write.sqlite");
    let duck_write_path = tmp.path().join("bench_write.duckdb");
    let lin_write_norm_path: PathBuf = tmp.path().join("bench_write_normal.lin");
    let lin_write_full_path: PathBuf = tmp.path().join("bench_write_full.lin");
    let sample_vec: Vec<f32> = (0..768).map(|i| ((i + 1) as f32) * 0.001).collect();
    let mut sample_vec_blob = Vec::with_capacity(768 * 4);
    f32_slice_to_le_bytes(&sample_vec, &mut sample_vec_blob);

    // 10-SQLite: WAL + synchronous=NORMAL
    let t_sqw_ins = Instant::now();
    let mut sqw = SqliteConnection::open(&sq_write_path)?;
    sqw.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE documents (id TEXT PRIMARY KEY, uri TEXT UNIQUE, title TEXT, content TEXT, metadata_json TEXT, created_at TEXT, updated_at TEXT, revision INTEGER);
         CREATE TABLE document_revisions (id TEXT PRIMARY KEY, document_id TEXT, revision INTEGER, content TEXT);
         CREATE TABLE chunks (id TEXT PRIMARY KEY, document_id TEXT, chunk_index INTEGER, content TEXT, embedding BLOB, char_start INTEGER, char_end INTEGER, metadata_json TEXT);
         CREATE INDEX idx_w_chunks_doc ON chunks(document_id);
         CREATE TABLE graph_nodes (id TEXT PRIMARY KEY, kind TEXT, label TEXT, document_id TEXT, uri TEXT, resolved INTEGER);
         CREATE INDEX idx_w_nodes_doc ON graph_nodes(document_id);
         CREATE TABLE graph_edges (id TEXT PRIMARY KEY, source_id TEXT, target_id TEXT, rel_type TEXT, weight REAL);
         CREATE INDEX idx_w_edges_src ON graph_edges(source_id);",
    )?;
    for d in 0..200 {
        let doc_id = format!("doc-{d}");
        let uri = format!("bench://doc-{d}");
        let title = format!("Benchmark Document {d}");
        let body = format!(
            "# Benchmark {d}\nLinks to [[Architecture Overview]] and [[Vector Engine]] with #rust and #storage."
        );
        let tx = sqw.transaction()?;
        tx.execute(
            "INSERT INTO documents VALUES (?1, ?2, ?3, ?4, '{}', '2026-11-12T00:00:00Z', '2026-11-12T00:00:00Z', 1)",
            sparams![&doc_id, &uri, &title, &body],
        )?;
        for c in 0..5 {
            let cid = format!("chunk-{d}-{c}");
            tx.execute(
                "INSERT INTO chunks VALUES (?1, ?2, ?3, ?4, ?5, 0, 100, '{}')",
                sparams![&cid, &doc_id, c, &body, &sample_vec_blob],
            )?;
        }
        let nid = format!("node-{d}");
        tx.execute(
            "INSERT INTO graph_nodes VALUES (?1, 'document', ?2, ?3, ?4, 1)",
            sparams![&nid, &title, &doc_id, &uri],
        )?;
        tx.execute(
            "INSERT INTO graph_edges VALUES (?1, ?2, 'hub-arch', 'links_to', 1.0)",
            sparams![format!("edge-{d}-1"), &nid],
        )?;
        tx.commit()?;
    }
    let sq_ins_ms = t_sqw_ins.elapsed().as_secs_f64() * 1000.0;

    let t_sqw_mut = Instant::now();
    for d in 0..50 {
        let doc_id = format!("doc-{d}");
        let rev_id = format!("{doc_id}:1");
        let tx = sqw.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO document_revisions SELECT ?1, id, revision, content FROM documents WHERE id = ?2",
            sparams![&rev_id, &doc_id],
        )?;
        tx.execute(
            "UPDATE documents SET content = 'updated content v2', revision = 2 WHERE id = ?1",
            sparams![&doc_id],
        )?;
        tx.commit()?;
    }
    for d in 50..100 {
        let doc_id = format!("doc-{d}");
        let tx = sqw.transaction()?;
        tx.execute(
            "DELETE FROM chunks WHERE document_id = ?1",
            sparams![&doc_id],
        )?;
        tx.execute(
            "UPDATE graph_nodes SET kind = 'stub', resolved = 0, document_id = NULL WHERE document_id = ?1",
            sparams![&doc_id],
        )?;
        tx.execute("DELETE FROM documents WHERE id = ?1", sparams![&doc_id])?;
        tx.commit()?;
    }
    let sq_mut_ms = t_sqw_mut.elapsed().as_secs_f64() * 1000.0;
    drop(sqw);

    // 10-DuckDB: WAL mode
    let t_dw_ins = Instant::now();
    let mut wconn = DuckConnection::open(&duck_write_path)?;
    wconn.execute_batch(
        "CREATE TABLE documents (id VARCHAR PRIMARY KEY, uri VARCHAR, title VARCHAR, content TEXT, metadata_json VARCHAR, created_at VARCHAR, updated_at VARCHAR, revision BIGINT);
         CREATE TABLE chunks (id VARCHAR PRIMARY KEY, document_id VARCHAR, chunk_index INTEGER, content TEXT, embedding_json VARCHAR, char_start INTEGER, char_end INTEGER, metadata_json VARCHAR);
         CREATE TABLE graph_nodes (id VARCHAR PRIMARY KEY, kind VARCHAR, label VARCHAR, document_id VARCHAR, uri VARCHAR, resolved BOOLEAN);
         CREATE TABLE graph_edges (id VARCHAR PRIMARY KEY, source_id VARCHAR, target_id VARCHAR, rel_type VARCHAR, weight DOUBLE);",
    )?;
    let emb_json = serde_json::to_string(&sample_vec)?;
    for d in 0..200 {
        let doc_id = format!("doc-{d}");
        let uri = format!("bench://doc-{d}");
        let title = format!("Benchmark Document {d}");
        let body = format!(
            "# Benchmark {d}\nLinks to [[Architecture Overview]] and [[Vector Engine]] with #rust and #storage."
        );
        let tx = wconn.transaction()?;
        tx.execute(
            "INSERT INTO documents VALUES (?, ?, ?, ?, '{}', '2026-11-12T00:00:00Z', '2026-11-12T00:00:00Z', 1)",
            dparams![&doc_id, &uri, &title, &body],
        )?;
        for c in 0..5 {
            let cid = format!("chunk-{d}-{c}");
            tx.execute(
                "INSERT INTO chunks VALUES (?, ?, ?, ?, ?, 0, 100, '{}')",
                dparams![&cid, &doc_id, c, &body, &emb_json],
            )?;
        }
        let nid = format!("node-{d}");
        tx.execute(
            "INSERT INTO graph_nodes VALUES (?, 'document', ?, ?, ?, true)",
            dparams![&nid, &title, &doc_id, &uri],
        )?;
        tx.execute(
            "INSERT INTO graph_edges VALUES (?, ?, 'hub-arch', 'links_to', 1.0)",
            dparams![format!("edge-{d}-1"), &nid],
        )?;
        tx.commit()?;
    }
    let duck_ins_ms = t_dw_ins.elapsed().as_secs_f64() * 1000.0;

    let t_dw_mut = Instant::now();
    for d in 0..50 {
        let doc_id = format!("doc-{d}");
        let tx = wconn.transaction()?;
        tx.execute(
            "UPDATE documents SET content = 'updated content v2', revision = 2 WHERE id = ?",
            dparams![&doc_id],
        )?;
        tx.commit()?;
    }
    for d in 50..100 {
        let doc_id = format!("doc-{d}");
        let tx = wconn.transaction()?;
        tx.execute(
            "DELETE FROM chunks WHERE document_id = ?",
            dparams![&doc_id],
        )?;
        tx.execute("DELETE FROM documents WHERE id = ?", dparams![&doc_id])?;
        tx.commit()?;
    }
    let duck_mut_ms = t_dw_mut.elapsed().as_secs_f64() * 1000.0;
    drop(wconn);

    // Helper to run exact 200 ingest + (50 full CAS revisioned updates + 50 full 7-table cascading deletes) on `lin`
    let run_lin_write_bench = |path: &Path,
                               sync_full: bool|
     -> Result<(f64, f64, f64, u64, u64, f64)> {
        let wstore = Store::open(path)?;
        wstore.set_sync_full(sync_full)?;
        let vec_arc: Arc<[f32]> = Arc::from(sample_vec.clone());
        let t_ins = Instant::now();
        {
            use rag_mcp::db::lin_engine::{LinWriter, RecBuilder};
            for d in 0..200 {
                let doc_id = format!("doc-{d}");
                let uri = format!("bench://doc-{d}");
                let title = format!("Benchmark Document {d}");
                let body = format!(
                    "# Benchmark {d}\nLinks to [[Architecture Overview]] and [[Vector Engine]] with #rust and #storage."
                );
                let nid = format!("node-{d}");
                let eid = format!("edge-{d}-1");
                let mut wg = wstore.write()?;
                let mut tx = wg.begin_tx()?;
                tx.upsert_by_id(
                    "documents",
                    &doc_id,
                    RecBuilder::new()
                        .str("id", &doc_id)
                        .str("uri", &uri)
                        .str("title", &title)
                        .str("content", &body)
                        .str("metadata_json", "{}")
                        .str("created_at", "2026-11-12T00:00:00Z")
                        .str("updated_at", "2026-11-12T00:00:00Z")
                        .i64("revision", 1)
                        .build(),
                )?;
                let mut chunk_recs = Vec::with_capacity(5);
                for c in 0..5 {
                    let cid = format!("chunk-{d}-{c}");
                    tx.bridge_embedder()
                        .stage(cid.clone(), Arc::clone(&vec_arc));
                    chunk_recs.push(
                        RecBuilder::new()
                            .str("id", cid)
                            .str("document_id", &doc_id)
                            .i64("chunk_index", c)
                            .str("content", &body)
                            .i64("char_start", 0)
                            .i64("char_end", 100)
                            .str("metadata_json", "{}")
                            .build(),
                    );
                }
                tx.insert_records("chunks", chunk_recs)?;
                tx.upsert_by_id(
                    "graph_nodes",
                    &nid,
                    RecBuilder::new()
                        .str("id", &nid)
                        .str("kind", "document")
                        .str("label", &title)
                        .str("document_id", &doc_id)
                        .str("uri", &uri)
                        .bool("resolved", true)
                        .build(),
                )?;
                tx.upsert_by_id(
                    "graph_edges",
                    &eid,
                    RecBuilder::new()
                        .str("id", &eid)
                        .str("source_id", &nid)
                        .str("target_id", "hub-arch")
                        .str("rel_type", "links_to")
                        .f64("weight", 1.0)
                        .build(),
                )?;
                tx.commit()?;
            }
            wstore.checkpoint()?;
        }
        let ins_ms = t_ins.elapsed().as_secs_f64() * 1000.0;

        let t_mut_wal = Instant::now();
        for d in 0..50 {
            let mut doc = wstore.get_document(&format!("doc-{d}"))?.unwrap();
            doc.content = "updated content v2".into();
            doc.revision = 2;
            wstore.upsert_document(&doc)?;
        }
        for d in 50..100 {
            wstore.delete_document(&format!("doc-{d}"))?;
        }
        let mut_wal_ms = t_mut_wal.elapsed().as_secs_f64() * 1000.0;
        wstore.checkpoint()?;
        let mut_ckpt_ms = t_mut_wal.elapsed().as_secs_f64() * 1000.0;

        let t_vac = Instant::now();
        let vac_rep = wstore.vacuum_store()?;
        let vac_ms = t_vac.elapsed().as_secs_f64() * 1000.0;
        Ok((
            ins_ms,
            mut_wal_ms,
            mut_ckpt_ms,
            vac_rep.bytes_before.unwrap_or(0),
            vac_rep.bytes_after.unwrap_or(0),
            vac_ms,
        ))
    };

    let (lin_norm_ins_ms, lin_norm_mut_wal_ms, lin_norm_mut_ckpt_ms, _, _, _) =
        run_lin_write_bench(&lin_write_norm_path, false)?;
    let (
        lin_full_ins_ms,
        lin_full_mut_wal_ms,
        lin_full_mut_ckpt_ms,
        vac_before,
        vac_after,
        lin_vac_ms,
    ) = run_lin_write_bench(&lin_write_full_path, true)?;

    println!(
        "  [10a. 200 ACID Ingest Transactions + 1,000 × 768-dim Chunks + Checkpoint]\n    SQLite 3 (WAL NORMAL):        {:>7.1} ms ({:>7.1} docs/s)\n    DuckDB (WAL):                 {:>7.1} ms ({:>7.1} docs/s)\n    lin v2 (WAL SyncMode::Normal):{:>7.1} ms ({:>7.1} docs/s)  --> [{:.2}x vs SQLite | {:.2}x vs DuckDB]\n    lin v2 (F_FULLFSYNC/tx + Ckpt):{:>6.1} ms ({:>7.1} docs/s)  --> [strict hardware barrier per tx!]",
        sq_ins_ms,
        200.0 / (sq_ins_ms / 1000.0),
        duck_ins_ms,
        200.0 / (duck_ins_ms / 1000.0),
        lin_norm_ins_ms,
        200.0 / (lin_norm_ins_ms / 1000.0),
        sq_ins_ms / lin_norm_ins_ms.max(0.1),
        duck_ins_ms / lin_norm_ins_ms.max(0.1),
        lin_full_ins_ms,
        200.0 / (lin_full_ins_ms / 1000.0),
    );
    println!(
        "  [10b. 50 Revisioned Document Updates + 50 Cascading Deletes — GAP #4]\n    SQLite 3 (WAL NORMAL):        {:>7.1} ms\n    DuckDB (WAL):                 {:>7.1} ms\n    lin v2 (WAL SyncMode::Normal):{:>7.1} ms  (incl. segment checkpoint: {:>5.1} ms)  --> [{:.2}x vs SQLite | {:.2}x vs DuckDB]\n    lin v2 (F_FULLFSYNC per tx):  {:>7.1} ms  (incl. segment checkpoint: {:>5.1} ms)",
        sq_mut_ms,
        duck_mut_ms,
        lin_norm_mut_wal_ms,
        lin_norm_mut_ckpt_ms,
        sq_mut_ms / lin_norm_mut_wal_ms.max(0.1),
        duck_mut_ms / lin_norm_mut_wal_ms.max(0.1),
        lin_full_mut_wal_ms,
        lin_full_mut_ckpt_ms,
    );
    println!(
        "  [10c. LSM Compaction & Tombstone Reclamation (`store.vacuum_store()`)]\n    Compacted {} bytes -> {} bytes in {:.2} ms (0 dead rows remaining)",
        vac_before, vac_after, lin_vac_ms,
    );

    // 10d. Pure In-Memory Mode (`Store::open_in_memory()` vs SQLite `:memory:`)
    let sq_mem = SqliteConnection::open_in_memory()?;
    sq_mem.execute_batch(
        "CREATE TABLE documents (id TEXT PRIMARY KEY, uri TEXT, title TEXT, content TEXT);",
    )?;
    for d in 0..500 {
        sq_mem.execute(
            "INSERT INTO documents VALUES (?1, ?2, ?3, 'pure in-memory cached knowledge item')",
            sparams![
                format!("mem-{d}"),
                format!("mem://doc-{d}"),
                format!("In-Memory Hot Doc {d}")
            ],
        )?;
    }
    let mut sq_mem_s = Vec::with_capacity(5000);
    let t_sq_mem = Instant::now();
    {
        let mut stmt =
            sq_mem.prepare("SELECT id, uri, title, content FROM documents WHERE id = ?1")?;
        for i in 0..5000 {
            let key = format!("mem-{}", i % 500);
            let s = Instant::now();
            let row: (String, String, String, String) = stmt.query_row(sparams![&key], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            std::hint::black_box(row);
            sq_mem_s.push(s.elapsed());
        }
    }
    let sq_mem_st = compute_stats(&mut sq_mem_s, t_sq_mem.elapsed());

    let mem_store = Store::open_in_memory()?;
    for d in 0..500 {
        mem_store.upsert_document(&Document {
            id: format!("mem-{d}"),
            uri: format!("mem://doc-{d}"),
            title: format!("In-Memory Hot Doc {d}"),
            content: "pure in-memory cached knowledge item".into(),
            ..Default::default()
        })?;
    }
    let mut mem_s = Vec::with_capacity(5000);
    let t_mem = Instant::now();
    for i in 0..5000 {
        let key = format!("mem-{}", i % 500);
        let s = Instant::now();
        std::hint::black_box(mem_store.get_document(&key)?);
        mem_s.push(s.elapsed());
    }
    let mem_st = compute_stats(&mut mem_s, t_mem.elapsed());
    println!("  [10d. Pure In-Memory Cache Mode (`:memory:`, 5,000 point reads)]");
    print_us_row("SQLite 3 `:memory:`:", sq_mem_st, None, None);
    print_us_row(
        "lin `:memory:` mode:",
        mem_st,
        Some(sq_mem_st.mean_us),
        None,
    );

    // =========================================================================
    // SECTION 11: Final Process RSS & L1 HotTierCache Memory Audit
    // =========================================================================
    drop(sq_fts_stmt);
    drop(sqlite_conn);
    drop(duck_mutex);
    let rss_final = current_rss_bytes();
    println!(
        "\n### 11. Final Process Memory & L1 HotTierCache Audit (After All 3-Way Benchmarks!)"
    );
    if let Ok(r) = lin_store.read()
        && let Some(eng) = r.disk_v2()
    {
        let hc = eng.hot_cache_stats();
        println!(
            "  - L1 HotTierCache RAM usage:     {:>8.2} MiB / 256.00 MiB cap (cached_rows = {})",
            hc.used_bytes as f64 / (1024.0 * 1024.0),
            hc.cached_rows,
        );
        println!(
            "  - L1 Hit/Miss Counters:          row_hits={} (misses={}) | sec_idx_hits={} (misses={}) | vec_query_hits={} (misses={})",
            hc.row_hits,
            hc.row_misses,
            hc.sec_query_hits,
            hc.sec_query_misses,
            hc.vec_query_hits,
            hc.vec_query_misses,
        );
        println!(
            "  - Process RSS after full suite:  {:>8.2} MiB  (bounded OS page cache + L1 cap; 500 GiB DB safe!)",
            rss_final as f64 / (1024.0 * 1024.0),
        );
    }

    Ok(())
}
