# SQL secondary write qualification — 2026-10-25

[Contract](../../SQL_OUTPUT_USAGE.md). Four new native regressions cover MERGE
OUTPUT target column lists; INSERT/UPDATE/DELETE sinks versus RETURNING;
SELECT INTO with quoted names, CTE sources and excluded variable/multiple targets;
and migration projection uncertainty propagating to schema/write_check.

The isolated stdio MCP fixture checks secondary writes, read roles, source owner
and line, project isolation, warm reuse, stale-source exclusion and renamed sink
refresh through sync_sources. It also checks that a projection table is not
invented and write_check stays indeterminate under incomplete migration coverage.
The fixture uses mock embeddings and temporary databases. No ingested SQL runs.

This extends static SQL table-use navigation relative to the RAGRAF comparison
scope pinned at 075caf86dc2f6274b9c48487e521915bcdfeaf2c. It does not establish
reference correctness superiority or representative quality/scale. Unprefixed
routine variables, dialect binding, projections, triggers, indirect ORM/dataflow
and bulk-write validation remain incomplete. Full RAGRAF parity remains open.

See runtime.json and verification.json for final measured gates and source hashes.
Database schema 25; syntax index format 28. Production database untouched.

Final gates: **1,219 workspace tests passed, 0 failed, 1 ignored**; all 12
focused SQL tests passed (four new). Clippy with `-D warnings`, formatting and
`git diff --check` passed. Rebuilt binary passed the new secondary-write MCP
scenario plus bulk SQL and documentation regressions. Logs and hashes are stored
next to this report. This is fixture qualification, not exhaustive SQL coverage.
