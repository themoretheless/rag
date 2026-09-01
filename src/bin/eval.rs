//! Versioned retrieval evaluation for the existing lex / vec / hybrid paths.

use anyhow::{bail, Context, Result};
use rag_mcp::db::search::{search, SearchQuery};
use rag_mcp::embeddings::{build_provider, EmbeddingProvider};
use rag_mcp::models::{SearchHit, SearchMode};
use rag_mcp::wiki;
use rag_mcp::{Config, Store};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

const VERSION: u32 = 1;
const DEFAULT_DATASET: &str = "data/eval/example-v1.json";
const ANN_CHUNKS: u64 = 50_000;
const ANN_P95_MS: f64 = 200.0;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Dataset {
    version: u32,
    name: String,
    corpus: Vec<String>,
    queries: Vec<LabeledQuery>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LabeledQuery {
    id: String,
    query: String,
    relevant: Vec<Label>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Label {
    document_title: String,
    relevance: u32,
}

#[derive(Serialize)]
struct Report {
    dataset_version: u32,
    dataset_name: String,
    top_k: usize,
    corpus: CorpusDiagnostics,
    modes: Vec<ModeReport>,
    scale_recommendation: Recommendation,
}
#[derive(Serialize)]
struct CorpusDiagnostics {
    documents: u64,
    chunks: u64,
    ingest_ms: f64,
}
#[derive(Serialize)]
struct ModeReport {
    mode: String,
    recall_at_k: f64,
    mrr: f64,
    ndcg_at_k: f64,
    mean_search_ms: f64,
    p95_search_ms: f64,
    queries: Vec<QueryReport>,
}
#[derive(Serialize)]
struct QueryReport {
    id: String,
    recall_at_k: f64,
    reciprocal_rank: f64,
    ndcg_at_k: f64,
    embedding_ms: f64,
    search_ms: f64,
    results: Vec<ResultItem>,
}
#[derive(Serialize)]
struct ResultItem {
    rank: usize,
    document_title: String,
    relevance: u32,
    score: f32,
}
#[derive(Serialize)]
struct Recommendation {
    path: String,
    reason: String,
    chunk_threshold: u64,
    p95_search_ms_threshold: f64,
}

struct Args {
    root: PathBuf,
    dataset: PathBuf,
    top_k: usize,
    modes: Vec<SearchMode>,
    json: bool,
    min_recall_at_k: Option<f64>,
    min_mrr: Option<f64>,
    max_p95_ms: Option<f64>,
    feedback_jsonl: Option<PathBuf>,
    history_jsonl: Option<PathBuf>,
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
    tokio::runtime::Runtime::new()?.block_on(run(args))
}

fn parse_args() -> Result<Args> {
    let mut root = PathBuf::from(".");
    let mut dataset = None;
    let mut top_k = 5;
    let mut modes = vec![SearchMode::Lex, SearchMode::Vec, SearchMode::Hybrid];
    let mut json = false;
    let mut min_recall_at_k: Option<f64> = None;
    let mut min_mrr: Option<f64> = None;
    let mut max_p95_ms: Option<f64> = None;
    let mut feedback_jsonl = None;
    let mut history_jsonl = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(it.next().context("--root needs path")?),
            "--dataset" | "--golden" => {
                dataset = Some(PathBuf::from(it.next().context("--dataset needs path")?))
            }
            "--top-k" => top_k = it.next().context("--top-k needs value")?.parse()?,
            "--modes" => {
                modes = it
                    .next()
                    .context("--modes needs csv (lex,vec,hybrid)")?
                    .split(',')
                    .map(|m| SearchMode::parse(m.trim()))
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(anyhow::Error::msg)?
            }
            "--json" => json = true,
            "--min-recall-at-k" => {
                min_recall_at_k = Some(
                    it.next()
                        .context("--min-recall-at-k needs value")?
                        .parse()?,
                )
            }
            "--min-mrr" => min_mrr = Some(it.next().context("--min-mrr needs value")?.parse()?),
            "--max-p95-ms" => {
                max_p95_ms = Some(it.next().context("--max-p95-ms needs value")?.parse()?)
            }
            "--feedback-jsonl" => {
                feedback_jsonl = Some(PathBuf::from(
                    it.next().context("--feedback-jsonl needs path")?,
                ))
            }
            "--history-jsonl" => {
                history_jsonl = Some(PathBuf::from(
                    it.next().context("--history-jsonl needs path")?,
                ))
            }
            "-h" | "--help" => {
                eprintln!(
                    "Usage: eval [--root DIR] [--dataset FILE.json] [--top-k N]\n\
                     \t[--modes lex,vec,hybrid] [--json] [--min-recall-at-k N]\n\
                     \t[--min-mrr N] [--max-p95-ms N] [--feedback-jsonl FILE]\n\
                     \t[--history-jsonl FILE]\n\
                     Uses a throwaway database. --golden remains a --dataset alias."
                );
                std::process::exit(0);
            }
            other => bail!("unknown arg: {other} (try --help)"),
        }
    }
    if top_k == 0 {
        bail!("--top-k must be greater than zero");
    }
    let root = root.canonicalize().unwrap_or(root);
    for (name, value) in [("min-recall-at-k", min_recall_at_k), ("min-mrr", min_mrr)] {
        if value.is_some_and(|v| !(0.0..=1.0).contains(&v)) {
            bail!("--{name} must be between 0 and 1");
        }
    }
    if max_p95_ms.is_some_and(|v| !v.is_finite() || v <= 0.0) {
        bail!("--max-p95-ms must be greater than zero");
    }
    Ok(Args {
        dataset: dataset.unwrap_or_else(|| root.join(DEFAULT_DATASET)),
        root,
        top_k,
        modes,
        json,
        min_recall_at_k,
        min_mrr,
        max_p95_ms,
        feedback_jsonl,
        history_jsonl,
    })
}

