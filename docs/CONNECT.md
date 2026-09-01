# Подключение rag-mcp (по порядку)

Локальный MCP-сервер: **Rust + DuckDB**, tools для RAG / wiki / graph.

Транспорты (один и тот же tool surface / одна DuckDB):

| Режим | Как | Когда |
|-------|-----|--------|
| **HTTP gateway** (рекомендуется) | `RAG_HTTP_BIND` + `/mcp` (streamable HTTP) | Несколько клиентов, UI, Desktop + Code |
| **stdio** | клиент сам стартует binary | Один writer, legacy / offline |

Клиенты: Claude Desktop (через `mcp-remote`), Claude Code (`type: http` или stdio), Zed, ChatGPT connector.  
Один writer на `rag.duckdb`: не открывай DB дважды.

---

## 0. Что нужно заранее

- Rust (cargo)
- Один из клиентов: Claude Desktop / Claude Code / Zed
- (Опционально) Ollama или OpenAI API - для нормального semantic search  
  Без них работает **mock** (smoke, слабый «смысловой» поиск)

Пути в примерах - твоя машина:

| Что | Путь |
|-----|------|
| Репо | `/Users/themoretheless/Documents/Sources/rag` |
| Binary | `.../target/release/rag-mcp` |
| База | `.../rag.duckdb` |

---

## 1. Собрать сервер

```bash
cd /Users/themoretheless/Documents/Sources/rag
cargo build --release
ls target/release/rag-mcp
```

Повтори `cargo build --release` после обновления кода.

---

## 2. Выбрать режим tools

| Env | Смысл |
|-----|--------|
| `RAG_TOOLS=spine` | **по умолчанию** - **31** tools, index-first (рекомендуется) |
| `RAG_TOOLS=full` | все tools (MemPalace kg/diary, maintain, …) |

Список spine: `src/mcp/surface.rs` / `docs/SPINE_TOOLS.md`  
(в т.ч. L2: `get_graph`, `export_graph_snapshot` + neighbors/backlinks/find/link).

Playbook для агента (в `get_info` и `schema://agents`):

```
query_with_index → search_wiki → search → neighbors → get_document → pack_context
```

---

## 3. Базовые env (общие для всех клиентов)

Минимум (smoke, без API):

```bash
RAG_DB_PATH=/Users/themoretheless/Documents/Sources/rag/rag.duckdb
RAG_EMBEDDING_PROVIDER=mock
RAG_INGEST_ROOTS=/Users/themoretheless/Documents/Sources/rag,/Users/themoretheless/Documents
RAG_TOOLS=spine
RAG_LLM_ENABLED=false
RUST_LOG=info
```

Лучше semantic search (Ollama):

```bash
# ollama pull nomic-embed-text
RAG_EMBEDDING_PROVIDER=ollama
RAG_EMBEDDING_BASE_URL=http://127.0.0.1:11434
RAG_EMBEDDING_MODEL=nomic-embed-text
RAG_EMBEDDING_DIMS=768
RAG_EMBEDDING_API_KEY=ollama
```

Один и тот же `RAG_DB_PATH` = общая база у Desktop / Code / Zed.  
Не гоняй несколько **stdio**-процессов одновременно на один файл.

---

## 4. Claude Desktop

### 4a. Рекомендуется: remote MCP → уже запущенный сервер

Сначала подними **один** gateway (см. `docs/PROD_RUN.md` §3):

```bash
RAG_HTTP_ONLY=true RAG_HTTP_BIND=127.0.0.1:7432 \
  RAG_DB_PATH=.../rag.duckdb ... ./target/release/rag-mcp
```

**Файл:** `~/Library/Application Support/Claude/claude_desktop_config.json`

Local MCP в Desktop **не** принимает голый `"url"` (ошибка *not valid MCP server configurations*).  
Нужен stdio-прокси `mcp-remote` → твой streamable HTTP:

```json
{
  "mcpServers": {
    "rag-mcp": {
      "command": "/opt/homebrew/opt/node@22/bin/npx",
      "args": [
        "-y",
        "--registry=https://registry.npmjs.org/",
        "mcp-remote@0.1.38",
        "http://127.0.0.1:7432/mcp"
      ]
    }
  }
}
```

