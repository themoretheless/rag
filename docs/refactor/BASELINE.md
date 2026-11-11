# Baseline (before the refactor)

Measured on `origin/main` `facf533` plus the Stage 0 commits, 2026-11-09.
Later stages add a column to each table; they do not overwrite "before".

Machine: Linux x86_64, 4 vCPU, 15 GB RAM, shared with other build jobs while
measuring, so wall-clock numbers carry noise of roughly ±15%. Environment for
every cargo number:

```
RAG_EMBEDDING_PROVIDER=mock RAG_EMBEDDING_DIMS=32 RAG_LLM_ENABLED=false
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
```

## Structure

How: `scripts/refactor_metrics.py` (brace-matching scanner; "production" means
not in a `#[cfg(test)]` item, not `#[test]`, not in a test file), plus the two
`grep` commands shown.

| metric | before (facf533) | after Stage 1 | after Stage 2 | after Stage 3 | after Stage 4 | final |
|---|---:|---:|---:|---:|---:|---:|
| Rust LOC, root crate `src/` | 149 172 | | | 153 170 | 154 830 | |
| Rust LOC, `crates/` | 21 620 | | | 21 620 | 21 620 | |
| Rust LOC, `tests/` | 5 162 | | | 6 345 | 6 646 | |
| production functions | 3 083 | | | 3 103 | 3 204 | |
| production functions > 80 lines | 255 (218 `src/`, 37 `crates/`) | | | 256 (219 `src/`, 37 `crates/`) | 253 (216 `src/`, 37 `crates/`) | |
| `impl Store` blocks (`grep -rnE '^\s*impl(<[^>]*>)? Store\b' src`) | 46 (27 in `src/db`, 19 outside) | | | 3 (all in `src/db`) | | |
| `Store` methods (production) | 258 (218 in `src/db`, 40 outside) | | | 37 (8 core, 14 accessors, 15 kept for rag-mcp-ui), 0 outside | | |
| `self.lock()` sites in `src/db` (`grep -rn 'self\.lock()'`) | 140 | | | 1 (repositories use `self.store.lock()`: 168) | | |
| `self.lock()` sites outside `src/db` | 33 | | | 0 | | |
| integration test binaries linking DuckDB | 4 (`it`, `file_ingest_test`, `query_transform_test`, `rerank_test`) | 1 (`it`) | | 1 | | |

After Stage 3 (`10417d2` + the test moves, 2026-11-10): the Store methods
moved into 14 repository types reached through accessors
(`store.documents()`, `store.graph()`, …). Production methods per
repository: `Documents` 60, `GraphRepo` 35, `Maintenance` 18, `Chunks` 13,
`SyncRepo` 13, `OpsLog` 11, `KgRepo` 8, `WikiIndexRepo` 8,
`EmbeddingManifestRepo` 8, `CorpusLane` 7, `Recovery` 6,
`SourceManifestRepo` 6, `ReembedProgress` 5, `Collections` 4 (202 in all;
counted as `fn` items directly inside `impl <Repo><'_>` blocks). The 15
Store methods kept for `rag-mcp-ui` forward to these. Tests: 1 540 run
(+3 new, −3 deleted with the Storage/Markdown code, see the Stage 3
commits), all passing.

After Stage 4 (second pass, 2026-11-10): `maintain_compress` 451 → 119
(longest phase, `plan_near_merges`), `maintain_organize` 356 → 131,
`reembed_all` 263 → 146 lines. Functions over 80 lines rose by two
(251 → 253): the splits left `plan_near_merges`, `apply_refiles`,
`reembed_batches` and `plan_exact_merges` just over the line, and the clap
`validate_args` in `bin/eval.rs` is 95 lines; the 192-line `parse_args_from`
is gone. `blocking_section` call sites outside `util`: 86 (from 166 at the
start of the pass; the remaining ones are listed in INVENTORY.md, Stage 4
status). Tests: lib 1 282, `it` 79 (+5: 3 admin parity tests, 2 bin CLI
tests), all passing. Eval gates (mock embeddings, 32 dims, the CI commands in
`.github/workflows/checks.yml`): example set lex recall@k 1.0 / MRR 1.0;
`data/eval/repository-v1.json` lex recall@k 0.958 / MRR 0.800 (gates 0.9 /
0.75).

