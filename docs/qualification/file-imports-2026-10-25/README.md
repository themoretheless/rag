# File import candidates — 2026-10-25

Extends the incomplete file dependency/import portion of the RAGRAF matrix.
See [contract](../../FILE_IMPORT_RELATIONS.md). Python and JS/TS AST imports
resolve to captured same-project file candidates independently of symbol lookup.
Side-effect imports, re-exports and missing exports retain their file dependency.
No calls or routine liveness are fabricated. Static candidates are not proven
loader behavior or runtime impact; full RAGRAF parity remains open.

Four new native regressions cover symbol-free/missing-export imports, package
initializers/relative submodules, ambiguity/unsupported loaders/project isolation,
file-impact/dead-file integration, stale exclusion, refresh and frozen/legacy
diffs. The real MCP fixture uses a temporary corpus/database and mock embeddings.
Existing SQL table impact, file impact and file dead-code scenarios are rerun.
No production database or source application/SQL is executed.

Final results, binary/source hashes and logs are recorded in verification.json.
The repository is being edited by another user-owned chat: these logs describe
the compiled working-tree snapshot and isolated executable, not a frozen final
checkout or whole-product parity/performance qualification.

The initial workspace attempt exposed missing trace_path access/telemetry registration in concurrent work. Both were added and the workspace rerun. The initial MCP attempt timed out before initialize at 60 seconds; that log is retained separately. Only the new runner increases cold initialization to 180 seconds; ordinary requests retain 60 seconds.

Final gates: **1,298 workspace tests passed, 0 failed, 1 ignored**. All four new native cases and all four real MCP scenarios passed. Workspace/all-target Clippy with `-D warnings`, owned-file formatting and `git diff --check` passed. Actual runtime: index format 34, store schema 25. Global checkout formatting/stability is not claimed for the concurrent user's ongoing edits.