Gateway уже должен слушать `:7432` (`RAG_HTTP_ONLY=true`).  
`npx` path подставь свой (`which npx`). Первый запуск может скачать пакет.

**Не** ставь рядом второй `"command": "…/rag-mcp"` на тот же `rag.duckdb`.

Пример: `examples/claude-desktop.http.mcp.json`.

### 4b. Legacy: local stdio (Claude сам стартует binary)

Только если **не** крутится другой writer на тот же DB:

```json
{
  "mcpServers": {
    "rag-mcp": {
      "command": "/Users/themoretheless/Documents/Sources/rag/target/release/rag-mcp",
      "args": [],
      "env": {
        "RAG_DB_PATH": "/Users/themoretheless/Documents/Sources/rag/rag.duckdb",
        "RAG_HTTP_BIND": "127.0.0.1:7432",
        "RAG_EMBEDDING_PROVIDER": "mock",
        "RAG_INGEST_ROOTS": "/Users/themoretheless/Documents/Sources/rag,/Users/themoretheless/Documents",
        "RAG_TOOLS": "spine",
        "RAG_LLM_ENABLED": "false",
        "RUST_LOG": "info"
      }
    }
  }
}
```

**Порядок:**

1. Сохранить JSON (не сломай остальные ключи: `preferences` и т.д.).
2. **Cmd+Q** - полный выход из Claude Desktop.
3. Открыть снова → новый чат.
4. Проверить tools / MCP → **rag-mcp** в списке.

Пример stdio: `examples/claude-desktop.mcp.json`.

---

## 5. Claude Code

### 5a. Remote HTTP (общий сервер) - рекомендуется

С gateway на `:7432` - project `.mcp.json` или user config:

```json
{
  "mcpServers": {
    "rag-mcp": {
      "type": "http",
      "url": "http://127.0.0.1:7432/mcp"
    }
  }
}
```

Пример: `examples/claude-code.http.mcp.json`.  
Сервер уже должен слушать (`RAG_HTTP_ONLY=true`).

### 5b. Project config (корень репо)

В корне **`.mcp.json` уже HTTP**, не stdio `command`:

```json
{
  "mcpServers": {
    "rag-mcp": {
      "type": "http",
      "url": "http://127.0.0.1:7432/mcp"
    }
  }
}
```

Нужен запущенный gateway на `127.0.0.1:7432`. Без него Claude Code покажет offline / connection error.

```bash
cd /Users/themoretheless/Documents/Sources/rag
# терминал 1: gateway (см. §3 / PROD_RUN §3a)
# терминал 2:
claude
```

1. Если server **Pending approval** → approve (`/mcp` или prompt).
2. Проверка:

```bash
claude mcp list
claude mcp get rag-mcp
```

### 5c. User-wide stdio (legacy, все проекты)

Только если **нет** другого writer на `rag.duckdb` (и нет shared gateway):

```bash
claude mcp add rag-mcp \
  -s user \
  -e RAG_DB_PATH=/Users/themoretheless/Documents/Sources/rag/rag.duckdb \
  -e RAG_EMBEDDING_PROVIDER=mock \
  -e RAG_INGEST_ROOTS=/Users/themoretheless/Documents/Sources/rag,/Users/themoretheless/Documents \
  -e RAG_TOOLS=spine \
  -e RAG_LLM_ENABLED=false \
  -e RUST_LOG=info \
  -- /Users/themoretheless/Documents/Sources/rag/target/release/rag-mcp
```

### 5d. Из Desktop

```bash
claude mcp add-from-claude-desktop
```

Подробнее: `examples/claude-code.md`.

---

## 6. Zed

### 6a. Project (exclusive stdio)

Файл: **`.zed/settings.json`** (уже в репо) - `command` + env, stdio.

Открой workspace `rag` в Zed.  
**Не** совмещай с gateway/`RAG_HTTP_ONLY` writer на тот же `rag.duckdb`.

### 6b. Global (exclusive stdio)

Файл: `~/.config/zed/settings.json` → блок `context_servers.rag-mcp`  
(или Settings → AI → MCP Servers → Add Local Server).

Пример: `examples/zed.settings.json` (stdio only).

