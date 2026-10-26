# Explicit Rust configuration qualification

Scope: [one captured compilation target per package](../../RUST_BUILD_CONFIGURATIONS.md),
index format 33. This is fixture evidence, not whole-Cargo or real-corpus proof.

- `code-index-tests.log`: 243 current code-index tests passed, including seven
  configuration tests, invalid configuration derived-fact exclusion, duplicate
  manifest refusal, manifest-only raw-cache reuse and frozen configured history.
- `clippy.log`: strict `cargo clippy --lib --tests -- -D warnings` passed.
- `build.log`: debug MCP binary build passed.
- `compiler-fast.log` and `compiler-default.log`: the fixture compiles in both
  actual Cargo feature selections, offline and with separate temporary targets.
- `runtime.json`: real stdio MCP; explicit fast/leaf versus default/slow, module
  gating, target exclusion, contextual IDs, source body identity, no-op reuse,
  stale manifest barrier, manifest-only refresh and immutable context/edge diff.
- `cargo-regression.json`, `receiver-regression.json`, `trait-regression.json`:
  the same binary passes the previous independently labelled 5/12/10 call-target
  MCP fixtures and their lifecycle assertions, without explicit configuration.

Reproduce from the repository root:

```sh
cargo test --lib code_index::
cargo clippy --lib --tests -- -D warnings
cargo build --bin rag-mcp
python3 scripts/smoke_rust_cfg.py target/debug/rag-mcp /tmp/rust-cfg.json
cargo check --manifest-path tests/fixtures/code-intelligence/rust-cfg/Cargo.toml --all-targets --no-default-features --features fast --offline --target-dir /tmp/rust-cfg-fast
cargo check --manifest-path tests/fixtures/code-intelligence/rust-cfg/Cargo.toml --all-targets --offline --target-dir /tmp/rust-cfg-default
```

All runtime runners create isolated temporary stores; production DuckDB is not
opened. Binary SHA-256 is recorded in each report. Compiler commands validate
positive source fixtures separately; the MCP runner does not invoke a compiler.
The earlier workspace-wide run passed before final configuration hardening;
current focused qualification above covers the final configuration implementation.
Later path-feature edits are outside this frozen runtime artifact's scope.

Open: multiple compilation units, cross-dependency feature unification, complete
Cargo workspace/target/cfg/macro semantics, independently labelled real-repository
quality, memory/latency and equivalent competitor measurements.
