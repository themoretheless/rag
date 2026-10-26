# Ranked repository maps

`repo_map` returns a compact outline of fresh indexed definitions. It uses resolved
static calls and non-call references for 24 deterministic file-graph PageRank
iterations, then selects definitions in rounds across ranked files. Within a file,
resolved incoming references and entrypoint evidence take precedence over names.
This is static structural importance, not runtime activity.

```json
{"project":"rag","path_prefix":"/captured/repository/src","budget_chars":4000,"budget_tokens":1000,"limit":100}
```

The MCP operation requires `RAG_TOOLS=full`. `project` is an exact stored wing;
`path_prefix` compares path components, so `/src/lib` excludes `/src/lib-copy`.
Paths are captured source paths; the operation does not traverse the filesystem.
Existing exact `path` and `symbol` selectors remain available. The graph is scoped
by project/prefix before ranking, while exact path/symbol selectors choose the
outline's definitions. An omitted project includes all projects, but identical
paths in different projects stay distinct and the outline labels their projects.

`budget_chars` bounds the actual Unicode scalar count of `map` (default 4000,
clamped to 128..20000). Optional `budget_tokens` bounds that text's estimate
(clamped to 1..8192), using the shared configured tokenizer or script-aware
heuristic. `estimated_tokens` is not a guarantee about a remote model's billing.
Both budgets apply when both are supplied. JSON metadata is explicitly outside
the budgets. A tiny budget can produce an empty map: even the first definition
must fit. An oversized definition is skipped so later fitting definitions can
still be included. Definitions are never cut in the middle of a name.

The response reports emitted/omitted definitions, budget skips, emitted/total
files, graph nodes/edges, ranking method and iteration count, the index revision,
and stale/unindexed/parse-error coverage for the selected project/path. Other
projects' source changes do not affect scoped maps or their coverage. Rankings
can change when in-scope inputs change. `index_rev` identifies the published input
revision; `view=fresh_subset_of_revision_inputs` makes exclusion of subsequently
changed inputs explicit. No index-format migration is needed for this query-only
extension.

The MCP handler runs snapshot/ranking work on the blocking pool. Project selection
is bound in the source query before source blobs or cached facts are decoded, so
a foreign project's broken cached payload cannot fail a scoped map. Directory
selection keeps the selected project's binding context before filtering graph
files. The published index revision can advance for changes in another project. Language binding
limitations, malformed sources, dynamic relations and representative large-corpus
quality/performance remain subject to the feature matrix's broader qualification.

See [qualification](qualification/repo-map-2026-10-24/README.md).
