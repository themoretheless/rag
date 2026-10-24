# Explicit SQL expression capture and frozen diff — 2026-10-24

Index format 19 retains canonical SQL AST text for explicit column defaults, generated expressions and generation clauses. SET DEFAULT replaces the captured default expression and DROP DEFAULT removes it. Existing presence flags and write-check behavior are retained; implicit serial/identity behavior is not fabricated as an explicit default expression.

Frozen schema diffs now detect expression-only changes such as DEFAULT 1 → DEFAULT 2 or (age + 1) → (age + 2), even when column type/nullability/default/generated flags stay unchanged. `expression_changes_available` is false if either revision predates capture support; newly captured fields are omitted from such comparisons to avoid false changes. Historical graphs are never rewritten or replayed.

Verification:

- Full library suite: 820 passed, 0 failed, 1 ignored.
- Code-index suite: 106 passed, including new default/generated capture, SET/DROP replay, expression-only frozen changes, legacy coverage and historical immutability.
- Server and gate CLI builds passed.
- Actual stdio MCP expression/schema/diff regression: `runtime.json`.
- Existing migration/usage/write-check regression: `sql-regression.json`.
- Gate stdio/authenticated HTTP/actual hook regression: `gate-regression.json`.

All test data and databases are temporary and embeddings are mocked. No SQL or source scripts are executed and production state is untouched. Captured expression text is schema metadata and may include literal defaults; it is not an evaluated value or a live-write guarantee. Syntactic AST text is compared, not general semantic equivalence. Unsupported ALTER forms, arbitrary constraints, implicit generated expressions and live schema state remain outside this qualification. The full transfer goal remains open.
