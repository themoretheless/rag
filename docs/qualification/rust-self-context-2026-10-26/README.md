# Captured impl Self and inline declaration contexts

Format 60 captures impl-owner AST types in Function facts and uses original
inline-module context for declaration lookups. It fixes nominal receiver lookup
for lifetime-parameterized impls and inline impls/field types. Explicit method/impl
substitutions still take precedence. Symbol display names and source anchors are
unchanged. Contract: [RUST_RECEIVER_TYPES.md](../../RUST_RECEIVER_TYPES.md).

## Fixed repository controls

| Pinned library | Before correct / labelled | After correct / labelled |
|---|---:|---:|
| itoa | 9 / 9 | 9 / 9 |
| ryu | 93 / 95 | 93 / 95 |
| memchr | 164 / 466 | 256 / 466 |
| Combined | 266 / 570 | 358 / 570 |

The 92 newly resolved sites all belong to the direct-call subset, which improves
241/486 to 333/486 (49.59% to 68.52%). Combined callable-reference correspondence
improves 46.67% to 62.81%. Observed false targets remain zero in both phases;
imports, macro tokens and value/other labels retain separate metrics.

`summary.json` enforces identical source/configuration, SCIP and label hashes and
complete native/MCP candidate parity. The corpus pins, cfg overlays and admitted
label population are unchanged. Baseline artifact hashes reference
`../rust-reexports-2026-10-26/controlled-*-after/`; new reports, labels and full
scored cases are stored locally. The two improvements were applied together;
their individual contributions were not isolated experimentally.

This is a small source-definition correspondence corpus with no external-negative
sites and explicit ambiguous/generated/out-of-unit exclusions. It is not
whole-Rust precision/recall or runtime proof. The original SCIP denominator is
preserved, including previously documented disabled-cfg ryu sites. Memchr still
has 210 misses: 86 receiver-type observations, 71 nested-cfg observations, 36
macro-token observations, one shadowed/external observation and 16 sites without
an extracted reference. Those missing cases are retained, not hidden as success.

## Native and compiler checks

Four added native regressions cover lifetime impl Self and Self returns,
same-named root versus inline impls, super-relative field types and wrong-owner
exclusion, nested-function Self isolation, and unknown generic-field abstention.
The final focused Rust suite passes 56 tests. Existing legacy-cache, generic/
trait, source configuration, re-export and inference-budget regressions pass.

The compiler-valid fixture is
`tests/fixtures/code-intelligence/rust-self-context`; independent call labels are
stored in expected-call-targets.json. `runtime.json` records six exact actual-MCP
call targets, including both calls in a Self-returning method chain. A warm
index operation reuses both unchanged documents and creates no revision. Actual
Cargo test executes one passing unit test that distinguishes the owners by
different return values: lifetime Worker=3, root Item=1, inline Item=4 and the
super-relative provider Service=2 instead of unrelated root Service=99.

Full workspace: 1470 tests passed, zero failures, one ignored external-DB fixture.
Strict library/test/example Clippy and three Python oracle/metric regressions
passed. These totals include concurrent unrelated work, not attributed to this
change; the shared source checkout was not frozen.

All corpus and MCP fixture runs share frozen format-60 server SHA-256
708dd03fed1b2a1b30c1319bd7a24429abe76e723d312107e498bbecd5359227 and native
helper 16895bd138b5d42c985a6fb1c2bf162a958e5a740c254eb54f5169a90730ab25.
MCP and native helpers open only owned temporary stores, exclusively in sequence.
Production DuckDB/installed MCP are untouched. Timing fields are uncontrolled
debug diagnostics and do not prove a performance improvement.

## Reproduction and remaining work

```sh
cargo test --lib code_index::rust_
cargo clippy --lib --tests --examples -- -D warnings
cargo build --bin rag-mcp --example code_quality_capture
python3 scripts/smoke_rust_self_context.py /path/to/frozen/rag-mcp /tmp/new-runtime.json
```

Prepare the unchanged pinned corpus following the prior qualification, run
qualify_rust_corpus.py for each library with one frozen server/helper pair and
compare controlled-*-before/after folders using summarize_rust_quality.py.

This advances requirements 2/3/4 and does not close the original ten-item goal.
Unproven generic/associated/lifetime/const-generic cases, nested cfg and macro
expansion, broader representative corpora/external negatives and equal-work
release/competitor performance/RSS remain open. No commit or push occurred.
