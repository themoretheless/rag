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
| Canonical live DB | `/Users/themoretheless/.local/share/rag-mcp/rag.duckdb` |
| Gateway | `http://127.0.0.1:7432` (`local.rag-mcp`) |

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
| `RAG_TOOLS=spine` | **по умолчанию** - **33** tools, index-first (рекомендуется) |
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
RAG_DB_PATH=/Users/themoretheless/.local/share/rag-mcp/rag.duckdb
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

Это env **единственного gateway**, а не каждого клиента. Desktop / Code / Zed
используют его `/mcp`; не запускай клиентские **stdio**-процессы рядом с
`local.rag-mcp`.

---

## 4. Claude Desktop

### 4a. Рекомендуется: remote MCP → уже запущенный сервер

Используй уже запущенный `local.rag-mcp`. Команда ниже нужна только как
foreground replacement: сначала останови service, затем подними **один** gateway
(см. `docs/PROD_RUN.md` §3):

```bash
RAG_HTTP_ONLY=true RAG_HTTP_BIND=127.0.0.1:7432 \
  RAG_DB_PATH=/Users/themoretheless/.local/share/rag-mcp/rag.duckdb \
  ... ./target/release/rag-mcp
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

### 4b. Legacy: exclusive local stdio (Claude сам стартует binary)

Только для offline/legacy работы: сначала полностью останови `local.rag-mcp` и
убедись, что другого owner у canonical DB нет. Не используй этот блок параллельно
с gateway.

```json
{
  "mcpServers": {
    "rag-mcp": {
      "command": "/Users/themoretheless/Documents/Sources/rag/target/release/rag-mcp",
      "args": [],
      "env": {
        "RAG_DB_PATH": "/Users/themoretheless/.local/share/rag-mcp/rag.duckdb",
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

Standalone examples use the safe shared-gateway bridge:
`examples/claude-desktop.mcp.json` and
`examples/claude-desktop.http.mcp.json`. The exclusive stdio block above is kept
only as an explicitly guarded legacy reference.

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

### 5c. User-wide stdio (exclusive legacy, все проекты)

Только offline и только после полной остановки `local.rag-mcp`; canonical DB не
может одновременно принадлежать shared gateway и этому stdio process:

```bash
claude mcp add rag-mcp \
  -s user \
  -e RAG_DB_PATH=/Users/themoretheless/.local/share/rag-mcp/rag.duckdb \
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

### 6a. Project (shared gateway, recommended)

Файл: **`.zed/settings.json`** (уже в репо):

```json
{
  "context_servers": {
    "rag-mcp": {
      "url": "http://127.0.0.1:7432/mcp"
    }
  }
}
```

Zed поддерживает remote MCP нативно; второй `rag-mcp` process и
вторая DuckDB не запускаются.

### 6b. Global (shared gateway)

Файл: `~/.config/zed/settings.json` → блок `context_servers.rag-mcp`
(или Settings → AI → MCP Servers → Add Remote Server).

Пример: `examples/zed.settings.json`.

**Порядок:**

1. Убедиться, что gateway отвечает на `/ready`.
2. Сохранить settings.
3. Reload window / restart Zed.
4. Settings → AI → MCP Servers → **зелёная точка** у `rag-mcp`.
5. Agent Panel: упомяни `rag-mcp` в промпте.

Exclusive stdio остаётся только offline/dev вариантом: перед ним
останови gateway и укажи явный `RAG_DB_PATH`; не создавай project-local
`rag.duckdb` рядом с живым gateway.

---

## 7. HTTP gateway: routes and contracts

Один process с `RAG_HTTP_BIND` (часто + `RAG_HTTP_ONLY=true`):

| Path | Метод | Назначение |
|------|-------|------------|
| `/mcp` | POST | stateless Streamable HTTP MCP clients; SSE response framing is retained |
| `/health` | GET | counts, integrity, WAL and nested runtime/startup/autosync/backup state |
| `/live`, `/ready` | GET | process liveness and store readiness |
| `/v1/status`, `/v1/doctor` | GET | MCP-parity status and integrity reports; status carries `pid`, `uptime_seconds`, `db_file_bytes`, `wal_bytes`, and computes raw/wiki/index compilation health with one SQL aggregate rather than loading bodies/backlinks |
| `/v1/runtime` | GET | startup phases, autosync / auto-backup state |
| `/v1/calls`, `/v1/agents` | GET | bounded in-memory call timing and per-agent presence; argument values and raw errors are never retained. `RAG_CALL_LOG_CAPACITY` defaults to 2000 |
| `/v1/capabilities`, `/v1/version`, `/v1/routes` | GET | feature, version and exact method/path discovery |
| `/v1/projects`, `/v1/project-home` | GET | project catalog and scoped inventory |
| `/v1/search` | POST | full `SearchParams` mirror (`min_score`, `diversity`, `group_by`, `recency_half_life_days`, `max_chunks_per_document`, `context_expansion`, `neighbor_chunks`, `rrf_k`, …). Hits carry `rank_vec` / `rank_lex`; `timings` has `embed_ms`, `vec_ms`, `lex_ms`, `retrieval_ms`, `postprocess_ms`, `total_ms`. An active exclusive corpus mutation makes `lex`/`hybrid` fail fast with generic retryable `STORE_BUSY` |
| `/v1/pack-context` | POST | `pack_context` mirror: hits → token-budgeted citation block |
| `/v1/ops-log` | GET | `read_log` mirror (`limit`, `id`, `seq`; client filters `agent`, `prefix`) |
| `/v1/taxonomy`, `/v1/wings`, `/v1/rooms` | GET | wing → room tree with counts |
| `/v1/diary`, `/v1/kg`, `/v1/kg/timeline`, `/v1/kg/stats`, `/v1/tunnels` | GET | L4 reads (`agent`, `subject`, `predicate`, `object`, `at_time`, `node_id`) |
| `/v1/llm-status`, `/v1/embedding-manifest`, `/v1/lint-wiki` | GET | model probe, manifest vs live config, wiki lint |

| `/v1/eval/history` | GET | last runs from the `--history-jsonl` file named by `RAG_EVAL_HISTORY` (never a request path) |
| `/v1/multi-get` | POST | ordered batch document retrieval |
| `/v1/expand-chunks`, `/v1/find-similar` | GET | retrieval helpers |
| `/v1/documents` | GET | lean unified library; server-side `q`, `wing`, `room`, `layer`, `kind`, `status`, archive filters and cursor pagination |
| `/v1/document`, `/v1/source-file` | GET | selected document body or allowlisted original source download |
| `/v1/graph`, `/v1/neighbors` | GET | optional SQL project scope, bounded to 300 nodes and depth 3 |
| `/v1/find` | GET | global node lookup used to resolve a navigation seed |
| `/v1/wiki` | GET, PUT | cursor-paginated lean catalog and CAS-protected write |
| `/v1/backlinks` | GET | project-scoped wiki backlinks for a document |
| `/v1/activity` | GET | bounded sanitized process-local activity; path-parameter job resources are retained as `/v1/jobs/{id}`, never as concrete job identifiers |
| `/v1/jobs/sync` | POST | enqueue an allowlisted incremental source sync without blocking the request |
| `/v1/jobs`, `/v1/jobs/{id}` | GET | list jobs or read progress, counters, result and error for one job |
| `/v1/jobs/{id}` | DELETE | immediately cancel a queued job or cooperatively cancel a running job |
| `/v1/revisions` | GET | lean cursor-paginated revision timeline |
| `/v1/revisions/snapshot`, `/v1/revisions/diff` | GET | one full historical snapshot on demand or bounded line diff |
| `/v1/revisions/restore` | POST | restore an old revision as a new CAS-protected head revision |
| `/v1/operations/checkpoint` | POST | checkpoint/vacuum the live store through its sole writer process |
| `/v1/operations/backup` | POST | create an allowlisted backup; `dry_run` defaults to `true` and the live DB target is refused |

`/mcp` не хранит серверную сессию: все долговечные данные уже находятся в
DuckDB, а tool-вызовы самодостаточны. Это также не даёт старому SSE polling
продолжать слать `GET /mcp` с удалённым `Mcp-Session-Id` и заполнять Activity
ожидаемыми `404 Session not found`. Клиенту достаточно повторить обычный MCP
initialize через `POST /mcp` после потери соединения.

The additional console routes are read-only. The unauthenticated REST gateway
does not expose generic ingest, delete, re-embed, vacuum, or repair mirrors;
writes stay behind the existing CAS wiki/revision endpoints, serialized sync
jobs, allowlisted backup/checkpoint operations, or the configured MCP surface.

UI: `rag-mcp-ui --http http://127.0.0.1:7432` (Home opens first in HTTP mode).
The client treats `/v1/projects` as the authoritative catalog and retries it
independently from `/v1/graph`: catalog failure retains the last successful
list/selection, while graph failure is confined to Connections and does not
block Home, Library, Search, History, Wiki or Operations. Direct UI `--db` is
strictly read-only; Wiki writes use the one-writer HTTP gateway.
Код: `src/http_api/`. Полный runbook: `docs/PROD_RUN.md`.

### 7a. Bind guard (`parse_bind`)

- `RAG_HTTP_BIND=127.0.0.1:7432` (или любой loopback) - ок.
- Пустой / unset - HTTP не слушает.
- **Non-loopback** (например `0.0.0.0:7432`) **отклонён**, пока не выставлен
  `RAG_HTTP_ALLOW_REMOTE=1|true|yes|on`.
MCP/HTTP **без auth** - remote bind опасен.

Mounted `/mcp` additionally checks the HTTP `Host` authority against an rmcp
allowlist. Loopback names/addresses are always allowed and a concrete bind IP
is added automatically. With `0.0.0.0` or `[::]`, list every remote MCP
hostname or IP explicitly as comma-separated `RAG_HTTP_ALLOWED_HOSTS`, for
example:

```bash
RAG_HTTP_ALLOWED_HOSTS=192.168.50.205,tmtl-macbook-pro-m4.local
```

### 7b. Общий лимит HTTP body

Gateway ограничивает тело каждого запроса к `/v1/*` и к смонтированному
`/mcp` значением **1 048 576 bytes (1 MiB)**. Проверка читает реальный поток,
поэтому работает и без `Content-Length`. Превышение возвращает `413`:

```json
{
  "ok": false,
  "code": "BODY_TOO_LARGE",
  "error": "request body exceeds 1048576 bytes",
  "request_id": "..."
}
```

Это общий admission boundary, а не только проверка `PUT /v1/wiki`.
Ограничение относится к request body; response body этим 1 MiB лимитом не
ограничивается.

### 7c. Catalog pagination и `GET` / `PUT /v1/wiki`

Cursor непрозрачен для клиента: первый запрос идёт без `cursor`, затем клиент
передаёт полученный `page.next_cursor` без разбора.

- `GET /v1/documents`: `limit` по умолчанию 50, диапазон 1–200; фильтры
  `q`, `wing`, `room`, `layer`, `kind`, `status`, `include_archived`; ответ
  `{ ok, items, page: { limit, next_cursor, total } }`. Bodies в catalog rows
  не загружаются.
- `GET /v1/wiki`: optional `q`, `limit`, `cursor`, `kind`, `category`, `wing`,
  `room`; legacy `offset` поддержан и при наличии имеет приоритет над cursor.
  Ответ сохраняет совместимые `items` и `pages`, а также
  `page: { limit, next_cursor, total }`. `ETag` / `If-None-Match` дают `304` для
  неизменившейся страницы.

- GET: `Store::list_wiki_page_metas_filtered` (`layer=wiki`, без `content`).
- GET row: `id`, `uri`, `slug`, `title`, `kind`, `summary`, `category`,
  `revision`, `etag`, `updated_at`.
- PUT body: `slug` (or `uri=wiki://…`), `title`, `content`; optional `if_match_revision` / `if_match_etag` → **409** on CAS mismatch. UI may also send `id` (ignored as write key).

### 7d. Background source-sync jobs

`POST /v1/jobs/sync` принимает JSON:

```json
{
  "path": "/allowlisted/source/root",
  "remove_deleted": true,
  "wing": "project-name",
  "room": null,
  "max_file_bytes": 10485760
}
```

`path` обязателен; `project` принят как alias для `wing`. Ответ `202` содержит
начальный `{ ok, job }`. Все HTTP write jobs используют один serialized lane и
тот же `Store` / embedder / config, что MCP; второй DuckDB writer не создаётся.
Изменённые небольшие документы объединяются в bounded embedding microbatches
(до 64 документов и 64 chunks), после чего каждый документ фиксируется
атомарно в canonical path order. Ошибка provider не записывает ни одного
документа текущего batch; cancellation до commit также отбрасывает весь batch,
а между commits оставляет точный уже зафиксированный prefix.

`Store` координирует exact lexical retrieval и exclusive corpus mutations через
process-local read/write lane. Write side держат source sync и guarded
corpus-scale `doctor_repair`, duplicate cleanup, delete-by-source, recovery
import и maintenance apply/refresh/compress — до завершения derived-index
finalization. `lex` и `hybrid` берут non-blocking read guard **до** query
embedding/FTS/vector work: уже активная mutation не заставляет запрос ждать и
не вызывает embedding provider. HTTP возвращает `503 Service Unavailable`,
`Retry-After: 1` и JSON
`{"ok":false,"code":"STORE_BUSY","error":"busy: exclusive corpus mutation is active; retry lexical or hybrid search after it completes"}`. MCP сохраняет JSON-RPC error и кладёт
в `data` структурированный
`{"code":"STORE_BUSY","retryable":true,"retry_after_ms":1000}`. После release
write guard обычный read-your-writes search снова доступен.

Source sync явно показывает terminal-preparation фазу `refreshing_fts`;
остальные guarded corpus-scale workflows выполняют ту же финализацию внутри
apply-вызова. Normal terminal success возвращается только после того, как FTS
догнал зафиксированные text/row mutations. Обычный single-document authoring и
fail/cancel до финализации сохраняют dirty marker: следующая lexical read
выполняет безопасный single-flight refresh. Embedding-only update продвигает
vector/chunk generation и, если lexical generation была чистой, продвигает её
без rebuild неизменного текста; уже существующий FTS debt остаётся dirty.
Если mutation уже зафиксирована, но eager FTS finalization завершилась
ошибкой, bulk-отчёт имеет `success=false`, сохраняет committed
счётчики/действия и возвращает structured finalization detail:
`code=FTS_FINALIZATION_FAILED`, `durable_mutation_committed`,
`retryable=true`, `fallback_dirty_marked` и, если нужно,
`dirty_marker_error`/`ops_log_error`. Source-sync сохраняет detail в
`report.finalization_error` и завершается как `completed_with_errors`, даже
если все file mutations уже commit. FTS остаётся dirty для next-read retry;
сначала проверь committed flag, автоматически повторять саму mutation нельзя.

Embedding manifest описывает corpus-wide provider/model/dims/base-endpoint
identity. `reembed_document` разрешён только как refresh одного документа при
совпадающей identity. После изменения любого компонента vec/hybrid и новые
vector writes остаются закрыты, пока один uncapped `reembed_all` не завершится
без skipped/failed документов и не опубликует новый manifest. Перед первым
vector write сохраняется persistent incompatible migration marker: partial
failure или возврат runtime config к старой identity поиск не переоткрывает.
Если manifest вообще отсутствует при уже существующих chunks, startup не
подписывает неизвестные legacy vectors текущей config. Gateway остаётся
доступен для `status`/`doctor` и repair, но vector reads/writes fail closed,
`ready_for_search=false`; восстановление — полный uncapped `reembed_all` без
skipped/failed, а не single-document refresh.

Для публичного `vec`/`hybrid` непустой `document_id`, `wing`, `room` или
`source_file` строит transient snapshot только из подходящих chunks через SQL
join с `documents`; `layer` и archive policy применяются в том же SQL. Такой
selective snapshot не читает и не заменяет database-wide cache,
поэтому commits в других проектах не вынуждают materialize весь corpus.
Нефильтрованный поиск сохраняет generation-aware global snapshot; один лишь
default active/archive filter не переводит его в scoped path.

Статусы: `queued`, `running`, `succeeded`, `completed_with_errors`, `failed`,
`cancelled`. Running progress проходит через `scanning`, `syncing`,
`removing_deleted` и перед terminal completion через `refreshing_fts`.
`completed_with_errors` означает, что обход завершён, но отдельные файлы
(например oversized source) дали ошибки; это не чистый success. В
`report` есть `added_count`, `updated_count`, `skipped_count`, `deleted_count`,
`error_count`, до 20 `error_samples` и подробные `counters`.

- `GET /v1/jobs` — newest first; `GET /v1/jobs/{id}` — один snapshot с
  progress/current file/report/error и timestamps.
- `DELETE /v1/jobs/{id}` — queued job сразу становится terminal
  `cancelled`; running job отменяется cooperative. Ответ `202` для
  принятой running-отмены и `200` для уже terminal job. Если cancel
  гоняется с завершением, terminal result не переписывается ложным
  статусом.
- Registry process-local и хранит максимум 100 jobs; terminal jobs вытесняются
  первыми, а при 100 active jobs admission возвращает busy error.

Schema migration создаёт lookup indexes `idx_documents_uri` и
`idx_graph_nodes_uri`. Document URI index намеренно не объявляет новую physical
uniqueness policy: ownership по-прежнему защищён Store CAS, включая legacy data.

### 7e. Revisions: timeline, lazy snapshot, diff, restore

- `GET /v1/revisions?document_id=...&limit=50&cursor=...` возвращает lean
  summaries и `page { limit, next_cursor, total }`; `limit` по умолчанию 50 и
  ограничен 1–200. Summary содержит identity/placement/status/timestamps,
  `revision`, `content_chars`, `content_lines`, но не body или metadata JSON.
- `GET /v1/revisions/snapshot?document_id=...&revision=N` загружает ровно один
  полный исторический `Document` как `{ ok, result }`. Используй его только
  после выбора строки timeline.
- `GET /v1/revisions/diff?document_id=...&from_revision=N&to_revision=M`
  сравнивает две версии; если `to_revision` не задан, используется current head.
  Line diff ограничен 400 changes, длинные строки помечаются truncated.
- `POST /v1/revisions/restore` принимает
  `{ "document_id": "...", "revision": N, "if_match_revision": HEAD }` и
  создаёт новую head revision. Stale head даёт `409`; immutable raw document
  даёт `403` и должен восстанавливаться через source file + sync.

### 7f. Activity privacy

`GET /v1/activity?limit=200&after=<seq>` возвращает хронологические `items`,
`latest_seq` и `capacity=1000`. История bounded и process-local; это не
`ops_log`.

Если клиент передаёт `X-RAG-Client-Host`, Activity и HTTP-лог используют
`host:<имя-компьютера>`. Допустимы только ASCII-буквы, цифры, `.`, `_`, `-`,
максимум 80 байт; имя приводится к нижнему регистру. Например:

```text
X-RAG-Client-Host: tmtl-macbook-pro-m4.local
```

Без заголовка raw IP и User-Agent не сохраняются: из них получается стабильный
`client-<hash>`. HTTP event хранит method + route path без query/body. MCP event
хранит tool action и только безопасные lineage fields. Source paths, titles,
content, search query, полные tool args/results и остальные headers в Activity
не попадают.

### 7g. `GET /v1/backlinks?id=<document_id>&wing=<project>`

- Источник: `Store::wiki_backlinks_for_document` (wikilink edges → label + id).
- `id` обязателен (document id); `wing` задаёт project scope; без limit/offset.
- Ответ: `{ ok, count, backlinks: [{ label, id }] }`.
- UI sidebar/backlinks всегда передаёт выбранный `wing`; ссылки из других
  проектов в результат не попадают.
- Только успешный ответ с `count = 0` означает пустой список. Transport/DB
  ошибка показывается отдельно в Page info с Retry и не маскируется под
  отсутствие backlinks. Initial-load failure не оставляет backlink result;
  refresh failure сохраняет прежний список с potentially-stale marker.

### 7h. Smoke curl

```bash
curl -s http://127.0.0.1:7432/health
curl -s http://127.0.0.1:7432/v1/wiki | head
curl -s 'http://127.0.0.1:7432/v1/revisions?document_id=YOUR_DOC_ID&limit=20'
curl -s http://127.0.0.1:7432/v1/jobs
curl -s 'http://127.0.0.1:7432/v1/backlinks?id=YOUR_DOC_ID&wing=YOUR_PROJECT'
```

### 7i. Recovery publication and import collisions

`backup_db` через gateway кратко выполняет `CHECKPOINT` и clone connection под
Store mutex, затем начинает read transaction и копирует одну pinned MVCC
generation в staged DuckDB через `COPY FROM DATABASE` уже без удержания shared
mutex. Обычные Store queries остаются доступны. Все три artifact (database,
`.sha256`, `.metadata.json`) stage/sync-ятся, sidecar paths публикуются раньше,
а verified database path — последним commit marker этой serialized generation;
это coordinated publication, а не обещание одного atomic rename для трёх paths.

`export_bundle` (MCP и offline `recovery export-bundle`) сначала записывает
полный JSON/JSONL во временный файл рядом с destination и синхронизирует его.
При `overwrite=false` publication атомарно не затирает файл, даже если другой
process создал destination после preflight. При явном `overwrite=true`
используется atomic replace; parent directory синхронизируется, staging file
удаляется и при success, и после ошибки. Offline CLI также отклоняет live DB как
output, включая тот же filesystem inode под другим path.

`import_bundle conflict_policy=overwrite` заменяет ноль или один существующий
document, найденный по id **или** URI. Если один bundle item одновременно
совпадает с двумя разными documents (id одного + URI другого), import возвращает
conflict до удаления и откатывает всю transaction, включая уже обработанные
items текущего bundle.

Новый export создаёт recovery bundle **v2**. Его JSON/JSONL содержит canonical
`embedding_manifest` для всех serialized chunk vectors; import проверяет
provider, model, dimensions, base URL, canonical fingerprint и размерность
каждого vector. Для непустого target identity должна точно совпасть, пустой
target принимает bundle identity только при apply. JSONL обязан иметь ровно
один manifest header до document records; headerless/empty/duplicate/unknown
version не угадываются и отклоняются.

Legacy v1 не доказывает identity записанных vectors. Metadata-only v1 (без
chunks) можно импортировать безопасно. Для v1 с chunks используй только MCP
`import_bundle` через единственный gateway, сначала `dry_run=true`, затем apply
с явным `reembed_legacy=true`: gateway bounded batches заменяет каждый vector
live embedding provider и возвращает `legacy_bundle_version`,
`embeddings_reembed_planned`, `embeddings_reembedded` и
`durable_mutation_committed`. Offline `recovery import-bundle` vector-bearing
v1 намеренно отклоняет; не переписывай version/manifest вручную.

Portable JSON/JSONL materialization ограничен 64 MiB, 10,000 documents и
50,000 chunks. Export отклоняется по SQL preflight до загрузки полного corpus,
import — по file metadata/bounded read до DB mutation. Для большего corpus
используй verified DuckDB backup.

---

## 8. Multi-LLM wiki: etag / revision (CAS)

Чтобы Claude и Kimi/ChatGPT не затирали одну wiki друг другу:

1. `get_wiki_page` → в ответе `revision` (число) и `etag` (`W/"3"`).
2. `write_wiki_page` / `update_wiki_page` с **`if_match_revision: 3`** (или `if_match_etag: "W/\"3\""`).
3. Если другой агент уже записал → **conflict** (current revision); re-get и merge, не blind overwrite.
4. **Без** `if_match_*` - last-write-wins только при
   `RAG_WIKI_REQUIRE_IF_MATCH=false` (default); при `true` update без CAS
   отклоняется.

Детали:

| Тема | Поведение сейчас |
|------|------------------|
| Schema | `documents.revision` (+1 на successful write); current schema v9 |
| Оба параметра | если заданы и `if_match_revision`, и `if_match_etag` - **revision wins** |
| Require flag | `RAG_WIKI_REQUIRE_IF_MATCH` (default false): when true, wiki **updates** must pass if_match |
| MCP conflict | `AppError::Conflict` → MCP `invalid_params` (не отдельный conflict code) |
| HTTP | GET catalog/document отдаёт `revision`/`etag`; **PUT /v1/wiki** with if_match → 409 on mismatch |
| Preserving update | `update_wiki_page` требует existing page; omitted kind/category/summary и placement/lifecycle/pin/boost/source/unrelated metadata сохраняются |
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
| `STORE_BUSY` / HTTP 503 на `lex` или `hybrid` | exclusive corpus mutation держит lane; уважать `Retry-After: 1` и повторить после её завершения (для source sync — после terminal job) |
| vec/ingest identity mismatch | не использовать single-document reembed как migration; запустить `maintain_refresh` с `reembed_all=true` и `max_docs` не меньше текущего числа документов, добиться zero skipped/failed и только затем повторить search/write |
| Desktop + Code одновременно | один writer: **gateway** + remote clients, не два stdio |
| `RAG_HTTP_BIND` non-loopback fail | set `RAG_HTTP_ALLOW_REMOTE=true` (опасно) или bind `127.0.0.1` |
| Remote `/mcp` rejects `Host` | add the exact hostname/IP to comma-separated `RAG_HTTP_ALLOWED_HOSTS`; wildcard bind alone does not allow it |
| wiki conflict / clobber | pass `if_match_revision` from `get_wiki_page`; re-fetch on conflict |
| successful empty `/v1/backlinks` | verify document `id` and `wing`; otherwise the scoped document has no wikilink edges yet. Transport/DB failures appear separately with Retry |

Логи сервера: **stderr** (`RUST_LOG=info`), не stdout.

---

## 12. Чеклист «готово»

- [ ] `target/release/rag-mcp` существует  
- [ ] Gateway: `RAG_HTTP_ONLY=true` + `RAG_HTTP_BIND=127.0.0.1:7432` **или** exclusive stdio  
- [ ] Клиент: HTTP url / mcp-remote **или** absolute `command` + `RAG_DB_PATH`  
- [ ] `RAG_TOOLS=spine` (или full осознанно)  
- [ ] Restart клиента  
- [ ] `status` / list tools виден (33 spine)
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
