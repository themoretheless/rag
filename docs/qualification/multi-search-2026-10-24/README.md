# Multi-search result contract — 2026-10-24

Compared the pinned RAGRAF facade (`075caf86dc2f6274b9c48487e521915bcdfeaf2c`, `op_search_multi`) with the local implementation. Added independent per-query result windows, including empty queries' result lists, document counts, bounded occurrences and truncation. Combined all/any results remain backward compatible. Added project-isolated filename-family groups with explicit coverage/truncation, unverified roles and no duplicate-content assertion. Original source encoding remains unknown rather than being mislabeled as UTF-8.

The initial MCP probe exposed an assumption about random document-ID ordering. Corpus results now sort by stored path and then ID; the regression fixture uses IDs in reverse path order. Path filtering also precedes full source reconstruction.

Verification:

- Full library suite after the main change: 810 passed, 0 failed, 1 ignored.
- Targeted corpus tests rerun after final ordering: four tests.
- Binary build and actual stdio MCP regression: see `runtime.json` and `verification.json`.

The smoke script `scripts/smoke_search_multi.py` uses a temporary corpus/database and mock embeddings. It tests independent query limits, missing terms, conjunction/union, repeated occurrences, filename families, deterministic responses and path scope.

This does not close full RAGRAF parity: evidence-backed activity ranking, original source encodings, richer reference grouping and representative performance gates remain. The current contract is documented in `docs/MULTI_SEARCH.md`.
