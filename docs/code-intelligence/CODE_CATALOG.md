# Code catalog contract v1

`catalog` accepts `CatalogQuery`, preserving the old `SourceQuery` fields. Default
`view: files` preserves the immutable manifest operation; `view: projects` adds a
captured project registry. Both execute in one read transaction, without source
filesystem reads, URL requests, indexing, repair or FTS rebuilding.

## Immutable file pages

Optional exact `project`, `document_id`, `path` filters precede the page limit
(default 100, clamped 1..1000). `index_revision` chooses a published revision;
omission starts at the latest revision. Responses retain index_rev, items, total,
truncated and view=immutable_revision_manifest. They add has_more and next_cursor.
Pass next_cursor and the same filters to continue; limit may change. The cursor
pins the original revision even after newer revisions are published. A conflicting
explicit index_revision or changed filters/view is invalid. Ordering is captured
document ID, including distinct documents that share a path.

Only page metadata is fetched: ID/path/project/revision, captured content hash,
UTF-8 source byte length, analysis-payload presence and source-blob availability.
`syntax_indexed` preserves its earlier meaning: captured analysis payload exists;
it does not validate the parser or JSON. `bytes` can be null when the blob is
missing. The source text is not materialised in Rust or returned to clients;
DuckDB may access its source column to measure byte length. This is not a physical
I/O/memory/latency bound. `content_checksums_verified: false` explicitly states
that this lean query does not recompute source hashes or decode analysis JSON.
The immutable manifest identity is frozen; blob availability/length are current
storage observations and do not prove blob integrity. Freshness/code/body tools
retain their independent verification behavior.

## Captured project registry

`view: projects` groups the union of current active corpus documents and the
selected immutable revision by exact project/wing. The unassigned null group is
distinct from a project literally named "null". Optional project filters apply
before metadata/roots are read; source-file/document filters are rejected in this
view. All active stored documents are included, matching the index manifest's
corpus scope; syntax-indexed counts are reported separately. This is not only a
count of physical source files or code files.

Each row reports current/indexed document counts, syntax-payload count, captured
corpus hashes, the selected shared revision/publication time and current sync-root
summaries. Roots come from source_manifest joined to active current documents;
they are not inferred from path prefixes and are not historical root snapshots.
Root manifest counts/timestamps do not prove current files exist or are complete.
Projects can appear before the first published revision. Empty projects without
any persisted documents have no registry row.

Corpus hashes are BLAKE3 over length-delimited JSON (document ID, source path,
declared source hash) tuples in document-ID order. Current preserved-source
metadata is used for markup; other documents use their stored content hash.
Frozen tuples use the revision's captured source hashes. Revision numbers, roots,
timestamps and row order do not participate in this corpus hash. Missing declared
hashes make the current hash unavailable. No bodies/blobs are verified by this
comparison. State names deliberately avoid claiming health:

- unindexed: current documents but none in the selected revision;
- absent_from_current_corpus: indexed project has no current active documents;
- captured_drift: both captured hash sets exist and differ;
- captured_match_unverified: captured hash sets agree; source integrity and live
  filesystem/config freshness remain unverified;
- unavailable_hash: required hash metadata is missing/unusable.

`captured_hashes_match` only compares captured declarations. `hash_matches` (live
reference-compatible meaning) and config_hash/config_matches remain null;
filesystem_verified/source_integrity_verified are false. No per-project config
identity, enable/disable policy, physical storage isolation, independent index
revision or per-project run history is invented. Those reference contracts remain
open; storage/revisions here are shared and logically project-scoped.

Project pages pin the comparison revision and carry an observation hash. Changes
to any matching current project summary, including roots/timestamps, cause
CATALOG_OBSERVATION_CHANGED on continuation. Restart without cursor. This prevents
mixing current registry observations across pages; it is not a retained snapshot
of current documents. Historical project requests compare the chosen old revision
against current stored documents, not against old filesystem state.

Cursors are opaque versioned JSON strings with revision/view/filter identity and
last key; they are continuation data, not authentication. total is the full scoped
match count, has_more means a later page exists, and truncated also stays true on
continuations because earlier records are omitted. Live-read SourceQuery fields
are retained for compatibility but are unused by catalog. No index/schema format
migration is needed. Per-page record limits do not bound all project aggregation
work, root payload size, full response bytes or representative large-index costs.

## Committed index-run history

`catalog(view="runs")` lists the existing successful committed native index-run
ledger, newest sequence first. Default scope is all revisions; index_revision
optionally filters one known published revision. The ledger is corpus_global:
project, path, document, source/range and character filters are rejected. The
query never attributes global measurements to a project. No source body is read.

Each valid row contains sequence, run ID, recording time, index revision,
revision_created, validated phase metrics and metrics_available=true. Publication
and no-op runs are separate rows. Invalid metrics and payloads exceeding 16 KiB
return metrics=null, metrics_available=false and an explicit reason; oversized
JSON is not loaded into Rust. Validation is shared with index_status. Failed or
rolled-back attempts are absent from this committed ledger. Commit, metric-write,
embedding/FTS and queue timings remain unmeasured as previously documented.

Pages default to 50 and cap at 500. The first page pins the maximum sequence and
its run ID. Continuations retain that upper bound and revision filter, so new
runs cannot shift older pages. Limits may change between pages. Replacing or
losing the anchor is a conflict. Each page uses a single read transaction; totals
refer to the pinned matching population. has_more and next_cursor describe the
remaining page; truncated also indicates that preceding rows were paged out.
Normal ledger operations only append. Administrative mutation of older records
is not an immutable-history guarantee. No schema or extraction-format change is
required for this query feature (database schema 25).
