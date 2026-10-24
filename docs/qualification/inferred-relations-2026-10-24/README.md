# Frozen inferred database relations: local qualification, 2026-10-24

This extends the native database block with separately labelled naming hypotheses.
It does not establish full RAGRAF parity or whole-product superiority. The
[operation contract](../../EXTERNAL_DATABASES.md) defines the supported rules and
remaining limits; the [acceptance matrix](../../RAGRAF_FEATURE_MATRIX.md) keeps the
full transfer/improvement goal open.

## Current-state verification

- `cargo test --lib`: **756 passed, 0 failed, 1 ignored**. The ignored native DB
  fixture test was separately provisioned and executed successfully.
- `scripts/smoke_external_db.py`: actual PostgreSQL 16/MySQL 8.4 fixtures and real
  stdio/HTTP MCP checks passed. Only uniquely owned disposable containers and
  stores were used and removed. No production database or existing server was
  reconfigured/restarted.
- Each fixture captures four naming hypotheses: `user_id`, `UserID`, `AuthorUserID`
  to `users.id`, plus `company_id` to `companies.id`. Target native single-column
  key and compatible integer types are recorded. PostgreSQL CamelCase fields are
  quoted in SQL; this does not pretend unquoted metadata retains lost case.
- `declared_user_id` remains only in the declared FK array. A compound
  `accounts(id,tenant)` key does not become individually unique `accounts.id`;
  `account_id` receives a key-evidence diagnostic. A TEXT `report_user_id` is not
  promoted to an integer-key relation; its type mismatch is retained.
- Actual MCP confirms separate declared/inferred arrays, uncalibrated scores
  no greater than 0.4, `relation_kind` filtering, independent array truncation,
  table-detail exposure, stable IDs on unchanged refresh, and historical capture
  reads. Read/Write/Admin HTTP permissions, optional samples, actual structural
  DDL drift and failed-refresh retention remain covered by the same smoke run.
- Unit cases verify `_id`, CamelCase/acronym boundaries, compact lowercase names,
  company/companies pluralization, plain-word `paid` even with a matching `pas`
  table, exact quoted `ID` targets, case and role-prefix ambiguity, no cross-schema
  fallback, key/type/metadata uncertainty and declared-FK suppression.
- A 1,050-hypothesis input is capped at 1,000 with an explicit analysis flag;
  230 diagnostics retain 200 records with the full reason count and truncation.
  These prove the bounds, not representative speed/quality at production scale.
- Format 3 persists analysis version 1. Reopening the local database returns the
  original captured hypotheses instead of rerunning current rules. Legacy format
  2 reports unavailable inference until refresh. Structural formats 2/3 compare
  without fake DDL changes, and altering only the stored hypothesis set does not
  alter `db_drift`. Unknown/new-format incomplete payloads remain rejected.

Machine-readable evidence: [runtime.json](runtime.json),
[verification.json](verification.json), [source-sha256.json](source-sha256.json).
Native assertions live in `src/external_db/mod.rs`, rule/history/budget cases in
`src/external_db/relations.rs`, and protocol assertions in the smoke script.

## Controlled reference comparison

[reference-cases.json](reference-cases.json) executes unchanged RAGRAF
Column/Table classes, naming constants and `relation_base`/`infer_relations`
function AST from commit `075caf86dc2f6274b9c48487e521915bcdfeaf2c` with controlled
metadata. It is not a full reference-server execution.

Both implementations recognize the ordinary `user_id` example. In this pinned
reference code the candidates for `company_id` omit `companies`; the local
pluralization is exercised on both native servers and through MCP. The reference
also emits single-target guesses for compound/nonindividual keys, a case-colliding
table set and the plain word `paid` matched to `pas`. The local operation exposes
uncertainty/ambiguity or withholds that word match instead of fabricating key
proof or choosing the first table. These are narrow, reproducible rule/contract
improvements, not a representative precision/recall or performance benchmark.

## Open acceptance work

The current inferred set is deliberately evidence-qualified, not every possible
logical join. Self-links, cross-schema/search-path inference, irregular/multilingual
morphology, full custom/domain/enum and cast compatibility, compound-key inference,
broader relationship recall, calibration and labelled representative corpora remain
open. Guesses do not prove runtime usage, actual row values, FK enforcement,
collation compatibility or referential integrity. Historical formats 1/2 do not
receive silently recomputed analysis; a new explicit capture is required.
