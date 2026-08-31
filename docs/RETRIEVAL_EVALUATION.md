# Retrieval evaluation and scale diagnostics

The `eval` CLI runs labeled queries through the same `lex`, `vec`, and `hybrid`
search implementation used by the server. It creates a temporary DuckDB database,
ingests only the dataset corpus, and never modifies `RAG_DB_PATH`.

```bash
RAG_EMBEDDING_PROVIDER=mock cargo run --bin eval
cargo run --bin eval -- --dataset path/to/eval-v1.json --top-k 10 --json
cargo run --release --bin eval -- --min-recall-at-k 0.5 --min-mrr 0.4 --max-p95-ms 300
```

`--modes lex,hybrid` selects a subset. `--golden` remains an alias for
`--dataset`. Human and JSON output include every ranked result and relevance.
Threshold flags make the command exit non-zero when any selected mode regresses,
so the same versioned dataset can gate CI or a release checklist.

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

## Metrics and diagnostics

Metrics are macro-averaged across queries for each mode: recall@k is retrieved
labeled documents divided by all labeled documents; MRR uses the first relevant
rank; nDCG@k uses graded gain normalized by the ideal ordering. Each query reports
those metrics, ranked titles, grades, scores, embedding time, and search-only
time. The summary includes document/chunk counts, ingest time, and mean/p95
search-only latency. These are lightweight diagnostics, not benchmark results.

## Threshold-based scale recommendation

Keep the current Rust cosine full scan while the corpus is at most **50,000
chunks** and worst vector/hybrid p95 search-only latency is at most **300 ms** on
the intended machine with representative queries. If either threshold is
exceeded, the CLI recommends `investigate_future_ann`: profile and evaluate a
future optional ANN path such as DuckDB VSS/HNSW. It does not add or select an
external backend. Lexical-only runs use the corpus-size threshold because they
do not measure vector scan latency.
