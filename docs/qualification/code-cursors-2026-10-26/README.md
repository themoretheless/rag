# Source continuation and byte windows — 2026-10-26

[Contract](../../CODE_SOURCE_PAGINATION.md). This closes the previously documented
unreachable-tail limitation of a bounded single-line symbol read. RAGRAF pinned
at 075caf86dc2f6274b9c48487e521915bcdfeaf2c accepts comma/whitespace `entry_ids`;
local code now accepts that form alongside typed arrays. Cursor continuation is
an independently qualified improvement, not an assertion that the reference has
this same token protocol or that all code/source-mode contracts are equivalent.

Six new native cases check complete reconstruction of a >120000-character Unicode
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
