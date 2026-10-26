# Duplicate primary comparisons — 2026-10-26

Implemented [member comparison contract](../../DUPLICATE_PRIMARY_COMPARISON.md)
and retained the previously qualified static SQL-table impact behavior.

**1,424 workspace tests passed, 0 failed, 1 ignored**; Clippy with `-D warnings`,
owned-module formatting, Python syntax and `git diff --check` passed. Seven new
native cases cover Unicode/CRLF normalization, empty/disjoint sets, directional
counts, decimal rounding, all phase limits, caching and returned-member scope.
The focused corpus log predates the final rounding case; the workspace run
includes it. `reference.json` records execution of only the pinned RAGRAF pure
`_similarity`/`_line_delta` functions on six fixed inputs and rounding controls.
This is finite metric-contract evidence, not a quality benchmark.

An isolated copy of the rebuilt MCP executable passed the new comparison
scenario: member/primary identity, normalized metrics, body-free deterministic
responses, all phase limits, member-window scope and source-sync replacement.
Five additional real MCP scripts passed: SQL table impact, duplicate roles,
search roles, literal path roles and file imports. SQL coverage includes direct
readers/writers, ownerless SQL, indirect callers, read/write filters, independent
budgets, project/schema disambiguation, invalid identifiers, warm reuse, stale
exclusion and sync refresh. All use temporary sources/databases and mock
embeddings. No ingested source or SQL was executed; no production DB was touched.

Runtime reports store schema **25**, extraction format **51**. This DTO/output
extension requires no extraction-version bump. Executable and log hashes,
observed source hashes and detailed results are in `verification.json`.
The shared dirty checkout was concurrently active; this qualifies the checked
build and isolated executable, not a frozen checkout snapshot.

Runtime/dynamic SQL binding, representative corpus quality/scale, semantic
merge/deletion safety and full RAGRAF parity remain open. Comparison budgets
exclude corpus loading, grouping and role analysis; see the contract for scope.
