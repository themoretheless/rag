# Python route-method subset — 2026-10-25

Python `@app.route('/path', methods=['GET', 'POST'])` and
`@app.api_route('/path', methods=('PATCH', 'DELETE'))` now produce one static
registration candidate per explicit method. Lists, tuples and sets of plain
literal standard HTTP methods are normalised to uppercase, deduplicated and
sorted. Each registration binds the decorated definition by its source span,
including separate same-name definitions.

Previously `route` discarded the methods as ANY, `api_route` was missed, and
keyword arguments could create a second unresolved registration. Decorator
calls are now collected only through their decorated definition. Empty,
malformed, computed, interpolated, concatenated, escaped or spread method
collections retain an ANY candidate; ANY means unknown here, not verified
support for every method. Framework defaults and implicit HEAD/OPTIONS are not
inferred. Receiver/framework identity and runtime registration are unverified.
The routes response declares these limits explicitly in `detail`.

Index format **21** invalidates cached extraction from earlier formats; normal
`index_code`/source sync refreshes it. No production index was migrated or
service restarted by this qualification. Existing immutable revisions keep
their original format and facts.

Two new parser/analysis regressions cover multi-method sets, duplicate/lowercase
methods, exact same-name binding, keyword handling and all-or-unknown fallback
for dynamic, empty, escaped, scalar, spread, interpolated and concatenated
collections. Existing Express, Axum, imported-handler and structural-analysis
regressions remain in the focused test group.

The owned-database stdio MCP script `scripts/smoke_python_routes.py` checks real
route extraction, deterministic replay, exact handler binding, stale exclusion
and refresh after source ingestion/sync. It uses mock embeddings and executes
no source application code. Reference scope is `op_routes` in RAGRAF revision
`075caf86dc2f6274b9c48487e521915bcdfeaf2c`, inspected locally. The reference also
has markup endpoint kinds; this extension does not implement those or establish
full route parity or representative superiority.

Reproduce:

```sh
cargo test --lib code_index::analysis --locked --offline
cargo build --bin rag-mcp --locked --offline
python3 scripts/smoke_python_routes.py target/debug/rag-mcp /tmp/python-routes.json
```

Final validation: **1,175 workspace tests passed, 0 failed, 1 ignored**; all
five focused analysis tests passed. Workspace Clippy with `-D warnings`,
formatting and `git diff --check` passed. The rebuilt binary passed the
[route MCP scenario](runtime.json), [index-status](index-status-regression.json)
and [repo-map](repo-map-regression.json) regressions.
[Verification manifest](verification.json) records working-tree source and
binary hashes. No source application/server was executed.
