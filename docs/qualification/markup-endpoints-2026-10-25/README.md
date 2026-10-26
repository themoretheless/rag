# Markup endpoint-reference subset — 2026-10-25

`routes` now includes `http_endpoint_reference` separately from
`http_route_candidate`. These are syntax observations of root-relative markup
URLs, not discovered server handlers. Endpoint records have `symbol_id: null`,
`method: null`, `resolution: unbound_reference` and `provenance: syntax`.
`reference_kind` identifies the exact tag/attribute category. They never become
roots in `entrypoints` or make a same-named function live in `dead_code`.

Supported observations: `form action`, `button/input formaction`, `a/link href`
and `script/img/iframe/source/video/audio src`. HTML entities are decoded;
query/fragment spelling is retained. Script source attributes are collected,
but comments and inline JavaScript strings are not interpreted as markup.
Network-relative, external, relative, empty, fragment-only, templated, whitespace,
control-character, oversized (>4096 bytes) and duplicate-attribute targets are
excluded. Dynamic URL evaluation, origin/base resolution, filesystem lookup and
network fetching are not performed. This does not imply excluded expressions
are unused or that a target exists. JSX, JS fetch, framework dynamic binding,
redirects and markup-to-handler matching remain unimplemented here.

All entrypoint/route observations now include the source `document_id`, project,
revision, content hash and byte offset. Separate occurrences on one line and
same-path documents retain distinct identities. URL references are frozen with
index revisions and appear in project-scoped `diff` as `endpoint_reference`
relations. Relation comparison ignores line/offset moves; repeated identical
source-document/URL/category facts are one semantic relation in the diff, while
`routes` preserves each occurrence. Older graphs do not contain these new facts;
`analysis_format_changed` makes the extraction-format transition visible.

Index format **22** invalidates earlier extraction caches at the next explicit
index/source sync. Immutable older revisions remain unchanged. Status recognises
endpoint-only documents as structural evidence, without promoting unknown
quality to healthy. No production database was changed or service restarted.

Tests cover entity/case/unquoted values, script sources, dynamic/external/duplicate
rejection, comment/script-text exclusion, separation from liveness, source path
filtering and occurrence identity. The real stdio MCP scenario uses an isolated
mock-provider database and checks provenance, stale exclusion/source-sync refresh,
status and project-scoped immutable relation diff.

Reference scope: `op_routes` at RAGRAF revision
`075caf86dc2f6274b9c48487e521915bcdfeaf2c` exposes route registrations and endpoint
references as separate trust categories. This implements the markup subset;
full route/endpoint coverage, comparative correctness and representative scale
remain open.

Reproduce:

```sh
cargo test --lib code_index:: --locked --offline
cargo build --bin rag-mcp --locked --offline
python3 scripts/smoke_markup_endpoints.py target/debug/rag-mcp /tmp/markup-endpoints.json
```

Final validation: **1,180 workspace tests passed, 0 failed, 1 ignored**; all
134 focused code-index tests passed on the final source. Workspace Clippy with
`-D warnings`, formatting and `git diff --check` passed. The rebuilt binary
passed the [markup MCP scenario](runtime.json) and
[Python routes](python-routes-regression.json),
[index-status](index-status-regression.json) and
[repo-map](repo-map-regression.json) regressions.
[Verification manifest](verification.json) records working-tree source and
binary hashes. Actual MCP evidence includes unchanged semantic relations after
a line move and scope-isolated relation additions/removals after URL changes.