async fn run(args: Args) -> Result<()> {
    let dataset = load_dataset(&args.dataset)?;
    let db_path = std::env::temp_dir().join(format!("rag-eval-{}.duckdb", std::process::id()));
    let _ = std::fs::remove_file(&db_path);
    let result = evaluate(&args, dataset, &db_path).await;
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("duckdb.wal"));
    let report = result?;
    if let Some(path) = &args.feedback_jsonl {
        append_feedback(path, &report)?;
    }
    if let Some(path) = &args.history_jsonl {
        append_jsonl(path, &report)?;
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }
    for mode in &report.modes {
        if args
            .min_recall_at_k
            .is_some_and(|min| mode.recall_at_k < min)
        {
            bail!(
                "{} recall@{} {:.4} is below threshold {:.4}",
                mode.mode,
                args.top_k,
                mode.recall_at_k,
                args.min_recall_at_k.unwrap()
            );
        }
        if args.min_mrr.is_some_and(|min| mode.mrr < min) {
            bail!(
                "{} MRR {:.4} is below threshold {:.4}",
                mode.mode,
                mode.mrr,
                args.min_mrr.unwrap()
            );
        }
        if args.max_p95_ms.is_some_and(|max| mode.p95_search_ms > max) {
            bail!(
                "{} p95 {:.2}ms exceeds threshold {:.2}ms",
                mode.mode,
                mode.p95_search_ms,
                args.max_p95_ms.unwrap()
            );
        }
    }
    Ok(())
}

fn append_jsonl(path: &Path, value: &impl Serialize) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open JSONL output {}", path.display()))?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn append_feedback(path: &Path, report: &Report) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open feedback output {}", path.display()))?;
    for mode in &report.modes {
        for query in &mode.queries {
            serde_json::to_writer(
                &mut file,
                &serde_json::json!({
                    "dataset": report.dataset_name, "mode": mode.mode, "top_k": report.top_k,
                    "query_id": query.id, "recall_at_k": query.recall_at_k,
                    "reciprocal_rank": query.reciprocal_rank, "ndcg_at_k": query.ndcg_at_k,
                    "results": query.results,
                }),
            )?;
            file.write_all(b"\n")?;
        }
    }
    Ok(())
}

