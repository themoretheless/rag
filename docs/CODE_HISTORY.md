# Exact code search, duplicate evidence and immutable index history

These tools are available on `RAG_TOOLS=full`. They extend the code index without
changing generic `multi_query_search` or document history.

## Search and copies

`search_multi` scans complete stored document text, independently of chunk
boundaries. It accepts 1..32 literal queries (default) or bounded Rust regular
expressions, each at most 4096 bytes. `match_mode=all` requires every query in a
document; `any` returns their union. `per_query_document_counts` covers the whole
selected scope. Hits contain source line/column, byte offsets and bounded snippets.
At most three occurrences per query/document are returned, with an explicit flag
when there are more. Regexes that match empty text or exceed compilation limits
are rejected. Project filters use stored `wing`; path prefixes use path-component
boundaries rather than raw string prefixes. No filesystem files are read.

`duplicates` combines exact stored-text hashes with numbered/copy/backup filename
families confirmed by token-shingle similarity. It never equates normalized PDF
text with original PDF bytes. Similarity groups are connected components; the
returned pairwise evidence explains each link, rather than claiming every pair
in a group clears the threshold. Near-copy work has a comparison budget and
explicit sampling/cap flags. Group/member/evidence outputs are bounded.

The primary file is a candidate ranked by observed entrypoint/call evidence, then
stable name/path preferences. Incoming-call evidence can come from outside a
requested copy-directory scope, but its document hash/revision must agree with
the captured corpus. No observed references is not proof of dead code. The report
retains `primary_is_proven=false` and `deletion_safe=false`. Filesystem liveness,
dynamic loads and all markup/string path references are not inferred yet.

```json
{"name":"search_multi","arguments":{"queries":["Allowed formats","upload"],"match_mode":"all","project":"portal"}}
{"name":"search_multi","arguments":{"queries":["Files[0-9]+\\.php"],"regex":true,"match_mode":"any"}}
{"name":"duplicates","arguments":{"project":"portal","min_similarity":0.8,"max_comparisons":10000,"max_members":100}}
```

## Revisions and semantic diff

Schema v20 includes immutable revision headers/members, content-addressed UTF-8
source blobs and a drift ledger. `index_code` and source-sync finalization publish
the current syntax cache, frozen call/inheritance/entrypoint graph and source
manifest in the same transaction. Late publication failure rolls them all back.
A warm sync reuses its latest revision. Returning to an earlier corpus still
creates a new chronological revision. All active stored documents are in the
manifest, including text-only inputs; only supported source languages have AST
payloads. Revision IDs and document revisions are distinct.

`index_revisions` lists revision headers with a stable `before_revision` cursor.
`diff` compares frozen revisions, including file metadata/content hashes, added,
removed or changed definitions (body hashes included), and static candidate call
links. Definition identity uses document/qualified-name/kind, so shifting byte
positions alone does not invent a replacement definition. Overload groups are
compared together. Response truncation is explicit and source bodies are omitted.
`relations_added` / `relations_removed` compare frozen non-call candidate references,
inheritance links and symbol-backed entrypoints. Their semantic identities omit
byte offsets and line numbers, so moving a definition does not create a false
relation removal/addition. Repeated links collapse into sets; overloads sharing
the existing document/qualified-name/kind key are not compiler-distinct identities.
`relations_available` is false if either historical graph lacks required arrays;
missing facts are never reconstructed from current files.

From index format 18, graph publication also freezes the derived SQL snapshot.
`schema_diff` reports table additions/removals and column type, nullability,
default-presence and generated-flag changes, plus completeness and source evidence.
From format 19, canonical explicit default/generated expressions and generation
clauses are also compared. `expression_changes_available` requires both
revisions to capture these fields; comparisons involving older snapshots omit
these fields instead of reporting capture-format changes as schema changes.
Expression comparison is syntactic SQL AST text, not algebraic equivalence or
evaluation. From format 20, table and column constraint metadata also participates in the
diff. `constraint_changes_available` requires both snapshots to support capture;
legacy-missing fields do not become invented changes. Table constraints include
added/removed sets and bounded before/after declarations. Changes in column
constraints appear in the affected column record. Constraint text is not
evaluated, dependency rewrites are diagnosed and live schema state is unverified. Diagnostics are project-filtered. Table, column, evidence and
diagnostic windows obey the requested limit with truncation indicators. Older
revisions without SQL snapshots return `available: false` rather than replaying
old migrations with today's algorithm. Existing historical records stay unchanged.

