# PHP route-registration subset — 2026-10-25

`routes` now inventories PHP/PHTML registration syntax through tree-sitter AST:
`Route::get/post/put/patch/delete/head/options/trace/connect`, `any/all`,
`Route::match([...], '/path', action)`, `Router::...`, the fully qualified
`Illuminate\Support\Facades\Route` spelling, and known variable receiver names
`$app`, `$router`, `$server`. PHP static class/method matching is case-insensitive;
variable receiver names are case-sensitive. Recognised receivers are syntactic
candidates, not proof of framework identity.

`match` accepts plain literal method arrays, normalises standard verbs and emits
one candidate per distinct method. Dynamic/empty/spread/unsupported method sets
remain ANY (unknown). Only root-relative plain literal paths up to 4096 bytes are
accepted; interpolation, escapes, concatenation, computed/named/unpacked arguments
and unrelated receivers are excluded. Comments and string contents cannot create
registrations. Unknown/dynamic handler expressions are represented by fixed
markers, without returning closure bodies.

Literal handler names and `[Controller::class, 'method']` preserve bounded static
handler descriptions. They are **not bound** to similarly named declarations:
PHP namespaces, import aliases, container resolution and framework dispatch are
still unmodelled. All new PHP candidates have null `symbol_id`, unresolved
resolution and `php_handler_binding_verified: false`. They do not become liveness
roots or prove that same-named functions are used.

Paths are declared literals, not composed/effective mount paths. Group prefixes,
domains, middleware, fluent composition, resource routes, invokable controllers,
aliased facades, custom routers and framework defaults remain open. No PHP code
is executed. Source records retain document/project/revision/hash/byte evidence.

Frozen historical diffs now include `unbound_route_registration` relations for
unresolved registration candidates (including existing supported languages),
with document scope, path, method and handler description. This closes the earlier
omission of registrations that had no resolved handler symbol. Semantic comparison
ignores line/offset moves and duplicate occurrences, while `routes` preserves
individual source occurrences. Old graphs without document identity cannot
supply this new evidence; format transitions remain explicit.

Index format **24** refreshes earlier caches at explicit indexing/source sync;
immutable old revisions remain unchanged. The reference scope is PHP route
inventory and route/endpoint trust separation in RAGRAF revision
`075caf86dc2f6274b9c48487e521915bcdfeaf2c` (`extract/php_ast.py`, `op_routes`).
This is not full PHP/language parity or representative quality superiority.

Three extraction tests cover supported calls/method arrays and conservative
comment/string/dynamic exclusions. A fourth analysis regression verifies that
same-name functions are not guessed as handlers or made live. The owned-database
MCP scenario covers actual tool discovery, method sets, descriptions, staleness,
sync refresh and project-isolated immutable unbound-method changes.

Reproduce:

```sh
cargo test --lib code_index:: --locked --offline
cargo build --bin rag-mcp --locked --offline
python3 scripts/smoke_php_routes.py target/debug/rag-mcp /tmp/php-routes.json
```

Final verification: **1,187 workspace tests passed, 0 failed, 1 ignored**;
141 focused code-index tests passed. Workspace Clippy with `-D warnings`,
formatting and `git diff --check` passed. The rebuilt binary passed the
[PHP MCP scenario](runtime.json) plus [fetch](fetch-regression.json),
[markup](markup-regression.json), [Python routes](python-routes-regression.json)
and [index-status](index-status-regression.json) regressions.
[Verification manifest](verification.json) records source/binary hashes. MCP
verifies GET→DELETE unbound-registration changes, scope isolation, and no guessed
liveness root despite a matching local function name. No PHP runtime or
production-service qualification is claimed.
