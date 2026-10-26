# Native indexing timing subset — 2026-10-25

Previously `index_status` always reported index-stage timings as unavailable.
The new [contract](../../INDEX_TIMINGS.md) records real successful native phase
measurements, scoped to the shared corpus and linked atomically to the committed
cache/revision. Publication and subsequent no-change run identities are separate.
Schema migration is 24→25; syntax index format stays 24.

Four owned-DuckDB tests check finite/nonnegative phases and real input counters,
no-op revision reuse without overwriting publication timing, persistence across
reopen and newer revisions, rollback of cache/publication when ledger writes fail,
explicit missing/corrupt/legacy observations, old-schema migration preserving cache
and revision, and global timing scope in project-filtered status. The failure test
forces ledger sequence overflow after a new revision/cache has been prepared;
rollback is checked against the prior payload, revision and record count.

The real stdio MCP scenario covers source-sync measurement persistence, explicit
index_code through the blocking worker, publication/no-op separation, finite phase
values, excluded work, unchanged status reads, old measurements during source
staleness, a new publication's own run, and exact global-vs-project counts. It uses
only an owned temporary database with mock providers and no production restart.

Reference scope is phase visibility in `op_index_status` at pinned RAGRAF revision
`075caf86dc2f6274b9c48487e521915bcdfeaf2c`. Native phase names/boundaries differ from
reference pipeline stages; no timing/throughput superiority is claimed. Commit,
failure, per-project, full sync/MCP latency, retention, representative performance
and full parity remain open.

```sh
cargo test --lib code_index::timings --locked --offline
cargo build --bin rag-mcp --locked --offline
python3 scripts/smoke_index_timings.py target/debug/rag-mcp /tmp/index-timings.json
```

Final verification: **1,199 workspace tests passed, 0 failed, 1 ignored**,
including all four new timing/migration/rollback tests. Workspace Clippy with
`-D warnings`, formatting, `git diff --check` and the UI client check without
default/direct-DB features passed. The rebuilt binary passed
[timing MCP evidence](runtime.json) and [status](status-regression.json),
[catalog](catalog-regression.json), [routes](routes-regression.json) regressions.
The runtime artifact retains actual publication/no-op/new-publication measurement
examples; their small mock workload timings are observations, not benchmarks.
[Verification manifest](verification.json) records source/binary hashes and gates.
Production databases/services remain untouched.
