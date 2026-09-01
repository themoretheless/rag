//! Bulk-ingest a project tree into the rag-mcp DuckDB store.
//!
//! Example (from rag repo root):
//! ```bash
//! RAG_DB_PATH=./rag.duckdb RAG_EMBEDDING_PROVIDER=mock \
//!   cargo run --release --bin ingest_project -- \
//!   --root /path/to/downloader --wing projects --room downloader
//! ```

use anyhow::{bail, Context, Result};
use rag_mcp::embeddings::{build_provider, EmbeddingProvider};
use rag_mcp::source_scan::{collect_source_files, SourceScanPolicy};
use rag_mcp::wiki;
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
}

fn parse_args() -> Result<Args> {
    let mut root = None;
    let mut wing = "projects".to_string();
    let mut room = "default".to_string();
    let mut max_bytes = 512 * 1024u64;
    let mut dry_run = false;
    let mut exts: Option<Vec<String>> = None;
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
            "--dry-run" => dry_run = true,
            "-h" | "--help" => {
                eprintln!(
                    "Usage: ingest_project --root DIR [--wing projects] [--room NAME] \\\n\
                     \t[--max-bytes N] [--ext rs,md,toml] [--dry-run]\n\
                     Env: RAG_DB_PATH, RAG_EMBEDDING_* (same as rag-mcp)"
                );
                std::process::exit(0);
            }
            other => bail!("unknown arg: {other} (try --help)"),
        }
    }
    let root = root.context("required: --root DIR")?;
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
    })
}

async fn run(args: Args) -> Result<()> {
    let config = Config::from_env().context("Config::from_env")?;
    let store = Store::open(&config.db_path).context("open DuckDB")?;
    let _ = store.ensure_embedding_manifest(&config);
    let embedder: Arc<dyn EmbeddingProvider> =
        build_provider(&config).context("embedding provider")?;

    eprintln!(
        "ingest_project: root={} wing={} room={} db={} provider={:?} dry_run={}",
        args.root.display(),
        args.wing,
        args.room,
        config.db_path.display(),
        config.embedding_provider,
        args.dry_run
    );

    let mut policy = SourceScanPolicy::default().with_max_bytes(args.max_bytes);
    if let Some(exts) = &args.exts {
        policy = policy.with_extensions(exts);
    }
    let files = collect_source_files(&args.root, &policy)?;
    eprintln!("files to ingest: {}", files.len());

    let mut ok = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;

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

        if args.dry_run {
            eprintln!("  [dry-run] {uri}");
            ok += 1;
            continue;
        }

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

        // Prefer add_drawer path via ingest_raw + metadata: wing/room set on document
        match wiki::ingest_raw(
            &store,
            &embedder,
            &config,
            text,
            Some(format!("{}/{}", args.room, rel)),
            Some(uri.clone()),
            Some(args.wing.clone()),
            Some(args.room.clone()),
            Some(path.display().to_string()),
        )
        .await
        {
            Ok(r) => {
                // patch metadata_json / title if needed via store get+upsert
                if let Ok(Some(mut doc)) = store.get_document(&r.document_id) {
                    doc.metadata_json = meta;
                    doc.title = title;
                    let _ = store.upsert_document(&doc);
                }
                eprintln!("  OK {} chunks={} id={}", rel, r.chunk_count, r.document_id);
                ok += 1;
            }
            Err(e) => {
                eprintln!("  FAIL {rel}: {e}");
                failed += 1;
            }
        }
    }

    eprintln!(
        "done: ok={ok} skipped={skipped} failed={failed} wing={} room={}",
        args.wing, args.room
    );
    if failed > 0 {
        std::process::exit(2);
    }
    Ok(())
}
