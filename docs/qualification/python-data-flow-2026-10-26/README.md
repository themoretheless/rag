# Python value-dependency slices — 2026-10-26

[Contract](../../PYTHON_DATA_FLOW.md).

The previous `slice` operation remains a structural call subgraph. The new
`data_slice` implements local def/use and explicit positional argument/return
relations as a distinct native backend. The pinned RAGRAF facade requires full
Joern capability for its slice; no Joern runtime or comparative whole-CPG quality
claim is made here.

Nine native cases cover latest-write semantics, exact Unicode spans, ambiguous
definition selection, Python NFKC binding identity with retained raw spelling,
unimported-module rejection, forward/backward call-site isolation, actual argument and
return boundaries, constant-return non-influence, imported function/project
scope, unsupported syntax/global/module rebinding, independent budgets, stale
sources, persistent cache repair and separately unavailable value-relation diff
with frozen capture provenance. Initial compile diagnostics are retained in the
focused logs; the final native pass is recorded separately.

The real MCP scenario verifies tool registration, node/edge/witness shape,
context-stack transitions, forward/back/both isolation, local-write versions,
UTF-8 spans, explicit ambiguity/unsupported results, independent budgets,
project isolation, structural-slice distinction, capability reporting,
warm/full-resolution consistency, stale callee boundaries, sync changes and
immutable capture/diff availability. Existing SQL, test/Git/trait and index
surfaces are rerun against the same copied executable.

All fixtures use temporary DuckDB stores, mock embeddings and disabled LLMs.
Analyzed Python/PHP/SQL source is never executed. Existing Rust command fixtures
run only their own temporary package. No production database or source checkout
is indexed. The executable is copied and hash-checked; selected working-tree
hashes are observed without claiming a frozen source checkout.

These are source-level value candidates for a declared subset. Control-flow,
heap/alias analysis, full Python/compiler/runtime semantics, other languages,
normalized historical value comparison and representative quality/scale remain
open. Runtime/whole-program/absence proof flags remain false. Full RAGRAF parity
and overall superiority are unproven.

See final logs and verification.json for final gates and identities.

The initial MCP harness looked for the structural slice marker at the wrong
response level; its traceback and failed verification are retained separately.
The correct `detail.structural_only` assertion preserves that existing contract.
A final capture review added a leading f-string guard (with native and MCP
regressions) and NFKC local/shadow/export binding keys. The local Python AST
parser oracle records `𝒙` as binding `x` without executing the analyzed source.
Capture format 3 invalidates earlier value facts and checks async through AST
keyword tokens, including tab-separated syntax. Both refinements are included
in final gates; the earlier executable was no longer available for a separate
formatted-string before replay, and no such runtime before claim is made.

Final gates (native-final4/workspace-final4/clippy-final4/build-final4):
**1625 workspace tests passed, 0 failed, 1 ignored**. All nine focused value-flow
cases passed, including NFKC overwrite/shadow keys and AST async guards. Seven
status cases additionally passed while updating partial capability reporting.
Workspace/all-target Clippy with `-D warnings`, binary build, formatting, Python
syntax checks and `git diff --check` passed. All eight real stdio MCP suites
passed on one copied executable: Python data flow, PHPUnit host impact,
effective PHPUnit, PHP affected contexts, existing affected tests, SQL table
impact, trait-aware trace and Git contexts. The new scenario passed 21 checks.
Runtime index format **82**, store schema **25**, value capture/frozen marker
**3**. Executable hash and selected observed source/dependency hashes stayed
unchanged during the final MCP run. Earlier failures/intermediate qualifications
remain retained separately and do not replace these final gates.
