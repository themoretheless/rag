# Captured Cargo workspace qualification — 2026-10-25

Scope: the initial Cargo boundary implementation in
[RUST_CARGO_GRAPH.md](../../RUST_CARGO_GRAPH.md), not completion of the full
[code intelligence plan](../../CODE_INTELLIGENCE_PLAN.md).

## Reproduction

```sh
cargo test --workspace --locked --offline
cargo clippy --lib --tests --locked --offline -- -D warnings
cargo build --bin rag-mcp --locked --offline
cargo check --manifest-path tests/fixtures/code-intelligence/cargo-workspace/Cargo.toml --workspace --all-targets --locked --offline --target-dir /tmp/rag-cargo-fixture-target
python3 scripts/smoke_cargo_workspace.py target/debug/rag-mcp docs/qualification/cargo-workspace-2026-10-25/runtime.json
```

`runtime.json` is emitted only after every assertion passes. The runner copies a
checked-in fixture into a temporary canonical path and starts a real MCP server
with an isolated DuckDB and mock embeddings. It does not touch the production
database, start its watcher, call external providers or execute source build
scripts. Fixture compiler checking is a separate explicit qualification command;
the test workspace has no build scripts or proc macros.

The fixture labels five expected calls independently of the index, across a
workspace with three packages and five targets. Exact target path sets reject
extra wrong-owner edges. Additional assertions cover warm extraction reuse,
custom library roots, renamed inherited path dependencies, stale workspace facts,
manifest-only rebinding, unchanged source symbols and frozen history. Its cold
sync time is a single debug fixture observation, not a throughput benchmark.

Native regression cases additionally cover explicit module membership, ordinary
path attributes, orphan files, invalid/empty manifests, lib/bin/test isolation,
conflicting module files, custom crate names, nested workspace barriers and
unselected required-feature targets. Broader Rust semantics, representative
precision/recall, compiler configuration identity, performance and competitor
comparison remain unqualified.

## Evidence status

- Post-hardening workspace run: **1254 passed, 0 failed, 1 ignored**, exit 0;
  [log](workspace-tests.log), [summary and source fingerprints](checks.json).
- Strict library/test Clippy: exit 0; [log](clippy.log).
- Real MCP run: all five labelled calls have exact expected target sets, no
  wrong-owner links; all lifecycle assertions passed. [JSON report](runtime.json)
  records binary SHA-256 and index format 30. This is a debug-build fixture run.
- The checked-in fixture separately passes offline compiler checking for the
  complete workspace and all targets; no source build scripts exist in it.
