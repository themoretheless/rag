# Rust receiver qualification — 2026-10-25

Scope: captured AST receiver typing, as described in
[RUST_RECEIVER_TYPES.md](../../RUST_RECEIVER_TYPES.md). The complete
[code intelligence plan](../../CODE_INTELLIGENCE_PLAN.md) remains open.

## Reproduce

```sh
cargo test --workspace --locked --offline
cargo clippy --lib --tests --locked --offline -- -D warnings
cargo build --bin rag-mcp --locked --offline
cargo check --manifest-path tests/fixtures/code-intelligence/rust-receivers/Cargo.toml --all-targets --locked --offline --target-dir /tmp/rag-rust-receivers-fixture-target
python3 scripts/smoke_rust_receivers.py target/debug/rag-mcp docs/qualification/rust-receivers-2026-10-25/runtime.json
```

The runner copies a compiler-checkable fixture into a temporary canonical path
and uses an actual stdio MCP process, isolated DuckDB and mock embeddings. The
production database, installed server, external models and source build scripts
are not used. Compiler checking is a separate explicit command.

Twelve expected callee/owner/file combinations are checked against labels stored
independently of the index. Exact sets reject additional wrong-owner edges.
Patterns include return chains, async await, explicit generic return arguments,
fields, slice/Vec indexing, standard pointer wrappers, local factory inference
and a same-named type in a remote declaration document. Inner calls sharing a
starting byte must retain their own targets. Additional lifecycle assertions
check no-op reuse, stale-provider omission, provider-return-only rebinding and
immutable historical diffs.

Native fixtures additionally test custom wrappers, unknown/unawaited receivers,
generic-name shadowing, lexical/destructuring/closure shadowing, branch
reassignment and associated functions that are not instance methods. Some native
negative fixtures intentionally test syntactically valid but compiler-invalid
inputs. The positive MCP fixture is checked with the actual Rust compiler.

## Status

The final workspace run passed **1268 tests, 0 failed, 1 ignored**; see
[workspace log](workspace-tests.log) and [checks/source fingerprints](checks.json).
Strict library/test Clippy passed; see [log](clippy.log). The positive workspace
passes offline compiler checking; see [compiler log](fixture-compiler.log).
Actual MCP qualification passed all 12 independently labelled calls and lifecycle
assertions; [runtime.json](runtime.json) records the exact binary hash and index
format 31. Only a successful runner writes this report.
No representative Rust precision/recall, complete trait/CFG/macro semantics or
competitor superiority is inferred from this qualification.
