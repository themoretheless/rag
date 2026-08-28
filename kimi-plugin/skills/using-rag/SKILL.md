---
name: using-rag
description: Work with the rag-mcp knowledge base (mcp__rag__* tools) — query cascade, wiki compile loop, ingest rules. Use when the user asks to remember something, to look something up in the knowledge base/wiki, or when project memory would help.
---

# Using the rag-mcp knowledge base

The `mcp__rag__*` tools talk to a local rag-mcp server (DuckDB store). Treat it as
durable project memory and a wiki.

## Query cascade (always top-down)

1. `query_with_index` — wiki catalog match (slug/title/summary).
2. `search_wiki` — search compiled wiki pages only.
3. `search` with `mode: "lex"` — raw chunk retrieval (escape hatch). Embeddings
   are `mock` unless Ollama is configured, so `vec` is not semantic; prefer `lex`.
4. `get_neighbors` / `get_backlinks` — graph structure once you have a node.
5. `get_document` / `get_source` — full text for 1-2 documents max.
6. `pack_context` — compress hits under a token budget before long synthesis.

Do not dump `list_documents`/`get_graph` over the whole corpus. Cite document ids
and wiki slugs in answers that rely on the store.

## Writing

- **Raw is immutable.** `ingest_raw`/`add_drawer` store verbatim; replacing means
  re-ingesting (same `uri` + changed content is refused for raw — use a new uri or
  non-raw layer). `add_drawer` requires `wing` and `room`.
- **Wiki is compiled by you.** After research or architecture decisions, write a
  page: `write_wiki_page` (upsert by `slug`), link related pages with
  `[[wikilinks]]` (they become graph edges), then `rebuild_index` if the catalog
  looks stale.
- **CAS on updates.** For an existing page: `get_wiki_page`, then update with
  `if_match_revision` set to the fetched `revision`. On conflict: re-fetch, merge,
  retry. Never blind-overwrite.
- `file_answer` persists a cited Q&A as a wiki page; `append_log` records ops.

## Placement conventions

- `wing` = project or area (e.g. `ruos`), `room` = sub-area (`research`, `meta`,
  `src`, `docs`). Keep names stable so `wing`/`room` filters on `search` and
  `list_documents` stay useful.

## Environment

Configured by the plugin wrapper (`bin/run.sh`): binary from `$RAG_BIN` /
`~/.cargo/bin/rag-mcp` / repo release build; DB at `$RAG_DB_PATH` or
`~/.local/share/rag-mcp/rag.duckdb`. One writer per DB file — do not run a second
rag-mcp (or the UI in live `--db` mode) against the same file concurrently.
