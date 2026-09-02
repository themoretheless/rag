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
| `http://127.0.0.1:7432/live` | process liveness, PID, uptime; no DuckDB query |
| `http://127.0.0.1:7432/ready` | readiness gate; HTTP 503 until startup completes and FTS/store respond |
| `http://127.0.0.1:7432/v1/project-home` | scoped project inventory |
| `http://127.0.0.1:7432/v1/documents` | lean filtered/paginated library catalog |
| `http://127.0.0.1:7432/v1/search` | lex/vector/hybrid product search |
| `http://127.0.0.1:7432/v1/graph` | rag-mcp-ui graph (`?max_nodes=&include_tags=`) |
| `http://127.0.0.1:7432/v1/neighbors` | neighborhood (`?seed=&depth=&max_nodes=`) |
| `http://127.0.0.1:7432/v1/find` | resolve node (`?q=`) |
| `http://127.0.0.1:7432/v1/document` | document body (`?id=` / `?uri=` / `?q=`; returns `revision`/`etag`) |
| `http://127.0.0.1:7432/v1/wiki` | lean cursor-paginated wiki catalog; `PUT` uses CAS |
| `http://127.0.0.1:7432/v1/backlinks` | wiki backlinks (`?id=<document_id>` only) |
| `http://127.0.0.1:7432/v1/activity`, `/v1/jobs*` | sanitized activity plus background sync lifecycle |
| `http://127.0.0.1:7432/v1/revisions*` | lean timeline, lazy snapshot/diff and CAS restore |
| `http://127.0.0.1:7432/v1/operations/*` | checkpoint and allowlisted backup |

The gateway is the live product port and the sole-writer operations boundary.
The exact current method/path list is available at `GET /v1/routes`; detailed
request contracts are in [`CONNECT.md`](CONNECT.md).

Клиенты **не** стартуют второй binary на тот же `.duckdb`.

Во время source sync gateway держит process-local write side coordination lane.
Новые `lex`/`hybrid` запросы не ждут долгую переиндексацию и не вызывают
embedding provider: они сразу получают HTTP `503`, `Retry-After: 1`,
`code=STORE_BUSY` (MCP: `data.code=STORE_BUSY`, `retryable=true`,
`retry_after_ms=1000`). Уже начавшийся search удерживает read side до конца, а
sync стартует после его завершения. Повторяй запрос после terminal sync state;
чистый `vec` этим lexical lane не блокируется.

Project/document/source-scoped vector search materializes only matching chunks
through SQL and deliberately bypasses the database-wide vector snapshot cache.
Unscoped exact search keeps the generation-aware global cache. URI lookup for
documents and graph nodes is indexed by the schema migration; URI ownership is
still enforced by Store CAS rather than a new unique index.

### 3a. Bind: loopback vs remote

| Env | Meaning |
|-----|---------|
| `RAG_HTTP_BIND` | e.g. `127.0.0.1:7432` or `[::1]:7432`. Empty / unset → no HTTP. |
| `RAG_HTTP_ONLY` | `true` → gateway only (no stdio). Requires `RAG_HTTP_BIND`. Process blocks on HTTP until exit. Without it, HTTP is spawned in the background and stdio MCP still runs. |
| `RAG_MCP_HTTP` | default on when bind is set; `false` disables `/mcp` (graph/wiki HTTP stays). |
| `RAG_HTTP_ALLOW_REMOTE` | required for non-loopback binds (`1`/`true`/`yes`/`on`). MCP/HTTP are **unauthenticated**. |
| `RAG_HTTP_ALLOWED_HOSTS` | comma-separated extra Host names/IPs accepted by mounted `/mcp`; required for remote authorities when binding `0.0.0.0` or `[::]` |

`parse_bind` rejects non-loopback addresses unless `RAG_HTTP_ALLOW_REMOTE` is
set. The mounted MCP transport separately allows loopback authorities and a
concrete bind IP; wildcard binds do not infer remote hosts. For a LAN rollout,
set the exact authorities, for example
`RAG_HTTP_ALLOWED_HOSTS=192.168.50.205,tmtl-macbook-pro-m4.local`. Prefer
`127.0.0.1` on a single machine; do not expose `0.0.0.0` without a reverse
proxy and auth you trust.

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

With `--http`, the app opens on **Home** and exposes Home, Library, Search,
Wiki, Connections and Operations. Exclusive `--db` and snapshot modes remain
limited graph/wiki inspection paths. Details: [`EGUI_USAGE.md`](EGUI_USAGE.md).

Native UI не показывает raw transport diagnostics: timeout/connect/other
failures превращаются в стабильные сообщения без method, URL и query details;
HTTP errors показываются компактно как status, action context, stable code и
server-provided human-readable message вместо raw JSON envelope. Пока выполняется wiki save, revision restore или Operations mutation
(sync/cancel/checkpoint/backup), project selector заблокирован и pending result
не отбрасывается. Wiki editor нельзя Cancel/Reload/Back/follow-link во время
Save, а History restore нельзя отменить локально. Для verified backup у клиента
отдельный timeout **30 минут**.

**Через агента / MCP Inspector** (writes + CAS):

```
read_index
list_wiki_pages
get_wiki_page slug=...
# update: get_wiki_page → revision/etag → update_wiki_page with if_match_revision
```

HTTP `GET /v1/document` returns `revision` and `etag`; `PUT /v1/wiki` accepts
`if_match_revision` / `if_match_etag` and returns `409` on stale CAS. Apply the
same read-then-CAS rule to MCP wiki writes. Omitting CAS retains last-write-wins
compatibility behavior.

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
| 7 | Бэкап: `POST /v1/operations/backup` с allowlisted path и `dry_run=false`; проверить `.sha256` и `.metadata.json` sidecars. Не копировать live DuckDB мимо gateway. |

Portable recovery bundle публикуется иначе, чем raw copy: MCP `export_bundle`
и `recovery export-bundle` полностью stage+sync файл рядом с destination, затем
делают no-clobber publish по умолчанию или atomic replace только при явном
`overwrite=true`; временный файл очищается и parent directory синхронизируется.
Offline CLI откажется писать bundle поверх самой DB или её hard-link. При
`import_bundle conflict_policy=overwrite` совпадение id и URI с двумя разными
existing documents является conflict и откатывает всю import transaction.

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
