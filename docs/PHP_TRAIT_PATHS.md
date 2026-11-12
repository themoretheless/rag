# Captured PHP trait-frame impact traversal

Impact now follows PHP trait call/reference paths using `(symbol, lexical consumer)`
states. It no longer joins paths solely because two classes share the same trait
method symbol. A change to A's method reached through a shared trait can reach A's
entry caller without incorrectly including B's incompatible entry caller.

Call bindings retain the target trait body's effective lexical owner. Normal
class/function entries into a trait body expose `php_captured_trait_entry` context
evidence; its `owner` and `target_owner` identify that target consumer. Body context
rows retain the source consumer in `owner` and, when the target is another trait
body, the target consumer in `target_owner`. These owners can differ when a body
calls an inherited trait method. All rows preserve original definition IDs and
source evidence; no class-specific method definition is synthesized.

Incoming traversal matches a required target consumer against `target_owner`, then
propagates the body's source consumer. Normal callers leave the trait frame.
Unqualified roots can explore all compatible captured contexts. A symbol's second
context is explored even after that symbol has a returned witness, preventing
loss of a valid branch. Results still contain one shortest discovered witness per
symbol and unique affected-file counts. `trait_context` on a witness records the
source/target lexical state used at that step.

Matching applies to the shared symbol traversal for calls, references and all
relation modes, including SQL-table and literal adapters that delegate to it.
Callable values remain references, not calls. SQL read/write and identity filters
still select roots before the walk. Project/freshness/parse-error exclusions apply
to consumer owners as well as source/target symbols. A required frame with missing
upstream metadata is not guessed; the walk reports incomplete metadata.

`trait_context_traversal` reports matching mode, visited state count, pruned
transition events and incomplete/work/state-limit flags. Work is bounded to
262,144 charged steps and 65,536 states per query, supplementing symbol and depth
limits. Symbol witnesses are deduplicated independently of state visitation.
`truncated` includes refused result items or traversal work/state limits;
`depth_limited` considers compatible unvisited states, not incompatible branches.
Missing encountered context metadata prevents `traversal_complete=true`.

PHP capture format 5 repairs older payloads. Context cache validation includes
entry versus body kind, required frames, project/class/method identities and the
aggregate target union. Frozen snapshots carry `php_trait_paths_format`; diff
adds separately guarded entry-context relations and target-owner identities only
when both snapshots have that capability. Legacy enrichment does not establish a
source-semantic change. Qualified runtime: index format 78, store schema 25.

## Limits of this qualification

This matches captured lexical trait frames, not actual runtime objects. `$this`
subtypes, late static binding, dynamic receivers, compiler signature/trait validity,
load order and full PHP grammar/version coverage remain unverified. Other analyses
that consume aggregate candidate unions are not made context sensitive by this
change. `context_correlation_proven` and runtime proof flags remain false; bounded
matching diagnostics do not claim universal program-path or execution proof.

[Qualification](qualification/php-trait-paths-2026-10-26/README.md) includes exact
negative/positive consumer cases, nested inherited frames, multiple states per
symbol, callable values, SQL write impact, limits, cache and frozen history.
