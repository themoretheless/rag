# Python handler/else flow qualification — 2026-10-26

Value capture 7 / extraction 87 adds candidate `try/except/else/finally` routing, typed/bare handler alternatives, caught-object alias values/cleanup and captured bare re-raise. Else/handler failures bypass sibling handlers. Normal continuations merge definite local definitions; finalizers retain saved-return/updated-local precedence. See [contract](../../PYTHON_EXCEPTION_FLOW.md).

Scope capture 3 shares one alias adapter between core references and data flow. It handles the observed tree-sitter `except_clause value: as_pattern ... alias: as_pattern_target(identifier)` representation, hoists local alias shadowing and applies cleanup masks without blocking unrelated imports. Initialization positions also avoid a synthetic binder winning a tie with the first real reassignment in the body.

Frozen value normalization 3 supports capture formats 5..7. Context-bearing owners are excluded across different capture versions to prevent analysis enrichment masquerading as source changes. Same-format handler contexts and source-position-only shifts are qualified; legacy plain compatible graphs retain availability.

Final validation:

- `cargo test --offline --workspace`: **1685 passed, 0 failed, 1 ignored**, `workspace.log`.
- Focused code-index run: **634 passed**, `focused7.log`; the final workspace additionally includes the attribute-selector/comment regression. Added coverage includes handler/else reaching values, no handler retry, alias return versus cleanup, bare re-raise, finalizer precedence, loop jump state, route budgets, alias/type cleanup, parser shape and frozen-context migration.
- Strict workspace/all-target Clippy: passed, `clippy-final.log`.
- Binary build: passed, `build-final.log`.
- **Eleven real stdio MCP scripts passed** on one copied executable, `verification.json`: Python except, finally, frozen value diff, value slice, name binding, SQL table impact, PHPUnit host dependencies, effective PHPUnit tests, affected tests, contextual PHP trace and Git impact.
- New except MCP passed **20 named checks**, `python_except.json`: bare handler definitions/merges, typed unproven candidates, independent call frames, caught-alias UTF-8 values, alias cleanup, captured bare re-raise, normal-only else, else/handler escape boundaries, finalizer override, core alias shadow/cleanup, unrelated imports, dynamic selector guard, independent work/output limits, project isolation, warm/full resolution equivalence, stale exclusion, source sync and frozen handler diff.
- Existing finally/value-slice/value-diff MCP passed **20/35/12 checks**; SQL table impact retained **15 passing checks**.

Binary SHA256: `aa801e7f695f80a2cca869f60d51a1a2e1f0279d4f5b7ab1c9dcb5fc242bd59e`.
`verification.json` records exact totals, eleven MCP exit codes and selected source/dependency hashes before/after MCP. The copied executable and selected files were unchanged; the shared working tree was not frozen. `run_mcp.py` repeats the MCP batch from the repository root.

Earlier logs are retained. Initial focused runs include staged format migration and obsolete unsupported-handler expectations. Later failures exposed the real alias AST shape; `parser-shape.log` preserves that observation. The adapter then exposed a binder/reassignment initialization-position tie, corrected before final qualification. Initial Clippy found range-pattern/collapsible-match style issues; the final strict pass is clean. Assertions were replaced with positive handler tests and actual unsupported dynamic-filter guards, not dropped.

Fixtures use temporary databases, mock embeddings and disabled LLMs. Analyzed Python/SQL is never executed and production data was untouched. No runtime, old-binary performance or representative corpus superiority claim is made.

Still open: class-hierarchy/type and filter evaluation proof, exception groups, dynamic/chained exception forms, bare raises without captured active exception, uninitialized exceptional reads, arbitrary asynchronous/resource/operator/iterator/heap effects, complete compiler/CFG/alias semantics, predicate/iteration feasibility and termination, semantic/minimal-edit matching, broader language/corpus/scale and full RAGRAF/CPG acceptance. Work limits are logical normalization/traversal bounds; full-response byte/memory/time qualification remains open.
