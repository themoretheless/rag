# Frozen Python value-history diff qualification — 2026-10-26

`diff.data_flow_comparison` now compares compatible capture-5 immutable source/value graphs using normalization format 1. It separates node syntax-fingerprint changes from local/cyclic and uniquely bound positional-call edge changes. Source positions are evidence rather than comparison identity; ordinary spacing/comments and NFKC spelling changes are normalized. See [contract](../../PYTHON_VALUE_DIFF.md).

Coverage guards exclude unsupported functions and incomplete, mismatched or missing documents on both sides. AST/symbol/fact function inventories must agree; omitted facts cannot impersonate source deletion. Frozen call provenance and endpoint ownership are validated. Legacy capture versions are unavailable and do not invent zero totals. All source/runtime semantic and absence proof flags remain false.

Final validation:

- `cargo test --offline --workspace`: **1655 passed, 0 failed, 1 ignored**, `workspace-final.log`. Includes nine new normalization/coverage tests plus the existing persistence integration assertion updated to require available normalized comparison.
- `cargo clippy --offline --workspace --all-targets -- -D warnings`: passed, `clippy-final2.log`.
- `cargo build --offline --bin rag-mcp`: passed, `build-final2.log`.
- **Nine real stdio MCP scripts** passed on one copied executable, `verification.json`: frozen Python value diff, Python value slicing, Python binding, SQL table impact, PHPUnit host dependencies, effective PHPUnit tests, affected tests, PHP contextual trace and Git impact.
- New value-diff MCP scenario passed **12 named checks**, `python_value_diff.json`: formatting/comments/NFKC equivalence, offset stability, import-only call redirect, actual positional argument/return edges, literal/expression edit, unsupported coverage guard, unknown boundary disclosure, repeated immutable diff after later edits, project isolation, bounded collections/outer truncation, no bodies/semantic proof and capability advertisement.
- Existing Python value-flow MCP retained **35 passing checks**, now requiring compatible frozen normalized diff availability. SQL impact retained all **15 passing checks**.

Native regressions additionally check operator/literal changes with unchanged topology, local reassignment edge changes, legacy/missing/corrupt/foreign captures, exact per-collection limits, explicit normalization-work exhaustion, frozen call provenance mismatch and omitted function inventories. These are captured-graph correctness fixtures, not representative corpus or runtime superiority qualification.

Binary SHA256: `0628fc85eda667a5addbaa82a448ac7d789b649a2fe1e1878de801910580faa6`.
`verification.json` records exact totals, nine MCP exit codes and selected source/dependency hashes before/after MCP. The executable and selected files were unchanged; the shared worktree was not frozen. `run_mcp.py` repeats the MCP batch from the repository root.

Earlier compile/focused logs are retained. `focused.log` records initial Rust inference/serialization errors, corrected before testing. `focused2.log` retains the obsolete assertion that normalized value diff must be unavailable. Later focused runs passed. `verification-initial.json` records the passing MCP batch before the additional omitted-inventory guard; final verification covers that guard too.

MCP fixtures use temporary databases, mock embeddings and disabled LLMs. Analyzed Python and SQL are never executed. Production data was untouched. No source extraction/index version bump is needed: existing format-5 captures contain the inputs.

Remaining: minimal edit/occurrence matching, semantic/runtime difference proof, complete CFG/exception/heap/alias coverage, predicate/iteration feasibility and termination, broader languages, representative quality/scale and full RAGRAF/CPG acceptance. Normalization work excludes database/JSON loading, scope scanning and report assembly; full-response byte/memory/time budgets are not qualified. `available` is algorithm availability, not whole-program comparability; per-response exclusions and unknown boundaries remain mandatory evidence.
