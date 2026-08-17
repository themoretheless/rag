//! Retrieval eval harness: golden Q&A set → recall@k / MRR per search mode.
//!
//! Builds a throwaway DuckDB store, ingests the referenced docs with the
//! configured embedding provider, then answers every golden question in
//! `lex` / `vec` / `hybrid` mode and scores document-level relevance.
//!
//! Example (from rag repo root):
//! ```bash
//! RAG_EMBEDDING_PROVIDER=mock cargo run --release --bin eval
//! RAG_EMBEDDING_PROVIDER=ollama RAG_EMBEDDING_DIMS=768 cargo run --release --bin eval
//! ```

use anyhow::{bail, Context, Result};
use rag_mcp::db::search::{search, SearchQuery};
use rag_mcp::embeddings::{build_provider, EmbeddingProvider};
use rag_mcp::models::SearchMode;
use rag_mcp::wiki;
use rag_mcp::{Config, Store};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Docs the default golden set references, relative to `--root`.
const DEFAULT_DOCS: &[&str] = &[
    "README.md",
    "SPEC.md",
    "FEATURES.md",
    "docs/CONNECT.md",
    "docs/SPINE_TOOLS.md",
    "docs/ARCHITECTURE_VISION.md",
    "docs/ROADMAP.md",
    "docs/PROD_RUN.md",
];

#[derive(Debug, Deserialize)]
struct GoldenItem {
    question: String,
    /// Substring the correct hit's `document_title` must contain.
    expect_title: String,
}

struct Args {
    root: PathBuf,
    golden: PathBuf,
    top_k: usize,
    modes: Vec<SearchMode>,
    verbose: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let args = parse_args()?;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run(args))
}

fn parse_args() -> Result<Args> {
    let mut root = PathBuf::from(".");
    let mut golden = None;
    let mut top_k = 5usize;
    let mut modes = vec![SearchMode::Lex, SearchMode::Vec, SearchMode::Hybrid];
    let mut verbose = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--root" => root = PathBuf::from(it.next().context("--root needs path")?),
            "--golden" => golden = Some(PathBuf::from(it.next().context("--golden needs path")?)),
            "--top-k" => top_k = it.next().context("--top-k needs value")?.parse()?,
            "--modes" => {
                let list = it.next().context("--modes needs csv (lex,vec,hybrid)")?;
                modes = list
                    .split(',')
                    .map(|s| SearchMode::parse(s.trim()))
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(anyhow::Error::msg)?;
            }
            "--verbose" => verbose = true,
            "-h" | "--help" => {
                eprintln!(
                    "Usage: eval [--root DIR] [--golden FILE.jsonl] [--top-k N] \\\n\
                     \t[--modes lex,vec,hybrid] [--verbose]\n\
                     Env: RAG_EMBEDDING_* selects the provider under test (mock|openai|ollama).\n\
                     Uses a throwaway DB; never touches RAG_DB_PATH."
                );
                std::process::exit(0);
            }
            other => bail!("unknown arg: {other} (try --help)"),
        }
    }
    let root = root.canonicalize().unwrap_or(root);
    let golden = golden.unwrap_or_else(|| root.join("data/eval/golden.jsonl"));
    Ok(Args {
        root,
        golden,
        top_k,
        modes,
        verbose,
    })
}

async fn run(args: Args) -> Result<()> {
    let golden = load_golden(&args.golden)?;
    if golden.is_empty() {
        bail!("golden set is empty: {}", args.golden.display());
    }

    // Throwaway DB so eval never fights the live server for the file lock.
    let db_path = std::env::temp_dir().join(format!("rag-eval-{}.duckdb", std::process::id()));
    let _ = std::fs::remove_file(&db_path);

    let mut config = Config::from_env().context("Config::from_env")?;
    config.db_path = db_path.clone();

    let store = Store::open(&config.db_path).context("open throwaway DuckDB")?;
    store.ensure_embedding_manifest(&config)?;
    let embedder: Arc<dyn EmbeddingProvider> =
        build_provider(&config).context("embedding provider")?;

    println!(
        "eval: provider={:?} model={} dims={} top_k={} golden={} ({} questions)",
        config.embedding_provider,
        config.embedding_model,
        config.embedding_dims,
        args.top_k,
        args.golden.display(),
        golden.len()
    );

    ingest_docs(&store, &embedder, &config, &args, &golden).await?;

    println!("\n{:<8} {:>10} {:>8} {:>7}", "mode", "recall@k", "mrr", "miss");
    for mode in &args.modes {
        let (recall, mrr, misses) =
            eval_mode(&store, &embedder, &config, &golden, *mode, args.top_k).await?;
        println!(
            "{:<8} {:>10.3} {:>8.3} {:>7}",
            mode.as_str(),
            recall,
            mrr,
            misses.len()
        );
        if args.verbose {
            for (q, expect) in &misses {
                println!("    MISS [{expect}] {q}");
            }
        }
    }

    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("duckdb.wal"));
    Ok(())
}

fn load_golden(path: &PathBuf) -> Result<Vec<GoldenItem>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read golden set {}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let item: GoldenItem = serde_json::from_str(line)
            .with_context(|| format!("{}:{} bad golden line", path.display(), i + 1))?;
        out.push(item);
    }
    Ok(out)
}

async fn ingest_docs(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    args: &Args,
    golden: &[GoldenItem],
) -> Result<()> {
    // Every doc the golden set expects must be present; DEFAULT_DOCS is the base corpus.
    let mut wanted: BTreeSet<String> = DEFAULT_DOCS.iter().map(|s| (*s).to_string()).collect();
    for g in golden {
        if !wanted.iter().any(|d| d.ends_with(&g.expect_title)) {
            eprintln!(
                "warning: golden expects '{}' which is not in the ingest list",
                g.expect_title
            );
        }
    }
    let mut total_chunks = 0usize;
    for rel in std::mem::take(&mut wanted) {
        let path = args.root.join(&rel);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("read doc {}", path.display()))?;
        let title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("doc")
            .to_string();
        let r = wiki::ingest_raw(
            store,
            embedder,
            config,
            text,
            Some(title),
            Some(format!("eval://{rel}")),
            None,
            None,
            Some(path.display().to_string()),
        )
        .await
        .with_context(|| format!("ingest {rel}"))?;
        total_chunks += r.chunk_count;
    }
    println!("ingested {} docs, {} chunks", DEFAULT_DOCS.len(), total_chunks);
    Ok(())
}

async fn eval_mode(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    golden: &[GoldenItem],
    mode: SearchMode,
    top_k: usize,
) -> Result<(f64, f64, Vec<(String, String)>)> {
    let mut hits = 0usize;
    let mut rr_sum = 0f64;
    let mut misses = Vec::new();

    for g in golden {
        let query_embedding = if mode.needs_embedding() {
            let mut vecs = embedder.embed(&[g.question.clone()]).await?;
            Some(vecs.remove(0))
        } else {
            None
        };
        let results = search(
            store,
            &SearchQuery {
                mode,
                top_k,
                query_text: Some(g.question.clone()),
                query_embedding,
                fts_stemmer: config.fts_stemmer.clone(),
                ..SearchQuery::default()
            },
        )?;
        let rank = results
            .iter()
            .position(|h| h.document_title.contains(&g.expect_title));
        match rank {
            Some(r) => {
                hits += 1;
                rr_sum += 1.0 / (r as f64 + 1.0);
            }
            None => misses.push((g.question.clone(), g.expect_title.clone())),
        }
    }

    let n = golden.len() as f64;
    Ok((hits as f64 / n, rr_sum / n, misses))
}
