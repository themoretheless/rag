# PHP inheritance qualification — 2026-10-26

[Contract and remaining boundaries](../../PHP_INHERITANCE.md).

- Native PHP tests: 17 passed. The seven added regressions cover imported multi-level bases, parent versus local override, visibility/static guards, cycles/unknown/duplicate/conditional/final bases, abstract targets, trait precedence barriers, interface relations, depth bounds, comments, cache migration and frozen history capability guards.
- Workspace: 1545 passed, 0 failed, 1 ignored.
- Workspace/all-target Clippy with `-D warnings` and debug binary build passed.
- Eight real stdio MCP suites passed on the copied binary: PHP inheritance, PHP names, PHPDoc, PHP routes, SQL table impact, literal impact, non-code references and source cursors.
- The inheritance suite passes 16 checks, including callers/impact, source offsets, project isolation, warm reuse, stale exclusion, import-only base changes, base-file-only method removal, frozen history and forced-full resolution equivalence.

Runtime index format 73, store schema 25. Reports and final logs are retained in
this directory. `verification.json` records selected source hashes and the copied
binary hash. Sources are shared with another active task; this is observational
qualification, not proof of a frozen checkout or deployed gateway.

All MCP fixtures use isolated temporary DuckDB stores, mock embeddings and disabled
LLM calls. Source PHP/SQL applications and production databases are never executed.
PHP CLI is unavailable. Compiler signature/variance validity, runtime subtype and
late-static dispatch, trait composition, interface implementation dispatch,
representative quality/scale and full RAGRAF parity remain open. Declared-method
and inheritance edges are syntactic candidates with explicit limitations.
