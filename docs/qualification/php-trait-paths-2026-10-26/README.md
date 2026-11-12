# Captured PHP trait impact path qualification — 2026-10-26

[Contract and boundaries](../../PHP_TRAIT_PATHS.md).

- Focused PHP suite: 40 passed. Seven added regressions cover incompatible entries, multiple states per symbol, nested inherited trait frames, missing entry metadata, callable values/relation modes, cache-frame repair and legacy-qualified frozen entry history.
- Workspace: 1576 passed, 0 failed, 1 ignored.
- Workspace/all-target Clippy with `-D warnings` and debug binary build passed.
- Eight isolated real stdio MCP suites passed: trait paths, trait-body consumers, trait composition, PHP inheritance, namespaces, SQL table impact, non-code references and literal impact.
- Trait-path MCP: 15 checks passed, including negative/positive consumer branches, nested inherited frames, value references, depth/output caps, SQL INSERT write impact/read-filter isolation, warm/full-cache equivalence, stale host exclusion, entry-change refresh, immutable entry diff and project isolation.

Runtime index format 78, store schema 25; PHP capture format 5.
`binary.json` checks the copied executable hash before/after all MCP runs.
`verification.json` records observational selected source hashes. The checkout is
shared with another active task; this is not a frozen checkout or proof of a
deployed gateway. Other extraction work can advance the current source version.

Fixtures use temporary DuckDB stores, mock embeddings and disabled LLM calls.
PHP/SQL applications and production databases are never executed. Matching is for
captured lexical trait frames only. Runtime subtypes, late static/dynamic dispatch,
compiler/grammar completeness, context sensitivity of other analyses and
representative quality/scale remain open. False runtime proof flags are retained.
Full RAGRAF parity is not established by this qualification.
