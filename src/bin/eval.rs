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
// Retained in the JSON report as a historical scale marker, never as an ANN trigger.
const CHUNK_OBSERVATION_MARKER: u64 = 50_000;
const ANN_P95_MS: f64 = 300.0;
const SYNTHETIC_CHUNKS_PER_DOCUMENT: usize = 256;

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
    sampling: SamplingDiagnostics,
    resources: ResourceDiagnostics,
    modes: Vec<ModeReport>,
    scale_recommendation: Recommendation,
}
#[derive(Serialize)]
struct CorpusDiagnostics {
    documents: u64,
    chunks: u64,
    ingest_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    synthetic: Option<SyntheticDiagnostics>,
}
#[derive(Serialize)]
struct SyntheticDiagnostics {
    requested_chunks: usize,
    generated_documents: usize,
}
#[derive(Serialize)]
struct SamplingDiagnostics {
    queries: usize,
    warmup: usize,
    repeat: usize,
    samples_per_mode: usize,
}
#[derive(Serialize)]
struct ResourceDiagnostics {
    #[serde(skip_serializing_if = "Option::is_none")]
    rss_mib: Option<f64>,
}
#[derive(Serialize)]
struct ModeReport {
    mode: String,
    recall_at_k: f64,
    mrr: f64,
    ndcg_at_k: f64,
    mean_search_ms: f64,
    p95_search_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_throughput_qps: Option<f64>,
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
    /// Historical report field retained for JSON compatibility; observation only.
    chunk_threshold: u64,
    p95_search_ms_threshold: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    rss_mib_threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    throughput_qps_threshold: Option<f64>,
    failed_thresholds: Vec<String>,
}