Missing/corrupted historical blobs fail with an error, not a false deletion diff.

```json
{"name":"index_revisions","arguments":{"limit":20}}
{"name":"diff","arguments":{"before":1,"after":2,"project":"portal","limit":100}}
{"name":"catalog","arguments":{"index_revision":1,"project":"portal","limit":100}}
{"name":"index_status","arguments":{}}
```

Current graph queries attach `index_rev` and `view=fresh_subset_of_revision_inputs`:
stale source facts are excluded and the remaining graph is evaluated against that
fresh subset. Historical diff uses frozen graph facts instead. `catalog` returns
the immutable manifest, not an assertion that files on disk are unchanged.
`index_status` reports static capabilities and missing/stale/parse-error coverage;
it does not claim compiler or runtime dispatch verification.

History is retained; there is no automatic pruning. Source blobs are deduplicated,
but per-revision metadata/analysis still consume space. Whole-database verified
backups retain these tables. Portable document/chunk bundles do not preserve this
derived history; rebuilding after import starts a new history in the target DB.

## Live reads, freshness and acknowledgements

`freshness` and `live_read` compare a chosen immutable revision against actual
files decoded with the ingestion decoder by default. Files must already be in that revision manifest and must
pass the existing `RAG_INGEST_ROOTS` policy, including symlink targets. File reads
are capped at 4 MiB of original bytes; unreadable, binary, oversized, changing or disallowed files
never receive a positive freshness result. Missing files are checked against a
resolved allowed ancestor before they are classified as missing. Checksums compare decoded UTF-8 text; `observed_source` reports separate byte/hash/encoding evidence. An encoding-only change may preserve text freshness while changing its byte fingerprint.

Set `source=stored` explicitly to compare current stored documents instead. That
mode does not claim to have checked disk. `live_read` requires one indexed source,
returns a bounded line/character range and common-prefix/suffix changed ranges,
and never overwrites source/index content. Range comparison is not a minimal edit
script. Display is limited to 64000 Unicode characters; bulk freshness results
are capped at 2000 selected files and report truncation/unavailability.

```json
{"name":"freshness","arguments":{"project":"portal","source":"filesystem"}}
{"name":"live_read","arguments":{"document_id":"source-id","start_line":10,"end_line":40,"max_chars":16000}}
{"name":"drift_report","arguments":{"project":"portal","limit":100}}
{"name":"drift_ack","arguments":{"event_ids":["returned-event-id"],"note":"Reviewed against the intended change"}}
```

Changed/missing observations are stored idempotently by baseline/source/hash.
Source sync also records added/removed/changed stored inputs in its publication
transaction. These are observations, not an exhaustive filesystem monitor. The
ledger has stable `before_seq` pagination and preserves first/last seen times.
`drift_ack` requires write access, validates every ID before committing, preserves
the first acknowledgement, and changes neither files nor index facts. Unknown
IDs roll back the whole acknowledgement. An acknowledged observation stays
acknowledged when re-observed; a new observed hash produces a separate event.
Unacknowledged historical observations remain until explicitly reviewed, even
when a newer revision has already been published.

## Original HTML sources

Local HTML ingestion keeps the readable document body for ordinary retrieval and
stores original markup separately as a verified content-addressed source blob.
Document metadata contains only `code_source_hash` and `code_source_kind`, not the
raw markup. Code history, source freshness and whole-source corpus queries use
the original bytes. Changes confined to styles or scripts therefore still refresh
the source revision even when the readable text is unchanged. Source sync repairs
missing or corrupt original-source blobs from the allowlisted file; a missing
blob otherwise causes an explicit resync error. Portable document-only exports
must also preserve these source attachments to support original-source analysis.

Translation-key candidates now have a separately frozen capability marker and
per-document capture coverage. Key relation diffs preserve repeat counts, ignore
position/equivalent-literal-spelling changes, and exclude legacy or incomplete
captures from negative comparisons. See
[translation-key history](TRANSLATION_KEY_REFERENCES.md#immutable-history).
