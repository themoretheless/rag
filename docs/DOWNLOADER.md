# Project: downloader (flowget) in rag-mcp

## Placement

| Field | Value |
|-------|--------|
| wing | `projects` |
| room | `downloader` |
| uri scheme | `project://projects/downloader/<relpath>` |
| default DB | `data/downloader.duckdb` (or shared `rag.duckdb`) |

## Ingest (one shot)

From the rag repo:

```bash
mkdir -p data
export RAG_DB_PATH="$PWD/data/downloader.duckdb"
export RAG_EMBEDDING_PROVIDER=mock
export RAG_EMBEDDING_DIMS=64
export RAG_INGEST_ROOTS="/Users/themoretheless/Documents/Sources/downloader"

cargo run --release --bin ingest_project -- \
  --root /Users/themoretheless/Documents/Sources/downloader \
  --wing projects \
  --room downloader
```

Dry-run file list:

```bash
cargo run --release --bin ingest_project -- \
  --root /Users/themoretheless/Documents/Sources/downloader \
  --wing projects --room downloader --dry-run
```

Skips: `target/`, `.git/`, `.grok/`, large files (>512 KiB by default).  
Extensions: `rs,md,toml,txt,json,yml,yaml,js,ts,html,css,sh,rhai`.

With Ollama embeddings (optional):

```bash
export RAG_EMBEDDING_PROVIDER=ollama
export RAG_EMBEDDING_BASE_URL=http://127.0.0.1:11434/v1
export RAG_EMBEDDING_API_KEY=ollama
export RAG_EMBEDDING_MODEL=nomic-embed-text
export RAG_EMBEDDING_DIMS=768
# re-run ingest_project (same uri → rewrite chunks)
```

## MCP client

See [`examples/downloader.mcp.json`](../examples/downloader.mcp.json).

Search scoped to this project:

```text
search query="queue lock" wing=projects room=downloader mode=hybrid
list_documents wing=projects room=downloader
list_sources wing=projects room=downloader
```

## Re-ingest after code changes

Same command is idempotent by uri (`project://projects/downloader/...`):
content change rewrites chunks; same content is a cheap re-register for
immutable raw paths depending on store policy.
