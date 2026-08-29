//! Single-file ingest CLI (parity with MCP `ingest_file`).
//!
//! Example (from rag repo root):
//! ```bash
//! RAG_INGEST_ROOTS=/path/to/allowed RAG_EMBEDDING_PROVIDER=mock \
//!   cargo run --release --bin ingest_file -- \
//!   --path /path/to/allowed/notes.md --wing projects --room notes --db ./rag.duckdb
//! ```

use anyhow::{bail, Context, Result};
use rag_mcp::embeddings::{build_provider, EmbeddingProvider};
use rag_mcp::wiki;
use rag_mcp::file_ingest::extract_file;
use rag_mcp::{check_path_allowlist, Config, Store};
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
    db: Option<PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut path = None;
    let mut wing = None;
    let mut room = None;
    let mut db = None;
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
            "-h" | "--help" => {
                eprintln!(
                    "Usage: ingest_file --path FILE [--wing WING] [--room ROOM] [--db PATH]\n\
                     Read text, Markdown, HTML, PDF, or source code and upsert into DuckDB.\n\
                     Path must be under RAG_INGEST_ROOTS (same as MCP ingest_file).\n\
                     Env: RAG_DB_PATH (overridden by --db), RAG_INGEST_ROOTS, RAG_EMBEDDING_*"
                );
                std::process::exit(0);
            }
            other => bail!("unknown arg: {other} (try --help)"),
        }
    }
    let path = path.context("required: --path FILE")?;
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
    if let Some(db) = args.db {
        config.db_path = db;
    }

    check_path_allowlist(&args.path, &config.ingest_roots)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("RAG_INGEST_ROOTS allowlist")?;

    let store = Store::open(&config.db_path).context("open DuckDB")?;
    let _ = store.ensure_embedding_manifest(&config);
    let embedder: Arc<dyn EmbeddingProvider> =
        build_provider(&config).context("embedding provider")?;

    let canon = args
        .path
        .canonicalize()
        .unwrap_or_else(|_| args.path.clone());
    let uri = format!("file://{}", canon.display());
    let title = args
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());
    let source_file = Some(canon.display().to_string());

    let extracted = extract_file(&args.path).with_context(|| format!("extract {}", args.path.display()))?;

    eprintln!(
        "ingest_file: path={} wing={:?} room={:?} db={} uri={}",
        args.path.display(),
        args.wing,
        args.room,
        config.db_path.display(),
        uri
    );

    let result = wiki::ingest_raw(
        &store,
        &embedder,
        &config,
        extracted.text,
        title,
        Some(uri),
        args.wing,
        args.room,
        source_file,
    )
    .await
    .context("ingest")?;

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
