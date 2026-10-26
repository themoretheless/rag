# Rust conditional-member and extern-namespace qualification

Status: bounded milestone passed; full code-intelligence plan remains open.

## Controlled quality comparison

| Repository | Before correct | After correct | Labelled sites | False targets after |
|---|---:|---:|---:|---:|
| itoa | 9 | 9 | 9 | 0 |
| ryu | 93 | 93 | 95 | 0 |
| memchr | 256 | 308 | 466 | 0 |
| Combined | 358 | 410 | 570 | 0 |

Direct calls improve 333/486 to 364/486, imports 25/39 to 39/39 and
value/other references 0/7 to 7/7. Macro-token references remain 0/38.
All 52 recovered sites are in memchr; zero previously correct sites are lost.
Transitions retain before/after observations, source paths and original ranges.

The baseline is the previous Self-context milestone's captured results, hashed
in baseline-artifacts.json. All three pinned repositories preserve source bytes,
Cargo/config inputs, original independently derived rust-analyzer SCIP/CST oracle
and labels. summarize_rust_quality.py verifies input/oracle/label equality,
recomputes metrics and checks one binary/helper pair per phase. All labelled
observations have native/actual-MCP parity. These are source-definition
correspondence labels, not observed runtime dispatch or universal Rust precision.
The original 570-site denominator is unchanged, including disabled branches.
The corpus has no external-negative labels; independent RA error logs, excluded
ambiguous/generated anchors and scope limits remain in the original corpus report.

## Implementation and negative cases

Named struct fields retain raw cfg alternatives with exact owner IDs and source
anchors. Unknown/duplicate alternatives do not yield a unique receiver. Field
initializer attributes project their owner AST node, including early-return
policy suppression. Unsupported descendants in known-disabled owners no longer
block unrelated live binding. Disabled item macros need no expansion; active
conditional item macros remain unmodeled.

Namespace declarations retain original cfg anchors despite lexical hoisting.
Reserved std/alloc extern tags distinguish actual extern aliases from ordinary
use imports and legal source names. Negative regressions cover local/root
shadows, custom standard-named dependencies, namespace alias collisions,
duplicate/unknown fields, inactive owner fields, ordinary-use versus extern
resolution and preserving existing direct captured Cargo dependencies.
Nonstandard renamed extern crates remain unverified.

## Frozen runtime and validation

Final server SHA-256:
120940eeb0290e804d6c223032c3ea0ac1f56bf0a732654efabe7ec915353e32.
Native helper SHA-256:
5af9b8691606696d6b370fd99e99bc13ae50297c5952e964129c74f6e853bcea.
Index format 66. All corpus runs and runtime.json use this final server.

The compiler-valid rust-cfg-members fixture switches fast/slow features through
captured manifest metadata, checks eight exact MCP targets, excludes stale edges
before refresh, reindexes one changed manifest with one unchanged source, verifies
warm no-op revision reuse and captures frozen graph changes. Cargo executes one
passing test per configuration, including distinct Fast/Slow values and std Vec
receiver identity. See scripts/smoke_rust_cfg_members.py and runtime.json.

Workspace: 1510 passed, zero failed, one ignored. Focused Rust: 65 passed.
Strict library/test/example Clippy passed. Python SCIP adapter/metric tests: three
passed; smoke script byte-compiles. Logs and checks.json retain validation.
The shared dirty checkout includes unrelated concurrent work; it was not frozen
and those changes are not attributed to this milestone. Runtime executables were
frozen before all final corpus and fixture runs. Production binary was not
replaced; qualification used exclusively owned disposable stores.

## Remaining work

160/570 labels remain unresolved (158 memchr and two ryu); macros, receiver
inference and missing extracted references remain. Full Cargo compilation-unit
and feature unification, complete compiler/namespace/trait semantics, macro
expansion, union/tuple/enum field coverage, broader external-negative corpora and
representative quality are unqualified. No competitor superiority, release speed
or RSS claim follows from these debug-build quality runs.
