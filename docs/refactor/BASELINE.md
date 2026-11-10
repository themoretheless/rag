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
