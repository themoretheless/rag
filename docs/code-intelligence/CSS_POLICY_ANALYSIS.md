# CSS class usage and permission decision traces

The full MCP surface (`RAG_TOOLS=full`) exposes `class_usage` and `policy_trace`.
Both use the verified, fresh subset of the persisted code-index revision.
`sync_sources` refreshes this index automatically. After direct document writes,
run `index_code`; stale facts are omitted and reported until then. These tools do
not read arbitrary filesystem paths or execute application code.

## Class usage

```json
{"name":"class_usage","arguments":{"name":"button-primary","project":"frontend"}}
{"name":"class_usage","arguments":{"mode":"missing","project":"frontend","limit":100}}
{"name":"class_usage","arguments":{"mode":"unused","project":"frontend"}}
```

The CSS tree-sitter grammar extracts class-selector mentions, including escaped
and Unicode identifiers, and enclosing media/supports contexts. The HTML grammar
extracts static class attributes, decodes HTML entities, and parses inline styles
and scripts with their original source offsets. JSX/TSX attributes and static DOM
`classList.add/remove/toggle/contains/replace`, `setAttribute('class', ...)` and
`className` assignments contribute application/reference evidence. Comments,
attribute-selector string values and CSS declaration strings are not definitions.
Dynamic expressions and unsupported escapes are diagnosed rather than fabricated.
Vue/Svelte markup is best effort with parse diagnostics; it is not a compiler for
framework directives or preprocessors. Svelte class directives are conditional
mentions. Only indexed CSS, HTML/HTM, Vue/Svelte, JS/TS/JSX/TSX are covered.

Class names match exactly within a project; separate projects never satisfy each
other's missing definitions. With no project filter, separate project groups are
returned. `all` lists all observed classes, `missing` shows uses with no indexed
selector mentions, and `unused` shows selector mentions with no indexed uses.
A name accepts one optional leading dot. These are **corpus observations**, not
browser results, proof of missing visual styling, or permission to remove CSS.
No cascade, selector satisfaction, import/load order, CSS-module scoping, runtime
class construction, external stylesheets or preprocessors are resolved. External
links/imports and dynamic applications make incomplete coverage explicit.

Each site includes document/path, source line and byte column/offset, revision,
hash, syntax origin, selector and conditions. HTML class evidence points to its
attribute value; entity decoding does not invent more precise token positions.
Responses bound class records and total evidence sites (1–500), return full
observed counts and separate truncation flags. Per-document extraction caps at
50,000 facts and 2,000 diagnostics, with explicit budget diagnostics. Local HTML
retains original markup separately from its readable retrieval body; changes that
leave readable text unchanged still refresh the index.

## Permission traces

```json
{"name":"policy_trace","arguments":{"symbol":"Permissions::allows","path":"/repo/permissions.php","action":"approve","depth":3}}
```

Rust, Python, JavaScript/TypeScript and basic PHP functions/methods are supported.
The target must be unambiguous; use a symbol ID, path or project to disambiguate.
Explicit boolean returns are split into `allows` / `denies`; computed returns are
`unknown`, never presumed permissions. Literal ternary returns are split into
branches. Nested functions/closures, comments and unreachable statements after
straight-line returns are excluded. Conditions include enclosing if/else branches
and prior terminating guards. Switch cases retain grouped/fallthrough labels;
action filtering retains default, unconditional and computed-label context.

`default_behavior` follows the **syntactic fallback**: all explicit if guards
false, then the switch default arm and following statements. It can be `allow`,
`deny`, `unknown`, `implicit_return` or `no_return`. A conditional denial followed
by unconditional `true` has fallback `allow`; the mere presence of a denial does
not imply fail-closed behavior. `fail_closed` is true/false only for literal
fallback deny/allow, otherwise null, and is explicitly scoped to that fallback.
This is not full control-flow, role evaluation or a proof of authorization.
Loops, try/finally and unsupported control flow are diagnosed and block a confident
fallback result. Rust match exhaustiveness, side effects/exceptions, condition
satisfiability, runtime roles and dynamic dispatch are not evaluated.

Callers use bounded reverse traversal of the existing static candidate graph,
with resolution/provenance and depth. Missing PHP namespace/import binding support
leaves those calls unresolved with diagnostics; cross-file PHP imports, receiver
types and inheritance remain unqualified. PHP source parsing does not yet extract
embedded markup classes, ORM queries or framework routes. Decision and caller
outputs have separate budgets/truncation flags. Conditions/expressions are bounded
to 512 characters, so source evidence should be opened for long expressions.
