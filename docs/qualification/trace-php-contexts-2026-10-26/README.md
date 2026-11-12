# Trait-aware trace_path qualification — 2026-10-26

[Contract and boundaries](../../TRACE_TRAIT_CONTEXTS.md).

- Focused path suite: 14 passed. Nine added regressions cover forward/back/both frames, incompatible consumers, conditional wildcard paths, inherited nested frames, values and true ambiguity opt-in, stale contexts, missing body/entry metadata, enum/reverse roots, shared work limits and independent non-trait ambiguity candidates.
- Final workspace: 1599 passed, 0 failed, 1 ignored.
- Workspace/all-target Clippy with `-D warnings` and debug binary build passed.
- Eight isolated real stdio MCP suites passed: new PHP context trace, baseline symbol paths, captured trait impact paths, Git PHP contexts, SQL table impact, literal path roles, literal impact and non-code references.
- New trace MCP: 14 checks passed, including per-step source/context provenance, directional/conditional paths, genuine ambiguity semantics, enums, values, budgets, determinism, project/stale guards, cache equivalence and sync refresh.

The first shared-tree workspace run had two Rust stdarch failures outside these
path changes. Current stdarch recheck passed all five cases; the final workspace
rerun is green. Initial and final logs are retained to distinguish observations.

Qualified runtime index format 80, store schema 25.
This is a query change; no stored extraction migration is required. `binary.json`
checks the copied executable before/after all MCP runs. Selected source hashes are
observational: another task shares and changes the checkout. This does not prove
a frozen checkout or deployment to the running gateway.

Fixtures use temporary DuckDB stores/repositories, mock embeddings and disabled
LLM calls. Source PHP/SQL applications and production databases/repositories are
not executed or used. Captured lexical matching does not establish runtime
subtype/late-static/dynamic dispatch, compiler/grammar completeness, full dataflow
or representative quality/scale. Full RAGRAF parity remains unproven.
