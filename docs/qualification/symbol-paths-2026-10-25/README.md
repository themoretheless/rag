# Symbol path qualification

Contract: [SYMBOL_PATHS.md](../../SYMBOL_PATHS.md). This implements plan
requirement 7 over the supported captured symbol relationships; graph extraction
coverage and compiler/runtime semantics retain their documented limits.

The independently labelled `tests/fixtures/code-intelligence/symbol-paths`
contains two cross-file three-edge call paths, a value reference and ambiguous
short names. `expected.json` supplies complete expected symbol sequences for
this finite fixture. `scripts/smoke_symbol_paths.py` uses an actual stdio MCP
process and a temporary database with mock embeddings.

Evidence:

- `runtime.json`: exact labelled paths, per-step document/revision/hash/line,
  deterministic results, typed relation input, depth/work/result caps, reverse
  traversal, ambiguity and exact-ID disambiguation, value-reference opt-in,
  cross-project rejection, captured-source semantics, stale bridge exclusion and
  graph refresh after bridge removal. Binary and fixture SHA-256 are recorded.
- `native-tests.log`: native tests additionally exercise ambiguous edge opt-in,
  cross-project candidate exclusion, inheritance/override candidate labels,
  identity paths and input-order determinism.
- `compiler.log`: actual offline Cargo check of the positive fixture.
- `workspace-tests.log`, `clippy.log`, `build.log`: final regression, strict lint
  and binary build outputs. See `checks.json` for command outcomes and counts.
- `config-regression.json`: captured Rust configuration lifecycle passes on the
  same format-34 binary as the final path runner. Format 34 also contains
  concurrent baseline file-import work; this directory does not attribute that
  work to the symbol-path implementation. The worktree was not frozen.

Reproduction:

```sh
cargo test --workspace
cargo clippy --lib --tests -- -D warnings
cargo build --bin rag-mcp
python3 scripts/smoke_symbol_paths.py target/debug/rag-mcp /tmp/symbol-paths.json
cargo check --manifest-path tests/fixtures/code-intelligence/symbol-paths/Cargo.toml --all-targets --offline --target-dir /tmp/symbol-path-fixture
```

An earlier workspace run caught missing MCP access/telemetry registrations;
the current registrations include `trace_path`, with read access and graph
telemetry. The evidence set reports the final rerun, not the earlier failed run.
No production database or executable is replaced. No representative repository
precision/recall, runtime path, complete absence or competitor-speed claim is
made. The broader ten-item plan remains active.
