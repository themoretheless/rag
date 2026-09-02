# ChatGPT → rag-mcp (remote MCP)

ChatGPT (Developer mode / Connectors) expects a **remote** streamable-HTTP or SSE MCP URL, not a local stdio binary.

## 1. Start the shared gateway

Normally `local.rag-mcp` is already running. The foreground command below is a
replacement only: stop that service first and run exactly one gateway.

```bash
cd /Users/themoretheless/Documents/Sources/rag

export RAG_DB_PATH=/Users/themoretheless/.local/share/rag-mcp/rag.duckdb
export RAG_HTTP_BIND=127.0.0.1:7432
export RAG_HTTP_ONLY=true
export RAG_TOOLS=spine
export RAG_EMBEDDING_PROVIDER=mock
export RAG_INGEST_ROOTS=./,/Users/themoretheless/Documents
export RAG_LLM_ENABLED=false

./target/release/rag-mcp
```

Endpoint: **`http://127.0.0.1:7432/mcp`**

Local-only bind. For ChatGPT cloud you need a public HTTPS tunnel (e.g. ngrok, cloudflared) pointing at that port, then use the public URL `https://…/mcp`.

## 2. ChatGPT UI

1. Settings → Connectors → Advanced → enable **Developer mode** (if required by your plan).
2. Create connector / import MCP server.
3. URL: `http://127.0.0.1:7432/mcp` (local only works if the client can reach your machine) **or** your HTTPS tunnel URL.
4. Auth: none (local default).

## 3. Same server as Claude + UI

| Client | Config |
|--------|--------|
| Claude Desktop | `mcp-remote` stdio bridge → `http://127.0.0.1:7432/mcp` |
| Claude Code | `"type": "http", "url": "http://127.0.0.1:7432/mcp"` |
| ChatGPT | Connector URL `…/mcp` |
| rag-mcp-ui | `--http http://127.0.0.1:7432` |

One process, one DuckDB. Do not also run stdio `command` MCP against the same file.
