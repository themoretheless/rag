# Literal path role qualification — 2026-10-26

See the [contract](../../LITERAL_PATH_ROLE_EVIDENCE.md). RAGRAF's pinned
075caf86dc2f6274b9c48487e521915bcdfeaf2c literal_refs helper and facade examples
were inspected, including an endpoint constant with no code graph edge.

Five new native regressions cover a JS endpoint constant selecting a PHP copy,
source evidence outside the target prefix, exact Unicode/byte anchors, project-
wide basename ambiguity outside the selected folder, qualified disambiguation,
substring/unrelated-directory/URL boundaries, disabled mode, byte budgets,
stale-source exclusion, source refresh and complete 25-source counts with 20
retained facts. Sixteen prior corpus cases remain gates, including typed import
counts unaffected by supplemental text matching and coherent read transactions.

The real MCP scenario checks actual PHP/JS ingestion, search and duplicate roles,
weaker unbound text evidence, UTF-8 addresses, project/prefix isolation, disabled/
budget/stale states, sync replacement, ambiguous short names, qualified paths and
a 20-of-26 retention window. Existing search/duplicate/import/resource/identity/
inline-style/SQL-impact MCP scenarios are regression gates.

All databases/source roots are temporary and embeddings are mocked. No ingested
code, URL, source SQL or application is executed; no production DB is touched.
Logs and an isolated executable identify the checked build. The shared checkout
is concurrently edited by another user-owned chat; after-check hashes are
observations, not a frozen whole-checkout attestation. Final gate results and
hashes are in verification.json. Complete parity, calibrated ranking quality,
representative scale and runtime activity remain unqualified.

Final gates: **1,415 workspace tests passed, 0 failed,
1 ignored**. All twenty-one corpus cases passed, including five
new literal-path regressions. Clippy with warnings denied, owned-file formatting
and whitespace checks passed. The isolated format-51 executable passed all
eight actual MCP scenarios; store schema remains 25. The extraction version
advanced in the other active work; this query-time feature did not require an
extraction-format change.
