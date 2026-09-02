# Retrieval evaluation and scale diagnostics

The `eval` CLI runs labeled queries through the same `lex`, `vec`, and `hybrid`
search implementation used by the server. It creates a temporary DuckDB database,
ingests only the dataset corpus, and never modifies `RAG_DB_PATH`.

```bash
RAG_EMBEDDING_PROVIDER=mock cargo run --bin eval
cargo run --bin eval -- --dataset path/to/eval-v1.json --top-k 10 --json
cargo run --release --bin eval -- --min-recall-at-k 0.5 --min-mrr 0.4 --max-p95-ms 300
cargo run --release --bin eval -- --max-rss-mib 2048 --min-throughput-qps 5
cargo run --bin eval -- --feedback-jsonl retrieval-feedback.jsonl --history-jsonl benchmark-history.jsonl
RAG_EMBEDDING_PROVIDER=mock cargo run --release --bin eval -- \
  --synthetic-chunks 10000 --warmup 1 --repeat 5 --history-jsonl benchmark-history.jsonl
```

`--modes lex,hybrid` selects a subset. `--golden` remains an alias for
`--dataset`. Human and JSON output include every ranked result and relevance.
Threshold flags make the command exit non-zero when any selected mode regresses,
so the same versioned dataset can gate CI or a release checklist.
`--max-rss-mib` and `--min-throughput-qps` are optional, operator-defined
resource gates; the values above are examples, not built-in defaults. Existing
commands and flags retain their previous defaults and meanings.

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
regression checks, and 100,000 or more for an explicit scale run. These are
observation points, not ANN cutoffs. Use a release build and pin embedding
dimensions and chunking environment variables for latency comparisons.

## Metrics and diagnostics

Metrics are macro-averaged across queries for each mode: recall@k is retrieved
labeled documents divided by all labeled documents; MRR uses the first relevant
rank; nDCG@k uses graded gain normalized by the ideal ordering. Each query reports
those metrics, ranked titles, grades, scores, embedding time, and search-only
time. The summary includes document/chunk counts, ingest time, and mean/p95
search-only latency. These are lightweight diagnostics, not benchmark results.
Each mode also reports sequential search-only throughput in queries per second
(`samples / summed search time`). The top-level `resources.rss_mib` is a
best-effort process RSS observation after measured searches: Linux uses peak RSS
from `/proc/self/status`, other Unix systems use a current `ps` snapshot, and
unsupported platforms omit it. It is not a replacement for an external
profiler.

JSON keeps the existing report fields and adds `resources`,
`modes[].search_throughput_qps`, and explicit recommendation threshold/failure
fields. Synthetic runs additionally add `corpus.synthetic`; ordinary dataset
runs omit that field. The historical recommendation field `chunk_threshold`
remains for JSON compatibility, but is now only a scale observation marker.

## Measured threshold-based scale recommendation

Chunk count is context, not a failure condition. The evaluator keeps
`full_scan` regardless of corpus size until a **measured** vector/hybrid signal
crosses its configured threshold:

- worst vector/hybrid p95 search-only latency exceeds `--max-p95-ms`, or the
  built-in **300 ms** target when that flag is absent;
- observed RSS exceeds `--max-rss-mib`, when that optional gate is supplied;
- minimum vector/hybrid sequential throughput falls below
  `--min-throughput-qps`, when that optional gate is supplied.

Unavailable measurements and thresholds that were not configured do not count
as failures. A lexical-only run does not manufacture an ANN recommendation from
corpus size. When one or more measured vector-path thresholds fail, the CLI
reports `investigate_future_ann`: profile the failing resource and evaluate a
future optional ANN path such as DuckDB VSS/HNSW. It does not add or select an
external backend.

The release measurements from this optimization pass support that policy:

| Corpus chunks | Hybrid p95 search-only | Share of 300 ms target | Result |
| ---: | ---: | ---: | --- |
| 10,111 | 48.83 ms | 16.3% | keep full scan |
| 100,111 | 133.98 ms | 44.7% | keep full scan |

The 100,111-chunk run is already beyond the historical 50,000 marker while
remaining well inside the latency target. RSS and throughput thresholds were
not recorded for these two measurements, so they cannot be used to justify ANN
for this pass. Record them with the optional gates on the intended machine
before changing the backend.

`--feedback-jsonl` appends one record per evaluated query and mode for later relevance analysis. `--history-jsonl` appends the complete versioned run report, making performance and quality changes comparable across commits.