async fn evaluate(args: &Args, dataset: Dataset, db_path: &Path) -> Result<Report> {
    let mut config = Config::from_env()?;
    config.db_path = db_path.to_path_buf();
    let store = Store::open(&config.db_path)?;
    store.ensure_embedding_manifest(&config)?;
    let embedder: Arc<dyn EmbeddingProvider> = build_provider(&config)?;
    let started = Instant::now();
    for relative in &dataset.corpus {
        let path = args.root.join(relative);
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let title = path
            .file_name()
            .and_then(|n| n.to_str())
            .context("non-UTF-8 corpus filename")?
            .to_string();
        wiki::ingest_raw(
            &store,
            &embedder,
            &config,
            text,
            Some(title),
            Some(format!("eval://{relative}")),
            None,
            None,
            Some(path.display().to_string()),
        )
        .await
        .with_context(|| format!("ingest {relative}"))?;
    }
    let ingest_ms = ms(started);
    let (documents, chunks, _, _) = store.stats()?;
    let mut modes = Vec::new();
    for mode in &args.modes {
        modes.push(
            eval_mode(
                &store,
                &embedder,
                &config,
                &dataset.queries,
                *mode,
                args.top_k,
            )
            .await?,
        );
    }
    let worst_p95 = modes
        .iter()
        .filter(|m| m.mode != "lex")
        .map(|m| m.p95_search_ms)
        .fold(0.0, f64::max);
    Ok(Report {
        dataset_version: dataset.version,
        dataset_name: dataset.name,
        top_k: args.top_k,
        corpus: CorpusDiagnostics {
            documents,
            chunks,
            ingest_ms,
        },
        modes,
        scale_recommendation: recommend(chunks, worst_p95),
    })
}

fn load_dataset(path: &Path) -> Result<Dataset> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read dataset {}", path.display()))?;
    let data: Dataset =
        serde_json::from_str(&raw).with_context(|| format!("parse dataset {}", path.display()))?;
    if data.version != VERSION {
        bail!(
            "unsupported dataset version {}; expected {VERSION}",
            data.version
        );
    }
    if data.corpus.is_empty() || data.queries.is_empty() {
        bail!("dataset corpus and queries must not be empty");
    }
    for q in &data.queries {
        if q.id.trim().is_empty() || q.query.trim().is_empty() || q.relevant.is_empty() {
            bail!("each query needs a non-empty id, query, and relevant labels");
        }
        if q.relevant.iter().any(|l| l.relevance == 0) {
            bail!(
                "query '{}' contains relevance 0; omit non-relevant documents",
                q.id
            );
        }
        let unique: HashSet<_> = q.relevant.iter().map(|l| &l.document_title).collect();
        if unique.len() != q.relevant.len() {
            bail!("query '{}' contains duplicate document labels", q.id);
        }
    }
    Ok(data)
}

async fn eval_mode(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    queries: &[LabeledQuery],
    mode: SearchMode,
    top_k: usize,
) -> Result<ModeReport> {
    let mut reports = Vec::new();
    for q in queries {
        let embedding_started = Instant::now();
        let query_embedding = if mode.needs_embedding() {
            Some(embedder.embed(&[q.query.clone()]).await?.remove(0))
        } else {
            None
        };
        let embedding_ms = ms(embedding_started);
        let search_started = Instant::now();
        let hits = search(
            store,
            &SearchQuery {
                mode,
                top_k,
                query_text: Some(q.query.clone()),
                query_embedding,
                fts_stemmer: config.fts_stemmer.clone(),
                ..SearchQuery::default()
            },
        )?;
        reports.push(score(q, hits, embedding_ms, ms(search_started), top_k));
    }
    let n = reports.len() as f64;
    let mut timings: Vec<_> = reports.iter().map(|q| q.search_ms).collect();
    timings.sort_by(f64::total_cmp);
    Ok(ModeReport {
        mode: mode.as_str().into(),
        recall_at_k: reports.iter().map(|q| q.recall_at_k).sum::<f64>() / n,
        mrr: reports.iter().map(|q| q.reciprocal_rank).sum::<f64>() / n,
        ndcg_at_k: reports.iter().map(|q| q.ndcg_at_k).sum::<f64>() / n,
        mean_search_ms: timings.iter().sum::<f64>() / n,
        p95_search_ms: percentile(&timings),
        queries: reports,
    })
}

