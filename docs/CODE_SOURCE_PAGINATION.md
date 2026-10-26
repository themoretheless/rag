# View-checked source pagination

`code` now returns `next_cursor` when a record, character or segment page limit
stops traversal. Repeat the same selectors/windows with that `cursor`; only
`limit` and `max_chars` may change. `read_complete` means no selected work remains
after that page, and `is_continuation`/`span_offset` distinguish continuation
responses. Single-symbol pages keep the legacy root `symbol`, `code` and
`source_ranges` fields. File/ID pages retain segmented `items`.

```json
{"symbol":"target","project":"alpha","max_chars":20000}
{"symbol":"target","project":"alpha","max_chars":100000,"cursor":"<next_cursor>"}
```

Cursor format `cr1` binds normalized selectors, line/byte windows, container mode
and source mode to the project-scoped captured view: global index revision,
extractor version, document identities/fingerprints/freshness and selected-view
symbol identities/byte bounds/build contexts. Changing a filter, ingesting a
changed in-scope source, or publishing another revision returns `CONFLICT` with
`CODE_VIEW_CHANGED`; restart without a cursor. A foreign-project ingest without
index publication does not invalidate a project-scoped view. Global publication
may conservatively invalidate it even if the selected source did not change.

These are optimistic current-view tokens, not retained historical snapshots,
authentication, encryption or permission grants. Format/length/numeric bounds,
selected-span position and UTF-8 boundaries are validated; a caller cannot use a
cursor to read beyond the original selector/window. Current scope and freshness
checks still apply. Per-request selection/source reads share one transaction;
separate pages do not hold a database transaction or lock open.

Pagination uses canonical source interval partitions. Previously traversed
logical spans reserve their full selected intervals when a later page is planned;
partially overlapping chunk IDs cannot reintroduce bytes returned on an earlier
page. Parent header/footer partitions similarly remain separate from child bodies.
A large single-line function can be fully reconstructed across character-limited
pages. Page order is deterministic span/partition order, not necessarily increasing
file byte order when parent/footer metadata precedes nested child definitions.

`start_byte` (inclusive) and `end_byte` (exclusive) optionally intersect the line
window and selected spans. Explicit byte bounds must be ordered UTF-8 boundaries
within each returned document's stored decoded source. A split multibyte scalar
or out-of-source boundary is rejected, and equal bounds produce an explicit empty
window. These are decoded UTF-8 source offsets, not original legacy-encoding bytes.

`entry_ids` accepts either an array or RAGRAF-style comma/Unicode-whitespace text.
IDs are case-sensitive, at most 512 bytes each and at most 100 per request. The
string parser retains at most 101 tokens before the common count rejection;
array transport allocation remains governed by the existing protocol limits.
Duplicate/order-only changes in IDs do not change the logical query/cursor.
Only exact fresh symbol IDs or chunk offsets/text that map to source resolve.

Budgets remain per response: record cap, returned source characters and 1000
segments. Metadata/index/source loading and hashing remain outside the source
text budget. A partial page is never advertised as a complete body. There is no
whole-response/RSS/latency guarantee or verified representative scale claim.
Historical reads, source-mode/style CLI contracts and transformed chunk mappings
remain open. See [base contract](CODE_READS.md) and
[qualification](qualification/code-cursors-2026-10-26/README.md).
