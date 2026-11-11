# Route and endpoint query contract

MCP `routes` and native `Store::code_routes` accept `RouteQuery` (query contract 1).
Old empty requests and `path`, `symbol`, `limit` parameters remain supported.
This is a query-only extension: index format remains 24 and existing immutable
extraction revisions are not rewritten.

| Parameter | Meaning |
|---|---|
| project | Exact project/wing; also scopes source/cache decoding and coverage. |
| path | Exact ingested source file path, preserving the earlier CodeQuery meaning. |
| symbol | Existing exact symbol ID/name/qualified-name selection semantics. Unbound references/registrations cannot match a symbol. |
| route | Exact captured address or declared registration literal. |
| route_contains | Case-sensitive literal substring; %, _, . and * are ordinary characters. |
| kind | all (default), registration or endpoint; unknown values are invalid. |
| method | Case-insensitive exact server registration method. ANY matches unknown candidates only; it is not a wildcard. |
| request_method | Case-insensitive exact explicitly captured client request method. No defaults inferred. |
| reference_kind | Exact endpoint category, e.g. html:form:action or js:fetch. |
| limit | 1..500 returned records (default 50, clamped). |
| offset | Matching-record offset, default 0. Nonzero requires expected_view_token. |
| expected_view_token | Token from the first page of the same query. Changed query/view returns ROUTES_VIEW_CHANGED conflict. |

Filters combine with AND and run **before** pagination. Registration and endpoint
method fields do not substitute for each other. A URL filter never executes a
regex, SQL pattern, router parameter matcher or template. Source paths and route
literals remain different fields. Existing unused CodeQuery fields are retained
by the flattened compatibility schema; traversal/map fields do not affect routes.

`total` and `counts_by_kind` describe all matching records; absent count categories
are omitted. `items` contains only this page. `pagination` reports offset, clamped
limit, returned, has_more and next_offset. `truncated` means some matching records
are absent from this response (including records before offset), whereas has_more
means there is a later page. Empty/out-of-range pages do not overflow offsets.

The first response supplies `view_token`. Repeat identical filters with that token
and the returned next_offset; limit may change without invalidating the token.
Tokens hash the query filters, complete matching evidence, scoped coverage and
binding diagnostics, and the **global** published index revision. They exclude
limit/offset/expected token. They are concurrency checks, not authentication.
Token version rv1 uses length-delimited BLAKE3 hashing. A changed captured source
can invalidate a token via stale/unindexed coverage **without a new index revision**.
A global index publication may conservatively invalidate project tokens even
when caused by another project. Foreign-project ingestion without publication
does not affect a scoped view.

Each request observes one DuckDB read transaction. Pages are optimistic current
views, not retained snapshots: changes cause an explicit conflict, never an
implicit continuation into a different fresh subset. Restart at offset 0 without
a token after conflict. There is no live-filesystem check; an edit not yet ingested
is outside the captured view. Path/URL filters do not narrow project coverage;
coverage/diagnostics remain for the selected project (or all projects if omitted).

No reindex, FTS rebuild, URL fetch, application execution or source-file read is
performed. Per-page record limits are not full-response byte, whole-index memory
or latency guarantees. Scope selection occurs before foreign cache decoding;
corruption in the selected scope still fails explicitly. Static extraction and
unbound/unknown-method limitations remain as reported by the routes envelope.

Client evidence extension: [jQuery/Axios syntax candidates](CLIENT_ENDPOINT_REFERENCES.md) also participate in request_method filters. Their request_method_basis distinguishes explicit options from method-name candidates; effective runtime methods remain unverified. The original query contract remains 1; extraction format 64 refreshes caches.
