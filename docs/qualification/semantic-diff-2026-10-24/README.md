# Frozen schema and relation diffs — 2026-10-24

Index format 18 freezes a derived SQL snapshot alongside candidate graph facts when publishing an immutable index revision. Existing historical graphs are preserved; absent schema/relation snapshots return explicit availability flags instead of being reconstructed using current files or a changed replay algorithm.

`diff` now reports non-call candidate references, inheritance and symbol-backed entrypoints as semantic sets. Schema changes cover table presence, columns, types, nullability, default-presence/generated flags and completeness, with source evidence. Moving source lines does not create relation/schema changes. Before/after analysis formats and their difference are explicit so algorithm-version changes are not silently presented as source-only changes.

Table/column/evidence/diagnostic and relation windows are bounded with truncation flags. SQL diagnostics respect the project filter. Source bodies are omitted; database writes and live-schema queries are never performed.

Verification:

- Full library suite after the implementation: 818 passed, 0 failed, 1 ignored.
- Six history tests, including new frozen schema/relation/project/no-line-churn and missing-schema cases; final targeted verification reruns after format-envelope changes.
- Actual stdio MCP regression: `scripts/smoke_semantic_diff.py`, report `runtime.json`. It verifies callback/inheritance/test-entrypoint changes, column deltas, nested limits, source-body omission and independence from later unsynchronized physical file changes.
- Both server and gate CLI builds passed. Authenticated HTTP/stdio/actual-hook gate regression passed (`gate-regression.json`).
- Decoder/live/gate MCP regression on index format 18: `live-regression.json`.

The tests use temporary stores/processes and mock embeddings; production state is untouched. Schema diffs are limited to tracked derived metadata, not default/generated expressions, arbitrary constraints or live schema state. Repeated relations collapse into sets; overloads with identical existing semantic keys are grouped. Unresolved/non-symbol-backed relation kinds and regression interpretation remain incomplete. See `docs/CODE_HISTORY.md` and the transfer matrix; the full transfer goal remains open.
