# Source continuation and byte windows — 2026-10-26

[Contract](../../CODE_SOURCE_PAGINATION.md). This closes the previously documented
unreachable-tail limitation of a bounded single-line symbol read. RAGRAF pinned
at 075caf86dc2f6274b9c48487e521915bcdfeaf2c accepts comma/whitespace `entry_ids`;
local code now accepts that form alongside typed arrays. Cursor continuation is
an independently qualified improvement, not an assertion that the reference has
this same token protocol or that all code/source-mode contracts are equivalent.

Eight new native cases check complete reconstruction of a >120000-character Unicode
single-line function while changing page size, cross-page chunk overlap,
UTF-8 byte bounds/empty windows, changed query/stale/republished view conflicts,
malformed/out-of-span tokens, array/delimited ID equivalence and bounded parsing,
and complete unique-byte traversal through the 1000-segment cap of >1000 methods.
Existing stored-code-read regressions remain gates.

Real MCP checks use actual source-sync chunks, giant single-line source pages,
array/CSV ID forms, byte-window proof, project-scoped foreign-ingest consistency,
record/text-limited nested containers, exact coverage with no repeated bytes,
protocol error codes and stale/republished view conflicts. No source program is
executed. Temporary databases use mock embeddings; production DBs are not opened.

All MCP scenarios use one fixed copied executable. Native checks/build use the
dirty shared checkout, concurrently edited by another user-owned Rust-analysis
chat. Observed post-check source hashes are not an atomic frozen checkout or
deployment assertion. JSON/logs and verification.json record final evidence.

These cursors validate a current view; they do not retain old source revisions or
hold a transaction across pages. Metadata, graph/source loading and hashing are
outside the source-text cap; broader quality/scale, source modes, styles and full
RAGRAF acceptance remain open.

An additional corruption fixture reproduced a panic: a cached child definition
started inside a UTF-8 scalar, and the parent partition indexed an unchecked
string boundary. corrupt-before.log retains the red test. Derived source
partitions now use checked slicing and return an explicit index-span error; the
regression also verifies that the store remains usable after that error and
that reversed child intervals are rejected even under a one-record limit. This is
a native corruption guard, not automatic repair of arbitrary malformed caches.

The native rechunk fixture also reproduced missing invalidation when chunk rows
were deleted/reinserted with new bounds while document/index revisions stayed
unchanged. rechunk-before.log retains the failed CONFLICT assertion. The view
is now sealed with resolved span identities/ranges and unresolved IDs before
cursor validation. Changed/deleted selected chunks now return CONFLICT rather
than resuming a changed selection. This case is qualified at the native store
layer; MCP separately exercises actual overlapping source-sync chunk IDs.

Final gates after both fixes: **1,528 workspace tests passed, 0 failed,
1 ignored**, including all **8 new native regressions**. Workspace/all-target
Clippy with `-D warnings`, binary build, owned-module formatting and
`git diff --check` passed. Both red repro logs remain alongside the final green
focused/workspace logs; the final evidence does not hide those earlier defects.

All **8 real MCP runners passed** against the same fixed executable: source
cursors, existing code reads, raw documentation, PHP parameter contracts, client
endpoints, SQL table impact, route queries and non-code references. The giant
line reconstructed over 3 pages; nested definitions over 16 small pages. Actual
source-sync chunk character intervals [0,800) and [680,1480) partially overlap;
their source text was reconstructed over 2 record-limited pages without repeated
bytes. No-op sync and foreign-project ingest without publication preserved the
scoped token; changed filters, stale capture and publication returned conflicts.
Runtime extraction format 70 (including concurrent Rust work), store schema 25;
this query-only source extension requires no extraction/schema migration.
