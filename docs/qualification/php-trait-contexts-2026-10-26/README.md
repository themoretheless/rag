# PHP trait-body context qualification — 2026-10-26

[Contract and boundaries](../../PHP_TRAIT_CONTEXTS.md).

- Focused PHP tests: 33 passed. Seven new regressions cover per-consumer self/this/parent targets, retained aliases versus overrides, inherited lexical scope and trait imports, unresolved/multiple/value contexts, bounds, cache roundtrip/corruption/project guards and immutable owner-target swaps with unchanged aggregate edges.
- Workspace: 1564 passed, 0 failed, 1 ignored.
- Workspace/all-target Clippy with `-D warnings` and debug binary build passed.
- Eight real stdio MCP suites pass on one copied executable: trait-body contexts, trait composition, inheritance, namespaces, PHPDoc, SQL table impact, non-code references and literal impact.
- Trait-body MCP: 15 checks pass, including flat reference evidence, callers/impact provenance, stale host exclusion, consumer removal, frozen context diffs, project isolation and full cache equivalence.

Runtime index format 76, store schema 25; PHP capture format 4.
The checkout is shared with another active task, which also advances extraction
versions. `binary.json` verifies the copied executable hash before/after MCP runs;
`verification.json` records observational selected source hashes. This is not proof
of a frozen checkout or deployment to the running gateway.

All fixtures use isolated temporary DuckDB stores, mock embeddings and disabled
LLM calls. PHP/SQL applications and production databases are not executed. PHP CLI
is unavailable. Runtime subtype context completeness, correlation over whole
multi-edge paths, compiler/grammar completeness and representative quality/scale
remain open. The shared candidate union can over-approximate consumer paths; the
per-consumer rows and false proof flags preserve this limitation. Full RAGRAF
parity is not established by this qualification.
