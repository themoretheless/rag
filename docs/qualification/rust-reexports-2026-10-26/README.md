# Rust re-export qualification

Implemented captured Cargo-local re-export candidates, including named/grouped
aliases, multi-hop forwarding, module globs, cyclic globs, module/type aliases,
cfg filtering and conservative visibility checks. The complete original ten-item
goal remains active. Contract and unsupported cases: [RUST_REEXPORTS.md](../../RUST_REEXPORTS.md).

## Fixed public repository comparison

| Library | Before correct / labelled | After correct / labelled |
|---|---:|---:|
| itoa | 9 / 9 | 9 / 9 |
| ryu | 91 / 95 | 93 / 95 |
| memchr | 164 / 466 | 164 / 466 |
| Combined | 264 / 570 | 266 / 570 |

Observed false targets remain zero in both versions. Direct-call correspondence
improves 239/486 to 241/486. The same pinned source bytes, explicit compiler/cfg
overlays, SCIP oracle hashes and label hashes are enforced by
`scripts/summarize_rust_quality.py`; no labels were removed. Original baseline
artifacts remain under `../rust-cfg-expressions-2026-10-26/controlled-*-after/`.
`baseline-artifacts.json` pins their hashes. New raw run reports, labels and
scored cases are stored in the three local `controlled-*-after/` directories.

The recovered calls are ryu's raw::format32/raw::format64 through a public facade
over a private implementation module. Both remaining ryu SCIP misses are in
small-feature-disabled code. A separate same-rustc HIR path-anchor audit retains
the original 71 direct-call sites and corroborates 68 source positions; all
68/68 now resolve, with zero observed false targets. Three unverified HIR anchors
remain explicit: the two disabled references and a valid method with a different
HIR span representation. Missing anchors never prove inactivity. Compiler output
is only a source-activity check; target labels still come from SCIP.

This small correspondence corpus has no external-negative labels, excludes
ambiguous/generated anchors and is not representative whole-Rust precision or
runtime dispatch proof. Memchr's 302 misses remain visible. Receiver inference,
other nested cfg forms and macro expansion remain open; performance fields are
uncontrolled debug diagnostics. No competitor speed/quality superiority follows.

## Native and actual MCP/compiler checks

Seven namespace regressions cover multi-hop/grouped aliases and exact targets,
inline import scope, glob ambiguity, cyclic globs,
private targets, scoped visibility, crate boundaries, module alias bases, unknown
cfg alternatives, duplicate named exports and depth exhaustion after an earlier
candidate. Hardening tests reject associated methods and function-local items as
free imports, Type::* as a module glob, and private type/nested-module exposure
through a glob whose declaring module can access those private items.

The compiler-valid two-package fixture is
`tests/fixtures/code-intelligence/rust-reexports`, with independent labels in
`expected-call-targets.json`. `runtime.json` records actual stdio MCP runs in both
feature configurations. Eight labelled consumer calls select the original source
definitions, including a static method through an exported type alias. A
pub(crate) call within the provider still binds; the same import from the other
crate has no candidate and actual Cargo check rejects it with E0603. Both positive
Cargo test commands execute one passing feature-sensitive unit test. A
manifest-only switch reuses five unchanged source/manifest parses, excludes stale
bindings before refresh and records added/removed frozen graph edges.

All corpus runs use owned disposable stores. MCP stops before the native helper
opens that same store; the helper stops before MCP restarts. Every admitted
native/MCP candidate set agrees, with bounded target queries falling back to
exact symbol IDs on truncation. Production DuckDB and installed MCP are untouched.

Final qualification: 1461 workspace tests passed, zero failures, one ignored
external-DB fixture; 52 focused Rust tests passed; the final seven namespace
regressions passed after extra private-path assertions. Strict library/test/
example Clippy and the three Python label/metric regressions passed. Exact logs
and checks are retained locally. Full workspace includes concurrent unrelated
changes; they are not attributed to this feature. The shared source checkout was
not frozen.

The new facts began in format 56 and namespace hardening advances to format 59.
Concurrent translation-source work accounts for intervening format changes.
All final corpus, compiler-audit and MCP fixture reports share format-59 server
SHA-256 5954915eab06d4491570abd9db36d69284fb77e874ddbb9eafd828542b4eabe5
and native helper ec546b8a70d85c6c430cbef254d26d5430b87884e38bac82ae01e4b10c5b57f2.

## Reproduction

```sh
cargo test --lib code_index::rust_
cargo clippy --lib --tests --examples -- -D warnings
cargo build --bin rag-mcp --example code_quality_capture
python3 scripts/smoke_rust_reexports.py /path/to/frozen/rag-mcp /tmp/new-runtime.json
```

Use the pinned corpus preparation documented in the previous qualification, run
`qualify_rust_corpus.py` for all three libraries with a frozen server/helper pair,
and compare with `summarize_rust_quality.py` using controlled-*-before/after
directories. For the additional ryu source-activity audit:

```sh
python3 scripts/audit_rust_compiler_anchors.py /tmp/corpus/ryu /tmp/results/controlled-ryu-after /tmp/new-compiler-audit
```

No commit, push or production executable replacement was performed. Remaining
namespace/compiler semantics, broader corpus/external negatives, full plan gates
and equal-work release/competitor performance/RSS stay open.
