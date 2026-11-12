# Git PHP context qualification — 2026-10-26

[Contract and remaining boundaries](../../GIT_TRAIT_CONTEXTS.md).

- Focused Git suite: 11 passed. Five added regressions cover distinct witnesses through a shared body, owner-target swaps with unchanged unions/body, padding/ID stability, incompatible/value-only branches and nested inherited frames.
- Workspace: 1585 passed, 0 failed, 1 ignored.
- Workspace/all-target Clippy with `-D warnings` and debug binary build passed.
- Eight real stdio MCP scripts pass on the same copied executable: Git PHP contexts, baseline Git impact, captured trait impact paths, trait-body contexts, SQL table impact, literal impact, non-code references and PHP namespaces.
- Git PHP MCP: 11 checks pass, including staged/working paths, per-step context/source provenance, unchanged index/source, determinism, pinned-commit independence, context-only binding changes, byte/ID movement exclusion, incompatible callers, limits and no persistent code index requirement.

Runtime index format 79; Git analysis contract 2. This query change does
not introduce a persistent extraction schema migration. Fixtures use temporary Git
repositories/DuckDB stores, mock embeddings and disabled LLM calls. The analyzer
never executes PHP/SQL, runs clean filters, checks out commits or writes source/
Git-index state; fixture setup intentionally creates its own local commits.
Production repositories and databases are not used by qualification scripts.

`binary.json` verifies the copied executable before/after MCP runs. Selected source
hashes are observational in `verification.json`: another task shares the checkout.
This is not a frozen-checkout or deployed-gateway claim. Captured lexical frames
are qualified; PHP runtime subtypes/dynamic dispatch/compiler semantics, test
recommendation context sensitivity and representative scale remain open. A retained
path to a seed is not runtime execution/absence proof or full RAGRAF parity.
