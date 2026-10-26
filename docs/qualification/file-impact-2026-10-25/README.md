# File dependency impact — 2026-10-25

This transfers the remaining file-target navigation scope from RAGRAF's impact
contract (reference pinned at 075caf86dc2f6274b9c48487e521915bcdfeaf2c) into an
explicit local target mode. [Contract](../../IMPACT_RELATIONS.md#file-targets).
The graph uses captured bound occurrences, including ownerless module values,
plus optional structural candidates. It does not claim complete import/resource
or runtime dependency resolution, or measured reference accuracy superiority.

Four regressions cover ownerless values feeding transitive file callers; bounded
stable traversal and stale target/bridge exclusions; identical paths across
projects with document-ID selection and mixed-target rejection; and cyclic files,
missing targets and invalid modes. Existing symbol/table impact cases also run.
The real stdio MCP fixture checks module registration without a caller, a second
file hop, call/reference separation, depth/item caps, exact document selection,
source-sync reuse/refresh, stale bridge removal and absence of source bodies.
Owned temporary sources/databases and mock providers isolate the scenario.

Format 29 and schema 25 remain unchanged. Unresolved namespace/module imports,
assets, generated/runtime links and representative quality/scale remain open.
Full RAGRAF parity remains open. Final evidence: verification.json and runtime.json.

Final gates: **1,242 workspace tests passed, 0 failed, 1 ignored**. Four new
file-impact and ten existing symbol/table impact cases passed. Clippy with
`-D warnings`, formatting and `git diff --check` passed. Rebuilt binary passed
file, table and symbol-impact stdio MCP scenarios. Source/binary hashes and logs
are retained here. These controlled fixtures do not establish exhaustive recall.
