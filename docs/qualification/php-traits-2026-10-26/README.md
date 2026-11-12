# PHP trait composition qualification — 2026-10-26

[Contract and limitations](../../PHP_TRAIT_COMPOSITION.md).

- Native PHP suite: 26 passed, including nine new trait regressions. They cover nested/imported providers, consumer visibility, precedence, aliases/exclusions, conflicts, conditional/duplicate/unknown/wrong-kind providers, depth/work/capture bounds, unsupported final grammar forms, cache migration and immutable selection/use-relation history.
- Workspace: 1554 passed, 0 failed, 1 ignored.
- Workspace/all-target Clippy with `-D warnings` and debug binary build passed.
- Eight isolated real stdio MCP scripts passed on the same copied binary: traits, inheritance, namespace bindings, PHPDoc, PHP routes, SQL table impact, non-code references and literal impact.
- Trait MCP: 20 checks passed, including enum use, separate trait-use impact, original byte identity, callable values, project isolation, selection-only changes, trait-file-only removal without fallback, historical immutability and forced-full cache equivalence.

Runtime index format 74, store schema 25. Final logs and reports are retained here.
`binary.json` checks the copied executable hash before/after qualification;
`verification.json` records selected source hashes. This is a shared checkout with
concurrent work, not a frozen checkout or verified deployed gateway.

Fixtures use temporary DuckDB stores, mock embeddings and disabled LLM calls.
PHP/SQL applications and production databases are not executed. PHP CLI is
unavailable. Consumer-specific trait-body contexts, runtime dispatch, compiler
signature/property/final/abstract compatibility, full PHP grammar coverage and
representative quality/scale remain open. Full RAGRAF parity is not established.
