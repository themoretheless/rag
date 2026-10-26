# Non-code captured resource identities

HTML/CSS resource edges now resolve to active stored source-file documents even
when there is no symbol/parser extractor for their format. JSON, XML/SVG and
other text or extracted document sources do not become code-indexed languages.
SVG files are discovered by source sync as XML text, preserving markup/scripts
without rendering, fetching or executing them.

The published graph captures a separate `file_resource_targets` inventory for
nonindexable source attachments. Each record retains exact document/path/project
identity, document revision and decoded/extracted stored-source hash. The live
captured view validates both that inventory and the revision-file table against
current stored source text. Source attachment membership is part of the index
fingerprint: adding an attachment with unchanged content/revision still creates
a new publication. URI-only wiki documents are not file-resource targets.

Current target states are `fresh`, `stale`, `unpublished` and `unavailable`.
Changed identity/revision/hash, absent or malformed inventories, duplicate IDs,
revision-table disagreements and missing/corrupt original source blobs cannot
qualify freshness. Old revisions lacking the inventory remain explicitly
unqualified; no metadata is reconstructed from a path-looking URI.

File impact merges code and resource identities for exact path/document/project
target selection. Unavailable targets remain found but cannot seed traversal.
It returns body-free captured target metadata and bounded resource diagnostics.
The existing code stale/unindexed/parse-error arrays keep their parser meaning;
non-code target diagnostics are separate. Equal project identity remains required
on each edge; multiple matching attachments require disambiguation.

The `dead_code` files view protects fresh resource targets with incoming edges.
Unavailable resource identity makes negative liveness checks incomplete in that
project; positive literal refutations remain usable. Candidates are never
removal permission. `index_status.resource_targets` reports separate inventory/
freshness coverage and adds `RESOURCE_TARGETS_UNAVAILABLE` to observed problems
when appropriate. Unsupported syntax extensions remain unsupported syntax.

Snapshot reads use one database transaction, including resource identity and
revision checks. Explicitly project-scoped MCP impact reads apply that scope
before loading source bodies and run on the blocking worker pool. A path index
is built once for resource matching rather than scanning every document for each
URL. These are implementation properties, not measured performance wins.

Freshness describes the stored corpus versus its published index, not current
filesystem bytes. Merely changing a disk file does not change the stored view;
ingestion makes its target stale, and source sync/indexing publishes the new
identity. Hashes cover stored decoded/extracted text, not raw binary bytes.
Unregistered assets, binary byte fidelity/decoding, live deployment/browser URL
resolution and runtime resource use are unqualified. Metadata enumeration,
source hashing/decoding and graph validation are outside the dead-files literal
scan budget; total query IO/RSS/time and representative scale remain unqualified.

Extraction format 36 introduces this inventory; store schema 25 is unchanged.
Frozen resource relations use captured target identity. Diff exposes
`resource_target_relations_available`. If either side lacks target inventory,
non-code target relations are excluded from comparisons instead of fabricated
additions/removals; parsed-target resource relations can still be compared using
the independently available file-resource collection.

Qualification: [resource-targets-2026-10-25](qualification/resource-targets-2026-10-25/README.md).
