# Multi-query stored-source search

`search_multi` searches complete stored UTF-8 documents, independently of chunk boundaries. It supports literal or bounded regular expressions, Unicode case-insensitive matching, project isolation and component-aware path prefixes. Empty patterns, empty-match regexes and oversized query sets are rejected.

The existing `items` field obeys `match_mode`: `all` (the backward-compatible default) requires every query in the same document; `any` returns their union. `matching_documents_total` and `truncated` describe this combined window.

`per_query` always returns an independent window for each input query, including queries with no matches. Each contains its original index/text, exact matching-document count, items and its own truncation flag. The document limit applies independently to every query. Each document carries at most three source occurrences, with `more_occurrences` indicating additional matches. Byte offsets are UTF-8 byte positions; columns are Unicode scalar positions. Hashes and revisions identify the stored source evidence.

`union_matching_documents_total` counts documents matching any query regardless of the combined match mode. Duplicate query strings retain separate indices.

`families` groups the retained query windows by project and normalized filename (numbered/copy/backup suffix heuristic). It does not prove duplicate content or file activity. Roles use fresh static entrypoint, bound-call and bound-reference candidates. These distinguish `entrypoint_candidate`, `referenced_candidate`, `ambiguous_reference_candidate` and `no_observed_usage`. Absence of observed usage does not establish an unused file. At most 100 evidence records per member are returned, with exact observed counts and truncation flags. The index revision and stale/unindexed/parse coverage appear in `family_role_analysis`; runtime activity remains unverified. Markup references are not yet included. Family and member windows have independent bounds. `family_coverage` is `retained_per_query_windows`: omitted documents can belong to a displayed family, so retained-member totals are not global family sizes. Documents sort by stored path then ID; family members and families first sort by usage score, then deterministic path/project/name keys. Rankings apply only to retained query windows; they do not recover matches already omitted by per-query limits. Ordering is deterministic for an unchanged corpus.

`encoding: stored_utf8` describes the searched representation. `source_encoding` reports the captured byte decoder choice and `source_encoding_basis` distinguishes BOM, strict UTF-8 and explicitly heuristic Windows-1251/1252 choices. `source_bytes_hash` identifies the ingested bytes. These fields are shown only when versioned metadata matches the searched decoded-text hash; otherwise they are null. They are never inferred from already-decoded stored text. See [source encodings](SOURCE_ENCODINGS.md). Search reads stored documents and does not check live filesystem freshness.

Example:

```json
{"name":"search_multi","arguments":{"queries":["Allowed formats","upload"],"match_mode":"any","project":"app","limit":20}}
```

Verification: `cargo test --lib code_index::corpus::` and `python3 scripts/smoke_search_multi.py target/debug/rag-mcp`. The smoke test runs an isolated temporary MCP server with mock embeddings.
