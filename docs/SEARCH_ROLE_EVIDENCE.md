# Multi-search family role evidence, contract 2

search_multi now uses the same captured role inventory as duplicates: entrypoint,
bound call/value/type, module import and HTML/CSS resource candidates, including
published non-code source identities. An unresolved imported symbol does not
erase a captured file import. Responsive/inline resource evidence retains its
existing descriptor/property qualifiers without proving browser selection.

The path-prefix filter selects search targets, not potential referrers. Source
documents outside that folder and sources not matching the search strings can
contribute evidence. They must belong to the same project and match the fresh
index's source path/revision/hash. Target identity must match either the parsed
snapshot fingerprint or a fresh independently published non-code source identity.
Stale, unindexed and parse-error sources/targets are excluded; absence is unproven.

Both search_multi and duplicates now read stored corpus and scoped fresh index
under one DuckDB read transaction. Role evidence cannot combine a pre-publication
corpus with a later graph. A cloned-connection qualification publishes new source,
parsed cache and immutable graph history while an existing reader stays open:
that reader retains revision 1 and its roles; subsequent reads see revision 2 and
the removed dependency. This tests one shared database instance, not separate
database instances, operating-system processes or filesystem synchronization.

The corpus reader first inventories IDs/paths, applies exact Rust path-component
scope, then loads selected bodies in bound-parameter batches of at most 512 IDs.
It does not approximate path scope with SQL LIKE or decode unselected source blobs.
The fresh index still validates captured sources in the selected project. This
is not a representative performance, memory or response-budget qualification.

Search matching, all/any behavior, independent per-query windows, occurrence
snippets and decoded-source encoding evidence retain their prior contracts.
Families cover only retained per-query windows; this does not claim ranking all
matching documents before those windows. Within a retained family, observed
ordinal role score and evidence count precede the deterministic path tie.
Role evidence is independently capped at 100 with a complete occurrence count.
Canonical serialized evidence keys are computed once during bounded retention.

family_role_analysis advertises role_contract_version=2, import/markup inclusion
and single_read_transaction consistency. Zero-hit queries keep role analysis
null and skip reading the derived code index. A malformed index payload in a
different project is not decoded for an explicitly project-scoped search.
Positive queries with corrupt required index facts still fail explicitly.

Search results intentionally include bounded matching text snippets. The
body-free promise applies to role metadata, not those requested search excerpts.
No source program, browser resource or SQL is executed. Roles remain uncalibrated
static observations; they do not prove execution, correct primary choice,
unreferenced/dead files or deletion safety. A separate [literal path phase](LITERAL_PATH_ROLE_EVIDENCE.md) now adds weaker,
bounded exact text mentions with independent budgets/coverage. Dynamic/framework/
runtime usage, global search ranking and representative quality/scale remain open.

This stage changes query-time role computation, not the extraction format or
store schema. Qualification:
[search-role-observation-2026-10-26](qualification/search-role-observation-2026-10-26/README.md).
