# Rust tuple struct field qualification

Bounded positional-field milestone; full code-intelligence plan remains open.

## Controlled fixed-corpus results

| Repository | Before correct | After correct | Labels | False targets after |
|---|---:|---:|---:|---:|
| itoa | 9 | 9 | 9 | 0 |
| ryu | 93 | 93 | 95 | 0 |
| memchr | 308 | 313 | 466 | 0 |
| Combined | 410 | 415 | 570 | 0 |

All five recovered sites are direct calls through tuple struct fields:
`self.0.as_slice` and four `self.0.byteset.contains` sites. No previously correct
site is lost. Imports, value references and macro-token results are unchanged.
Source/config/oracle/labels are byte-identical to the preceding conditional-member
milestone. baseline-artifacts.json hashes those authoritative artifacts, verified
against the copied baseline. summarize_rust_quality.py recomputes metrics, checks
label/input identity, native/MCP parity and one server/helper pair per phase.
Transitions retain original ranges and before/after observations.

Original rust-analyzer SCIP/CST source-definition correspondence labels and their
570-site denominator remain unchanged. This small corpus has no external-negative
labels. RA errors, excluded ambiguous/generated anchors and configuration limits
remain as recorded in the original corpus qualification; these measurements are
not universal Rust precision or observed runtime dispatch.

## Implemented behavior and boundaries

Format 67 captures positional type fields only when their immediate owner is a
tuple struct. Named visibility nodes are not slot indices. Existing exact nominal
owner and generic substitutions flow through nested numeric field projections.
Out-of-range slots and enum payloads cannot borrow another owner's method.
Any tuple field attribute suppresses the layout, because conditional removal
renumbers subsequent slots; even inert attributes currently remain unsupported.
Built-in tuple values, enum destructuring, custom Deref/Index and complete
constructor/flow typing are not implemented by this change.

Two native regressions cover nested/generic ownership, distinct same-name method
targets, missing positions and unknown conditional/enum layouts. The actual MCP
fixture checks four labelled targets and warm no-op revision reuse. Cargo executes
one test checking the four distinct values [11,22,11,22]. The compiler fixture is
positive; deliberately invalid out-of-range accesses occur only in native
negative extraction tests, not as compiler-valid programs.

Frozen server SHA-256:
9afbb477667f8eee914785421011e05ed0b19e7a380f120090f72aec3d18d48d.
Native helper SHA-256:
9345277ab0f425eacd868818dd0f49caa229de3dabd0f95caba66899e368f296.
All final corpus and runtime runs use this pair. Focused Rust tests: 76 passed.
Strict library/test/example Clippy passed. Workspace: 1512 passed, zero failed, 1 ignored. Three Python
adapter/metric tests pass. checks.json and workspace.log retain the result.

The source checkout is shared/dirty and not frozen; unrelated concurrent changes
are not attributed to this milestone. Runtime executables were frozen before
qualification. Production executable was not replaced and runtime qualification
uses disposable exclusively owned stores. No release-speed, RSS or competitor
superiority claim is made. Remaining 155 corpus misses and broader compiler/build
semantics, negative quality and comparative performance keep the full goal open.
