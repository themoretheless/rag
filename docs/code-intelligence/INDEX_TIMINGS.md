# Persisted native index-run timing contract v1

The additive runtime `IndexReport.completion` contract now measures native time
through the successful commit, commit-call duration, metrics-write duration and
process-lifetime high-water RSS. It is returned after commit and is not written
back into this pre-commit ledger. `IndexReport.resolution` separately describes
binding-cache work. See [incremental resolution](INCREMENTAL_RESOLUTION.md).

`Store::index_code`, MCP `index_code` and source-sync index finalisation now persist
measured successful native runs in `code_index_runs`. `IndexReport.stage_timings`
is an additive optional wire field; older reports deserialize with no timings.
Database schema **25** adds this ledger/index through normal writer migration.
The timing migration itself does not invalidate source facts or publish a new
index revision (its original qualification used syntax format 24). Independent
extractor changes, including the Lua extension requiring syntax format 25, refresh
older extraction caches through normal indexing. Old revisions do not acquire invented measurements.
Existing strict read-only schema checks require the normal writer upgrade first.
No production database/service was migrated or restarted for qualification.

A run has a UUID, monotonic ledger sequence, target index revision,
revision_created flag, transaction-time recorded_at and contract-1 metrics. Its
row commits in the **same transaction** as cache replacement and revision
publication. A metrics-write/commit error does not return a successful report or
leave a separately committed measurement. Failed attempts are not recorded by
this successful-run ledger; unknown commit outcomes are not labelled successful.

Named wall-clock phases use `std::time::Instant`, reported as finite nonnegative
floating-point milliseconds:

| Metric | Measured boundary |
|---|---|
| lock_wait_ms | Native function entry through acquiring the Store lock. |
| source_load_decode_ms | Active document reads, preserved-source retrieval/verification and text preparation. |
| cache_parse_serialize_ms | Eligibility, cache lookup/validation, reuse/parsing and payload serialization. |
| cache_replace_ms | Persisting changed/deleted derived cache rows; unchanged payload rows are retained. The original implementation replaced all rows. |
| revision_publish_ms | History fingerprint/no-op check or graph binding/revision/blob/drift publication work. |
| transaction_body_ms | Transaction setup/body through measurement construction, before metrics persistence. |
| before_metrics_write_ms | Native entry through that same pre-persistence point; includes lock wait. |

Totals include overhead between phases and must not be reconstructed by assuming
an exact phase sum. recorded_at is a transaction timestamp, not a measured commit
completion timestamp. Cache hit/miss, unsupported/parse-error and loaded-document
counts describe the actual native invocation. A no-change run still performs
source/cache work; its shorter publication path is not a new revision build.

COMMIT, the metrics INSERT/serialization, embedding, FTS and caller/source-sync
preparation are **excluded**. MCP worker queue wait is excluded because native
measurement starts inside the worker. `index_code` now uses the existing bounded
blocking-work helper so native indexing does not execute on the async handler.
These measurements are not end-to-end request/sync latency, CPU time, peak memory,
benchmarks, representative performance or semantic quality evidence.

`index_status.stage_timings` reads the current revision's:

- publication_run: the successful run that actually created it;
- latest_successful_run: the latest successful invocation targeting it, including
  later no-change calls.

These identities are deliberately separate: repeat indexing cannot replace the
publication measurement. Older revision rows remain stored. Scope is always
`corpus_global`, including in a project-scoped status response. Publication/commit,
per-project, failure and other unmeasured boundaries stay explicit in the quality
unmeasured list. A legacy revision may have a later measured no-op but no measured
publication; the null publication entry remains visible.

No published revision, missing ledger, no measured run and invalid/unreadable
metrics have separate unavailable reasons. Invalid version/identity, nonfinite or
negative durations, incoherent input counts and incorrectly included excluded
work do not produce valid timing output. Raw corrupt JSON is not returned.
Repeated status reads do not append runs or reindex. The ledger currently retains
all successful invocations; pruning, failure-run telemetry, caller-trigger labels,
per-project stages and representative retention/throughput qualification remain
open. Metrics are operational observations, not tamper-proof audit signatures.
