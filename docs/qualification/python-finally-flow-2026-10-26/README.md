# Python finally completion-flow qualification — 2026-10-26

Value capture 6 / extraction format 86 adds explicit normal/return/break/continue/raise completion routing through `try/finally`, plus conservative protected expression-error prefixes. Context-specific nodes/call frames keep repeated executions of one physical finalizer separate. Differing incoming locals use completion-selection proxies; unrelated pending values do not influence constant finalizer returns. See [contract](../../PYTHON_FINALLY_FLOW.md).

Frozen diff normalization format 2 accepts capture formats 5 and 6 and incorporates completion context in identity. Plain legacy-5/current-6 facts compare without fabricated changes. Old unsupported finalizer coverage remains excluded instead of producing false semantic deltas. Query value-call boundaries now also honor the captured caller-module qualification guard, matching frozen comparison policy.

Final validation:

- `cargo test --offline --workspace`: **1671 passed, 0 failed, 1 ignored**, `workspace.log`.
- **47 focused data-flow tests passed**, `focused4.log`: 14 new finalizer/caller-guard/budget tests plus two new frozen-context/legacy-compatibility regressions, alongside the existing 31 tests.
- `cargo clippy --offline --workspace --all-targets -- -D warnings`: passed, `clippy.log`.
- `cargo build --offline --bin rag-mcp`: passed, `build.log`.
- **Ten real stdio MCP scripts passed** on one copied executable, `verification.json`: Python finally, frozen Python value diff, Python value slicing, Python binding, SQL table impact, PHPUnit host dependencies, effective PHPUnit tests, affected tests, PHP contextual trace and Git impact.
- New finally MCP passed **20 named checks**, `python_finally.json`: saved versus overriding return, pre-assignment error state, constant independence, nested local updates, distinct finalizer call frames, exact UTF-8 evidence, one physical definition/multiple context seeds, break/continue routing, explicit raise override, no successful-return seed after escaping raise, handler/uninitialized guards, work/output limits, capability scope, project isolation, warm/full resolution equivalence, stale exclusion, source-sync changes and frozen context diff.
- Existing value-slice MCP retained **35 passing checks**, frozen value diff **12**, SQL impact **15**.

Binary SHA256: `72f87890ddeb61cd33deed3433b2e4cf10f1d6a00084579a9c9d716fe07f18f7`.
`verification.json` records exact totals, ten MCP exit codes and selected source/dependency hashes before/after the MCP batch. The copied executable and selected files were unchanged; shared source was not frozen. `run_mcp.py` repeats the MCP batch from the repository root.

Earlier focused logs are retained. The first run exposed the staged capture-format migration and obsolete expectations that try/finally must be unsupported; format-5/6 normalization compatibility and explicit handler guard fixtures replace those expectations. Later focused runs pass. Assertions for saved/overridden values, exception-prefix locals and independent constant returns were added rather than dropping semantic checks.

All MCP fixtures use temporary databases, mock embeddings and disabled LLMs. Analyzed Python/SQL is never executed. Production data was untouched. No old-runtime performance or representative corpus superiority claim is made.

Still open: except/exception groups/type matching, try-else, bare/chained raises, uninitialized exceptional-read modeling, arbitrary asynchronous/resource/operator/iterator/heap effects, complete CFG, predicate/iteration feasibility and correlation, termination, semantic/minimal-edit value diff, broader languages/corpus/scale and full RAGRAF/CPG acceptance. These remain bounded syntactic may-candidates, not a complete Python interpreter or runtime-path proof. Full-response byte/memory/time budgets are not qualified.