fn score(
    query: &LabeledQuery,
    hits: Vec<SearchHit>,
    embedding_ms: f64,
    search_ms: f64,
    top_k: usize,
) -> QueryReport {
    let labels: HashMap<&str, u32> = query
        .relevant
        .iter()
        .map(|l| (l.document_title.as_str(), l.relevance))
        .collect();
    let mut seen = HashSet::new();
    let results: Vec<_> = hits
        .into_iter()
        .take(top_k)
        .enumerate()
        .map(|(i, h)| ResultItem {
            rank: i + 1,
            relevance: if seen.insert(h.document_title.clone()) {
                labels.get(h.document_title.as_str()).copied().unwrap_or(0)
            } else {
                0
            },
            document_title: h.document_title,
            score: h.score,
        })
        .collect();
    let reciprocal_rank = results
        .iter()
        .find(|h| h.relevance > 0)
        .map(|h| 1.0 / h.rank as f64)
        .unwrap_or(0.0);
    let dcg = gain(results.iter().map(|h| h.relevance));
    let mut ideal: Vec<_> = query.relevant.iter().map(|l| l.relevance).collect();
    ideal.sort_unstable_by(|a, b| b.cmp(a));
    ideal.truncate(top_k);
    let idcg = gain(ideal.into_iter());
    QueryReport {
        id: query.id.clone(),
        recall_at_k: results.iter().filter(|h| h.relevance > 0).count() as f64
            / query.relevant.len() as f64,
        reciprocal_rank,
        ndcg_at_k: if idcg == 0.0 { 0.0 } else { dcg / idcg },
        embedding_ms,
        search_ms,
        results,
    }
}

fn gain(values: impl Iterator<Item = u32>) -> f64 {
    values
        .enumerate()
        .map(|(i, r)| (2_f64.powi(r as i32) - 1.0) / ((i + 2) as f64).log2())
        .sum()
}
fn percentile(sorted: &[f64]) -> f64 {
    sorted[((sorted.len() - 1) as f64 * 0.95).ceil() as usize]
}
fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

fn recommend(chunks: u64, p95: f64) -> Recommendation {
    let full_scan = chunks <= ANN_CHUNKS && p95 <= ANN_P95_MS;
    Recommendation {
        path: if full_scan {
            "full_scan"
        } else {
            "investigate_future_ann"
        }
        .into(),
        reason: format!(
            "{} chunks and {:.2} ms worst vector/hybrid p95; profile before any backend change",
            chunks, p95
        ),
        chunk_threshold: ANN_CHUNKS,
        p95_search_ms_threshold: ANN_P95_MS,
    }
}

fn print_report(r: &Report) {
    println!(
        "dataset={} version={} top_k={} corpus={} docs/{} chunks ingest={:.2}ms",
        r.dataset_name,
        r.dataset_version,
        r.top_k,
        r.corpus.documents,
        r.corpus.chunks,
        r.corpus.ingest_ms
    );
    for mode in &r.modes {
        println!(
            "\n{} recall@{}={:.3} MRR={:.3} nDCG@{}={:.3} search_mean={:.2}ms search_p95={:.2}ms",
            mode.mode,
            r.top_k,
            mode.recall_at_k,
            mode.mrr,
            r.top_k,
            mode.ndcg_at_k,
            mode.mean_search_ms,
            mode.p95_search_ms
        );
        for q in &mode.queries {
            println!(
                "  {} recall={:.3} rr={:.3} ndcg={:.3} embed={:.2}ms search={:.2}ms",
                q.id, q.recall_at_k, q.reciprocal_rank, q.ndcg_at_k, q.embedding_ms, q.search_ms
            );
            for h in &q.results {
                println!(
                    "    {:>2}. rel={} score={:.4} {}",
                    h.rank, h.relevance, h.score, h.document_title
                );
            }
        }
    }
    println!(
        "\nscale={} — {}",
        r.scale_recommendation.path, r.scale_recommendation.reason
    );
}
