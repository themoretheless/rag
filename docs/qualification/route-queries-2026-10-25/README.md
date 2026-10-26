# Route-query subset qualification — 2026-10-25

The earlier route operation only selected a source path/symbol and limited the
result; the reference `op_routes` also selects route-address substrings and counts
trust categories. The new [query contract](../../ROUTE_QUERIES.md) keeps source
path and route-address selectors distinct, adds exact project/URL/kind/method/
category filters, and applies them before page limits. It does not treat unknown
ANY candidates or missing client methods as evidence of an explicit method.

Optimistic pages require a query/view token for nonzero offsets. Scoped coverage,
matching facts and global revision participate, so stale sources invalidate pages
even before a new revision is published. Reads use one transaction; foreign scope
is excluded before cached JSON decoding. There is no repair/reindex as a side effect.

Three owned-DuckDB tests cover literal/typed filter intersections before limits,
kind counts, token requirements/filter mismatches/limit independence, stale-view
changes without revision changes, maximum offsets, same-path project scope,
foreign corrupt caches and bound project parameters.

The real stdio MCP scenario covers public schema/legacy empty requests, scoped
mixed Python/JS/HTML facts, 2+2+1 complete nonoverlapping pages, missing/misused
and stale token failures, literal SQL/regex metacharacters, unknown methods and
foreign-project ingestion isolation. It uses an owned temporary database and
mock embeddings. No production database or service was touched.

Reference scope: `op_routes` at pinned RAGRAF revision
`075caf86dc2f6274b9c48487e521915bcdfeaf2c`, inspected locally: substring-address
filtering and route/endpoint category counts. The new pagination guards are a
local extension. This is not full reference contract/quality/scale qualification;
retained snapshot cursors and representative large-index budgets remain open.

```sh
cargo test --lib code_index::routes --locked --offline
cargo build --bin rag-mcp --locked --offline
python3 scripts/smoke_route_queries.py target/debug/rag-mcp /tmp/route-queries.json
```

Final verification: **1,190 workspace tests passed, 0 failed, 1 ignored**;
all 144 focused code-index tests (including three route-query tests) passed.
Workspace Clippy with `-D warnings`, formatting and `git diff --check` passed.
The rebuilt binary passed [query MCP evidence](runtime.json) and
[PHP](php-regression.json), [fetch](fetch-regression.json),
[markup](markup-regression.json) and [index-status](index-status-regression.json)
regressions. [Verification manifest](verification.json) records working-tree
source/binary hashes. Actual MCP evidence includes both missing/empty legacy
arguments, complete page coverage and stale-view conflicts without a revision
change. This is not production-service or representative-scale qualification.