Для **shared HTTP gateway** (как Code/Desktop) сейчас нет готового Zed snippet в `examples/`; подключай remote MCP к `http://127.0.0.1:7432/mcp`, если клиент это умеет, или оставь exclusive stdio.

**Порядок (stdio):**

1. `cargo build --release` (если binary нет).
2. Сохранить settings.
3. Reload window / restart Zed.
4. Settings → AI → MCP Servers → **зелёная точка** у `rag-mcp`.
5. Agent Panel: упомяни `rag-mcp` в промпте.

---

## 7. HTTP gateway: bind guard, wiki, backlinks

Один process с `RAG_HTTP_BIND` (часто + `RAG_HTTP_ONLY=true`):

| Path | Метод | Назначение |
|------|-------|------------|
| `/mcp` | streamable HTTP | MCP clients |
| `/health` | GET | counts, integrity, WAL and nested runtime/startup/autosync/backup state |
| `/live` | GET | process liveness, PID and uptime |
| `/ready` | GET | readiness; 503 until startup and store/FTS checks pass |
| `/v1/graph` | GET | graph UI |
| `/v1/neighbors` | GET | neighbors by seed |
| `/v1/find` | GET | find node |
| `/v1/document` | GET | document body (`id` / `uri` / `q`) |
| `/v1/wiki` | GET, PUT | catalog (metas); PUT write with optional CAS |
| `/v1/backlinks` | GET | wiki backlinks for a document |

UI: `rag-mcp-ui --http http://127.0.0.1:7432` (Wiki mode default).  
Код: `src/http_api.rs`. Полный runbook: `docs/PROD_RUN.md`.

### 7a. Bind guard (`parse_bind`)

- `RAG_HTTP_BIND=127.0.0.1:7432` (или любой loopback) - ок.
- Пустой / unset - HTTP не слушает.
- **Non-loopback** (например `0.0.0.0:7432`) **отклонён**, пока не выставлен  
  `RAG_HTTP_ALLOW_REMOTE=1|true|yes|on`.  
  MCP/HTTP **без auth** - remote bind опасен.

### 7b. `GET` / `PUT /v1/wiki`

- GET: `Store::list_wiki_page_metas_filtered` (`layer=wiki`, без `content`).
- GET query (optional): `q`, `limit`, `offset`, `kind`, `category`, `wing`, `room`.
- GET ответ: `{ ok, count, pages[] }` - `id`, `uri`, `slug`, `title`, `kind`, `summary`, `category`, `revision`, `etag`, `updated_at`.
- PUT body: `slug` (or `uri=wiki://…`), `title`, `content`; optional `if_match_revision` / `if_match_etag` → **409** on CAS mismatch. UI may also send `id` (ignored as write key).

### 7c. `GET /v1/backlinks?id=<document_id>`

- Источник: `Store::wiki_backlinks_for_document` (wikilink edges → label + id).
- `id` обязателен (document id); без limit/offset.
- Ответ: `{ ok, count, backlinks: [{ label, id }] }`.
- UI sidebar/backlinks ходят сюда; при HTTP-ошибке UI может показать пустой список (silent fail).

### 7d. Smoke curl

```bash
curl -s http://127.0.0.1:7432/health
curl -s http://127.0.0.1:7432/v1/wiki | head
curl -s 'http://127.0.0.1:7432/v1/backlinks?id=YOUR_DOC_ID'
```

---

## 8. Multi-LLM wiki: etag / revision (CAS)

Чтобы Claude и Kimi/ChatGPT не затирали одну wiki друг другу:

1. `get_wiki_page` → в ответе `revision` (число) и `etag` (`W/"3"`).
2. `write_wiki_page` / `update_wiki_page` с **`if_match_revision: 3`** (или `if_match_etag: "W/\"3\""`).
3. Если другой агент уже записал → **conflict** (current revision); re-get и merge, не blind overwrite.
4. **Без** `if_match_*` - last-write-wins (как раньше).

Детали:

