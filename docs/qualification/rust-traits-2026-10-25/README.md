# Rust trait/impl qualification — 2026-10-25

Scope: the bounded rules in [RUST_TRAIT_BINDING.md](../../RUST_TRAIT_BINDING.md).
The complete [implementation plan](../../CODE_INTELLIGENCE_PLAN.md) remains open.

## Reproduce

```sh
cargo test --workspace --locked --offline
cargo clippy --lib --tests --locked --offline -- -D warnings
cargo build --bin rag-mcp --locked --offline
cargo check --manifest-path tests/fixtures/code-intelligence/rust-traits/Cargo.toml --all-targets --locked --offline --target-dir /tmp/rag-rust-traits-fixture-target
python3 scripts/smoke_rust_traits.py target/debug/rag-mcp docs/qualification/rust-traits-2026-10-25/runtime.json
```

The positive fixture is a compiler-checkable four-source-file crate with explicit
labels independent of the index. The runner uses a real stdio MCP process,
temporary DuckDB and mock embeddings. It checks exact owner/file target sets for
cross-file inherent/trait impls, generic impl returns, aliased traits, UFCS,
default methods, generic/dyn declaration targets and associated projections.
Further assertions cover no-op reuse, stale impl omission while declarations
remain available, associated-type-only rebinding and immutable historical diffs.
No production database, external model, source build script or runtime vtable
inspection is used.

Native regressions include ambiguity, wrong-owner negatives, generic where-bound
evidence, qualified and generic associated projections, default Self returns and
cross-target inherent-impl exclusion. Unsupported compiler semantics remain
explicit; these are not representative precision/recall or speed measurements.

## Evidence status

The workspace regression run passed **1276 tests, 0 failed, 1 ignored** before
final budget/cache hardening; see [workspace log](workspace-tests.log). The
current receiver/trait suite then passed **20 tests**, including partial-search
rejection and format-31 cache migration; see [hardened log](hardened-tests.log).
Strict library/test Clippy passed; see [log](clippy.log). The positive fixture
passes offline compiler checking; see [compiler log](fixture-compiler.log).

Final real MCP qualification passed all **10** independent owner/file labels,
declaration-only dyn/bound targets and lifecycle checks; see
[runtime.json](runtime.json). The exact same format-32 binary also passes the
earlier [12-call receiver fixture](receiver-regression.json) and
[5-call Cargo fixture](cargo-regression.json). All three reports record its
SHA-256. [checks.json](checks.json) records source fingerprints and distinguishes
the earlier workspace run from the final hardening suite. These results do not
establish complete Rust compiler semantics or the complete user goal.
