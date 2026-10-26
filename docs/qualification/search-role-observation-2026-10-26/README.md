# Search roles and coherent observation — 2026-10-26

See the [contract](../../SEARCH_ROLE_EVIDENCE.md). RAGRAF's pinned search_multi
and duplicate-role evidence contracts motivate captured usage-based family
ranking. This stage extends search_multi with file imports/resources and makes
both search and duplicate observations use one scoped read transaction.

Five new native cases cover imports/HTML/CSS/SVG usage from sources outside the
target prefix, stale/unindexed source and target exclusion, project-scoped index
decoding, zero-hit queries bypassing corrupt index payloads, actual cloned-
connection publication during a read view, and 514 selected documents across
the 512-ID parameter-batch boundary with an adversarial ID. Eleven prior corpus
cases remain gates. The concurrency case uses real parser/cache/history helpers
and a cloned connection to one DuckDB instance; it is not a multi-process writer
qualification.

The actual MCP corpus checks independent all/any windows, retained-family roles,
referrers outside the target prefix and search matches, HTML/CSS and SVG identity,
missing-export module imports, bound function values, provenance, deterministic
output, metadata/excerpt separation, zero-hit behavior, stale exclusion before
sync, sync replacement and complete counts for a 100-of-105 evidence window.
Existing duplicate/import/resource/identity/inline-style/SQL-impact MCP cases are
regression gates. Sources/databases are temporary and embeddings are mocked.
No source application/SQL/browser resource is executed or production DB touched.

The shared checkout is concurrently edited by another user-owned chat. Logs and
an isolated executable identify the checked build. After-check source hashes are
observations, not a frozen checkout build attestation. Final results and hashes
are in verification.json. Full reference parity, representative semantic/ranking
quality, performance/scale and runtime activity remain unqualified.

Final gates: **1,409 workspace tests passed, 0 failed,
1 ignored**. All sixteen corpus cases passed, including five new
search/observation/batch regressions. The focused log precedes the final batch
case; the workspace log includes it. Clippy with warnings denied, owned-file
formatting and whitespace checks passed. The isolated format-49
executable passed all seven actual MCP scenarios; store schema remains 25. The
shared checkout's extraction version advanced in the other active work; this
query-role stage did not require an extraction-format change.
