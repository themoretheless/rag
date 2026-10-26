# Index-status subset qualification — 2026-10-25

This replaces the flat count-only status response with a versioned capability and
measured-quality envelope. It preserves earlier count/static-capability aliases,
adds explicit project scope and evidence/sample controls, and makes empty,
unavailable and unmeasured states distinguishable. It does not establish full
RAGRAF parity or representative quality superiority.

Six owned-DuckDB module tests cover:

1. Missing/published-empty indexes and valid structural facts with no inspected
   vectors: none is reported as fully healthy.
2. Real stored zero vectors, wrong dimensions, malformed vector JSON, oversized
   token estimates, project scope, evidence/sample limits and no FTS/revision
   repair as a side effect.
3. Stale evidence windows, unsupported Ruby source, graph-cache corruption with
   unavailable/null measurements and isolation from other project scopes.
4. Large text/vector cells explicitly marked unmeasured without emitting bodies.
5. A 100-row Unicode sample exhausting the 4 MiB inspection budget, and finite
   nonunit vectors correctly measured rather than rejected as invalid.
6. SQL read evidence with no declarations is recognised as structural evidence,
   rather than falsely reported as an empty index.

[Real stdio MCP evidence](runtime.json) exercises the rebuilt binary with a fresh
isolated database and mock embeddings. It checks the public parameter schema,
legacy calls without an arguments object, empty/unindexed behavior, measured vectors/tokens, explicit CPG degradation, a
bounded sample, no source-body output or revision/FTS rebuild, project isolation,
global stale-state reporting and read-transaction/unknown-health labels.

Reproduce:

```sh
cargo test --lib code_index::status
cargo build --bin rag-mcp
python3 scripts/smoke_index_status.py target/debug/rag-mcp /tmp/index-status-runtime.json
```

The reference scope is `op_index_status` / `_quality` in pinned RAGRAF revision
`075caf86dc2f6274b9c48487e521915bcdfeaf2c`, inspected from the local reference
checkout. The reference also reports revision/coverage, quality and backend
capabilities. This qualification proves the local subset above; reference
per-stage persisted timings, complete docblock/typed-signature metrics, optional
CPG, representative quality/recall, broader failure recovery and scale evidence
remain open. Sampling bounds are inspection bounds, not whole-operation physical
I/O or latency guarantees. Production databases/services were not changed.

Final verification on the same frozen Rust source tree: **1,173 workspace tests
passed, 0 failed, 1 ignored**; all six focused status tests passed. Workspace
Clippy with `-D warnings`, formatting and `git diff --check` passed. The rebuilt
binary passed the status MCP scenario and the
[repo-map](repo-map-regression.json) /
[explicit SQL defaults](explicit-defaults-regression.json) MCP regressions.
[Verification manifest](verification.json) records the base commit and working-tree
source/binary SHA-256 hashes. These are isolated mock-provider checks, not
production-service or representative-scale qualification.
