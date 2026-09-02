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

use anyhow::{bail, Context, Result};
use rag_mcp::embeddings::{build_provider, EmbeddingProvider};
use rag_mcp::ingest::{IngestFileCommand, IngestService};
use rag_mcp::{Config, Store};
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

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

fn parse_args() -> Result<Args> {
    let mut path = None;
    let mut wing = None;
    let mut room = None;
    let mut db = None;
    let mut offline = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--path" => {
                path = Some(PathBuf::from(it.next().context("--path needs value")?));
            }
            "--wing" => {
                wing = Some(it.next().context("--wing needs value")?);
            }
            "--room" => {
                room = Some(it.next().context("--room needs value")?);
            }
            "--db" => {
                db = Some(PathBuf::from(it.next().context("--db needs path")?));
            }
            "--offline" => offline = true,
            "-h" | "--help" => {
                eprintln!(
                    "Usage: ingest_file --offline --db PATH --path FILE [--wing WING] [--room ROOM]\n\
                     Offline-only direct DuckDB writer. Stop the rag-mcp gateway first.\n\
                     For a live database, use the MCP/HTTP ingest_file operation.\n\
                     Reads text, Markdown, HTML, PDF, or source code.\n\
                     Path must be under RAG_INGEST_ROOTS (same as MCP ingest_file).\n\
                     Env: RAG_INGEST_ROOTS, RAG_EMBEDDING_*"
                );
                std::process::exit(0);
            }
            other => bail!("unknown arg: {other} (try --help)"),
        }
    }
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
    let mut config = Config::from_env().context("Config::from_env")?;
    config.db_path = args.db;

    let store = Store::open(&config.db_path).context("open DuckDB")?;
    store
        .ensure_embedding_manifest(&config)
        .context("validate embedding manifest")?;
    let embedder: Arc<dyn EmbeddingProvider> =
        build_provider(&config).context("embedding provider")?;

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
        .ensure_fts(&config.fts_stemmer)
        .context("refresh lexical index")?;
    store.checkpoint().context("checkpoint DuckDB")?;

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
