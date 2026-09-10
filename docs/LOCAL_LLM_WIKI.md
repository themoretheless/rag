# Local LLM wiki + DB (Ollama + Karpathy layer)

Цель: **локальная LLM (Ollama)** упорядочивает knowledge base: immutable raw → compiled wiki pages → `wiki_index` catalog → `ops_log`, всё в одном DuckDB файле.

## Архитектура

```
raw sources (layer=raw, immutable content)
        │
        ▼  compile_source (Ollama chat)
wiki pages (layer=wiki, [[links]] #tags)
        │
        ├── wiki_index  (index.md analogue)
        ├── ops_log     (log.md analogue)
        ├── schema://agents (conventions)
        └── chunks + embeddings (search_wiki / hybrid)
```

LLM **не** переписывает raw. Она только предлагает / пишет wiki-страницы и каталог.

## Ollama setup

```bash
# install: https://ollama.com
ollama serve
ollama pull llama3.2
ollama pull nomic-embed-text   # optional local embeddings
```

## Env (recommended local stack)

Apply these settings to the single `local.rag-mcp` gateway. They are not a
recipe for a second wiki-specific database or process.

```bash
export RAG_DB_PATH=/Users/themoretheless/.local/share/rag-mcp/rag.duckdb
export RAG_INGEST_ROOTS="$HOME/Notes:$HOME/Sources"

# embeddings via Ollama OpenAI-compat (dims must match model)
export RAG_EMBEDDING_PROVIDER=ollama   # or openai with base below
export RAG_EMBEDDING_BASE_URL=http://127.0.0.1:11434/v1
export RAG_EMBEDDING_API_KEY=ollama
export RAG_EMBEDDING_MODEL=nomic-embed-text
export RAG_EMBEDDING_DIMS=768

# chat for compile_source
export RAG_LLM_ENABLED=true
export RAG_LLM_BASE_URL=http://127.0.0.1:11434/v1
export RAG_LLM_MODEL=llama3.2
export RAG_LLM_API_KEY=ollama
export RAG_LLM_TIMEOUT_SECS=600
export RAG_LLM_MAX_TOKENS=2048

export RAG_DEFAULT_SEARCH_MODE=hybrid
export RUST_LOG=info
```

Offline (no LLM / mock vectors):

```bash
export RAG_EMBEDDING_PROVIDER=mock
export RAG_LLM_ENABLED=false
```

## MCP tools (wiki layer)

| Tool | Role |
|------|------|
| `ingest_raw` | Immutable source → chunks + embed + graph |
| `list_sources` / `get_source` | Browse raw |
| `get_schema` / `update_schema` | `schema://agents` conventions |
| `write_wiki_page` / `update_wiki_page` / `get_wiki_page` / `list_wiki_pages` | Manual wiki CRUD |
| `compile_source` | **Local LLM** raw → multi-page wiki (`dry_run` supported) |
| `read_index` / `update_index_entry` / `rebuild_index` | Catalog (index.md) |
| `append_log` / `read_log` / `list_recent_ops` | Timeline |
| `search_wiki` / `query_with_index` | Index-first + wiki search |
| `file_answer` | Persist cited answer into wiki |
| `lint_wiki` | Structured index + link health: broken/duplicate/self links, unresolved stubs, orphan pages/documents, aggregate counts |

## Typical agent loop

1. `ingest_raw` (or `ingest_file` under allowlist) for notes/docs  
2. `get_schema` (seed default if missing)  
3. `compile_source` with `dry_run=true` → review  
4. `compile_source` apply → wiki pages + index + log  
5. `lint_wiki` → fix stubs / rebuild_index  
6. `query_with_index` / `search_wiki` for answers  
7. `file_answer` when a good answer should compound  

## Policy

### Concurrent edits and source freshness

`compile_source` and applying `consolidate` capture the wiki catalog's revisions
before calling the LLM. Generated updates use those revisions, including when
`RAG_WIKI_REQUIRE_IF_MATCH=true`; generated new pages require an absent URI in the
write transaction. An edit or new page created while the LLM is running produces
a conflict instead of being overwritten. Each generated page commits separately;
a conflict on a later page does not undo earlier successful pages in the compile.

Generated pages also persist typed `source_versions` metadata alongside their
body, chunks, catalog and journal in one transaction:

```json
{"source_versions":[{"document_id":"source-id","uri":"raw://source","content_hash":"blake3-of-full-source-body"}]}
```

The hash identifies the complete source snapshot read from the database, before
the prompt truncates long input. It does not imply the LLM received every byte.
Freshness checks compare the current source body to that snapshot. A source edit
during generation therefore makes the resulting page stale even when the page's
timestamp is newer. Timestamp or metadata changes with identical source content
do not make a versioned parent stale. A valid complete `source_versions` array
defines the entire parent set: old graph links, citations and source metadata
outside it do not affect freshness after recompiling from fewer sources. An
empty array explicitly records no parents. Metadata travels with wiki replication.

`find_stale_wiki` / `refresh_stale_wiki` report changed source content with
`link_kind=source_version`. An unavailable parent remains visible with
`link_kind=source_missing`, its recorded ID/URI and `raw_updated_at: null`;
automatic refresh skips it and asks for source restoration or relinking in its
notes. Lookup uses document ID, then URI when IDs differ between replicas.
Pages without a valid complete snapshot keep the legacy timestamp comparison
using graph links, source metadata arrays, citations and body markers. If any
snapshot entry is malformed, the whole page uses that fallback. Existing pages
gain hashes when compiled or consolidated again; no backfill invents source
versions for older output.

Diagnostic compatibility: `raw_updated_at` remains an RFC3339 string for present
sources and is now nullable for unavailable sources. `analyze_corpus.stale_wiki`
also includes `link_kind`, preserving the reason instead of treating every
finding as a timestamp comparison. Maintenance plans list restoration/relinking
as a no-op requiring attention when a source is unavailable.

### Content and embedding rules

- **raw** content is not updated in place (re-ingest same uri replaces)  
- **wiki** is agent/LLM mutable  
- Graph extract stays deterministic (`[[wikilink]]`, `#tag`)  
- `reembed_document` only refreshes one document when the stored and configured
  corpus identities already match. Changing embedding provider, model,
  dimensions, or base endpoint requires a complete, uncapped, successful
  `reembed_all` before the target manifest is published and vector/hybrid search
  resumes. A persistent incompatible marker is written before the first vector
  change, keeping partial failure and configuration rollback fail-closed.

## Build

```bash
cd /path/to/rag
cargo build --release
```

Deploy/restart the one gateway as described in [`PROD_RUN.md`](PROD_RUN.md),
then connect over `http://127.0.0.1:7432/mcp`. Do not launch this artifact as a
second stdio writer while `local.rag-mcp` owns the canonical DB.