PLAN.md quotes "261 methods, ~59 outside db" and "103+ functions over 80
lines" from the earlier read-only inventories. The numbers above are what the
committed script reproduces; the differences are counting rules (test-only
methods, and function length measured from the `fn` line to the closing
brace including the signature and blank lines).

### LOC per top-level module (root crate)

Includes in-file tests. `scripts/refactor_metrics.py --modules` prints the full
table with the largest functions; [INVENTORY.md](INVENTORY.md) carries it.

| module | LOC | fns >80 |
|---|---:|---:|
| code_index | 49 823 | 107 |
| db | 29 239 | 27 |
| http_api | 9 962 | 5 |
| maintain | 9 340 | 14 |
| mcp | 9 146 | 6 |
| wiki | 5 394 | 9 |
| external_db | 4 520 | 9 |
| source_sync | 2 851 | 3 |
| bin | 2 680 | 7 |
| eval | 1 899 | 0 |
| ingest | 1 568 | 3 |
| models | 1 442 | 0 |
| chunking | 1 311 | 2 |
| llm | 1 295 | 1 |
| config | 1 215 | 2 |
| embeddings | 1 205 | 0 |
| retrieval | 1 184 | 3 |
| diagnostics | 1 063 | 1 |
| storage | 1 021 | 2 |
| gate | 1 009 | 4 |
| file_ingest | 1 007 | 1 |
| 26 smaller modules (each ≤ 800) | 10 998 | 12 |

### LOC per crate

| crate | LOC |
|---|---:|
| rag-mcp (root, `src/`) | 149 172 |
| rag-mcp-ui | 20 047 |
| rag-mcp-markup | 847 |
| rag-mcp-core | 726 |
| web-svelte/src (ts + svelte, for reference) | 11 023 |

## Build and test

| metric | before | after Stage 1 | after Stage 8 |
|---|---:|---:|---:|
| clean `cargo build --workspace --all-targets --locked` (rustc 1.97.0) | 14 m 52 s (893 s) | | |
| clean `cargo clippy --workspace --all-targets --locked` (rustc 1.99.1) | 20 m 33 s (1233 s) | | |
| build of all test binaries after a clippy-only tree, `cargo test --workspace --no-run` (rustc 1.99.1) | 7 m 02 s (422 s) | | |
| full `cargo test --workspace --locked --no-fail-fast`, binaries already built | 7 m 56 s (476 s); the lib unit tests alone 422 s | 89 s (dep opt-level 1, 1535 tests) | |
| tests run | 1 523 (lib 1 273, `it` 45, the 3 standalone integration binaries 13, bins 14, core 4, markup 26, ui 148); 1 ignored before Stage 0, 4 with the timing tests | | |
| `target/debug/rag-mcp` (debug profile, no debug info) | 313 MB | | |
| `target/debug/rag-mcp-ui` | 211 MB | | |
| `target/debug/eval` | 151 MB | | |
| `target/debug/ingest_project` | 137 MB | | |
| `target/` after one clean build | 6.3 GB | | |

### Stage 1: `opt-level = 1` for dependencies (dev and test)

Same machine and rustc 1.99.1; full gate run from a clean `target/` after the
change (`[profile.dev.package."*"] opt-level = 1`, test inherits dev).
Before = the last gate before the change, at a warm `target/`.

| metric | dep opt-level 0 (before) | dep opt-level 1 (kept) |
|---|---:|---:|
| `cargo test --workspace` run, binaries built (1535 tests) | 324 s (lib 278 s) | **89 s** |
| clean `cargo clippy --workspace --all-targets` | 1233 s (from clean, Stage 0) | 692 s (from clean) |
| `cargo test --workspace --no-run` after clippy | 212 s | 353 s |

The test run is 3.6× faster: the lib suite is dominated by DuckDB, tokio and
tree-sitter work in dependencies. Building test binaries after clippy costs
~2.5 min more because optimised dependency codegen cannot reuse clippy's
metadata-only build; one clean build plus one test run is still faster
(1045 s vs ~1770 s), and incremental runs pay only the test-time gain.
The change is kept. Test count with the parity tests: 1535.

