# Python binding and SQL impact qualification — 2026-10-26

Implemented NFKC lookup keys with original public evidence, captured function-local/class evaluation scope, and conservative opaque-binder/super-shadow guards. See [contract](../../PYTHON_BINDINGS.md).

Final validation:

- `cargo test --offline --workspace`: 1632 passed, 0 failed, 1 ignored (`workspace.log`).
- Seven focused Python binding regressions passed (`focused3.log`); earlier focused runs retained.
- `cargo clippy --offline --workspace --all-targets -- -D warnings`: passed (`clippy.log`).
- `cargo build --offline --bin rag-mcp`: passed (`build.log`).
- Eight real stdio MCP scripts passed using one copied binary: Python bindings, Python value slices, SQL table impact, PHPUnit host impact, effective PHPUnit tests, affected tests, PHP contextual trace and Git impact.
- Binary SHA256: `555bfca6c65444b57d002e3c2619afdc2fd003b19359964f9e386c2070e769a6`.

`verification.json` records exit codes, exact totals and selected source/dependency hashes before and after MCP execution. The copied executable and selected files were unchanged; this shared working tree was not frozen.

SQL table impact passed 15 scenarios (`table_impact.json`): direct readers/writers, ownerless evidence, indirect call chain, read/write filters, direct/work/root budgets, project/schema disambiguation, schema-only identities before filters, schema-only tables without observed usage, qualified identity, identifier validation, warm reuse, stale exclusion and sync refresh. This qualifies captured static candidates, not runtime database effects or complete SQL semantics.

The new Python MCP fixture passed nine checks (`python_bindings.json`): canonical imported target, normalized parameter shadowing, future-local shadowing, opaque comprehension guard, value-slice interprocedural target consistency, warm identity, stale exclusion, import-only refresh and project separation. Native tests additionally cover source byte/hash preservation, class defaults, typed parameter reassignment and shadowed super.

All MCP cases use temporary databases and source fixtures, mock embeddings and disabled LLMs. Analyzed programs and SQL were not executed; production data was untouched. Full Python/compiler/CPG semantics, control-flow slicing, legacy semantic diff proof and representative performance/corpus superiority remain open. No runtime comparison against the old binary was performed.
