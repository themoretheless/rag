# PHP trait-body consumer contexts

`self::`, `$this->` and `parent::` method occurrences inside captured PHP/PHTML
trait bodies now receive separate candidates for the classes/enums that retain
that body. Direct composition and nested traits are considered in the same
project. A class override removes the original body unless an alias retains it.
A child that merely inherits the method does not become a new lexical consumer.
Explicit class names still use the trait's namespace/import table; only the
access/lexical owner is rebound to the consumer.

Call edges and bound flat `references` expose optional `context_evidence`:

```json
{
  "kind": "php_captured_trait_consumers",
  "enumeration_complete": true,
  "truncated": false,
  "runtime_contexts_complete": false,
  "context_correlation_proven": false,
  "contexts": [
    {"owner": "captured-class-symbol-id", "candidates": ["method-symbol-id"],
     "resolution": "resolved_static", "resolution_basis": "..."}
  ]
}
```

Each row keeps its class identity, target alternatives and failure reason. Public,
protected/private and static/instance checks use that row's consumer. The aggregate
candidate list is a union for conservative graph navigation, not a universal
runtime target. Multiple consumer rows remain `ambiguous`, even with a single
unique target, as does incomplete enumeration with surviving candidates. An
eligible single captured consumer can report `resolved_static` while retaining
explicit runtime limitations. First-class callable values carry context evidence
without becoming calls. `static::` remains unresolved with a reason per consumer.

`enumeration_complete` covers the eligible loaded captured subset only. It does
not establish a complete runtime corpus or imply that all rows resolve. Missing,
invalid or conflicting composition stays explicit. Enumeration is bounded by 128
direct consumer declarations, 128 candidate names per consumer, 32 nested levels
and an 8,192-step shared discovery/provider budget. Limit states preserve partial
results with incomplete/truncated flags. Discovery is memoized per original method
within one project resolver view; evidence is not shared between projects.

The new optional field is omitted for other edges, preserving their serialized
shape. The resolution cache retains and checks context metadata, expected sites,
class/method identity and project membership, aggregate union and unresolved
states. Missing/corrupt metadata, false runtime-proof flags or cross-project IDs
fall back to recomputation. First-class values and class-type references that
share a byte offset remain distinct cache sites.

PHP capture format 4 repairs older payloads through indexing/source sync. Frozen
graphs retain `php_trait_contexts_format`; diff adds capability-qualified
`php_trait_body_context_candidate` relations and
`php_trait_context_comparison`. A swap of owner-to-target mappings is visible even
when ordinary aggregate call edges are unchanged. Position-only differences are
excluded from these relation identities. Missing legacy context inventories are
not compared as removals/additions of context relations.

## Remaining boundaries

Candidates feed callers and impact with their context evidence. Callers/callees
retain a conservative union. Impact now matches captured entry/body lexical frames:
[trait-frame traversal](PHP_TRAIT_PATHS.md). Runtime correlation remains unproven,
and other aggregate-graph analyses can over-approximate consumer combinations. Runtime
subtype flow, dynamic receivers, late static binding, compiler signature/variance
and complete PHP grammar/version semantics remain open. Load order and actual
execution are not inferred from captured composition. PHP CLI is unavailable in
the qualification environment.

[Qualification](qualification/php-trait-contexts-2026-10-26/README.md).
Language references: [traits](https://www.php.net/manual/en/language.oop5.traits.php),
[scope resolution](https://www.php.net/manual/en/language.oop5.paamayim-nekudotayim.php).

`target_owner` optionally identifies a target trait body's lexical consumer; entry contexts and matching are specified in [PHP_TRAIT_PATHS.md](PHP_TRAIT_PATHS.md).
