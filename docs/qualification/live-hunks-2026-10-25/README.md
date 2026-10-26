# Live-read edit-range qualification — 2026-10-25

[Contract](../../LIVE_READ_DIFF.md). The previous local implementation exposed
one prefix/suffix changed range. The pinned RAGRAF op_live_read at
075caf86dc2f6274b9c48487e521915bcdfeaf2c exposes a requested text window and
changed_since_index checksums; its inspected source does not emit separate edit
ranges. This extension adds separate bounded range evidence without exposing
additional source bodies. This is a source-contract comparison, not a measured
correctness/performance contest against the reference runtime.

Four new native regressions cover separated replacement/insertion coordinates;
961 exhaustive pairs of short repeated-line inputs with reconstruction and
stable ordering; Unicode, empty sides, final LF and CRLF; and explicit cell-limit
fallback plus independent 64-hunk truncation. The focused drift suite also retains
four existing filesystem/decoder/path/ledger tests.

The actual stdio MCP scenario uses mock embeddings and temporary source/database
files. It checks separated edits without out-of-window text, stable drift event
identity, final LF/CRLF/restoration, large-region fallback, body budget, no index
publication by a live read, source-sync refresh and comparison against the frozen
old revision. No production database or source application is touched/executed.

Database schema 25 and extractor format 28 remain unchanged. Broader platform,
encoding, character/semantic diff and scale/memory qualification remain open.
Full RAGRAF parity remains open. Final gates and source hashes: verification.json;
runtime evidence: runtime.json.

Final verification: **1,223 workspace tests passed, 0 failed, 1 ignored**;
all eight focused drift tests passed (four new). Workspace Clippy with
`-D warnings`, formatting and `git diff --check` passed. Rebuilt binary passed
the new live-hunk MCP scenario and existing encoding/freshness MCP regression.
Logs and SHA-256 hashes are retained alongside this report. These fixtures do
not establish representative performance, platform or full reference parity.
