# SQL table impact — 2026-10-25

RAGRAF's pinned 075caf86dc2f6274b9c48487e521915bcdfeaf2c _impact_table returns
direct readers/writers and indirect incoming code links. The local extension
connects its existing static SQL usage evidence to bounded symbol dependency
traversal. See [contract](../../IMPACT_RELATIONS.md#sql-table-targets).

Three new regressions cover read/write owners plus raw ownerless SQL, independent
evidence/result limits with untruncated seed roots, project/schema ambiguity,
qualified/quoted names and rejected trailing SQL/mixed/invalid arguments. The
focused suite retains seven prior impact regressions. Actual MCP fixtures check
an additional indirect caller level, project/schema disambiguation, read/write
filters, stale exclusion and source-sync replacement, using temporary databases
and mock embeddings. No ingested SQL or live database is executed.

These are static candidates, not proven execution impact, physical database
binding or calibrated confidence. File seed semantics, ownerless module links,
ORM/dynamic SQL and representative quality/scale remain open. Full parity remains
open. Database schema 25; extraction format 29 unchanged. Final evidence and hashes
are in verification.json and runtime.json.

Final gates: **1,238 workspace tests passed, 0 failed, 1 ignored**. All ten
focused impact cases passed (three new). Clippy with `-D warnings`, formatting
and `git diff --check` passed. The rebuilt binary passed the table-target MCP
scenario and existing multi-file impact regression. Source/binary hashes and
logs are retained here. These fixtures do not establish representative scale.
