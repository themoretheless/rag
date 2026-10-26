# Stored source reads

MCP `code` and native `Store::code_read(CodeReadQuery)` now support exact `symbol`
(ID/name/FQN), exact `path`, `document_id` and up to 100 `entry_ids` (symbol IDs or
current chunk IDs, as an array or comma/whitespace text). `project` filters the snapshot before decoding source/index
facts. Named symbol ambiguity returns bounded candidate metadata without bodies;
file/document mode requires a unique active captured document identity. Competing
path identities remain ambiguous even when one has stale/unindexed facts.

Existing single-symbol clients retain `symbol` and `code` at the response root.
Batch/file/ID mode uses `items`, each with source identity, optional symbol
metadata and `segments` (code, exact UTF-8 byte interval and original line range).
The returned body payload is not duplicated at both the root and batch items. `start`/`end` are positive, one-based inclusive
source lines intersected with each selected byte span. Endpoints preserve original
CRLF/newline spelling; symbol spans themselves still stop at their AST boundary.
An outside window is explicit and does not silently return a different function.

By default, strictly nested spans are omitted in favor of their outermost selected
container. Equal byte spans are deduplicated. `include_containers=true` retains
nested metadata while partitioning a parent's source around all selected child
spans. Header/footer pieces remain separate; no synthetic joined code is produced
for batch items. Partially overlapping chunk/ID spans also omit already returned
bytes. All output segments are non-overlapping within a source document.

`limit` caps selected spans (default 50, clamp 1..500); full original/retained/
dropped counts and `spans_truncated` are independent of text budgets. A shared
`max_chars` budget (default 20000, accepted 1..100000 Unicode scalar values) caps
returned source text across all spans. A separate 1000-segment cap bounds text
fragment metadata. `text_truncated`, `segments_truncated`, `returned_chars` and
`overlap_bytes_omitted` expose omissions. Text budget excludes symbol/documentation
metadata, snapshot/source loading, hashing and graph resolution; it is not a full
JSON response, RSS or latency guarantee. No compiler or source execution occurs.

Chunk character offsets are converted to UTF-8 boundaries, then the exact current
source slice must equal the stored chunk text. A transformed, mismatched, missing,
foreign-project or stale-source chunk is listed in `unresolved_entry_ids`, never
located by guessing substring occurrences. Chunk text is loaded only after source
scope/freshness qualification. Duplicated IDs and source spans do not duplicate
returned bodies. Search-generated chunks whose transformed offsets/text cannot
map exactly remain unsupported and explicit.

Snapshot selection, freshness/identity checks, chunk resolution and source reads
share one DuckDB read transaction. Stored source blobs are hash/revision/path/
project checked against the chosen fresh index identity. Edits on disk without
ingest do not change this captured view. Ingest invalidates stale facts; source
sync refreshes them. `source=stored/index/auto` all select captured text; other
modes are rejected. Use the separate allowlisted `live_read` API for live disk
reads. No filesystem fallback occurs.

This is a query-only extension; extraction format is unchanged. General style
formatting, unresolved transformed-chunk mapping, retained historical code reads,
full reference CLI/source-mode contracts and representative scale remain open.
See [qualification](qualification/code-reads-2026-10-26/README.md).

File mode selects indexed definition spans, rather than promising every file
byte: imports, headers and gaps outside definitions may be absent. A file with
no indexed definitions uses a full-source fallback. Default containment filtering
runs linearly after deterministic byte-span sorting; this is an implementation
property, not a measured representative performance win.

[View-checked continuation cursors](CODE_SOURCE_PAGINATION.md) now support full
traversal after character, record or segment limits, including huge single-line
spans. Optional UTF-8 byte windows and RAGRAF-style delimited IDs are supported.
These current-view cursors do not retain historical snapshots.
