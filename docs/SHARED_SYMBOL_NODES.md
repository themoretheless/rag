# Shared captured symbol nodes

`symbol` and native `Store::code_symbols(SymbolQuery)` now inventory code
source definitions or shared captured translation-key/table/URL identities.
The default remains code definitions. Optional `project` scopes cache decoding,
facts and freshness diagnostics. No filesystem or runtime database/router is read.

## Selectors and modes

`symbol_kind` accepts `code`, `translation_key`, `table`, `route`, `all`.
`symbol` keeps its previous exact ID/name/qualified-name selection. `name` is an
alias for reference-style clients; passing both is an error. With no explicit
kind, `tpl:key`, `@table:public.users` and `route:/items` select their corresponding
shared domain. Rust-like `tpl::member` and `route::member` remain code selectors.
Explicit `symbol_kind=code` always preserves code interpretation.

`match_mode=contains` is an explicit case-sensitive literal substring of name or
qualified name: `%`, `_`, `.*` and regex syntax have no special meaning. Exact
is the default; there is no hidden substring fallback. For SQL tables an exact
unqualified identifier may list multiple qualified identities, using the existing
quoted SQL identifier rules. Listing never silently picks one schema/project.

Example: `symbol(symbol_kind="translation_key", project="web", name="reg.months")`.
Omit the name to list a domain. `symbol_kind=all` combines typed shared identities
and source definitions, with counts by domain before the 1..500 output limit.
Code definitions retain existing source/signature/documentation behavior; mixed
or contains-mode code rows add project and `node_domain=code` metadata.

## Stable identity and honest location

Shared IDs are a versioned BLAKE3 hash of domain, exact project (including null)
and canonical captured name. They do not depend on which source was last indexed,
source positions, text hashes, evidence windows or use counts. Equal names in
different projects have different IDs. Prefix-qualified names provide familiar
navigation, without injecting fake source definitions into the code call graph.

Shared nodes have null `path`, `start_line`, `end_line` and `signature`.
`definition_location_available=false` prevents a template key or URL from
acquiring whichever file happened to be visited last. `source_evidence` gives the
actual captured locations with document/project/revision/hash anchors instead.
Table `captured_schema_evidence` and nullable `captured_schema_complete` describe
captured SQL schema replay in the selected evidence scope, not a live DB schema.
Usage-only tables and schema-only tables remain distinguishable.

Each node provides `impact_target` for the [impact tool](LITERAL_TARGET_IMPACT.md)
and `reference_target`, e.g. `{key, project}`, `{table, project}`
or `{route, project}`, for the existing [references](NON_CODE_REFERENCES.md) tool.
For named projects and supported selector bounds these selectors roundtrip the
node's uses (keys: 120 Unicode characters, SQL identifiers: 512 bytes, URLs:
4,096 UTF-8 bytes). An unassigned
project currently cannot be explicitly selected when another project shares the
same name; this limitation is stated on the node rather than guessed away.

## Facts and limits

`observed_usage_total`, `schema_evidence_total` and `counts_by_relation` retain
full counts before proof/output limits. SQL uses retain access/operation; route
proofs distinguish registration and client request methods. Alternate handler
candidates at one registration/method are one source occurrence, not extra URL
uses or a bound runtime handler. Translation keys use the existing
[captured syntax and coverage contract](TRANSLATION_KEY_REFERENCES.md).

`path` scopes code definitions by definition path, and shared nodes by exact
source-evidence path before aggregation. IDs stay stable; counts and proofs refer
to the selected project/source window, not an unstated global corpus total.
A schema source outside that window does not become proof of a declaration in it.
`key_capture_coverage` separately reports selected-project syntax coverage and
limited captures; its scope is not falsely restricted to the matching name/path.

`evidence_limit` defaults to 5, clamps to 1..50 and is independent of node `limit`.
Proofs retain deterministic source path/document/numeric line/byte order, with
stable ties, full fact totals and explicit truncation. They are captured fact
records, not counts of distinct files. No source bodies or translation values
are added to shared rows. Corpus/snapshot loading and identity aggregation are
outside these output/proof budgets; whole-query memory/scale is not qualified.

Stale inputs are excluded and reported, and source sync replaces facts without
changing surviving shared identities. Catalogue definitions, locale/framework
binding, external placeholders/unresolved nodes, additional domains, implicit
reference substring ranking, broader CLI/HTTP/UI exposure and representative
quality/scale remain open. [Qualification](qualification/symbol-nodes-2026-10-26/README.md).
