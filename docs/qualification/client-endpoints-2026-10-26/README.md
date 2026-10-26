# Client endpoint syntax — 2026-10-26

[Contract](../../CLIENT_ENDPOINT_REFERENCES.md). Pinned RAGRAF reference commit
075caf86dc2f6274b9c48487e521915bcdfeaf2c contains an `ENDPOINT_CALL_RE`
including fetch/open/load/ajax/.post/.get and other call-name patterns.
The new local AST collector extends existing fetch evidence with jQuery/Axios
URL and settings forms. It separates explicit method options from name-based
method candidates and refuses an arbitrary winner for ambiguous URLs/methods.
It does not claim coverage of every reference regex pattern or relative URLs.

Eight new native regressions cover jQuery/Axios forms, data-versus-config
positions, method/URL ambiguity, category/method evidence, UTF-8 inline source
anchors, inert/external scripts, URL size/language guards and immutable client
method changes. A legacy revision lacking the new capture marker is explicitly
unavailable for those relation comparisons. Existing fetch tests remain gates.

The real MCP fixture exercises routes/reference/impact integration, method and
category filters, original source offsets, project isolation, warm reuse,
stale exclusion, source-sync refresh and frozen method changes. A pure regex
assignment is extracted from pinned reference source using Python AST; the
reference application is not imported or started. Its regex reports two URL
hits inside a controlled comment/string fixture; local AST-backed MCP evidence
excludes both. This is a controlled correctness improvement, not representative
precision/recall or a whole-reference equivalence claim.

All MCP runners use one fixed copied executable, temporary sources/databases
and mock embeddings. No ingested application or SQL is executed and no production
database is opened. Source compilation uses the shared dirty checkout, including
concurrent Rust analysis work from another user-owned chat. Post-check source
hashes are observational; this is not an atomic frozen source checkout.

Runtime binding, client aliases/instances, defaults/interceptors/ajaxSetup,
relative/base URL composition, XHR and load/open patterns, broader quality/scale
and whole-RAGRAF acceptance remain open. Per-scenario JSON/logs, binary.json and
verification.json record the final gates and exact copied executable.

The initial real MCP run reproduced duplicate classification: `axios.post` was
also captured by the old generic server-route collector. The endpoint-reference
and spurious registration appeared together in URL references. The raw failure
is retained in client_endpoints-before-fix.log. Name-shaped jQuery/Axios member
calls are now excluded from that generic server registration collector; a new
regression preserves ordinary app.post handlers while excluding fake client
handlers/registrations. Extraction format 64 invalidates earlier cached facts.

Final gates after the classification fix: **1,502 workspace tests passed,
0 failed, 1 ignored**, including all **8 new regressions**. Workspace/all-target
Clippy with `-D warnings`, binary build, owned-module formatting and
`git diff --check` passed. The previously observed Rust cfg test also passed in
this shared-tree run; no repair of that independently edited subsystem is
attributed to this milestone.

All **8 real MCP runners passed** against the same fixed executable: new client
endpoints, existing fetch endpoints, route queries, non-code references, literal
impact, shared symbol nodes, PHP parameter contracts and SQL table impact.
The original duplicate client/server classification scenario now passes without
changing its expected reference/impact counts. Controlled reference regex false
hits: 2; local MCP false hits on that fixture: 0. Runtime index format 64,
store schema 25. Broader feature/quality/scale acceptance remains open.
