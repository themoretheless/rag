# Fetch endpoint-reference subset — 2026-10-25

`routes` now captures literal root-relative URL operands in JavaScript/TypeScript
`fetch`, `window.fetch`, `globalThis.fetch` and `self.fetch` call syntax. JS, JSX,
MJS, CJS, TS and TSX use the same collector; HTML inline JavaScript uses it with
coordinates remapped to the complete ingested source. The MCP tool description
advertises both registrations and endpoint references.

These remain `http_endpoint_reference` / `js:fetch`, with null server `method`
and `symbol_id`, `resolution: unbound_reference`. Syntax does not establish that
`fetch` is the platform implementation: it can be shadowed, imported or mutated.
`detail.fetch_identity_verified` and runtime verification are false. Endpoint
references do not become handler/liveness roots.

An explicit plain standard HTTP method in an object literal is captured as
`request_method`, separately from the server registration method. Lowercase
standard verbs are normalised. Missing/dynamic/duplicate/computed/spread/shorthand
options or unsupported method values retain an unknown request method. No GET
or other default is inferred. This is not evaluation of JavaScript options.
Object properties that cannot statically affect the method may have dynamic
values; any unknown key/spread conservatively invalidates the method observation.

Only plain string or interpolation-free template literals are accepted. Escapes,
concatenation, variables, URL objects, dynamic templates, external/network-relative
or relative targets are not evaluated. URLs preserve literal query/fragment
spelling and do not decode HTML entities in JavaScript strings. No URLs are
fetched and no application code is run. Aliases, arbitrary client.fetch, bracket
access, axios and other request clients remain open.

Inline scripts are analysed only when they have no `src` attribute and a missing,
empty, module or recognised JavaScript/ECMAScript MIME type. JSON/inert types and
inline bodies ignored by browsers due to `src` are skipped. The source URL of
an external script remains separate HTML evidence. This filter also prevents
those inert bodies from creating CSS class-use facts through the shared embedded
JS collector. Legacy MIME/language variants and framework-specific script
semantics remain unqualified.

Index format **23** refreshes older extraction caches at normal explicit indexing
or source sync; frozen older revisions remain unchanged. Historical endpoint
relations include `request_method`, so POST→PUT changes produce a removal/addition
without a URL change. Older formats have absent method evidence and are marked
by `analysis_format_changed`, not treated as proof of past runtime behavior.

Three new tests cover literal/method extraction, conservative exclusions, embedded
coordinates and inert/external-script behavior. The isolated stdio MCP script
checks the public tool description, TS and HTML evidence, explicit versus unknown
methods, deterministic results, stale exclusion, source-sync refresh and frozen
project-isolated method changes. Reference scope is the route/endpoint trust
separation and fetch category in RAGRAF `op_routes` at revision
`075caf86dc2f6274b9c48487e521915bcdfeaf2c`. Full endpoint coverage and representative
quality/performance comparison remain open.

Reproduce:

```sh
cargo test --lib code_index:: --locked --offline
cargo build --bin rag-mcp --locked --offline
python3 scripts/smoke_fetch_endpoints.py target/debug/rag-mcp /tmp/fetch-endpoints.json
```

Final verification: **1,183 workspace tests passed, 0 failed, 1 ignored**;
137 focused code-index tests passed. Workspace Clippy with `-D warnings`,
formatting and `git diff --check` passed. The rebuilt binary passed
[fetch MCP evidence](runtime.json) plus
[markup](markup-regression.json), [Python routes](python-routes-regression.json),
[index-status](index-status-regression.json) and
[repo-map](repo-map-regression.json) regressions.
[Verification manifest](verification.json) records working-tree source and binary
hashes. MCP specifically proves POST→PUT changes with the same URL, unchanged
foreign-project relation sets, and ignored non-JavaScript/external-script bodies.
These isolated mock-provider checks are not production or representative-scale
qualification.
