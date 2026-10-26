# SQL table and URL reference targets

The MCP `references` tool and native `Store::code_references(ReferenceQuery)` now
accept one of `symbol`, `table`, `route` or `key`. The key syntax contract is
[documented separately](TRANSLATION_KEY_REFERENCES.md). Omitting a non-code target preserves
the existing symbol reference behavior, including `reference_mode=lexical`.
An optional exact `project` scopes the stored snapshot, cache decoding and
freshness diagnostics for both old and new targets. No filesystem is read.

## SQL targets

Example: `references(table="public.users", project="api", table_mode="read")`.
The `table_mode` values are `all` (default), `read`, `write`. Quoted SQL identifiers
are accepted through the shared bounded SQL identifier parser. Mixed target
selectors, trailing SQL and invalid modes fail before snapshot loading.

Each `queries` record preserves `access`, `operation`, exact project/table target
and captured document/path/line/revision/hash evidence. A validated owning symbol
is the `from` node; ownerless raw SQL stays a file-origin record. No SQL text or
source body is returned. Symbol summaries include captured ID, name, source
identity and hash/revision, without a signature/body payload.

Candidate identities come from both captured schema tables and static usages,
before applying source-path or access filters. Unqualified names spanning schemas
or projects return `needs_disambiguation` with bounded candidate identities and
full candidate counts. A write-only filter or a source path cannot silently hide
a competing table identity. Fully qualified identifiers and an exact project
resolve ordinary ambiguity. A schema-only table can be `target_observed` with
zero returned usages. This is not proof of no runtime usage.

## URL targets

Example: `references(route="/items", project="api")`. Selectors accept up to
4,096 UTF-8 bytes, aligned with endpoint capture and literal impact. URL identity is exact,
case-sensitive captured literal text: no wildcard interpretation, query/template
expansion, router composition, framework defaults or deployment resolution.
Identical literal URLs in multiple projects require disambiguation before a
source-path filter. Registration HTTP methods do not create separate URL
identities here; each method remains separately captured evidence.

Two relation kinds stay distinct:

* `routes`: captured route registration candidates, with `registration_method`;
* `endpoint_reference`: literal HTML/fetch URL mentions, with their separate
  `reference_kind` and explicit `request_method` when captured.

A URL match never binds a client mention to a handler. Every URL occurrence has
its source file as `from`. Registration `handler_candidates` are alternatives,
not source owners; they retain exact symbol/revision/hash identity and the
captured `handler_resolution`. Alternatives at the same source occurrence and registration method are
grouped into one result, with independent 50-candidate retention, full count and
truncation. Client mentions have no manufactured handlers or default methods.

## Filtering and evidence

For non-code targets `path` filters exact source-occurrence paths, after target
identity disambiguation. This differs from symbol mode, where path scopes the
selected definition. `limit` clamps to 1..500. Matching `total`, `ownerless_total`
and `counts_by_kind` are computed before result truncation. `ownerless_total`
counts file-origin rows; URL rows use file origins by design, without claiming
that no lexical enclosing function exists. Stable source order
and full tie-breaks make repeated responses deterministic. Counts refer to
captured static facts, not runtime requests/SQL executions or distinct physical
objects. Ambiguous results have no selected occurrence page.

Responses expose the index revision, a fresh-subset view, stale/unindexed/error
coverage and body-free provenance. SQL diagnostics are separately bounded and
scoped to project/path, not falsely labelled table-specific. Stale source facts
are excluded before query filtering; sync replaces their captured facts.

No extraction-format bump is required: this query uses already captured SQL
and route/endpoint facts. No source code or SQL is executed. Physical database
binding, handler dispatch, translation catalogue/framework binding, additional non-code kinds,
nullable-project target selection, representative corpus quality/scale and
cross-surface CLI/HTTP exposure remain open. See
[qualification](qualification/non-code-references-2026-10-26/README.md).

The [typed symbol inventory](SHARED_SYMBOL_NODES.md) lists captured key/table/URL
identities and provides reference_target selectors for navigation to these uses.

JS client references now include [jQuery/Axios call syntax](CLIENT_ENDPOINT_REFERENCES.md), preserving request_method_basis in references and literal impact without asserting library/runtime binding.
