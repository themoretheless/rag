# Standard architecture types in generic wrappers

The final format81 runtime gives imported standard architecture structs a
bounded source-backed identity, allowing captured generic-wrapper/local-trait
relationships to resolve. It distinguishes std/core aliases, exact vector types,
architecture selections, shadows and ambiguous globs. Invalid inherent impls on
external types are not accepted; direct external member lookup stays unverified
unless an explicit trait UFCS selection supplies the required evidence.

Two additional barriers were reproduced and fixed: Rust binding capture
discarded glob names, and an inactive optional core dependency was treated as an
active shadow. A separate adversarial regression also exhausted inference work
while testing unrelated trait impls. The solver now filters definite trait-name
mismatches before owner matching while retaining exact identity checks and the
same work limits. Exhaustion never publishes a partial target set.

## Accepted evidence

- Five new semantic regressions and one cfg regression qualify wrapper imports,
  aliases, wrong vector types, architecture/namespace/glob guards, invalid inherent
  impls, optional core activation and unrelated-impl work. All **102 focused Rust
  tests pass**. Some adversarial snippets are intentionally compiler-invalid;
  they qualify conservative AST behavior, not compiler acceptance.
- Actual stdio MCP checks **three exact targets** across multiple source files:
  two generic-wrapper methods and one explicit local-trait UFCS implementation.
  A decoy same-name method is excluded. Ordinary warm indexing reuses its
  revision. Cargo executes **one passing aarch64 test** with results `(7,9,7)`.
- Same pinned source/config/SCIP/CST and1183 labels with exact native/MCP candidate
  parity: **478/570 ->502/570** positives; memchr376/466 ->400/466. Twenty-four
  SIMD-wrapper calls recover their exact local definitions. No previously correct
  site is lost and no false local target appears. All613 external negatives
  remain without local candidates;608 have observations and5 do not. Positive
  recall is88.07%; pooled exact/negative abstentions are separate metrics.
- Full workspace **1600 passed, zero failed, one ignored**; strict library/test
  Clippy, five SCIP protocol/metric tests and two catalog-generator guards pass.
  Independent triage rechecks every source hash, oracle case and score.
- The compiled catalog regenerates byte-for-byte from the recorded rust-src
  inputs. All ten source hashes were rechecked. Module entry counts are112
  aarch64,104 arm,16 x86,16 x86_64 and1 wasm32. These are selected names, not a
  claim that all APIs/configurations were compiler-tested. Only the two aarch64
  fixture types were exercised by the compiler/runtime test.

The frozen final pair was copied before all accepted runs: server SHA256
`2ac909626475e59c3e533698c3d9bce25b82b7b353842fef2eee4ed7bd7e88a8`,
native exporter SHA256
`23d635628d6d6d5b0ea56808c113be3043fa20c497c4e7947e71bb4ff5f1a125`.
The initial implementation used format80; final shared format81 includes a
concurrent unrelated schema change. Shared dirty source and current source
hashes are not an atomic compiled snapshot; unrelated changes/tests are not
attributed here. This development build does not replace the release benchmark
candidate or qualify speed/memory/competitor superiority. Production executable
unchanged; qualification stores/compiler outputs are disposable and owned.

## Retained unsuccessful controls

`before.log` and `budget-before.log` retain the original failing regressions.
`preliminary/` retains the two guard failures and diagnostics before glob capture
was fixed. The first corpus cohort, before optional-core/budget corrections,
remained478/570 despite passing the small runtime fixture. Its summary and raw
artifact hash/path inventory are retained; the complete raw cohort remains at
`/tmp/rag-stdarch-types-20261026/qualified`. Only qualified-v2 is accepted here.
Source/label equality and exact per-site transitions prevent an aggregate-only
claim of improvement.

68 original positives remain unresolved:10 receiver,38 macro,18 missing
observations in disabled branches and2 other bindings. Their denominator stays
unchanged. Complete compiler/Cargo/sysroot/macro semantics, broader datasets,
equivalent-work release competitor measurements and all remaining original/
follow-up plan milestones remain open. Contract: [RUST_STDARCH_TYPES.md](../../RUST_STDARCH_TYPES.md).

```sh
cargo test --offline --locked --lib rust_
cargo test --offline --locked --workspace
cargo clippy --offline --locked --lib --tests -- -D warnings
python3 -m unittest discover -s scripts -p test_stdarch_catalog.py
python3 scripts/smoke_rust_stdarch_types.py /path/to/frozen/rag-mcp /tmp/new-stdarch-runtime.json
```
