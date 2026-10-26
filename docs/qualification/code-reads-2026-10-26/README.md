# Stored code reads — 2026-10-26

[Contract](../../CODE_READS.md). Pinned reference:
075caf86dc2f6274b9c48487e521915bcdfeaf2c, `Facade.op_code` and `_drop_nested`.
The local extension adds exact file/document and symbol/chunk-ID selection,
line intersections, non-overlapping source segments and independent record/
character/segment budgets while preserving the single-symbol root response.
Selection and source/chunk identity checks share one scoped read transaction.

Six new native regressions exercise UTF-8 and CRLF line boundaries, outside
windows, container/child partitions, Unicode-safe shared budgets, independent
span caps, exact chunk offset/text mapping, partial chunk overlap, invalid and
foreign/stale sources, project/path ambiguity and its deterministic replay,
empty-symbol files and a 1000-segment cap across more than 1000 methods.

The real MCP fixture tests `code` from both symbol and actual ingested chunk IDs,
then path/nested/window/budget modes, project exclusion, no live disk fallback,
ingest-induced stale exclusion and sync replacement. The pinned pure
`_drop_nested` function is extracted with Python AST, without importing or
starting RAGRAF. Its selected multiline definition set is compared with the
local default. Byte-range equality/non-overlap and budget improvements have
independent golden expectations, not inferred equivalence or scale claims.

All MCP scenarios use temporary source files and databases, mock embeddings and
one fixed copied executable. Source programs are not executed; no production
database is opened. Builds/checks use the dirty shared checkout with concurrent
Rust work from another user-owned chat. Source hashes captured after checks are
observational, not an atomic frozen checkout or deployment proof.

Remaining boundaries: text budgets exclude metadata and snapshot/source loading;
unknown/transformed chunk mappings stay unresolved; source-mode/style CLI
contracts, broader language/overload cases, historical reads and representative
memory/latency/reference acceptance remain open. See runtime.json, individual
runner JSON/logs, binary.json and verification.json for final gates/hashes.

Final gates: **1,510 workspace tests passed, 0 failed, 1 ignored**, including
all **6 new code-read regressions**. Workspace/all-target Clippy with
`-D warnings`, binary build, owned-module formatting and `git diff --check` passed.
All **8 real MCP runners passed** against one fixed copied executable: code reads,
raw documentation, PHP parameter contracts, C#, Ruby, Perl, PowerShell and client
endpoint references. The controlled nested-span selection matches the pinned
reference utility; exact source-range, budget and sync expectations also passed.
Runtime extraction format 66 (including concurrent Rust work), store schema 25.
This code-read extension itself requires no extraction/schema change.
