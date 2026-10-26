# Duplicate candidate roles, contract 2

The pinned RAGRAF duplicates operation selects a primary using entrypoint and
incoming code/path evidence. Local exact decoded-text and numbered-name
similarity groups already existed. Contract 2 extends their primary-candidate
ranking with the index's captured non-call references, file imports and HTML/CSS
resource candidates, including independently published non-code source identities.

The relation inventory is explicit: entrypoints, bound calls, bound value/type
references, static file imports and file resources. Import candidates can exist
without resolving an exported symbol. Resource candidates include responsive
choices and inline CSS facts, without proving browser selection or execution.
Evidence retains source document/path/project/revision/hash, line and byte
anchors, relation kind and resolution. Source bodies are not included.

Scope and freshness are checked independently of candidate grouping. The scoped fresh index validates source fingerprints before the target
path-prefix filter, so a referrer outside the selected folder can qualify. Cross-project links,
self-links, stale/unindexed/parse-error bridges and mismatched source identity
are excluded. Parsed targets require the same captured revision/hash/path/project;
non-code targets require a fresh published source identity with matching content.
A changed source or target between corpus and index observations cannot keep a
previous positive role merely because its path still exists.

Corpus and fresh-index reads now share one project-scoped read transaction,
with matching fingerprints required. This proves stored-view coherence, not
live filesystem freshness. Missing evidence never proves an unused file. Unregistered/raw binary
assets, unsupported loaders and dynamic/framework/runtime activity remain open.

Ranking is deterministic: role score descending, observed evidence count
descending, path length, path, then document identity. Conventional main/route
candidates score 4, resolved static incoming candidates 3, ambiguous candidates 2,
other entrypoint candidates 1, and no qualifying observed role 0. These ordinal
weights are uncalibrated heuristics. Counts measure captured relation occurrences,
not execution counts. A repeated source import is not repeated runtime loading.

The primary is selected from the same sorted member sequence before applying the
member limit, so even max_members=1 retains it. Each member returns evidence_total
and an independently bounded 100-item evidence window. Bounded retention is
canonical and independent of edge enumeration order; truncation remains explicit.
File resource identities are indexed by document ID during ranking rather than
rescanning the attachment inventory for every edge.

Duplicate grouping/comparison caps and stored-text semantics are unchanged.
The extraction format and store schema do not change: these roles are computed
on queries over existing captured facts. search_multi now shares this role inventory and read-observation contract;
see [search roles](SEARCH_ROLE_EVIDENCE.md) for its retained-window limits.

A separate [literal path phase](LITERAL_PATH_ROLE_EVIDENCE.md) now supplements
uncovered typed source-target pairs with weaker bounded text evidence and
independent coverage/count reports.

No primary is proven, no deletion is safe, and similarity components do not prove
every pair is identical. Reference-wide contract parity, representative ranking
precision/recall, whole-response budgets, large-corpus performance and runtime
activity remain unqualified.

Qualification: [duplicate-roles-2026-10-26](qualification/duplicate-roles-2026-10-26/README.md).