The toolchain changed during Stage 0: the clean build was timed on rustc
1.97.0; local stable was then updated to 1.99.1 to match CI (the clippy
failure on `main` only reproduces there). All later numbers are on 1.99.1.

One test failed on `facf533` itself
(`code_index::code_read::tests::code_cursor_corrupt_child_boundary_returns_error_without_poisoning_store`,
a panic on an unchecked slice); it is fixed in its own commit and is counted
as run above.

## Runtime (ignored timing tests)

How: `cargo test --lib -- --ignored timing_ --nocapture --test-threads=1`
(tests at the end of `src/db/search/tests.rs`). Debug profile, corpus of 2 000
documents × 4 chunks (8 000 chunks), 32-dim embeddings, top_k 10. Another
agent's release build was running on the same 4 cores during the "before" run,
so treat these as an upper bound and compare with a quiet rerun before
claiming a speed-up.

| metric | before | after Stage 1 (dep `opt-level`) | after Stage 5 |
|---|---:|---:|---:|
| vector search, cold (first query, builds the snapshot under the Store mutex) | 150.6 ms | | |
| vector search, warm p50 / p95 | 36.3 ms / 42.1 ms | | |
| hybrid search p50 / p95 / max over 50 queries (FTS ready) | 247.9 ms / 310.5 ms / 333.6 ms | | |
| document upsert, insert (`upsert_document` + `replace_chunks_for_document`) | 22 docs/s (2 000 docs in 92.5 s) | | |
| document upsert, update of the same ids | 12 docs/s (2 000 docs in 171.2 s) | | |

PLAN.md mentions "~4 s cold" for the vector snapshot; that figure is for a
production-size corpus. At 8 000 chunks the cold build is 0.15 s. Stage 5
should measure on a larger corpus before and after its change (raise
`TIMING_DOCS`).

Not measured yet (planned in PLAN.md Stage 0, deferred): MCP `tools/call`
overhead (needs the transport seam added with the Stage 1 parity tests) and
the web bundle size (owned by the Stage 7 agent).

## Stage 5: optimization (before/after per item)

Method for every runtime row below: the ignored timing tests in
`src/db/search/tests.rs`, release profile (`cargo test --release --lib --
--ignored --exact db::search::tests::<name> --nocapture`), with
`CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16` so a
test build takes minutes instead of a full fat link (same settings before and
after), `RAG_TIMING_DOCS=10000` (10 000 documents × 4 chunks = 40 000 chunks,
32 dims). Each test was run 3 times; the table shows the median of the three
runs' p50/p95. "Before" was measured on the same machine, back to back with
"after", from a worktree of the previous commit carrying only the harness
changes. 4 vCPU, load average 1.2–2.0 from other jobs during the runs (4.1 on
one "before" refresh run).

New timing tests added for this stage: `timing_concurrent_search_and_write`
(vector searches in a loop while another thread writes 50 documents) and
`timing_vector_refresh_after_write` (one document written, then one vector
search, 50 times). `RAG_TIMING_DOCS` overrides the corpus size.

### Item 1: vector snapshot owned by `Store`, built outside the mutex

Change: the process-global `VECTOR_SNAPSHOTS` / scoped caches keyed by path
became a `VectorCache` field of `Store` (shared by clones). The connection
mutex is held only to read rows (generation, scope ids, the chunk listing and
changed embeddings); JSON parsing, normalization, document-id interning and
the merge with the previous snapshot run after it is released. Document ids
are interned to `u32` (the table is shared with the previous snapshot while no
new document appears, and the scope check is a per-document mask instead of a
string hash per chunk), chunk ids are `Arc<str>`, and a change-log refresh
merges two sorted runs instead of re-sorting the corpus. Spans and section
metadata are read with the content for returned hits only, so the snapshot no
longer keeps `metadata_json` / `char_start` / `char_end` per chunk.

