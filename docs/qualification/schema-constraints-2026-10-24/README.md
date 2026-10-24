# Captured SQL constraints and frozen diffs — 2026-10-24

Index format 20 captures table/column CHECK, UNIQUE, primary-key and foreign-key declarations, including names, canonical AST text and ALTER ADD CONSTRAINT NOT VALID. Named DROP CONSTRAINT updates table and column metadata. Constraint ordering is deterministic.

Frozen schema diffs include column constraint changes and table constraint added/removed sets, bounded before/after declarations and truncation. Legacy snapshots without capture support return constraint_changes_available=false; missing fields are omitted from comparison rather than becoming phantom changes.

Constraint declarations are not live validation. Migration write_check retains its documented column/NULL/default scope and does not execute CHECKs or enforce key/foreign-key data. Unknown generated names, unsupported CASCADE behavior, duplicate declared names and dependency rewrites are diagnosed. Rename/drop operations in constrained/generated schemas conservatively leave the project incomplete; uncertain write plans cannot receive a positive verdict.

Verification:

- Full library run after the main implementation: 823 passed, 0 failed, 1 ignored.
- Code-index regression: 109 tests; final rerun also covers duplicate constraint names introduced by ADD COLUMN.
- Final history regression: 8 passed after explicit constraint-delta output was added.
- Server and gate CLI builds passed.
- Actual stdio MCP constraint/schema/diff/write-check workflow: runtime.json.
- Existing expression/frozen-diff regression: expressions-regression.json.
- Gate stdio/authenticated HTTP/actual hook regression: gate-regression.json.

All tests use owned temporary inputs/databases and mock embeddings. Migration SQL and source files are not executed; production state is untouched. Full constraint evaluation, exact dependency rewriting, server-generated names and broader dialect behavior remain outside this stage. The full transfer goal remains open.
