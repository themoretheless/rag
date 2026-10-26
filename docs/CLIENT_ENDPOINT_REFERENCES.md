# jQuery and Axios URL syntax candidates

The existing JS/TS and executable inline-HTML endpoint collector now captures
literal root-relative URLs from named jQuery and Axios call syntax. These facts
flow through `routes`, URL `references`, shared URL symbols and literal URL
`impact`, without turning a client URL into a route handler or entrypoint owner.
The collector parses AST calls/arguments/options; strings, comments and inert or
external-script bodies do not become fake calls. Client names can be shadowed or
imported from another implementation: library identity is explicitly unverified.

Supported jQuery spellings are `$.` and `jQuery.` with `get`, `post`, `getJSON`,
`getScript` and `ajax`. Literal first-URL arguments are captured; object settings
are supported for `get`, `post` and `ajax`, including `ajax(url, settings)`.
Axios captures `axios(url, config)`, `axios(config)`, `axios.request(config)` and
named `get`, `post`, `put`, `patch`, `delete`, `head`, `options`, `postForm`,
`putForm`, `patchForm` aliases. Post/put/patch data arguments are not config: their
`url`/`method` keys cannot change captured URL/method evidence.

`reference_kind` identifies syntax (`js:jquery:get`, `js:axios:post`, etc.).
`request_method_basis` distinguishes `explicit_options` from
`client_method_name_candidate`. HTTP method names from aliases are stated syntax
candidates, not an inferred effective runtime method. Generic requests have no
assumed GET default. jQuery `method`/`type` disagreement, duplicate/dynamic method
fields or a contradiction with an alias leave the method unknown; the URL may
remain captured. Options that can ambiguously replace a URL (spread/computed/
duplicate/dynamic URL fields, unknown options, or conflicting positional/config
URLs) are excluded. Known unrelated properties/data do not replace URL evidence.
These guards deliberately do not guess a version-specific merge order.

Like existing fetch evidence, URLs are limited to 4096 UTF-8 bytes, retain
query/fragment spelling, reject escapes/interpolation/concatenation and exclude
relative, external and scheme-relative addresses. There is no baseURL/origin
composition, global ajaxSetup/default/interceptor resolution, runtime execution,
library alias/instance resolution, XHR/open/load support or proven network call.

Query method/category filters apply before pages. URL references and impact
preserve the method basis and original enclosing-file UTF-8 byte/line identity.
Indirect impact uses the actual enclosing source callable, never a guessed
remote handler. Source sync upgrades extraction format 64 and replaces changed
facts; ingest alone makes old facts stale and excludes them.

Frozen revisions carry `client_endpoints_format: 1`. Client URL/method/category
relations compare only when both snapshots have that marker. A legacy gap is
explicit (`client_endpoint_relations_available: false`) and never becomes fake
added/removed client evidence. Source-position-only changes remain ignored.
The method-basis annotation itself is retained in snapshot entrypoints but is
not a separately classified semantic diff.

API vocabulary checked against [jQuery.get](https://api.jquery.com/jQuery.get/),
[jQuery.ajax](https://api.jquery.com/jQuery.ajax/) and the
[Axios API reference](https://axios.rest/pages/advanced/api-reference).
These documents explain call forms, not runtime binding of captured identifiers.
See [qualification](qualification/client-endpoints-2026-10-26/README.md).

The generic server registration collector excludes these same name-shaped
jQuery/Axios member calls, including forms whose URLs/configs remain unparsed.
This prevents double classification and callback/data arguments from becoming
fake remote handlers. An Express instance itself named axios/jQuery/$ is not
resolved as such; symbol/library-aware disambiguation remains open.
