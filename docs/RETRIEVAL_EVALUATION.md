# Retrieval evaluation and scale diagnostics

The `eval` CLI runs labeled queries through the same `lex`, `vec`, and `hybrid`
search implementation used by the server. It creates a temporary DuckDB database,
ingests only the dataset corpus, and never modifies `RAG_DB_PATH`.

```bash
RAG_EMBEDDING_PROVIDER=mock cargo run --bin eval
cargo run --bin eval -- --dataset path/to/eval-v1.json --top-k 10 --json
cargo run --release --bin eval -- --min-recall-at-k 0.5 --min-mrr 0.4 --max-p95-ms 300
cargo run --bin eval -- --feedback-jsonl retrieval-feedback.jsonl --history-jsonl benchmark-history.jsonl
RAG_EMBEDDING_PROVIDER=mock cargo run --release --bin eval -- \
  --synthetic-chunks 10000 --warmup 1 --repeat 5 --history-jsonl benchmark-history.jsonl
```

`--modes lex,hybrid` selects a subset. `--golden` remains an alias for
`--dataset`. Human and JSON output include every ranked result and relevance.
Threshold flags make the command exit non-zero when any selected mode regresses,
so the same versioned dataset can gate CI or a release checklist.

`--warmup N` runs `N` complete query passes per selected mode before measuring;
the default is `0`. `--repeat N` runs each query `N` measured times and must be
at least `1`; the default is `1`, preserving the original behavior. The report
records the query count, warm-up passes, repeats, and measured sample count per
mode (`queries × repeat`). Per-query embedding and search timings are the mean
of that query's measured repeats. Mode mean and p95 use every measured search
sample; warm-up samples are excluded.

## Dataset format, version 1

The format is one JSON object. `version` is required and currently must be `1`.
Unknown fields are rejected so accidental schema drift is visible.

```json
{
  "version": 1,
  "name": "my retrieval set",
  "corpus": ["docs/one.md", "docs/two.md"],
  "queries": [{
    "id": "stable-unique-id",
    "query": "What is the answer?",
    "relevant": [
      { "document_title": "one.md", "relevance": 3 },
      { "document_title": "two.md", "relevance": 1 }
    ]
  }]
}
```

- Corpus paths are relative to `--root` (the current directory by default).
- `document_title` is the corpus filename and matches returned titles exactly.
- `relevance` is a positive integer grade. Omit non-relevant documents.
- Query IDs should remain stable for per-query comparisons.

The small [`example-v1.json`](../data/eval/example-v1.json) is input data, not
generated benchmark output.

## Deterministic corpus scaling

`--synthetic-chunks N` appends exactly `N` deterministic, unlabeled distractor
chunks to the selected dataset before evaluation. Existing corpus documents,
queries, relevance labels, metric definitions, and dataset version semantics
are unchanged; metric values can expose scale-related ranking degradation.
Synthetic text is generated in memory from a fixed algorithm and is split into
bounded documents using the active `RAG_CHUNK_SIZE` and `RAG_CHUNK_OVERLAP`; no
large fixture is written to the repository. The command verifies the actual
chunk-count delta after ingest and fails if it differs from the request.

Use `RAG_EMBEDDING_PROVIDER=mock` for synthetic scale runs so they are
deterministic, local, and cannot incur remote embedding cost. A lightweight
progression is 1,000 chunks for pull-request smoke checks, 10,000 for scheduled
regression checks, and 50,000 for the current full-scan boundary. Use a release
build and pin embedding dimensions and chunking environment variables for
latency comparisons.

## Metrics and diagnostics

Metrics are macro-averaged across queries for each mode: recall@k is retrieved
labeled documents divided by all labeled documents; MRR uses the first relevant
rank; nDCG@k uses graded gain normalized by the ideal ordering. Each query reports
those metrics, ranked titles, grades, scores, embedding time, and search-only
time. The summary includes document/chunk counts, ingest time, and mean/p95
search-only latency. These are lightweight diagnostics, not benchmark results.
JSON keeps the existing report fields and adds a `sampling` object. Synthetic
runs additionally add `corpus.synthetic`; ordinary dataset runs omit that field.

## Threshold-based scale recommendation

Keep the current Rust cosine full scan while the corpus is at most **50,000
chunks** and worst vector/hybrid p95 search-only latency is at most **300 ms** on
the intended machine with representative queries. If either threshold is
exceeded, the CLI recommends `investigate_future_ann`: profile and evaluate a
future optional ANN path such as DuckDB VSS/HNSW. It does not add or select an
external backend. Lexical-only runs use the corpus-size threshold because they
do not measure vector scan latency.

`--feedback-jsonl` appends one record per evaluated query and mode for later relevance analysis. `--history-jsonl` appends the complete versioned run report, making performance and quality changes comparable across commits.