| metric (10 000 docs, 40 000 chunks) | before | after |
|---|---:|---:|
| vector search, cold (first query builds the snapshot) | 128.8 ms | 128.4 ms |
| vector search, warm p50 / p95 | 10.4 / 14.7 ms | 10.7 / 12.0 ms |
| vector search right after a one-document write, p50 / p95 | 25.5 / 31.0 ms | **17.1 / 20.7 ms** |
| concurrent: search p50 / p95 while writing | 53.7 / 76.7 ms | **37.0 / 54.6 ms** |
| concurrent: write p50 / p95 while searching | 19.2 / 41.0 ms | **17.5 / 24.3 ms** |
| hybrid search p50 / p95 (control; "after" on the first draft of this item, re-measured as item 3's "before") | 95.9 / 105.9 ms | 97.5 / 119.3 ms |

Kept. Cold build cost is the JSON parse of every vector, which item 2
removes; warm search is dominated by the scope listing and hydration, not by
the dot products.

### Item 2: schema v26, `chunks.embedding FLOAT[]` replaces `embedding_json`

Design:

- `SCHEMA_VERSION` 25 → 26. `migrate_with` reads the stored version first and
  runs the core DDL only when it differs; schema extensions run only when the
  blake3 digest of their DDL is not yet recorded in `meta`
  (`schema_extension:<name>`). A store already at v26 opens with reads only
  (`MigrateOutcome { core_ddl: false, extension_ddl: 0 }`).
- Fresh databases create `embedding FLOAT[] NOT NULL`. An older database gains
  a nullable `embedding FLOAT[]` (DuckDB cannot add `NOT NULL` to an indexed
  table), is backfilled with `CAST(embedding_json AS FLOAT[])` in batches of
  5 000 rows, one transaction per batch, and then loses `embedding_json`.
  DuckDB refuses `DROP COLUMN` while an index covers a later column and does
  not see an index dropped earlier in the same transaction, so the chunk
  indexes are dropped, the column dropped and the indexes re-created (from
  `duckdb_indexes().sql`) as separate statements. The version is recorded
  last: an open interrupted anywhere in this sequence repeats it, the
  backfill continues with the rows still `NULL`, and missing indexes are
  re-created by the idempotent DDL.
- Writers still serialise each vector to JSON (the input of
  `embedding_digest`, unchanged) and bind it as `CAST(? AS FLOAT[])`; the cast
  is exact for every finite `f32` (checked bit for bit, including subnormals,
  `-0.0`, `f32::MAX` and values printed with an exponent). Readers decode the
  list through the Arrow `Float32Array` of the row (one buffer copy), with a
  `Value::List` fallback.
- Other readers moved to the array: `rows::chunk`, chunk vectors by content,
  representative/first-chunk embeddings, the recovery export, the backup
  consistency check (now one `count(*)` over `len(embedding)`), the portable
  preflight estimate (`2 + len(embedding)` bytes per vector plus the existing
  per-value worst case), and the code-index status sampler
  (`CAST(embedding AS VARCHAR)`).

Tests (`src/db/schema/embedding_migration_tests.rs`): a database built from the
pre-v26 DDL (JSON embeddings written by `serde_json` as the old writers did,
plus the `parent_chunk_id` index that blocks a plain column drop) is opened
through `Store::open`; it reports v26, has no `embedding_json`, keeps both
chunk indexes, its vectors equal a natively ingested store's bit for bit, vec,
lex and hybrid searches return the same chunk ids with bit-identical scores,
and the exported recovery bundle equals the fixture. A second open runs no DDL.
An interrupted backfill (column added, one 4-row batch committed, no version
recorded) resumes on the next open with the same results and bundle.

| metric | before (v25, JSON) | after (v26, FLOAT[]) |
|---|---:|---:|
| vector snapshot build (cold vector search, 40 000 chunks) | 150.6 ms | **67.2 ms** (3 quiet runs; 88.1 ms median over 6 runs including load 3.5) |
| vector search right after a one-document write, p50 | 17.1 ms | 16.8 ms |
| warm vector search p50 | 10.5 ms | 10.0 ms |
| database file after `CHECKPOINT` (40 000 chunks, 32 dims) | 39.07 MB (38.81–39.07) | 39.07 MB (38.02–39.33): no measurable change |
| document upsert, insert / update (2 000 docs) | 93 / 61 docs/s | 86 / 56 docs/s (−7 %, the text → `FLOAT[]` cast) |

Kept (the migration was approved; snapshot builds are 2.2× faster and every
embedding read skips JSON). The file size does not move at 32 dims: DuckDB
compresses the JSON strings and allocates in blocks, and chunk text dominates;
larger dimensions should show a difference. The write path pays for the cast;
duckdb-rs 1.x rejects `Value::List` parameters; binding a borrowed Arrow
`ListArray` (`ValueRef::List`) might remove the cast and is left as a
follow-up.

### Item 3: search path (lex fallback, RRF, neighbours, variants, ranking fields)

Changes:

- The lexical leg calls `db::fts::search_bm25_with_stemmer` once. The removed
  "in-module fallback" (`search_lex`) re-ran that same call after it had just
  failed; `db::fts` already owns the real fallback (BM25, else term
  frequency). Lex mode now returns the first error; hybrid returns the
  vector hits, as it did when both calls failed.
- RRF accumulates `(list, rank)` indices per chunk id and moves (hybrid) or
  clones once (`fuse_rrf`, `fuse_rrf_many`) the winning hit, instead of
  cloning every hit into the map and merging clones.
- `Neighbors` context: one query for every hit's `index ± radius` window
  (`VALUES` list joined on document and range, deduplicated), instead of one
  query per hit.
- Query variants (rewrite legs, HyDE passage) are embedded in one embedder
  request in `retrieval.rs` and in the eval binary, instead of one request
  per leg. Not covered by the timing harness (mock embedder, no network); it
  saves `variants + hyde − 1` embedder round trips per transformed search.
- Ranking fields (title, uri, pinned, boost, updated_at) are read with the
  vector hits' documents during hydration and kept for `finalize_hits`, which
  reads only the documents it has not seen (lexical-only hits); `updated_at`
  is parsed only when recency applies.

Method: `timing_hybrid_search_p50_p95` (unchanged) and the new
`timing_hybrid_search_neighbors` (same queries with `context_expansion =
Neighbors`, radius 1), 10 000 documents. Before and after binaries were run
alternately (before, after, before, …), 3 runs each, load 1.4–2.0 (3.7 on the
first "before" run).

| metric | before | after |
|---|---:|---:|
| hybrid search p50 / p95 | 89.7 / 105.4 ms | 89.4 / 100.1 ms |
| hybrid search with neighbour context p50 / p95 | 111.3 / 124.4 ms | **94.7 / 109.8 ms** |

Kept. The plain hybrid path is within noise at p50 (−0.3 %) and 5 % lower at
p95: its cost is the BM25 scan and the vector scope listing, which these
changes do not touch. Neighbour context drops by 15 % (ten point queries per
search become one). Eval gates unchanged (below).

### Item 4: facade parses each tool result once

`call_tool` derived three things from every tool result, each by parsing the
first text content from JSON again: success for telemetry/activity
(`tool_result_succeeded`), the bounded count hint (`result_hint`) and, for
analysis tools, the evidence count. They now share one parse
(`summarize_tool_result`); a unit test checks the summary against the three
separate checks on success, failure, array, non-JSON and error results.

Method: ignored test `mcp::facade::audit_tests::timing_tool_result_summary`,
release profile, a search-shaped result (10 hits × ~800 characters of
content, ~10 KB of JSON), 20 000 calls per sample, median of 5 samples per
run; before and after binaries alternated, 3 runs each, load 2.4–3.3.

| per `tools/call` (search result) | before | after |
|---|---:|---:|
| telemetry post-processing, ordinary tool (2 parses → 1) | 34.7 µs | **17.9 µs** |
| telemetry post-processing, analysis tool (3 parses → 1) | 52.0 µs | **17.8 µs** |

Kept. Against a ~90 ms hybrid search the saving is small (0.02–0.04 % of the
call), but it grows with result size and applies to every tool call. An
end-to-end `tools/call` latency was not measured: the transport, JSON-RPC
framing and the search itself dominate and vary far more than 35 µs.

### Item 5: N+1 fixes

Changes:

- `set_document_tags`: existing tag edges in one read and new edges in one
  multi-row insert (was a count query and an insert per label); tag hubs
  already stored under `tag://{key}` are looked up in one `IN` query with the
  order `find_node_by_uri` uses, and only unknown labels take the per-label
  resolve path (legacy label match or a new hub).
- Collections: member existence in one `IN` query (was a count per entry);
  entries and dependencies written with multi-row inserts (was one statement
  each); `get_collection` reads all dependencies in one query (was one query
  per entry).
- KG: `kg_stats` computes its six counts in one scan (`FILTER` aggregates;
  was six queries). Batching the per-id `UPDATE` and reload in
  `kg_invalidate` / `kg_supersede` was measured and reverted: `kg_add` keeps
  open facts unique per subject/predicate/object, so those loops run once
  and the batched form did the same work (769.9 → 726.9 ms for 200 calls,
  within run-to-run spread of 698–811 ms).
- `health` chunkless count: one query (chunkless ids in a CTE, then the
  non-blank body check joined to those rows) instead of listing the ids and
  counting them in 512-id batches.

Method: ignored test `db::n_plus_one_timing_tests::timing_n_plus_one`,
release profile, one seeded store per run: 2 000 documents with one chunk
each plus 200 documents without chunks; 100 nodes × 20 shared tag labels
(3 rounds, replace and merge alternating); a 200-member collection where each
member depends on the previous one (3 creates + gets); 2 000 KG facts
(`kg_stats` × 20, then 200 `kg_invalidate` calls); `integrity_counts` × 20.
Medians; before and after binaries alternated, 3 runs each, load 1.6–3.5.

| operation | before | after |
|---|---:|---:|
| `set_document_tags`, 100 nodes × 20 labels | 6 691.6 ms | **706.9 ms** |
| `create_collection`, 200 members | 440.1 ms | **23.6 ms** |
| `get_collection`, 200 members | 160.6 ms | **3.1 ms** |
| `kg_stats` (2 000 facts) | 5.81 ms | **2.45 ms** |
| `kg_invalidate` × 200 (reverted, see above) | 769.9 ms | 726.9 ms |
| `integrity_counts` (`/health`), 2 200 documents | 12.10 ms | **9.43 ms** |

All kept except the `kg_invalidate` / `kg_supersede` batching.

### Item 6: release profile, `lto = "thin"` vs `"fat"`

Method: the real release profile (`codegen-units = 1`, `strip =
"debuginfo"`), only `CARGO_PROFILE_RELEASE_LTO` switched, each from an empty
`target/release`: `cargo build --release -p rag-mcp --bin rag-mcp` for size
and build time, and the lib test binary built with the same profile for
`timing_hybrid_search_p50_p95` (10 000 documents), run alternately 3 times
each, load 0.7–1.8.

| metric | thin (current) | fat |
|---|---:|---:|
| `target/release/rag-mcp` | 116.9 MB (116 877 256 B) | 113.2 MB (113 203 056 B), −3.1 % |
| clean build of `rag-mcp` | 1 138 s | 1 350 s (+19 %) |
| hybrid search p50 / p95 | 85.6 / 94.8 ms | 88.1 / 100.5 ms |

Kept `thin`: fat LTO is 3 % smaller but not faster (p50 +2.9 %, inside the
run-to-run spread of 84–91 ms) and costs 3.5 more minutes per release build;
neither wins clearly, so the current setting stays.

### Stage 5 summary (40 000 chunks, release, same harness settings)

| metric | before Stage 5 | after Stage 5 |
|---|---:|---:|
| vector snapshot build (cold vector search) | 128.8 ms (150.6 ms re-measured before item 2) | 67.2 ms |
| vector search right after a one-document write, p50 | 25.5 ms | 16.8 ms |
| concurrent search p50 / write p95 | 53.7 / 41.0 ms | 37.0 / 24.3 ms (item 1) |
| hybrid search p50 / p95 (LTO off, 16 codegen units) | 95.9 / 105.9 ms | 89.4 / 100.1 ms |
| hybrid search with neighbour context p50 | 111.3 ms | 94.7 ms |
| document insert throughput (2 000 docs) | 93 docs/s | 86 docs/s |
