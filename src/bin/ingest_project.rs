//! Offline bulk-ingest of a project tree into a rag-mcp DuckDB store.
//!
//! This binary opens DuckDB directly. It therefore requires an explicit database
//! path and an `--offline` acknowledgement. Stop the rag-mcp gateway before using
//! it. For a live database, use the MCP/HTTP `sync_sources` operation instead.
//!
//! Example (from rag repo root):
//! ```bash
//! RAG_EMBEDDING_PROVIDER=mock \
//!   cargo run --release --bin ingest_project -- \
//!   --offline --db ./rag.duckdb --root /path/to/downloader \
//!   --wing projects --room downloader
//! ```

use anyhow::{Context, Result, bail};
use rag_mcp::ingest::{IngestCommand, IngestService};
use rag_mcp::source_scan::{SourceScanPolicy, collect_source_files};
use std::path::PathBuf;

fn main() -> Result<()> {
    // stderr logging only (same convention as rag-mcp)
    rag_mcp::bootstrap::init_tracing("info");

    let args = parse_args()?;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run(args))
}

/// Offline-only direct DuckDB writer. Stop the rag-mcp gateway first.
/// For a live database, use the MCP/HTTP sync_sources operation.
/// Env: RAG_EMBEDDING_* (same as rag-mcp).
#[derive(clap::Parser)]
#[command(name = "ingest_project")]
struct Cli {
    /// Project directory to scan.
    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,
    #[arg(long, default_value = "projects")]
    wing: String,
    #[arg(long, value_name = "NAME", default_value = "default")]
    room: String,
    /// Skip files larger than N bytes.
    #[arg(long, value_name = "N", default_value_t = 512 * 1024)]
    max_bytes: u64,
    /// Comma-separated extensions, e.g. rs,md,toml.
    #[arg(long, value_name = "CSV")]
    ext: Option<String>,
    /// Database path; this tool never inherits RAG_DB_PATH or a default.
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,
    /// Acknowledge direct DuckDB access (gateway stopped).
    #[arg(long)]
    offline: bool,
    /// List the files without opening the database.
    #[arg(long)]
    dry_run: bool,
}

struct Args {
    root: PathBuf,
    wing: String,
    room: String,
    max_bytes: u64,
    dry_run: bool,
    exts: Option<Vec<String>>,
    db: PathBuf,
}

fn parse_args() -> Result<Args> {
    let cli = <Cli as clap::Parser>::parse();
    let exts = cli.ext.map(|list| {
        list.split(',')
            .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    });
    let root = cli.root.context("required: --root DIR")?;
    let db = cli.db.context(
        "required: --db PATH; this offline tool never inherits RAG_DB_PATH or a default database",
    )?;
    if !cli.offline {
        bail!(
            "refusing direct DuckDB access without --offline; stop the rag-mcp gateway first, \
             then pass --offline, or use the MCP/HTTP sync_sources operation"
        );
    }
    if !root.is_dir() {
        bail!("--root is not a directory: {}", root.display());
    }
    Ok(Args {
        root: root.canonicalize().unwrap_or(root),
        wing: cli.wing,
        room: cli.room,
        max_bytes: cli.max_bytes,
        dry_run: cli.dry_run,
        exts,
        db,
    })
}

async fn run(args: Args) -> Result<()> {
    let mut policy = SourceScanPolicy::default().with_max_bytes(args.max_bytes);
    if let Some(exts) = &args.exts {
        policy = policy.with_extensions(exts);
    }
    let files = collect_source_files(&args.root, &policy)?;
    eprintln!("files to ingest: {}", files.len());

    if args.dry_run {
        for path in &files {
            let rel = path
                .strip_prefix(&args.root)
                .unwrap_or(path.as_path())
                .display()
                .to_string();
            eprintln!(
                "  [dry-run] project://{}/{}/{}",
                args.wing,
                args.room,
                rel.replace('\\', "/")
            );
        }
        eprintln!(
            "done: ok={} skipped=0 failed=0 wing={} room={} (dry-run; database not opened)",
            files.len(),
            args.wing,
            args.room
        );
        return Ok(());
    }

    let rag_mcp::bootstrap::OfflineRuntime {
        config,
        store,
        embedder,
    } = rag_mcp::bootstrap::open_offline(args.db)?;

    eprintln!(
        "ingest_project: root={} wing={} room={} db={} provider={:?}",
        args.root.display(),
        args.wing,
        args.room,
        config.db_path.display(),
        config.embedding_provider
    );

    let mut ok = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;
    let ingest = IngestService::new(&store, &embedder, &config);

    for path in &files {
        let rel = path
            .strip_prefix(&args.root)
            .unwrap_or(path.as_path())
            .display()
            .to_string();
        let title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let uri = format!(
            "project://{}/{}/{}",
            args.wing,
            args.room,
            rel.replace('\\', "/")
        );
        let meta = serde_json::json!({
            "project": args.room,
            "wing": args.wing,
            "room": args.room,
            "rel": rel,
        })
        .to_string();

        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("  SKIP read {}: {e}", path.display());
                skipped += 1;
                continue;
            }
        };
        if text.chars().any(|c| c == '\0') {
            eprintln!("  SKIP binary-ish {}", path.display());
            skipped += 1;
            continue;
        }

        match ingest
            .ingest(IngestCommand {
                text,
                title: Some(title),
                uri: Some(uri),
                metadata_json: Some(meta),
                wing: Some(args.wing.clone()),
                room: Some(args.room.clone()),
                source_file: Some(path.display().to_string()),
                layer: "raw".into(),
                kind: "document".into(),
                immutable: false,
            })
            .await
        {
            Ok(r) => {
                eprintln!("  OK {} chunks={} id={}", rel, r.chunk_count, r.document_id);
                ok += 1;
            }
            Err(e) => {
                eprintln!("  FAIL {rel}: {e}");
                failed += 1;
            }
        }
    }

    store
        .maintenance()
        .ensure_fts(&config.fts_stemmer)
        .context("refresh lexical index")?;
    store
        .maintenance()
        .checkpoint()
        .context("checkpoint DuckDB")?;

    eprintln!(
        "done: ok={ok} skipped={skipped} failed={failed} wing={} room={}",
        args.wing, args.room
    );
    if failed > 0 {
        std::process::exit(2);
    }
    Ok(())
}
