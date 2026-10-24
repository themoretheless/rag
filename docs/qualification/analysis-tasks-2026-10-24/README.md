# Analysis task/notes qualification — 2026-10-24

Scope: `task_set`, `note`, `notes`, `task_stats`, durable MCP index-call attribution,
session isolation, schema-v21 persistence and existing access-role enforcement.
Whole-RAGRAF parity, external databases, gate decisions and saved-call/quality
benchmarks are not claimed.

Final current-source verification:

- Complete library suite: **729 passed**, no failures, four test threads.
- File-ingestion integration suite: **3 passed**, no failures.
- The actual `rag-mcp` executable built successfully without default features.
- Actual stdio and authenticated loopback HTTP MCP smoke: **passed**. The fixture
  uses a temporary database, mock embeddings and disposable local credentials.
  [mcp-smoke.json](mcp-smoke.json) records the checked contracts; it contains no
  credential values or user corpus content.
- [source-sha256.json](source-sha256.json) identifies the owning source/test inputs
  in the verified dirty worktree; these are not committed/released build claims.

The smoke covers tool discovery, task labels, generation conflicts, clearing while
preserving history, distinct append-only notes, comma-preserving JSON tags, literal
substring filtering and stable older-page traversal. Actual successful, missing
and invalid index calls produce the expected durable outcomes. An explicit second
session does not alter the first session's counters; an unused task reports zero.
HTTP requires an explicit task session, unscoped calls do not inherit another
client's label, read credentials cannot write notes/change tasks, and read
credentials can inspect notes and statistics. The read/write tests use the real
HTTP dispatcher and role middleware, not just a permission-name table.

During qualification the real MCP test exposed metadata normalization: rmcp moves
request `_meta` into `RequestContext.meta` before handler dispatch. Attribution
now reads that authoritative context (with request metadata as a direct-handler
fallback); the final smoke proves both stdio and HTTP scope attribution.

An existing search timeout test repeatedly expired during cold identity queries
before reaching its intended provider phase under load. The test now uses paused
async time, a prewarmed identity and enough setup budget, retaining its explicit
embedding-timeout assertion. It additionally verifies that the provider was
entered, its pending future was dropped, and the corpus lane became writable.
Search production behavior was not changed by this test repair.

Reproduction from repository root:

```sh
cargo test --lib --test file_ingest_test --no-default-features -- --test-threads=4
cargo build --bin rag-mcp --no-default-features
python3 scripts/smoke_analysis_tasks.py target/debug/rag-mcp /tmp/rag-task-report.json
```

Limits: the production gateway was not restarted or migrated; optional tokenizer
features were not part of this run. Portable exports, cross-node control-record
replication, every REST/CLI call path, retention/compaction, performance at scale,
gate effectiveness and calibration remain unqualified/open. Session IDs route
labels; they are not credentials. Notes are unverified agent assertions, and
returned-record counts are not correctness or savings measurements. See
[ANALYSIS_TASKS.md](../../ANALYSIS_TASKS.md) and the open
[RAGRAF_FEATURE_MATRIX.md](../../RAGRAF_FEATURE_MATRIX.md).
