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

use anyhow::{bail, Context, Result};
use rag_mcp::embeddings::{build_provider, EmbeddingProvider};
use rag_mcp::ingest::{IngestCommand, IngestService};
use rag_mcp::source_scan::{collect_source_files, SourceScanPolicy};
use rag_mcp::{Config, Store};
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> Result<()> {
    // stderr logging only (same convention as rag-mcp)
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
    root: PathBuf,
    wing: String,
    room: String,
    max_bytes: u64,
    dry_run: bool,
    exts: Option<Vec<String>>,
    db: PathBuf,
}

fn parse_args() -> Result<Args> {
    let mut root = None;
    let mut wing = "projects".to_string();
    let mut room = "default".to_string();
    let mut max_bytes = 512 * 1024u64;
    let mut dry_run = false;
    let mut exts: Option<Vec<String>> = None;
    let mut db = None;
    let mut offline = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--root" => {
                root = Some(PathBuf::from(it.next().context("--root needs path")?));
            }
            "--wing" => {
                wing = it.next().context("--wing needs value")?;
            }
            "--room" => {
                room = it.next().context("--room needs value")?;
            }
            "--max-bytes" => {
                max_bytes = it
                    .next()
                    .context("--max-bytes needs value")?
                    .parse()
                    .context("max-bytes")?;
            }
            "--ext" => {
                let list = it.next().context("--ext needs csv")?;
                exts = Some(
                    list.split(',')
                        .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
                        .filter(|s| !s.is_empty())
                        .collect(),
                );
            }
            "--db" => {
                db = Some(PathBuf::from(it.next().context("--db needs path")?));
            }
            "--offline" => offline = true,
            "--dry-run" => dry_run = true,
            "-h" | "--help" => {
                eprintln!(
                    "Usage: ingest_project --offline --db PATH --root DIR \\\n\
                     \t[--wing projects] [--room NAME] [--max-bytes N] \\\n\
                     \t[--ext rs,md,toml] [--dry-run]\n\
                     Offline-only direct DuckDB writer. Stop the rag-mcp gateway first.\n\
                     For a live database, use the MCP/HTTP sync_sources operation.\n\
                     Env: RAG_EMBEDDING_* (same as rag-mcp)"
                );
                std::process::exit(0);
            }
            other => bail!("unknown arg: {other} (try --help)"),
        }
    }
    let root = root.context("required: --root DIR")?;
    let db = db.context(
        "required: --db PATH; this offline tool never inherits RAG_DB_PATH or a default database",
    )?;
    if !offline {
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
        wing,
        room,
        max_bytes,
        dry_run,
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

    let mut config = Config::from_env().context("Config::from_env")?;
    config.db_path = args.db;
    let store = Store::open(&config.db_path).context("open DuckDB")?;
    store
        .ensure_embedding_manifest(&config)
        .context("validate embedding manifest")?;
    let embedder: Arc<dyn EmbeddingProvider> =
        build_provider(&config).context("embedding provider")?;

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
        .ensure_fts(&config.fts_stemmer)
        .context("refresh lexical index")?;
    store.checkpoint().context("checkpoint DuckDB")?;

    eprintln!(
        "done: ok={ok} skipped={skipped} failed={failed} wing={} room={}",
        args.wing, args.room
    );
    if failed > 0 {
        std::process::exit(2);
    }
    Ok(())
}
