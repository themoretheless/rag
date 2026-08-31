# Production run (MCP + graph UI)

Binary paths (after release build):

| Binary | Path |
|--------|------|
| MCP server | `target/release/rag-mcp` |
| Graph UI | `target/release/rag-mcp-ui` |
| DB | `./rag.duckdb` (or absolute path) |

---

## 1. One-time build

```bash
cd /Users/themoretheless/Documents/Sources/rag
cargo build --release -p rag-mcp
cargo build --release -p rag-mcp-ui
```

---

## 2. MCP for agents (Claude Desktop / Code / Zed)

**Prod-ish env example** (semantic search via Ollama; spine tools):

```bash
export RAG_DB_PATH=/Users/themoretheless/Documents/Sources/rag/rag.duckdb
export RAG_EMBEDDING_PROVIDER=ollama
export RAG_EMBEDDING_BASE_URL=http://127.0.0.1:11434
export RAG_EMBEDDING_MODEL=nomic-embed-text
export RAG_EMBEDDING_DIMS=768
export RAG_EMBEDDING_API_KEY=ollama
export RAG_INGEST_ROOTS=/Users/themoretheless/Documents/Sources/rag,/Users/themoretheless/Documents
export RAG_TOOLS=spine
export RAG_LLM_ENABLED=false
export RUST_LOG=info
```

Клиент запускает **только** binary (stdio), не `cargo run`:

```text
command = /Users/themoretheless/Documents/Sources/rag/target/release/rag-mcp
```

Те же env: в `claude_desktop_config.json` / `.mcp.json` / Zed `context_servers`.

Ручной smoke:

```bash
# не для продакшена UI: только проверка, что process стартует
./target/release/rag-mcp
# Ctrl+C: MCP ждёт JSON-RPC на stdin
```

**Предпочтительный prod:** один HTTP gateway (§3) + клиенты через `mcp-remote` / URL, а не отдельный stdio process на тот же `.duckdb` (см. `docs/CONNECT.md`).

---

## 3. Один сервер для всех (HTTP gateway)

**Один process** открывает DuckDB. На `RAG_HTTP_BIND`:

| URL | Кто / роль |
|-----|------------|
| `http://127.0.0.1:7432/mcp` | Claude / ChatGPT / streamable-HTTP MCP client |
| `http://127.0.0.1:7432/health` | health + backend/schema/FTS, counts, WAL/integrity, `mcp_http` |
| `http://127.0.0.1:7432/v1/graph` | rag-mcp-ui graph (`?max_nodes=&include_tags=`) |
| `http://127.0.0.1:7432/v1/neighbors` | neighborhood (`?seed=&depth=&max_nodes=`) |
| `http://127.0.0.1:7432/v1/find` | resolve node (`?q=`) |
| `http://127.0.0.1:7432/v1/document` | document body (`?id=` / `?uri=` / `?q=`; returns `revision`/`etag`) |
| `http://127.0.0.1:7432/v1/wiki` | wiki catalog (full list; no `q`/`limit`/`offset`) |
| `http://127.0.0.1:7432/v1/backlinks` | wiki backlinks (`?id=<document_id>` only) |

HTTP is mostly read for the graph UI; wiki also supports **`PUT /v1/wiki`** (slug + content + optional CAS). Ingest and most mutations remain MCP tools.

Клиенты **не** стартуют второй binary на тот же `.duckdb`.

### 3a. Bind: loopback vs remote

| Env | Meaning |
|-----|---------|
| `RAG_HTTP_BIND` | e.g. `127.0.0.1:7432` or `[::1]:7432`. Empty / unset → no HTTP. |
| `RAG_HTTP_ONLY` | `true` → gateway only (no stdio). Requires `RAG_HTTP_BIND`. Process blocks on HTTP until exit. Without it, HTTP is spawned in the background and stdio MCP still runs. |
| `RAG_MCP_HTTP` | default on when bind is set; `false` disables `/mcp` (graph/wiki HTTP stays). |
| `RAG_HTTP_ALLOW_REMOTE` | required for non-loopback binds (`1`/`true`/`yes`/`on`). MCP/HTTP are **unauthenticated**. |

`parse_bind` rejects non-loopback addresses unless `RAG_HTTP_ALLOW_REMOTE` is set. Prefer `127.0.0.1` on a single machine; do not expose `0.0.0.0` without a reverse proxy and auth you trust.

### 3b. Рекомендуется (shared server)

**Терминал 1: сервер (без stdio):**

