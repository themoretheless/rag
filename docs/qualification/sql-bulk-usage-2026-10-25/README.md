# Bulk SQL table-use subset — 2026-10-25

Previously MERGE/TRUNCATE had a generic unsupported-write diagnostic and COPY
had no directional table-use extraction. The [new contract](../../SQL_BULK_USAGE.md)
captures explicit static roles while preserving unverified secondary effects.
The sqlparser version remains 0.63.0; no dependency versions changed.

Three new regressions cover MERGE target/source/subquery reads and mutating versus
DO NOTHING actions; explicit TRUNCATE writes plus COPY FROM/TO/query direction and
CTE alias exclusion; and static source owner/line, COPY(query), ordinary copy-label
and dynamic-literal/descendant diagnostics. The focused group also runs five
existing SQL/schema/identifier regressions.

The isolated stdio MCP scenario checks mixed raw SQL/source literals, read/write
filters, project isolation, source ownership/line, warm reuse, stale-source
exclusion and refreshed COPY targets after sync. A nonexistent COPY input path
is only syntax evidence. Unsupported merge write_check is explicitly rejected.
Mock embeddings and owned temporary databases are used; no ingested SQL executes.

Reference scope: read/write table-use navigation in RAGRAF at pinned revision
`075caf86dc2f6274b9c48487e521915bcdfeaf2c`; this is a local static SQL coverage
extension, not a measured reference correctness/throughput comparison. Secondary
MERGE OUTPUT targets, TRUNCATE descendants/CASCADE, COPY INTO dialects, functions,
ORM/dataflow, bulk write validation and representative quality/scale remain open.
Syntax format 26 refreshes older caches; database schema stays 25.

```sh
cargo test --lib code_index::sql::tests::sql_ --locked --offline
cargo build --bin rag-mcp --locked --offline
python3 scripts/smoke_sql_bulk_usage.py target/debug/rag-mcp /tmp/sql-bulk-usage.json
```

Final verification: **1,209 workspace tests passed, 0 failed, 1 ignored**;
all eight focused SQL tests (three new) passed. Workspace Clippy with `-D warnings`,
formatting and `git diff --check` passed. The rebuilt binary passed
[bulk-use MCP evidence](runtime.json), [defaults](defaults-regression.json),
[Lua](lua-regression.json) and [timings](timings-regression.json) regressions.
[Verification manifest](verification.json) records working-tree source and
binary hashes. Actual MCP evidence includes no reversed COPY roles, exact
project-filtered write/read sets, stale omission and refreshed COPY targets.
These analysis-runtime checks do not execute ingested SQL or qualify live-server
permissions, constraints/data effects, representative quality or scale.
