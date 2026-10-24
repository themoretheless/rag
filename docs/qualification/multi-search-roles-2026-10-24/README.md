# Multi-search static usage roles — 2026-10-24

Extended search-family members with fresh static usage evidence. Entrypoint candidates, unambiguous incoming calls/value references and ambiguous reference candidates have distinct roles and scores. Members and retained families rank by that evidence before deterministic tie-breakers. No observed usage remains an observation, not an unused-file claim.

Role evidence includes source path, line, resolution and, for incoming references, revision/hash/document ID. Targets must match the searched stored text's hash and revision. The code snapshot excludes stale source bindings. Stale/unindexed/parse coverage and index revision are returned in `family_role_analysis`. Evidence is capped at 100 records per member with observed totals and explicit truncation.

Verification includes:

- Full library run after the main change: 811 passed, 0 failed, 1 ignored.
- Final targeted corpus run after assertion/role-label refinements: 5 passed, 0 failed.
- Corpus regression covering bound callback references, family ordering, stale callers and stale targets, evidence caps and omitted-copy behavior.
- Actual stdio MCP regression in `runtime.json`: callback usage, deterministic responses, stale binding omission and refresh, plus existing query-window/family/scope checks.
- Binary and source hashes and final library results in `verification.json`.

The script uses an owned temporary database and mock embeddings; production state is untouched. Role ranking covers retained per-query windows only. Full runtime use, markup relations, source encoding and representative performance qualification remain incomplete. See `docs/MULTI_SEARCH.md` and the transfer matrix; this stage does not complete the full goal.
