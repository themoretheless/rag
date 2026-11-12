# Context-preserving Git change impact

`detect_changes` analysis contract 2 uses the exact bounded walk state for every
returned before/after witness. A shared trait method may be visited for several
consuming classes even though the affected-symbol list returns that method once.
A caller's witness follows its own predecessor states, rather than stitching
through the first symbol-level witness retained for a different consumer.

Each path step retains original source evidence and adds `source_context` and
`target_context`. Adjacent steps agree on the shared symbol and consumer state.
These contexts can differ within one step when a trait body calls another trait
body inherited from a base. `path.trait_context_symbols` supplies original class/
enum definitions for matched binding rows and state frames, including wildcard
seed entries. It does not invent class-specific copies of trait methods.
`path.complete_to_seed` checks the captured seed and contiguous state path;
`captured_trait_frames_matched` is limited to these captured lexical frames.
`runtime_context_correlation_proven` remains false.

Binding comparison now includes normalized per-consumer candidates, target owners,
resolution reasons and enumeration/limit state. Class/target IDs are mapped to
relative-path/qualified-name/kind identities before comparing sides. Swapping
which base target belongs to each consumer can mark an unchanged trait method
`binding_changed`, even when the aggregate candidate union is identical. Moving
class/method byte offsets alone does not become a binding change in another file.

The change applies to working/index, staged and pinned commit graphs. Existing
capture/source guards and affected-symbol/depth/seed limits remain in force.
Predecessors are retained only for internal Git path requests; ordinary `impact`
responses keep their bounded symbol-witness shape. Walk state/work limits still
apply. Paths are bounded to the existing maximum depth of ten and retain original
binding uncertainty; response size is not an exact byte/token budget.

No stored extraction schema change or ingestion is required. These are ephemeral
source graphs; the MCP framework can still write ordinary audit telemetry.
The qualification uses isolated temporary repositories/stores, never the user's
repository or database. The analyzer does not write source/index files, checkout
commits, run filters or execute ingested PHP/SQL.

[Qualification](qualification/git-php-contexts-2026-10-26/README.md).
Runtime subtype flow, late-static/dynamic dispatch, full PHP grammar/compiler
semantics, test recommendation context sensitivity and representative scale remain
open. Source capture and a complete retained path are not execution/absence proof.
