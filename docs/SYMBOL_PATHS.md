# Symbol-to-symbol paths

`trace_path` exposes deterministic bounded simple paths over the captured fresh
code graph. It does not read the live filesystem or run code. The MCP entrypoint
uses a single project-scoped snapshot when `project` is provided; endpoint and
edge checks also prevent cross-project traversal when it is omitted.

```json
{
  "from": "handle_request",
  "to": "write_record",
  "project": "my-project",
  "relations": "calls",
  "direction": "forward",
  "max_depth": 6,
  "max_paths": 3,
  "max_work": 20000
}
```

Endpoints accept exact symbol IDs, qualified or short names, with optional
`from_path` and `to_path`. An exact ID takes precedence. Each endpoint must select
one fresh symbol; ambiguous names return candidates and totals instead of a
candidate union. Candidate samples are capped at 50. No result implies a file or
symbol is absent from the repository. Stale/unindexed/parse-error inputs and
binding diagnostics are sampled at 50 per list with counts and truncation.

Relations:

- `calls` (default): captured call edges.
- `references`: calls plus bound value/type references.
- `all`: references plus inheritance and method-override candidates. Multiple
  evidence kinds may describe the same pair, producing distinct paths.

Unique static targets are used by default. `include_ambiguous=true` permits
individual candidates from ambiguous edges; `ambiguous_target` remains true and
the original candidate set remains in the evidence. This never proves runtime
dispatch. Even unique trait/inheritance/static-call edges retain the underlying
resolver's semantics. `direction=back` reverses traversal, while `both` admits
both directions; each step keeps original source/target and `reversed`.

Breadth-first traversal returns shorter paths first with stable source/target/
relation/location ordering. Paths cannot repeat a symbol. Equal-symbol endpoints
produce a zero-edge identity path. Every step contains source/target definitions,
the relation type and original evidence including source location, revision and
hash where supplied by that edge type. Inheritance evidence uses its original
class source location plus the retained source definition's revision/hash.

Limits are clamped: depth 1..20, paths 1..20, work 1..200000. Defaults are 6, 3,
20000. Candidate inspection and traversal share the work budget; whole snapshot
loading and endpoint matching are separate. `work_exhausted`,
`depth_limit_reached` and `result_limit_reached` expose the reason a bounded search
may omit paths. Reaching the path cap is reported conservatively even if no more
paths exist. Query results are bounded records, not a strict byte/token budget.

Statuses: `found`, `ambiguous_endpoints`, `endpoint_unavailable`,
`project_mismatch`, `work_limit`, `not_found_in_bounded_graph`. A missing path
never proves absence, even if the work budget was not exhausted: source coverage,
supported semantics and depth remain limited. `absence_proven` and
`runtime_path_proven` are always false. There is no full data-flow slicing or
dynamic execution guarantee.

Qualification: [native and real-MCP fixture evidence](qualification/symbol-paths-2026-10-25/README.md).
