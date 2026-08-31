//! One-shot DB rebuild: EXPORT DATABASE → fresh file → IMPORT DATABASE.
//!
//! Fixes ART index corruption ("Failed to delete all rows from index") by
//! rebuilding every table and index from a full logical dump. The server must
//! be stopped (single writer). Original file is left untouched; the rebuilt
//! store is written to `--out`.
//!
//! ```bash
//! cargo run --release --bin db_repair -- --db ./rag.duckdb --out ./rag-rebuilt.duckdb
//! ```

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut db = None;
    let mut out = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--db" => db = Some(PathBuf::from(it.next().context("--db needs path")?)),
            "--out" => out = Some(PathBuf::from(it.next().context("--out needs path")?)),
            "-h" | "--help" => {
                eprintln!("Usage: db_repair --db SOURCE.duckdb --out REBUILT.duckdb");
                std::process::exit(0);
            }
            other => bail!("unknown arg: {other}"),
        }
    }
    let db = db.context("required: --db")?;
    let out = out.context("required: --out")?;
    if out.exists() {
        bail!("--out already exists: {}", out.display());
    }

    let dump_dir = std::env::temp_dir().join(format!("rag-db-repair-{}", std::process::id()));
    let dump = dump_dir.display().to_string().replace('\'', "''");

    eprintln!("export {} -> {}", db.display(), dump_dir.display());
    {
        let src = duckdb::Connection::open(&db).context("open source db")?;
        src.execute_batch(&format!("EXPORT DATABASE '{dump}' (FORMAT PARQUET);"))
            .context("EXPORT DATABASE")?;
    }

    eprintln!("import -> {}", out.display());
    {
        let dst = duckdb::Connection::open(&out).context("open rebuilt db")?;
        dst.execute_batch(&format!("IMPORT DATABASE '{dump}';"))
            .context("IMPORT DATABASE")?;
        dst.execute_batch("CHECKPOINT;").context("CHECKPOINT")?;

        let count = |table: &str| -> Result<i64> {
            let sql = format!("SELECT COUNT(*) FROM {table}");
            dst.query_row(&sql, [], |row| row.get(0))
                .with_context(|| format!("verify {table}"))
        };
        let documents = count("documents")?;
        let chunks = count("chunks")?;
        let nodes = count("graph_nodes")?;
        let edges = count("graph_edges")?;
        let orphan_chunks: i64 = dst.query_row(
            "SELECT COUNT(*) FROM chunks c WHERE NOT EXISTS \
             (SELECT 1 FROM documents d WHERE d.id = c.document_id)",
            [],
            |row| row.get(0),
        ).context("verify orphan chunks")?;
        let orphan_edges: i64 = dst.query_row(
            "SELECT COUNT(*) FROM graph_edges e WHERE NOT EXISTS \
             (SELECT 1 FROM graph_nodes n WHERE n.id = e.source_id) OR NOT EXISTS \
             (SELECT 1 FROM graph_nodes n WHERE n.id = e.target_id)",
            [],
            |row| row.get(0),
        ).context("verify orphan edges")?;
        if orphan_chunks != 0 || orphan_edges != 0 {
            bail!("rebuilt database failed integrity verification: orphan_chunks={orphan_chunks}, orphan_edges={orphan_edges}");
        }
        eprintln!(
            "verified: documents={documents} chunks={chunks} nodes={nodes} edges={edges} orphan_chunks=0 orphan_edges=0"
        );
    }

    let _ = std::fs::remove_dir_all(&dump_dir);
    eprintln!("done: rebuilt store at {}", out.display());
    Ok(())
}
