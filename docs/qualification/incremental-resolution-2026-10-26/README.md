# Dependency-scoped resolution qualification

This advances requirement 8 of [the complete plan](../../CODE_INTELLIGENCE_PLAN.md).
It does not close representative Rust quality, competitor performance, remaining
Cargo/compiler semantics, semantic rename or infrastructure requirements.

## Implemented and checked

- Content-addressed configured source/dependency binding cache, with Rust package
  closure and conservative project fallback. New/missing providers and manifest
  changes invalidate without relying on previously successful edges.
- Bulk AST-cache loading, unchanged serialized payload retention and transactional
  changed/deleted row persistence. Other derived graph families still recompute.
- Exact full-binding comparison for unchanged revisions via
  `index_code(force_full_resolution=true)`, without replacing immutable history.
- Runtime resolution counts and successful native completion through commit,
  separate from the existing pre-commit ledger. Process-lifetime high-water RSS
  is explicitly distinct from per-operation or sampled RSS.

The workspace log contains 1347 passing tests, zero failures and one ignored
external-database fixture test. All four resolution-cache native tests passed,
including provider addition/removal, body/signature/manifest invalidation,
corrupt/versioned cache fallback, mixed-language fallback and committed full
equivalence. Strict library/test Clippy and the debug binary build finished.
The query-transform test cache reset race was isolated with a shared test lock;
no query-transform production behavior changed. A Ruby helper argument refactor
and unsupported-extension fixture adjustment accommodate concurrent language work.

Five actual stdio MCP reports passed on the same frozen binary, SHA-256
`0d32dafcd3bf697e44473abd867c6150a777cb983d9b315dd77029f5751733a8`,
index format 42: `runtime.json`, `tests-regression.json`, `git-regression.json`,
`config-regression.json`, `trait-regression.json`. The resolution runner checks
warm no-op, provider staleness, body/signature/manifest changes, reused reference
sites, exact full audits and invalid typed options. The affected-tests runner
executes all seven suggested commands; each runs one passing test.

## Measurements and limits

The synthetic benchmark has three Cargo packages, 79 indexed source/manifest
documents and 864 functions, including 288 app-to-core calls. Source mutations
are body-only, signature plus matching caller adjustment, and manifest-only.
Cold sync includes ingestion/FTS; later measured index calls exclude ingestion.
All four source states passed actual Cargo checks in both exploratory runs.

`exploratory-before.json` and `exploratory-after.json` preserve single old/new
runs on identical fixture hashes. Body index time changed from 7433 ms to 463 ms;
warm no-op from 107 ms to 22 ms. Cold sync increased from 12030 ms to 13410 ms;
signature/manifest remained around 7.5–7.7 seconds and sampled RSS increased.
These are exploratory observations, not causal or production qualification:
the debug binaries include concurrent baseline differences, and the after run
includes full audits outside the timed calls.

`paired-controls/` retains three alternating same-binary full/incremental pairs
on one source root with isolated databases. The full warm case deliberately
audits all bindings and is not an ordinary no-op. Pair dispersion is recorded;
environment observations are retained in `control-environment.log`. Competing
Rust compilation was observed during measurement, so even low dispersion does
not establish isolated machine controls. No raw run is silently discarded.
Read `paired-controls/summary.json` for actual measurements and stability flags.

Observed medians (milliseconds, full / incremental): cold sync 12482 / 12837,
forced-full warm audit versus ordinary no-op 7793 / 22, body 7855 / 487,
signature 7816 / 7818, manifest 7942 / 7999. Cold, warm and body controls fail
the runner's 15% relative-range stability threshold; full warm/body ranges reach
174%/72%. Signature and manifest are stable by that narrow threshold and show
essentially no gain. Overall `controls_stable=false`; these controls are rejected
as production-speed evidence. The body observation remains a useful diagnostic,
not an accepted performance win.

Empty-reference document reuse is reported separately: reusing many documents
does not necessarily reuse call sites. The benchmark's signature and manifest
cases still recompute all 288 app call sites. Cached immutable edges duplicate
storage and can increase RSS/disk. External RSS is sampled at 50 ms, not an exact
peak. Three small pairs cannot establish representative production performance.

All runners use disposable databases and sources. The installed executable and
production database were not opened/replaced by qualification runners. The
worktree contains concurrent changes and is not a frozen source checkout; the
MCP/paired-control executable is frozen. No commit or push was performed.

## Reproduction

```sh
python3 scripts/smoke_resolution_cache.py /path/to/rag-mcp /tmp/resolution.json
python3 scripts/bench_incremental_index.py /path/to/rag-mcp /tmp/index.json --check-fixture --verify
python3 scripts/bench_resolution_pairs.py /path/to/rag-mcp /tmp/new-pairs-directory --pairs 3
```

The pair output directory must not already exist. `checks.json` records verified
outcomes and artifact hashes. Representative real-repository precision/recall,
release-mode throughput/RSS, equal-work competitor runs and full requirement 8
acceptance remain open.
