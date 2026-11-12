# Captured trait frames in trace_path

`trace_path` now constrains PHP trait method paths by captured lexical consumers
in forward, back and both directions. Entering a trait binds its effective class/
enum context; trait-body calls must match that source consumer and can enter a
different inherited trait frame. Adjacent steps keep a consistent node context.
An incompatible class cannot be reached by joining through the aggregate method
symbol shared with another consumer.

Paths add `node_contexts` parallel to `symbols`, original consumer definitions in
`trait_context_symbols`, and raw-edge `source_context`/`target_context` per step.
`reversed` still describes traversal relative to the original source/target;
context fields follow those original endpoints. Source/revision/hash evidence is
preserved and no class-specific copy of a trait definition is manufactured.

## Target ambiguity versus conditional consumers

The default requires target uniqueness within the selected captured consumer.
A multi-consumer aggregate can therefore yield a valid conditional path when its
matching row has one target. Such steps retain `aggregate_binding_ambiguous` and
`context_conditioned`, while `ambiguous_target` describes ambiguity inside the
selected row. `include_ambiguous=true` admits genuinely ambiguous row targets;
it never bypasses incompatible contexts or project/freshness guards.

A wildcard trait endpoint can yield multiple paths with the same symbol sequence
and different context assignments. They are distinct conditional paths and each
counts toward `max_paths`. An exact endpoint remains a source symbol identity,
not an assertion about an actual runtime instance. Simple paths still cannot
repeat a symbol; shorter paths and stable source/target/location/context ordering
are retained.

## Bounds and incomplete metadata

Graph candidate checks, missing-frame target inspection, context-row inspection
and traversal share `max_work`. Endpoint selection, snapshot loading and context
lookup preparation are separate work, not a wall-clock/RSS guarantee. Depth and
output limits are unchanged. Depth diagnostics test compatible next steps rather
than reporting incompatible branches as omitted continuations.

Missing required entry or scoped trait-body metadata is not interpreted as a
valid consumer-union bridge. `trait_context_traversal` reports pruned transitions,
incomplete input metadata and the context-local ambiguity policy. Independent
non-trait candidates are retained when ambiguity is explicitly enabled; an
unqualified trait candidate does not suppress unrelated valid alternatives.
Class/enum context definitions must belong to the fresh selected project subset.

Runtime path and absence flags remain false. Captured lexical matching does not
establish runtime subtype flow, late-static/dynamic dispatch, compiler/signature
validity, complete PHP grammar coverage or full control/data-flow slicing.
This query change does not mutate source, schema or stored extraction facts.

[Qualification](qualification/trace-php-contexts-2026-10-26/README.md) includes native
and isolated real MCP checks for directional matching, inherited frames, wildcard
contexts, enums, values, ambiguity, budgets, stale owners, cache equivalence and
source-sync refresh. Other analyses keep their separately qualified scope.
