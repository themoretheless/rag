# Inherited method declaration candidates

`inheritance` now returns a separate `method_relations` collection alongside
class extends/implements edges. Selecting a class finds its own declared member
candidates; selecting a method finds incoming/outgoing candidates. `references`
in bound mode returns incoming candidates to the selected base method in the
same separate collection. Lexical mode does not invent declaration links.
The existing occurrence `items/total/truncated` retain their meaning. Each
collection has its own query limit, total and truncation flag.

For Python/JavaScript/TypeScript class declarations with a complete static C3
hierarchy, a same-name declared method is linked to the nearest ancestor that
declares that name. Missing/cyclic/ambiguous hierarchies fail closed. Known
blocking fields and Python property/cached_property declarations stop lookup.
Imported base aliases use existing resolution. Candidates use exact document
identities and equal project identity, including when paths are identical.
JS/TS constructors and repeated same-qualified class declarations are excluded.

These are **override candidates**, not language-semantic override proofs.
Signatures, arity/variance, decorators, visibility, private/static/accessor rules,
metaclasses, monkey patching and runtime dispatch are not validated. A unique
static target does not prove substitutability. Every edge states
`semantic_override_proven=false`, carries its candidate IDs, owner class,
document/path/line, source revision/hash and resolution basis. Ambiguous member
declarations retain all nearest candidates. Unsupported syntax or absent links
are not evidence that no overrides exist. Java/C++/Rust trait/other language
implementations and interface contracts remain outside this collection.

These relations create no synthetic calls and do not change dead-code or impact
reachability. Fresh snapshot omission/stale diagnostics apply normally. Index
format 29 freezes relations in graph history. `diff` reports
`method_override_candidate` additions/removals using stable declaration keys,
without source bodies. `method_relations_available=false` if either revision lacks
the frozen collection; then no override additions/removals are fabricated. Other
available relation kinds continue to compare normally. Database schema stays 25;
normal indexing or source sync refreshes older extraction/publication caches.
