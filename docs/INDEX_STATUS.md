# Versioned index status and measured quality

`index_status` (MCP, `RAG_TOOLS=full`) accepts:

```json
{"project":"rag","limit":50,"sample_limit":128,"max_chunk_tokens":4096}
```

All fields are optional. `project` is an exact wing, selected before decoding
cached facts or source blobs. The diagnostic uses a single database read
transaction for the source inventory, graph, manifest, lexical generations and
chunk sample. It runs on the blocking pool. It does not run indexing, repair
source blobs, rebuild FTS or read live files. Normal MCP call attribution remains
separate from this read-only diagnostic.

`contract_version: 1` identifies the envelope. Earlier top-level counts and static
capability flags remain. Additional SQL-usage, styling and policy counters count
captured evidence sites/decisions, not runtime activity or unique objects. Additional fields include:

- `versions`: actual stored schema version, compiled index format and package.
- `coverage`: fresh/stale/unindexed/parse-error counts, source extensions and
  extensions unsupported **for code indexing** (not ingest failures). Evidence
  windows use `limit` (1..200); counts cover the selected scope, independently of
  those windows. Unsupported extension categories also expose truncation.
- `capabilities`: available extractor entries (including the TSX dialect,
  CSS/HTML and SQL), partial static binding qualifications, explicit unavailable
  CPG/data-flow/runtime analysis, the native CPU structural-graph backend and unverified representative precision/recall.
  Availability does not certify complete language or compiler semantics.
- `quality`: actual measured defects, static reference-resolution/diagnostic-code counts, fresh
  code files without definitions, signature lengths, vector/chunk samples and
  explicit unknown fields for docblocks, full parameter types and semantic tests.
- `retrieval.lexical`: configured backend and read-only dirty/generation/rebuild
  counters. These and the embedding manifest are labelled **corpus-global**;
  their state can change because of another project.

`state` is `not_indexed`, `empty_scope`, `unavailable`, `degraded`, or
`observed_checks_passed`. A missing/empty index or a measured defect produces
`quality.healthy=false`. With no detected defects it remains `null`: passing
observed checks is not proof of whole-index health. `observed_checks_passed` and
`exhaustive=false` make that distinction explicit. Corrupt graph cache JSON
returns `unavailable`, null graph/coverage measurements and a bounded error code;
it is not disguised as an empty healthy graph. An unreadable database can still
fail the operation.

## Chunk/vector sample

The chunk sample covers active retrieval documents in the selected project,
including text-only formats; code-index eligibility is a separate measurement.
The sample is ordered by stored chunk ID, with `sample_limit` clamped to 1..512.
Each row can contribute at most 16,384 characters of text and 65,536 bytes of vector
JSON. Larger cells are counted as unmeasured, not silently truncated for a health
claim. Total inspected text/vector payload is limited to 4 MiB. This bounds
inspection; database length/aggregate queries and graph loading are not a
whole-corpus latency or physical-I/O bound.

Vector diagnostics use the server's stored `f32` representation: valid/invalid
and exact-zero counts, dimensions versus the configured dimensions, and finite
norm min/max/upper-median (`norm_p50`). Unit-norm counts use a ±0.01 observation
window; nonunit vectors are **not** automatically invalid for cosine retrieval.
Missing manifests and incompatible corpus identity are explicit.

Text token counts use the existing configured tokenizer or script-aware estimate.
`max_chunk_tokens` is a diagnostic warning threshold (1..100000), explicitly not a
calibrated production rule. Oversized estimates, byte-budget exhaustion, omitted
large cells and incomplete samples are reported separately. Example chunk IDs
are count-bounded and capped at 128 characters, with a truncation flag. Source
bodies and vector arrays are never returned.

Freshness here compares captured source content/revisions with cached facts. It
does not prove that files on disk are unchanged; use `freshness` for allowlisted
live-source checks. Successful native build/no-change timings are now persisted
with separate publication/latest identities; see [timing contract](INDEX_TIMINGS.md).
Supported documentation-syntax coverage is now measured without returning doc
text; see [documentation contract](CODE_DOCUMENTATION.md).
Commit/failure/per-project timings, full documentation/type metrics,
representative sampling/quality, history-integrity audits, scale and the remaining
language/backend coverage are still open in the feature matrix.

See [qualification](qualification/index-status-2026-10-25/README.md).

`resource_targets` now reports separately captured non-code source-file target identities, freshness and bounded diagnostics. They do not count as syntax-supported/indexed code. Unqualified target identity adds `RESOURCE_TARGETS_UNAVAILABLE` to observed problems. See [resource target contract](RESOURCE_TARGET_IDENTITIES.md).