#[derive(Clone, Copy)]
struct RecommendationThresholds {
    p95_search_ms: f64,
    rss_mib: Option<f64>,
    throughput_qps: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyntheticDocument {
    title: String,
    uri: String,
    content: String,
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
    max_rss_mib: Option<f64>,
    min_throughput_qps: Option<f64>,
    feedback_jsonl: Option<PathBuf>,
    history_jsonl: Option<PathBuf>,
    warmup: usize,
    repeat: usize,
    synthetic_chunks: Option<usize>,
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
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from<I, S>(args: I) -> Result<Args>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut root = PathBuf::from(".");
    let mut dataset = None;
    let mut top_k = 5;
    let mut modes = vec![SearchMode::Lex, SearchMode::Vec, SearchMode::Hybrid];
    let mut json = false;
    let mut min_recall_at_k: Option<f64> = None;
    let mut min_mrr: Option<f64> = None;
    let mut max_p95_ms: Option<f64> = None;
    let mut max_rss_mib: Option<f64> = None;
    let mut min_throughput_qps: Option<f64> = None;
    let mut feedback_jsonl = None;
    let mut history_jsonl = None;
    let mut warmup = 0;
    let mut repeat = 1;
    let mut synthetic_chunks = None;
    let mut it = args.into_iter().map(Into::into);
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
            "--max-rss-mib" => {
                max_rss_mib = Some(it.next().context("--max-rss-mib needs value")?.parse()?)
            }
            "--min-throughput-qps" => {
                min_throughput_qps = Some(
                    it.next()
                        .context("--min-throughput-qps needs value")?
                        .parse()?,
                )
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
            "--warmup" => warmup = it.next().context("--warmup needs value")?.parse()?,
            "--repeat" => repeat = it.next().context("--repeat needs value")?.parse()?,
            "--synthetic-chunks" => {
                synthetic_chunks = Some(
                    it.next()
                        .context("--synthetic-chunks needs value")?
                        .parse()?,
                )
            }
            "-h" | "--help" => {
                eprintln!(
                    "Usage: eval [--root DIR] [--dataset FILE.json] [--top-k N]\n\
                     \t[--modes lex,vec,hybrid] [--json] [--min-recall-at-k N]\n\
                     \t[--min-mrr N] [--max-p95-ms N] [--max-rss-mib N]\n\
                     \t[--min-throughput-qps N] [--feedback-jsonl FILE]\n\
                     \t[--history-jsonl FILE] [--warmup N] [--repeat N]\n\
                     \t[--synthetic-chunks N]\n\
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
    if repeat == 0 {
        bail!("--repeat must be greater than zero");
    }
    if synthetic_chunks == Some(0) {
        bail!("--synthetic-chunks must be greater than zero");
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
    if max_rss_mib.is_some_and(|v| !v.is_finite() || v <= 0.0) {
        bail!("--max-rss-mib must be greater than zero");
    }
    if min_throughput_qps.is_some_and(|v| !v.is_finite() || v <= 0.0) {
        bail!("--min-throughput-qps must be greater than zero");
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
        max_rss_mib,
        min_throughput_qps,
        feedback_jsonl,
        history_jsonl,
        warmup,
        repeat,
        synthetic_chunks,
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
        if mode.mode != "lex" {
            if let Some(minimum) = args.min_throughput_qps {
                let measured = mode.search_throughput_qps.with_context(|| {
                    format!("{} search throughput could not be measured", mode.mode)
                })?;
                if measured < minimum {
                    bail!(
                        "{} throughput {:.2} qps is below threshold {:.2} qps",
                        mode.mode,
                        measured,
                        minimum
                    );
                }
            }
        }
    }
    if let Some(maximum) = args.max_rss_mib {
        let measured = report
            .resources
            .rss_mib
            .context("RSS could not be measured on this platform; cannot enforce --max-rss-mib")?;
        if measured > maximum {
            bail!(
                "RSS {:.2} MiB exceeds threshold {:.2} MiB",
                measured,
                maximum
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

fn generate_synthetic_documents(
    requested_chunks: usize,
    chunk_size: usize,
    chunk_overlap: usize,
) -> Result<Vec<SyntheticDocument>> {
    if requested_chunks == 0 {
        return Ok(Vec::new());
    }
    if chunk_size == 0 || chunk_overlap >= chunk_size {
        bail!("synthetic generation needs chunk_size > 0 and chunk_overlap < chunk_size");
    }
    let step = chunk_size - chunk_overlap;
    let mut remaining = requested_chunks;
    let mut documents = Vec::new();
    while remaining > 0 {
        let document_index = documents.len();
        let document_chunks = remaining.min(SYNTHETIC_CHUNKS_PER_DOCUMENT);
        let content_len = (document_chunks - 1)
            .checked_mul(step)
            .and_then(|tail| chunk_size.checked_add(tail))
            .context("synthetic corpus size overflows usize")?;
        let title = format!("synthetic-{document_index:06}.txt");
        documents.push(SyntheticDocument {
            uri: format!("eval://synthetic/{title}"),
            title,
            content: synthetic_content(document_index, content_len),
        });
        remaining -= document_chunks;
    }
    Ok(documents)
}

fn synthetic_content(document_index: usize, len: usize) -> String {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ document_index as u64;
    let mut content = String::with_capacity(len);
    for _ in 0..len {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        content.push(char::from(b'a' + (state % 26) as u8));
    }
    content
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
    let synthetic = if let Some(requested_chunks) = args.synthetic_chunks {
        let chunks_before = store.stats()?.1;
        let documents = generate_synthetic_documents(
            requested_chunks,
            config.chunk_size,
            config.chunk_overlap,
        )?;
        let generated_documents = documents.len();
        for document in documents {
            wiki::ingest_raw(
                &store,
                &embedder,
                &config,
                document.content,
                Some(document.title),
                Some(document.uri),
                None,
                None,
                None,
            )
            .await
            .context("ingest deterministic synthetic corpus")?;
        }
        let chunks_after = store.stats()?.1;
        let generated_chunks = chunks_after
            .checked_sub(chunks_before)
            .context("synthetic ingest reduced the chunk count")?;
        if generated_chunks != requested_chunks as u64 {
            bail!(
                "synthetic generator requested {requested_chunks} chunks but ingest produced \
                 {generated_chunks}; check chunking configuration"
            );
        }
        Some(SyntheticDiagnostics {
            requested_chunks,
            generated_documents,
        })
    } else {
        None
    };
    let ingest_ms = ms(started);
    let (documents, chunks, _, _) = store.stats()?;
    let samples_per_mode = dataset
        .queries
        .len()
        .checked_mul(args.repeat)
        .context("query count multiplied by --repeat overflows usize")?;
    let eval_context = EvalModeContext {
        store: &store,
        embedder: &embedder,
        config: &config,
        queries: &dataset.queries,
        top_k: args.top_k,
    };
    let mut modes = Vec::new();
    for mode in &args.modes {
        modes.push(eval_mode(&eval_context, *mode, args.warmup, args.repeat).await?);
    }
    let worst_p95 = modes
        .iter()
        .filter(|m| m.mode != "lex")
        .map(|m| m.p95_search_ms)
        .reduce(f64::max);
    let minimum_throughput_qps = modes
        .iter()
        .filter(|m| m.mode != "lex")
        .filter_map(|m| m.search_throughput_qps)
        .reduce(f64::min);
    let rss_mib = process_rss_mib();
    let thresholds = RecommendationThresholds {
        p95_search_ms: args.max_p95_ms.unwrap_or(ANN_P95_MS),
        rss_mib: args.max_rss_mib,
        throughput_qps: args.min_throughput_qps,
    };
    Ok(Report {
        dataset_version: dataset.version,
        dataset_name: dataset.name,
        top_k: args.top_k,
        corpus: CorpusDiagnostics {
            documents,
            chunks,
            ingest_ms,
            synthetic,
        },
        sampling: SamplingDiagnostics {
            queries: dataset.queries.len(),
            warmup: args.warmup,
            repeat: args.repeat,
            samples_per_mode,
        },
        resources: ResourceDiagnostics { rss_mib },
        modes,
        scale_recommendation: recommend(
            chunks,
            worst_p95,
            rss_mib,
            minimum_throughput_qps,
            thresholds,
        ),
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

struct EvalModeContext<'a> {
    store: &'a Store,
    embedder: &'a Arc<dyn EmbeddingProvider>,
    config: &'a Config,
    queries: &'a [LabeledQuery],
    top_k: usize,
}

async fn eval_mode(
    context: &EvalModeContext<'_>,
    mode: SearchMode,
    warmup: usize,
    repeat: usize,
) -> Result<ModeReport> {
    for _ in 0..warmup {
        for query in context.queries {
            let _ = eval_query(
                context.store,
                context.embedder,
                context.config,
                query,
                mode,
                context.top_k,
            )
            .await?;
        }
    }

    let mut reports = Vec::with_capacity(context.queries.len());
    let mut embedding_totals = vec![0.0; context.queries.len()];
    let mut search_totals = vec![0.0; context.queries.len()];
    let mut timings = Vec::with_capacity(context.queries.len() * repeat);
    for repetition in 0..repeat {
        for (query_index, query) in context.queries.iter().enumerate() {
            let report = eval_query(
                context.store,
                context.embedder,
                context.config,
                query,
                mode,
                context.top_k,
            )
            .await?;
            embedding_totals[query_index] += report.embedding_ms;
            search_totals[query_index] += report.search_ms;
            timings.push(report.search_ms);
            if repetition == 0 {
                reports.push(report);
            }
        }
    }
    for (query_index, report) in reports.iter_mut().enumerate() {
        report.embedding_ms = embedding_totals[query_index] / repeat as f64;
        report.search_ms = search_totals[query_index] / repeat as f64;
    }
    let n = reports.len() as f64;
    timings.sort_by(f64::total_cmp);
    let sample_count = timings.len() as f64;
    let total_search_ms = timings.iter().sum::<f64>();
    Ok(ModeReport {
        mode: mode.as_str().into(),
        recall_at_k: reports.iter().map(|q| q.recall_at_k).sum::<f64>() / n,
        mrr: reports.iter().map(|q| q.reciprocal_rank).sum::<f64>() / n,
        ndcg_at_k: reports.iter().map(|q| q.ndcg_at_k).sum::<f64>() / n,
        mean_search_ms: total_search_ms / sample_count,
        p95_search_ms: percentile(&timings),
        search_throughput_qps: measured_throughput_qps(timings.len(), total_search_ms),
        queries: reports,
    })
}

async fn eval_query(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    query: &LabeledQuery,
    mode: SearchMode,
    top_k: usize,
) -> Result<QueryReport> {
    let embedding_started = Instant::now();
    let query_embedding = if mode.needs_embedding() {
        Some(
            embedder
                .embed(std::slice::from_ref(&query.query))
                .await?
                .remove(0),
        )
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
            query_text: Some(query.query.clone()),
            query_embedding,
            fts_stemmer: config.fts_stemmer.clone(),
            ..SearchQuery::default()
        },
    )?;
    Ok(score(query, hits, embedding_ms, ms(search_started), top_k))
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

fn measured_throughput_qps(samples: usize, total_search_ms: f64) -> Option<f64> {
    (samples > 0 && total_search_ms.is_finite() && total_search_ms > 0.0)
        .then(|| samples as f64 * 1_000.0 / total_search_ms)
}

#[cfg(any(target_os = "linux", test))]
fn linux_rss_mib(status: &str) -> Option<f64> {
    ["VmHWM:", "VmRSS:"].into_iter().find_map(|field| {
        status
            .lines()
            .find(|line| line.starts_with(field))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<f64>().ok())
            .map(|kib| kib / 1_024.0)
    })
}

fn process_rss_mib() -> Option<f64> {
    #[cfg(target_os = "linux")]
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        if let Some(rss_mib) = linux_rss_mib(&status) {
            return Some(rss_mib);
        }
    }

    #[cfg(unix)]
    {
        let pid = std::process::id().to_string();
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", pid.as_str()])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8(output.stdout)
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()
            .map(|kib| kib / 1_024.0)
    }

    #[cfg(not(unix))]
    {
        None
    }
}

fn recommend(
    chunks: u64,
    p95_search_ms: Option<f64>,
    rss_mib: Option<f64>,
    throughput_qps: Option<f64>,
    thresholds: RecommendationThresholds,
) -> Recommendation {
    let vector_path_measured = p95_search_ms.is_some();
    let mut failed_thresholds = Vec::new();
    if p95_search_ms.is_some_and(|measured| measured > thresholds.p95_search_ms) {
        failed_thresholds.push("p95_search_ms".to_string());
    }
    if vector_path_measured
        && thresholds
            .rss_mib
            .zip(rss_mib)
            .is_some_and(|(maximum, measured)| measured > maximum)
    {
        failed_thresholds.push("rss_mib".to_string());
    }
    if thresholds
        .throughput_qps
        .zip(throughput_qps)
        .is_some_and(|(minimum, measured)| measured < minimum)
    {
        failed_thresholds.push("throughput_qps".to_string());
    }

    let mut observations = vec![format!(
        "{chunks} chunks observed (chunk count is not an ANN trigger)"
    )];
    match p95_search_ms {
        Some(measured) => observations.push(format!(
            "vector/hybrid p95 {measured:.2} ms against {:.2} ms",
            thresholds.p95_search_ms
        )),
        None => observations.push("vector/hybrid latency not measured".into()),
    }
    match (rss_mib, thresholds.rss_mib) {
        (Some(measured), Some(maximum)) => {
            observations.push(format!("RSS {measured:.2} MiB against {maximum:.2} MiB"))
        }
        (Some(measured), None) => observations.push(format!(
            "RSS {measured:.2} MiB observed without a configured threshold"
        )),
        (None, Some(_)) => observations.push("RSS unavailable; threshold not evaluated".into()),
        (None, None) => observations.push("RSS unavailable".into()),
    }
    match (throughput_qps, thresholds.throughput_qps) {
        (Some(measured), Some(minimum)) => observations.push(format!(
            "vector/hybrid throughput {measured:.2} qps against {minimum:.2} qps"
        )),
        (Some(measured), None) => observations.push(format!(
            "vector/hybrid throughput {measured:.2} qps observed without a configured threshold"
        )),
        (None, Some(_)) => observations
            .push("vector/hybrid throughput unavailable; threshold not evaluated".into()),
        (None, None) => observations.push("vector/hybrid throughput not measured".into()),
    }
    if failed_thresholds.is_empty() {
        observations.push("no measured ANN threshold failed".into());
    } else {
        observations.push(format!(
            "failed measured threshold(s): {}",
            failed_thresholds.join(", ")
        ));
    }

    Recommendation {
        path: if failed_thresholds.is_empty() {
            "full_scan"
        } else {
            "investigate_future_ann"
        }
        .into(),
        reason: observations.join("; "),
        chunk_threshold: CHUNK_OBSERVATION_MARKER,
        p95_search_ms_threshold: thresholds.p95_search_ms,
        rss_mib_threshold: thresholds.rss_mib,
        throughput_qps_threshold: thresholds.throughput_qps,
        failed_thresholds,
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
    if let Some(synthetic) = &r.corpus.synthetic {
        println!(
            "synthetic={} chunks/{} documents",
            synthetic.requested_chunks, synthetic.generated_documents
        );
    }
    println!(
        "sampling={} measured/mode ({} queries x repeat {}), warmup={} pass(es)",
        r.sampling.samples_per_mode, r.sampling.queries, r.sampling.repeat, r.sampling.warmup
    );
    if let Some(rss_mib) = r.resources.rss_mib {
        println!("rss={rss_mib:.2}MiB");
    } else {
        println!("rss=unavailable");
    }
    for mode in &r.modes {
        let throughput = mode
            .search_throughput_qps
            .map(|qps| format!("{qps:.2}"))
            .unwrap_or_else(|| "unavailable".into());
        println!(
            "\n{} recall@{}={:.3} MRR={:.3} nDCG@{}={:.3} search_mean={:.2}ms search_p95={:.2}ms search_qps={}",
            mode.mode,
            r.top_k,
            mode.recall_at_k,
            mode.mrr,
            r.top_k,
            mode.ndcg_at_k,
            mode.mean_search_ms,
            mode.p95_search_ms,
            throughput
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

#[cfg(test)]
mod tests {
    use super::*;
    use rag_mcp::chunking::{from_config, Chunker};

    #[test]
    fn parse_args_keeps_sampling_defaults_backward_compatible() {
        let args = parse_args_from(std::iter::empty::<&str>()).unwrap();

        assert_eq!(args.warmup, 0);
        assert_eq!(args.repeat, 1);
        assert_eq!(args.synthetic_chunks, None);
        assert_eq!(args.max_rss_mib, None);
        assert_eq!(args.min_throughput_qps, None);
        assert_eq!(args.top_k, 5);
        assert_eq!(args.modes.len(), 3);
    }

    #[test]
    fn parse_args_accepts_sampling_and_synthetic_options() {
        let args = parse_args_from([
            "--warmup",
            "2",
            "--repeat",
            "7",
            "--synthetic-chunks",
            "1000",
            "--max-rss-mib",
            "2048",
            "--min-throughput-qps",
            "5",
            "--modes",
            "vec",
        ])
        .unwrap();

        assert_eq!(args.warmup, 2);
        assert_eq!(args.repeat, 7);
        assert_eq!(args.synthetic_chunks, Some(1000));
        assert_eq!(args.max_rss_mib, Some(2048.0));
        assert_eq!(args.min_throughput_qps, Some(5.0));
        assert_eq!(args.modes.len(), 1);
        assert_eq!(args.modes[0], SearchMode::Vec);
    }

    #[test]
    fn parse_args_rejects_invalid_sampling_and_resource_thresholds() {
        let repeat_error = parse_args_from(["--repeat", "0"])
            .err()
            .unwrap()
            .to_string();
        let synthetic_error = parse_args_from(["--synthetic-chunks", "0"])
            .err()
            .unwrap()
            .to_string();
        let rss_error = parse_args_from(["--max-rss-mib", "0"])
            .err()
            .unwrap()
            .to_string();
        let throughput_error = parse_args_from(["--min-throughput-qps", "NaN"])
            .err()
            .unwrap()
            .to_string();

        assert!(repeat_error.contains("--repeat must be greater than zero"));
        assert!(synthetic_error.contains("--synthetic-chunks must be greater than zero"));
        assert!(rss_error.contains("--max-rss-mib must be greater than zero"));
        assert!(throughput_error.contains("--min-throughput-qps must be greater than zero"));
    }

    #[test]
    fn percentile_uses_the_nearest_rank_ceiling() {
        assert_eq!(percentile(&[7.0]), 7.0);
        assert_eq!(percentile(&[1.0, 2.0]), 2.0);
        let twenty: Vec<_> = (1..=20).map(f64::from).collect();
        assert_eq!(percentile(&twenty), 20.0);
    }

    #[test]
    fn synthetic_generation_is_deterministic_and_exact() {
        let first = generate_synthetic_documents(513, 16, 4).unwrap();
        let second = generate_synthetic_documents(513, 16, 4).unwrap();
        let chunker = from_config(16, 4);
        let actual_chunks: usize = first
            .iter()
            .map(|document| Chunker::chunk(&chunker, &document.content).len())
            .sum();

        assert_eq!(first, second);
        assert_eq!(first.len(), 3);
        assert_eq!(actual_chunks, 513);
        assert_eq!(first[0].title, "synthetic-000000.txt");
        assert!(first
            .iter()
            .all(|document| !document.content.chars().any(char::is_whitespace)));
    }

    #[test]
    fn measured_scale_results_keep_full_scan_beyond_chunk_marker() {
        let thresholds = RecommendationThresholds {
            p95_search_ms: 300.0,
            rss_mib: None,
            throughput_qps: None,
        };

        for (chunks, p95_search_ms) in [(10_111, 48.83), (100_111, 133.98)] {
            let recommendation = recommend(
                chunks,
                Some(p95_search_ms),
                Some(512.0),
                Some(1_000.0 / p95_search_ms),
                thresholds,
            );

            assert_eq!(recommendation.path, "full_scan");
            assert!(recommendation.failed_thresholds.is_empty());
            assert!(recommendation.reason.contains("not an ANN trigger"));
        }
    }

    #[test]
    fn ann_requires_a_measured_threshold_failure() {
        let thresholds = RecommendationThresholds {
            p95_search_ms: 300.0,
            rss_mib: Some(2_048.0),
            throughput_qps: Some(5.0),
        };
        let unmeasured = recommend(1_000_000, None, None, None, thresholds);
        assert_eq!(unmeasured.path, "full_scan");
        assert!(unmeasured.failed_thresholds.is_empty());

        let failed = recommend(1_000, Some(301.0), Some(2_049.0), Some(4.0), thresholds);
        assert_eq!(failed.path, "investigate_future_ann");
        assert_eq!(
            failed.failed_thresholds,
            ["p95_search_ms", "rss_mib", "throughput_qps"]
        );
    }

    #[test]
    fn throughput_and_linux_rss_observations_are_calculated() {
        assert_eq!(measured_throughput_qps(5, 250.0), Some(20.0));
        assert_eq!(measured_throughput_qps(0, 250.0), None);
        assert_eq!(measured_throughput_qps(5, 0.0), None);
        assert_eq!(
            linux_rss_mib("Name:\teval\nVmRSS:\t1024 kB\nVmHWM:\t2048 kB\n"),
            Some(2.0)
        );
    }
}
