# Captured dependency binding cache

Native indexing reuses reference/call bindings independently of AST reuse.
`code_index_documents` is bulk-read once; unchanged serialized payloads are kept
and only changed/deleted rows are written in the existing transaction. Failed
publication/metric writes still roll back the entire cache/index update.

Immutable graph `resolution_cache` contract 1 is versioned by the index format.
Entries contain a source/input key, derived edges and checksum. The source key
includes its complete configured extraction. Known Rust packages use captured
transitive path/workspace dependencies, source inventory, and all project
manifests as conservative guards. Optional/dev dependencies and unowned Rust
sources are retained conservatively. Missing providers remain part of
invalidation: scopes are recomputed from current inputs, not inferred from old
successful edges. Invalid boundaries and mixed/other-language projects fall
back to project scope.

Rust provider projections omit source hashes/revisions, documentation,
reference-site vectors and unrelated policy/SQL/style/resource facts. All other
facts, including declaration identity/spans, fields, returns, imports,
lexical/type facts, expressions, modules and cfg/trait data are retained. Thus
span/semantic body changes can invalidate dependents; a same-size literal change
can reuse callers while updating its own evidence. Other languages retain full
provider extractions. Future resolver dependencies must update this projection,
version and differential tests. This is not compiler proof.

Empty-reference documents can reuse empty edges without provider keys. Entries
are checked against source references, counts, checksum and known target IDs;
incompatible/corrupt entries are recomputed. Public edge order is canonical.
Other graph families (inheritance, entrypoints, SQL, imports/resources, styles,
policies) retain normal recomputation. Read queries use cached bindings only
when their fresh/test-view keys match. Stale/removed/unindexed providers change
dependent keys. Queries never repair the cache or rewrite immutable revisions.

`index_code` accepts `force_full_resolution` (boolean, default false). For
unchanged revision inputs, true recomputes all bindings and compares them with
immutable reference facts. Disagreement fails; agreement reports
`resolution.equivalent_to_existing=true` without creating a new revision. For
changed inputs, it publishes using full binding computation. This control is
not forced reparsing or reingestion.

Reports separate AST `indexed`/`unchanged`, existing stage metrics, and:

- `resolution`: documents/reference sites resolved or reused, Rust/project scope
  counts, empty-reference documents, dependency inputs, invalid entries, forced
  mode, elapsed resolution/cache time and equivalence.
- `completion`: native time through successful commit, commit-call duration,
  metric-write duration and Linux/macOS process-lifetime high-water RSS.

A true no-op skips resolution and does not invent cache-hit counts. Completion
includes lock wait, parse/publication, metrics write and commit, excluding
embedding/FTS ingestion and MCP queue/serialization/round-trip time. RSS is actual
`getrusage` process high-water RSS, not an operation peak or managed budget;
unsupported platforms report unavailable. Completion is returned after commit.
The older committed timing ledger still records only its pre-commit contract;
there is no second update presented as the original commit measurement.

Qualification/raw measurements:
[incremental evidence](qualification/incremental-resolution-2026-10-26/README.md).
Workloads include cold sync, warm no-op, body, signature and manifest changes,
sampled process RSS, actual Cargo checks and full recomputation audits. Cold sync
includes ingest/FTS; other timed index calls exclude ingestion. Alternating
same-binary cached/full controls isolate binding reuse; the forced warm control
is an audit, not an ordinary no-op. Old/new single runs are exploratory and
include concurrent baseline changes. Raw/rejected controls remain recorded.

Open: representative repository/time/memory results, equivalent competitor
comparisons, narrower other-language scopes, broader Cargo semantics and quality
qualification. Cached edges add immutable storage and can increase disk/RSS.
No universal latency/memory reduction or competitor superiority is claimed.
Requirement 4's real-repository precision/recall corpus remains separate.
