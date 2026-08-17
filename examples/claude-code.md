# Claude Code + rag-mcp

## One-shot (user scope)

```bash
claude mcp add rag-mcp \
  -s user \
  -e RAG_DB_PATH=/Users/themoretheless/Documents/Sources/rag/rag.duckdb \
  -e RAG_EMBEDDING_PROVIDER=mock \
  -e RAG_INGEST_ROOTS=/Users/themoretheless/Documents/Sources/rag,/Users/themoretheless/Documents \
  -e RAG_DEFAULT_SEARCH_MODE=vec \
  -e RAG_LLM_ENABLED=false \
  -e RUST_LOG=info \
  -- /Users/themoretheless/Documents/Sources/rag/target/release/rag-mcp
```

## Project scope

Repo already has `.mcp.json`. In Claude Code:

```bash
cd /Users/themoretheless/Documents/Sources/rag
claude
```

If the server shows as **Pending approval**, approve it when prompted (or use `/mcp`).

## Import from Claude Desktop

```bash
claude mcp add-from-claude-desktop
```

## Check

```bash
claude mcp list
claude mcp get rag-mcp
```

## Better semantic search (optional)

```bash
claude mcp remove rag-mcp -s user
claude mcp add rag-mcp -s user \
  -e RAG_DB_PATH=/Users/themoretheless/Documents/Sources/rag/rag.duckdb \
  -e RAG_EMBEDDING_PROVIDER=ollama \
  -e RAG_EMBEDDING_BASE_URL=http://127.0.0.1:11434 \
  -e RAG_EMBEDDING_MODEL=nomic-embed-text \
  -e RAG_EMBEDDING_DIMS=768 \
  -e RAG_EMBEDDING_API_KEY=ollama \
  -e RAG_INGEST_ROOTS=/Users/themoretheless/Documents/Sources/rag,/Users/themoretheless/Documents \
  -e RAG_LLM_ENABLED=false \
  -- /Users/themoretheless/Documents/Sources/rag/target/release/rag-mcp
```
