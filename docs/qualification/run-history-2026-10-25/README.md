# Committed native run history — 2026-10-25

[Contract](../../CODE_CATALOG.md#committed-index-run-history). This extends the
catalog/diagnostic acceptance work with queryable native run history. RAGRAF's
pinned 075caf86dc2f6274b9c48487e521915bcdfeaf2c catalog describes indexed projects,
revisions and checksums; this is a local auditability improvement, not a claim
that its exact registry or per-project run model was reproduced.

Four native regressions cover bounded pages across appended no-op/new-revision
runs and revision filters; filter/revision/anchor guards; invalid/oversized
metrics without returned bodies; and empty/read-only/persisted history. Existing
catalog and timing regressions retain publication/latest distinctions, rollback
and migration checks. The actual MCP scenario checks a pinned first page across
later publications, changing page limits, separate no-op/publication rows, exact
revision filters, read-only queries, rejected project scope and cursor view guards.
Owned temporary databases and mock providers isolate these checks.

Only committed successful native runs are recorded. Failure/commit/per-project
attribution, external administrative mutation, representative scale and full
RAGRAF parity remain open. History adds no schema/extractor migration; final qualification uses the current
schema and extractor versions recorded in verification.json.

Final gates: **1,254 workspace tests passed, 0 failed, 1 ignored**. Nine catalog
and four timing cases passed (four new history regressions). Clippy with
`-D warnings`, formatting and `git diff --check` passed. The binary passed the new
history, existing catalog and index-timing stdio MCP scenarios. Actual versions:
database schema 25, extractor format 30.

Earlier workspace attempts overlapped Cargo-boundary source changes and are
retained as superseded diagnostics. The final complete test ran against a stable
304-file native source/config inventory; the runtime binary hash stayed unchanged.
Two Rust line-wrap fixes were applied by cargo fmt afterward, with whitespace-only
comparison recorded. Test-time and current source hashes, binary hash and logs are
retained here. These fixtures do not establish representative history scale or
full RAGRAF parity.
