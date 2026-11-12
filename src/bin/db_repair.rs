//! One-shot DB rebuild for `lin` databases: portable snapshot -> fresh `.lin` directory + verification.
//!
//! ```bash
//! cargo run --release --bin db_repair -- --db ./rag.lin --out ./rag-rebuilt.lin
//! ```

use anyhow::{Context, Result, bail};
use rag_mcp::Store;
use rag_mcp::db::lin_engine::{LinReader, clean_db_path, req_str, restore_portable_backup_to_dir};
use std::collections::HashSet;
use std::path::PathBuf;

/// Rebuild a `lin` database through a portable logical snapshot and verify integrity.
#[derive(clap::Parser)]
#[command(name = "db_repair")]
struct Args {
    /// Source database (left untouched).
    #[arg(long)]
    db: PathBuf,
    /// Rebuilt database to create (must not exist).
    #[arg(long)]
    out: PathBuf,
}

fn main() -> Result<()> {
    let Args { db, out } = <Args as clap::Parser>::parse();
    let db = clean_db_path(&db);
    let out = clean_db_path(&out);
    if out.exists() {
        bail!("--out already exists: {}", out.display());
    }

    let snap_file =
        std::env::temp_dir().join(format!("rag-db-repair-{}.linbak", std::process::id()));
    let _ = std::fs::remove_file(&snap_file);

    eprintln!("export {} -> {}", db.display(), snap_file.display());
    {
        let src = Store::open(&db).context("open source db")?;
        src.export_portable_backup(&snap_file)
            .context("export portable snapshot")?;
    }

    eprintln!("import -> {}", out.display());
    restore_portable_backup_to_dir(&snap_file, &out).context("restore portable snapshot")?;
    let _ = std::fs::remove_file(&snap_file);

    {
        let dst = Store::open(&out).context("open rebuilt db")?;
        let r = dst.read().context("acquire read handle")?;
        let documents = r.count_all("documents")? as i64;
        let chunks = r.count_all("chunks")? as i64;
        let nodes = r.count_all("graph_nodes")? as i64;
        let edges = r.count_all("graph_edges")? as i64;

        let mut doc_ids = HashSet::new();
        r.scan_each("documents", |rec| {
            let did = req_str(rec, "id")?.to_string();
            doc_ids.insert(did);
            Ok(true)
        })?;
        let mut orphan_chunks = 0i64;
        r.scan_each("chunks", |rec| {
            let did = req_str(rec, "document_id")?;
            if !doc_ids.contains(did) {
                orphan_chunks += 1;
            }
            Ok(true)
        })?;

        let mut node_ids = HashSet::new();
        r.scan_each("graph_nodes", |rec| {
            let nid = req_str(rec, "id")?.to_string();
            node_ids.insert(nid);
            Ok(true)
        })?;
        let mut orphan_edges = 0i64;
        r.scan_each("graph_edges", |rec| {
            let sid = req_str(rec, "source_id")?;
            let tid = req_str(rec, "target_id")?;
            if !node_ids.contains(sid) || !node_ids.contains(tid) {
                orphan_edges += 1;
            }
            Ok(true)
        })?;

        if orphan_chunks != 0 || orphan_edges != 0 {
            bail!(
                "rebuilt database failed integrity verification: orphan_chunks={orphan_chunks}, orphan_edges={orphan_edges}"
            );
        }
        eprintln!(
            "verified: documents={documents} chunks={chunks} nodes={nodes} edges={edges} orphan_chunks=0 orphan_edges=0"
        );
    }

    eprintln!("done: rebuilt store at {}", out.display());
    Ok(())
}
