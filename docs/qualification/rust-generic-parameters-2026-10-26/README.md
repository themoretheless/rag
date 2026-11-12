# Generic Rust parameter identity and pointer receivers

Index format 76 retains a lexical generic parameter's identity, captured trait
bounds and conservative implicit-Sized status during expression inference.
Previously a parameter without captured local trait IDs became Unknown, so
`(self as *const T).distance(...)` could not select a local pointer extension
trait implementation even when the surrounding generic parameter was Sized.

Parameter identities include their captured function scope and parameter name.
Repeated parameters can unify; distinct parameters do not unify merely because
they have identical bounds. Declared trait-bound method candidates remain trait
declarations, not concrete runtime implementors. Optional/unsupported bound
syntax such as `?Sized` cannot establish Sized; unknown, slice, str and dyn
pointees keep their existing conservative exclusions from `impl<T>` matching.
Pointer const/mut separation and sysroot method-name precedence remain enforced.

## Qualification

- Three new native regressions cover generic const/mut pointers and casts,
  repeated versus distinct parameters, retained trait-declaration targets, and
  inline/where-clause `?Sized` exclusions. All 87 focused Rust tests pass.
- Actual stdio MCP selects three exact declaration owner/path/line targets;
  ordinary warm indexing reuses the revision. Cargo executes one passing test,
  checking `[11,22,11]`. Two separate negative rustc controls reject the inline
  and where-clause `?Sized` calls with E0599/unsatisfied Sized evidence.
- Same pinned source/config/SCIP/CST and all 1183 labels: 465/570 -> **467/570**
  correct positive targets. Memchr improves 363/466 -> 365/466; itoa and ryu are
  unchanged. Two recovered sites are `memchr/src/ext.rs:33,37`; no formerly
  correct site is lost and no false local target appears. All 613 external
  negatives remain without local candidates; 608 have observations and five do
  not. All labelled native/MCP candidate sets match.
- Workspace tests: **1564 passed, zero failed, one ignored**. Strict library/test
  Clippy and five Python SCIP protocol/metric tests pass. Scoped diff checks pass.

Exact reports, independent labels, scored cases, per-site transitions, baseline
hashes, binary identities and logs are retained here. The runtime pair was copied
before all corpus runs: server SHA256
`1650abf294d2bc57585c5fd053556d9c39bf417950ab89cec3970aaa5adb89c3`,
native exporter SHA256
`d5e078e5626a3f05170959393681a44ebb76304f30eb3c7466074f78895991e6`.
Build is development profile; this does not replace the previously qualified
frozen release performance candidate or establish a speed/memory improvement.

The source checkout is shared and contains concurrent unrelated language work;
that work and its extra tests are not attributed to this change. Current source
hashes record retained audit inputs, not an atomic build snapshot. Corpus input
and label hashes, frozen executable identity and actual MCP behavior are checked.
Qualification uses exclusively owned disposable stores and compiler outputs.
The installed production executable and database were not independently opened
or replaced by these runners.

## Remaining scope

103 original positive labels remain misses, including disabled-branch labels
which intentionally stay in the original denominator. This is not complete
generic/compiler inference. Precise generic ownership across enclosing methods,
optional-bound blanket implementations, complete Sized/unsized rules, HRTBs,
associated types, return/field chains, macros and full Cargo configurations remain
under existing limits. Bounded source correspondence does not prove runtime
dispatch or universal precision. Equal-work release competitor performance and
the full original ten-requirement plan remain open.

Reproduction:

```sh
cargo test --offline --locked --lib rust_
cargo test --offline --locked --workspace
cargo clippy --offline --locked --lib --tests -- -D warnings
python3 scripts/smoke_rust_generic_parameters.py /path/to/frozen/rag-mcp /tmp/new-runtime.json
```
