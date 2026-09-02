# rag-mcp project rules

## Knowledge store (DuckDB)

- One writer: `rag-mcp` with `RAG_HTTP_BIND` / MCP. Do not open `rag.duckdb` twice (not from UI, CLI, or a second MCP process).
- **Re-ingest:** after editing files on disk, call `ingest_file` with the same absolute path (upsert by `file://` URI). Path must sit under `RAG_INGEST_ROOTS`.
- **Wing / room (optional on `ingest_file`):** pass `wing` and `room` for project shelf placement (MemPalace-style). Example for this repo: `wing=rag`, `room=src` or `room=docs`. Prefer stable names so `search` / `list_documents` filters stay useful. `ingest_text` has no wing/room params; use `ingest_file`, `ingest_raw`, or `add_drawer` when placement matters. `add_drawer` requires both wing and room.
- **Wiki is compiled, not live:** `ingest_file` does **not** update wiki pages or wiki index. After meaningful doc/runbook/architecture changes: `get_wiki_page` then `update_wiki_page` / `write_wiki_page` (with CAS, below). Optional later: `rebuild_wiki_index` if the catalog is stale.

## Multi-LLM concurrency (CAS / etag)

Before updating any **existing** wiki page:

1. `get_wiki_page` → read `revision` and `etag` (`W/"n"`).
2. Write with `if_match_revision` (preferred) or `if_match_etag` set to that value.
3. On **conflict**: re-fetch, merge, retry. Never blind overwrite when concurrent agents may write.

Notes:

- `if_match` is optional by default (omit = last-write-wins). Agents **must** still send it for wiki updates when concurrent writers may run.
- **`RAG_WIKI_REQUIRE_IF_MATCH`** (default **false**): when `true`, wiki page **updates** reject missing `if_match_revision` / `if_match_etag` (creates may still omit). Enable for multi-agent / multi-LLM setups.
- If both `if_match_revision` and `if_match_etag` are set, **revision wins**.
- MCP maps store `Conflict` to `invalid_params` (not a distinct conflict code). Treat that as CAS failure: re-get and retry. Missing if_match under require mode is a config/invalid-params style error, not conflict.
- HTTP exposes wiki GET plus CAS-protected `PUT /v1/wiki` with
  `if_match_revision` / `if_match_etag`; stale writes return `409`. Agents should
  still prefer the MCP wiki tools and always use read-then-CAS when concurrent
  writers may run.
- Internal ingest upserts often call CAS with `if_match=None`; that path is last-write-wins for raw docs. Wiki agent writes are the concurrency-sensitive path.

## Hooks queue and flush (Claude Code)

Project hooks in `.claude/settings.json` (do not open DuckDB from hooks):

1. **PostToolUse** (`Edit|Write`): `.claude/hooks/queue-rag-ingest.sh` appends absolute paths to `.rag/pending-ingest.txt` (deduped). Extensions: `.md`, `.rs`, `.toml`, `.txt`, `.json`. Skips `target/`, `.git/`, `node_modules/`.
2. **Stop**: `.claude/hooks/remind-rag-flush.sh` injects a reminder when the queue is non-empty.

Agent workflow when the queue has paths (Stop reminder or you know you edited files):

1. Read `.rag/pending-ingest.txt` (one absolute path per line).
2. For each path: `ingest_file` with that path (and `wing`/`room` when placing on a shelf).
3. Clear the queue file after successful ingests (truncate or delete). **No auto-flush** after `ingest_file`; clearing is agent-only.
4. If wiki hub / runbook / architecture content changed: `get_wiki_page` then `update_wiki_page` / `write_wiki_page` with `if_match_revision`.

`.rag/` is local runtime state (pending queue). Keep it out of commits if it appears; hooks create it on demand.

## Index-first query

`query_with_index` → `search_wiki` → `search` → `get_neighbors` / `get_document` → `pack_context`.

Use `wing` / `room` on `search` and `list_documents` when the question is project-scoped.

## UI

- Wiki browser: `rag-mcp-ui --http http://127.0.0.1:7432` (mode Wiki; requires gateway).
- Graph: tab Graph, seed = full page title or document id.
- UI wiki Edit/Save: `PUT /v1/wiki` (HTTP) or exclusive `--db`; agents should still prefer MCP `write_wiki_page` / `update_wiki_page` with CAS.