```bash
cd /Users/themoretheless/Documents/Sources/rag

export RAG_DB_PATH=./rag.duckdb
export RAG_HTTP_BIND=127.0.0.1:7432
export RAG_HTTP_ONLY=true
export RAG_TOOLS=spine
export RAG_EMBEDDING_PROVIDER=mock
export RAG_INGEST_ROOTS=./,/Users/themoretheless/Documents
export RAG_LLM_ENABLED=false

./target/release/rag-mcp
```

MCP endpoint: **`http://127.0.0.1:7432/mcp`**  
(`RAG_MCP_HTTP=false` отключает MCP path, graph/wiki UI API остаётся.)

**Терминал 2: UI (Wiki mode by default with `--http`):**

```bash
cd /Users/themoretheless/Documents/Sources/rag

./target/release/rag-mcp-ui \
  --http http://127.0.0.1:7432 \
  --seed "YOUR_TITLE_OR_NODE_ID"
```

Проверка:

```bash
curl -s http://127.0.0.1:7432/health
# expect "mcp_http": true, "mcp_path": "/mcp"
curl -s 'http://127.0.0.1:7432/v1/graph?max_nodes=50' | head
curl -s http://127.0.0.1:7432/v1/wiki | head
curl -s 'http://127.0.0.1:7432/v1/backlinks?id=DOCUMENT_ID' | head
```

### 3c. Snapshot (без HTTP)

Tool `export_graph_snapshot` → `rag-mcp-ui --snapshot ./graph.json --seed ...`  
Snapshot mode has graph only (no live wiki catalog / backlinks).

### 3d. Только UI (exclusive live DB, MCP выключен)

```bash
./target/release/rag-mcp-ui --db ./rag.duckdb --seed "SEED"
```

Seed: title wiki/note, `node_id` или `document_id` (из `list_documents` / `list_wiki_pages` / `GET /v1/wiki`).  
Do not use `--db` while another process holds the same DuckDB (single writer).

---

## 4. Wiki смотреть

**В egui (рекомендуется с gateway):**

```bash
rag-mcp-ui --http http://127.0.0.1:7432
# or exclusive: rag-mcp-ui --db ./rag.duckdb
```

With `--http` or live `--db`, the app opens in **Wiki** mode by default (catalog + article + backlinks; toolbar Wiki | Graph). Details: `docs/EGUI_USAGE.md`.

**Через агента / MCP Inspector** (writes + CAS):

```
read_index
list_wiki_pages
get_wiki_page slug=...
# update: get_wiki_page → revision/etag → update_wiki_page with if_match_revision
```

HTTP `GET /v1/document` returns `revision` and `etag`, but there is no write+If-Match HTTP API. Concurrent wiki edits: always pass `if_match_revision` / `if_match_etag` on MCP writes (optional today = last-write-wins if omitted).

---

## 5. Full tools (kg, diary, maintain)

```bash
export RAG_TOOLS=full
```

в env MCP-клиента + restart клиента (or gateway process if HTTP-only).

---

## 6. Типовой prod-день

| Шаг | Действие |
|-----|----------|
| 1 | `ollama serve` + model embeddings (если не mock) |
| 2 | Gateway: `RAG_HTTP_ONLY=true RAG_HTTP_BIND=127.0.0.1:7432` + `target/release/rag-mcp` |
| 3 | Clients: Claude / Code / Zed → `http://127.0.0.1:7432/mcp` (mcp-remote or connector; see `docs/CONNECT.md`) |
| 4 | UI: `rag-mcp-ui --http http://127.0.0.1:7432` (Wiki + Graph) |
| 5 | Работа: ingest, wiki, search (MCP); browse via UI or `GET /v1/*` |
| 6 | Offline graph only if needed: export → `rag-mcp-ui --snapshot` |
| 7 | Бэкап: `cp rag.duckdb rag.duckdb.bak` (stop writers first if possible) |

---

## 7. Одной строкой (copy-paste)

Gateway + UI (shared DB, preferred):

```bash
cd /Users/themoretheless/Documents/Sources/rag
cargo build --release -p rag-mcp -p rag-mcp-ui
# terminal 1:
RAG_DB_PATH=./rag.duckdb RAG_HTTP_BIND=127.0.0.1:7432 RAG_HTTP_ONLY=true \
  RAG_TOOLS=spine RAG_EMBEDDING_PROVIDER=mock RAG_LLM_ENABLED=false \
  ./target/release/rag-mcp
# terminal 2:
./target/release/rag-mcp-ui --http http://127.0.0.1:7432 --seed "SEED"
```

Snapshot-only (no live DB):

```bash
./target/release/rag-mcp-ui export --db ./rag.duckdb -o ./graph.json
./target/release/rag-mcp-ui --snapshot ./graph.json --seed "SEED"
```

Замени `SEED` на реальный title/id из базы.
