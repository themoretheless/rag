//! Offline single-file ingest CLI.
//!
//! This binary opens DuckDB directly. It therefore requires an explicit database
//! path and an `--offline` acknowledgement. Stop the rag-mcp gateway before using
//! it. For a live database, use the MCP/HTTP `ingest_file` operation instead.
//!
//! Example (from rag repo root):
//! ```bash
//! RAG_INGEST_ROOTS=/path/to/allowed RAG_EMBEDDING_PROVIDER=mock \
//!   cargo run --release --bin ingest_file -- \
//!   --offline --db ./rag.duckdb --path /path/to/allowed/notes.md \
//!   --wing projects --room notes
//! ```

use anyhow::{Context, Result, bail};
use rag_mcp::ingest::{IngestFileCommand, IngestService};
use std::path::PathBuf;

fn main() -> Result<()> {
    rag_mcp::bootstrap::init_tracing("info");

    let args = parse_args()?;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run(args))
}

struct Args {
    path: PathBuf,
    wing: Option<String>,
    room: Option<String>,
    db: PathBuf,
}

/// Offline-only direct DuckDB writer. Stop the rag-mcp gateway first; for a
/// live database use the MCP/HTTP ingest_file operation. Reads text,
/// Markdown, HTML, PDF, or source code. The path must be under
/// RAG_INGEST_ROOTS. Env: RAG_INGEST_ROOTS, RAG_EMBEDDING_*.
#[derive(clap::Parser)]
#[command(name = "ingest_file")]
struct Cli {
    /// File to ingest.
    #[arg(long)]
    path: Option<PathBuf>,
    #[arg(long)]
    wing: Option<String>,
    #[arg(long)]
    room: Option<String>,
    /// Database file (never inherited from RAG_DB_PATH).
    #[arg(long)]
    db: Option<PathBuf>,
    /// Acknowledge direct DuckDB access (gateway stopped).
    #[arg(long)]
    offline: bool,
}

fn parse_args() -> Result<Args> {
    let Cli {
        path,
        wing,
        room,
        db,
        offline,
    } = <Cli as clap::Parser>::parse();
    let path = path.context("required: --path FILE")?;
    let db = db.context(
        "required: --db PATH; this offline tool never inherits RAG_DB_PATH or a default database",
    )?;
    if !offline {
        bail!(
            "refusing direct DuckDB access without --offline; stop the rag-mcp gateway first, \
             then pass --offline, or use the MCP/HTTP ingest_file operation"
        );
    }
    if !path.is_file() {
        bail!("--path is not a file: {}", path.display());
    }
    Ok(Args {
        path,
        wing: wing.filter(|s| !s.trim().is_empty()),
        room: room.filter(|s| !s.trim().is_empty()),
        db,
    })
}

async fn run(args: Args) -> Result<()> {
    let rag_mcp::bootstrap::OfflineRuntime {
        config,
        store,
        embedder,
    } = rag_mcp::bootstrap::open_offline(args.db)?;

    eprintln!(
        "ingest_file: path={} wing={:?} room={:?} db={}",
        args.path.display(),
        args.wing,
        args.room,
        config.db_path.display()
    );

    let result = IngestService::new(&store, &embedder, &config)
        .ingest_file(IngestFileCommand {
            path: args.path.display().to_string(),
            title: None,
            uri: None,
            metadata_json: None,
            wing: args.wing,
            room: args.room,
        })
        .await
        .context("ingest")?;

    store
        .maintenance()
        .ensure_fts(&config.fts_stemmer)
        .context("refresh lexical index")?;
    store
        .maintenance()
        .checkpoint()
        .context("checkpoint DuckDB")?;

    println!(
        "{}",
        serde_json::json!({
            "document_id": result.document_id,
            "chunk_count": result.chunk_count,
            "node_id": result.node_id,
            "edge_count": result.edge_count,
            "content_hash": result.content_hash,
            "op": result.op,
            "revision": result.revision,
            "etag": result.etag,
        })
    );
    Ok(())
}
