# Python branch value-flow qualification — 2026-10-26

Native value capture format 4 / extraction format 84 adds bounded `if/elif/else` value merges and condition dependencies to `data_slice`. It preserves incoming definitions on an absent-else path, drops returned paths from subsequent reaching definitions and rejects a later read not initialized on all continuing paths. Source IDs, raw token spans and matched interprocedural call frames remain intact. See [contract](../../PYTHON_DATA_FLOW.md).

Final validation:

- `cargo test --offline --workspace`: **1637 passed, 0 failed, 1 ignored**, `workspace-final.log`. Includes 14 value-flow tests: nested/multi-arm merges, Unicode identities, early/all-arm returns, incomplete initialization, irrelevant conditions, branch depth limits, prior format-3/missing fact repair, immutable capture and call-site isolation.
- Strict workspace/all-target Clippy passed, `clippy-final2.log`.
- Binary build passed, `build-final2.log`.
- Eight real stdio MCP scripts passed on one copied executable, `verification.json`: Python value flow, Python bindings, SQL table impact, PHPUnit host dependencies, effective PHPUnit tests, affected tests, PHP contextual trace and Git impact.
- Python value-flow script has 27 named checks (`python_data_flow.json`), including merges containing calls, conditions and branch-local values, implicit-else definitions, early-return exclusion, unrelated condition exclusion, partial initialization and loop guards. It also validates witnesses, independent budgets, warm/full resolution equivalence, stale callee boundaries, source sync and frozen capture availability.
- SQL table impact retained its 15 passing scenarios (`table_impact.json`).

Binary SHA256: `7fcc37e4bcfdc2f5e1e98d634740b522ba3a09e0c26783cea84f890ea1b70023`.
`verification.json` records selected source/dependency hashes before/after MCP execution: selected files and copied executable were unchanged. Shared source was not frozen. `run_mcp.py` reproduces the MCP batch from the repository root.

The first focused run retained the old expectation that any branch must be unsupported; the assertion was updated alongside positive branch tests (`focused.log` retained). The first MCP run used an unimported `identity` spelling where the fixture imported `chosen`; the resolver correctly reported unverified boundaries. Fixture spelling was corrected; `python_data_flow-initial.log` and `verification-initial.json` are retained. No product change was made to accept that unimported target.

All analyzed Python/SQL remained text, not executed programs. MCP used temporary databases, mock embeddings and disabled LLMs; production data was untouched.

Scope limits: these are syntactic may-dependencies with bounded reaching-definition merges and condition edges. No predicate polarity/feasibility/correlation proof, complete CFG, loops/exceptions/heap aliases, runtime path proof, whole-program slice or normalized value-history diff is claimed. Witness/edge unions can combine mutually infeasible syntactic paths. Representative quality/performance and full RAGRAF/CPG parity remain open.