| Тема | Поведение сейчас |
|------|------------------|
| Schema | `documents.revision` (+1 на successful write); schema v5 |
| Оба параметра | если заданы и `if_match_revision`, и `if_match_etag` - **revision wins** |
| Require flag | `RAG_WIKI_REQUIRE_IF_MATCH` (default false): when true, wiki **updates** must pass if_match |
| MCP conflict | `AppError::Conflict` → MCP `invalid_params` (не отдельный conflict code) |
| HTTP | GET catalog/document отдаёт `revision`/`etag`; **PUT /v1/wiki** with if_match → 409 on mismatch |
| Internal paths | часть внутренних wiki writers / ingest upsert зовут CAS с `None` (LWW) |

Агентам: всегда `get_wiki_page` → write с `if_match_*` при конкурентной работе.

---

## 9. Первый smoke (в любом клиенте)

После connect попроси агента:

1. `status` - база жива, counts  
2. `ingest_text` - короткий текст, title `"Smoke"`  
3. `query_with_index` или `search` - найти  
4. `write_wiki_page` - slug `smoke`, content с `[[Smoke]]`  
5. `rebuild_index` → `read_index`  
6. `file_answer` - сохранить ответ  

Index-first: сначала catalog/wiki, потом raw `search`.

**`ingest_file`:** path must be under `RAG_INGEST_ROOTS`. Optional placement: **`wing`**, **`room`** (project shelf). Same absolute path re-ingest = upsert.  
`ingest_text` **не** принимает wing/room (asymmetry). Wiki compile: `ingest_file` **не** обновляет wiki pages; после meaningful doc changes - `get_wiki_page` + `update_wiki_page` / `write_wiki_page` с CAS.

---

## 10. Full surface (когда нужны kg/diary/maintain)

```bash
# в env клиента / gateway:
RAG_TOOLS=full
```

Без этого `kg_add`, `wake_up`, `maintain_*` и т.п. **не в list_tools** и call вернёт ошибку.

---

## 11. Troubleshooting

| Симптом | Что сделать |
|---------|-------------|
| Tools нет | absolute path к binary; rebuild; restart client; для HTTP - gateway up |
| Claude Code offline | root `.mcp.json` is **HTTP** → start gateway on `:7432` |
| Claude Code Pending | approve `.mcp.json` в `/mcp` |
| ingest_file refuses | путь внутри `RAG_INGEST_ROOTS` |
| search «пустой/глупый» | mock → ollama/openai embeddings; **тот же** dims что при ingest |
| vec error / dims | `reembed_document` или новая DB после смены модели |
| Desktop + Code одновременно | один writer: **gateway** + remote clients, не два stdio |
| `RAG_HTTP_BIND` non-loopback fail | set `RAG_HTTP_ALLOW_REMOTE=true` (опасно) или bind `127.0.0.1` |
| wiki conflict / clobber | pass `if_match_revision` from `get_wiki_page`; re-fetch on conflict |
| empty `/v1/backlinks` | wrong `id` (need document id); or no wikilink edges yet |

Логи сервера: **stderr** (`RUST_LOG=info`), не stdout.

---

## 12. Чеклист «готово»

- [ ] `target/release/rag-mcp` существует  
- [ ] Gateway: `RAG_HTTP_ONLY=true` + `RAG_HTTP_BIND=127.0.0.1:7432` **или** exclusive stdio  
- [ ] Клиент: HTTP url / mcp-remote **или** absolute `command` + `RAG_DB_PATH`  
- [ ] `RAG_TOOLS=spine` (или full осознанно)  
- [ ] Restart клиента  
- [ ] `status` / list tools виден (~31 spine)  
- [ ] smoke ingest + search/wiki  
- [ ] multi-agent: wiki writes с `if_match_revision`  

---

## Связанные доки

| Doc | Тема |
|-----|------|
| `docs/SPINE_TOOLS.md` | список spine + cascade |
| `docs/PROD_RUN.md` | gateway, UI, prod day |
| `docs/EGUI_USAGE.md` | rag-mcp-ui modes (`--http` / `--db` / snapshot) |
| `docs/LLM_PROVIDERS.md` | ollama/claude/… |
| `docs/ARCHITECTURE_VISION.md` | зачем spine / index-first |
| `examples/claude-code.md` | только Code |
| `examples/claude-desktop.http.mcp.json` | Desktop + mcp-remote |
| `README.md` | обзор env |
