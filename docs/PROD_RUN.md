# Production run (MCP + graph UI)

Binary paths (after release build):

| Artifact | Path |
|----------|------|
| Built MCP server | `target/release/rag-mcp` |
| Installed live gateway | `/Users/themoretheless/.local/bin/rag-mcp` |
| Built Graph UI | `target/release/rag-mcp-ui` |
| Canonical live DB | `/Users/themoretheless/.local/share/rag-mcp/rag.duckdb` |

The launchd service `local.rag-mcp` owns the canonical DB. All normal clients
use `http://127.0.0.1:7432`; they do not start another binary or choose another
database path.

---

## 1. One-time build

```bash
cd /Users/themoretheless/Documents/Sources/rag
cargo build --release -p rag-mcp
cargo build --release -p rag-mcp-ui
```

---

## 2. MCP for agents (Claude Desktop / Code / Zed)

The production environment belongs to the one gateway service, not to each MCP
client. Its effective store/embedding configuration is equivalent to:

```bash
export RAG_DB_PATH=/Users/themoretheless/.local/share/rag-mcp/rag.duckdb
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

Clients connect to the already-running gateway:

```text
Zed / Claude Code: http://127.0.0.1:7432/mcp
Claude Desktop: mcp-remote → http://127.0.0.1:7432/mcp
```

For an isolated stdio smoke only, stop the gateway first and use a disposable
database that cannot be confused with production:

```bash
RAG_DB_PATH=/tmp/rag-mcp-offline-smoke.duckdb \
RAG_EMBEDDING_PROVIDER=mock \
RAG_INGEST_ROOTS=/tmp \
./target/release/rag-mcp
# Ctrl+C: MCP ждёт JSON-RPC на stdin
```

Delete the disposable file after the smoke. Do not point a client-launched
stdio process at the canonical DB while `local.rag-mcp` is running.

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
| `http://127.0.0.1:7432/v1/backlinks` | project-scoped wiki backlinks (`?id=<document_id>&wing=<project>`) |
| `http://127.0.0.1:7432/v1/activity`, `/v1/jobs*` | sanitized activity plus background sync lifecycle |
| `http://127.0.0.1:7432/v1/revisions*` | lean timeline, lazy snapshot/diff and CAS restore |
| `http://127.0.0.1:7432/v1/operations/*` | checkpoint and allowlisted backup |

The gateway is the live product port and the sole-writer operations boundary.
The exact current method/path list is available at `GET /v1/routes`; detailed
request contracts are in [`CONNECT.md`](CONNECT.md).

The shared admission middleware caps **request** bodies for both `/v1/*` and
mounted `/mcp` at 1 MiB, including requests without `Content-Length`. This is a
request-ingress limit, not a claim that HTTP responses are capped at 1 MiB.

Клиенты **не** стартуют второй binary на тот же `.duckdb`.

Source sync и guarded corpus-scale операции (`doctor_repair`, duplicate
cleanup, delete-by-source, recovery import, maintenance
apply/refresh/compress) держат process-local exclusive side coordination lane
до финализации derived index. Новые `lex`/`hybrid` запросы не ждут долгую
операцию и не вызывают embedding provider: они сразу получают HTTP `503`,
`Retry-After: 1`, `code=STORE_BUSY` (MCP: `data.code=STORE_BUSY`,
`retryable=true`, `retry_after_ms=1000`) с generic причиной `exclusive corpus
mutation is active`. Уже начавшийся search удерживает read side до конца.
Повторяй запрос после завершения текущей corpus mutation; чистый `vec` этой
lexical lane не блокируется.

Source-sync job показывает финальную фазу `refreshing_fts`; остальные guarded
corpus-scale workflows выполняют ту же финализацию внутри своего apply-вызова.
Normal terminal success возвращается только после выравнивания stale FTS.
Если после durable commit финализация падает, отчёт возвращает
`success=false`, не теряя committed counters/actions, и structured detail с
`code=FTS_FINALIZATION_FAILED`, `durable_mutation_committed`,
`retryable=true`, `fallback_dirty_marked` и возможными marker/log errors.
Source-sync сохраняет этот detail и получает terminal
`completed_with_errors`; dirty generation остаётся для безопасного next-read
retry. Не повторяй саму mutation, пока не проверил committed flag и её
идемпотентность.
Обычный single-document authoring, а также fail/cancel до финализации, оставляет
dirty marker для next-read single-flight fallback. Embedding-only update
продвигает vector/chunk generation, но сохраняет уже чистую lexical generation;
существующий FTS debt при этом не скрывается.

Embedding identity включает provider, model, dimensions и base endpoint.
`reembed_document` — только refresh одного документа при уже совпадающем
manifest. После смены identity search и новые vector writes остаются запрещены,
пока uncapped `reembed_all` не обработает весь corpus без skipped/failed и не
опубликует новый manifest. Для этого передай `max_docs` не меньше текущего числа
документов и сначала подними `RAG_MAINT_MAX_DOCS` до того же значения в env
gateway с последующим restart: per-call `max_docs` всегда clamp-ится к этому
hard cap. Default maintenance cap заранее откажет для большего corpus. Перед
первым vector write сохраняется persistent incompatible migration marker, поэтому
partial failure или возврат старой config identity не откроет mixed corpus.
Если chunks уже есть, а manifest отсутствует, startup оставляет gateway
доступным для диагностики/repair, но не самосертифицирует legacy vectors:
vector reads/writes fail closed и `ready_for_search=false`. Единственный repair
path — полный uncapped `reembed_all` без skipped/failed; `reembed_document` и
частичный run manifest не публикуют.

Все остальные maintenance `max_docs`, а также
`cleanup_source_duplicates.max_candidates`, также не могут превышать
`RAG_MAINT_MAX_DOCS`; per-call параметр выбирает полный configured batch или
меньший. Duplicate cleanup классифицирует и считает весь набор внутри DuckDB,
но materializes только ограниченный deterministic candidate batch и bounded
diagnostic detail. Apply требует `dry_run=false` вместе с `confirm=true` и
атомарно удаляет только выбранный batch; повторные вызовы сходятся по стабильному
порядку.

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

Normally launchd already runs this service. The foreground form below is only a
replacement for it: stop `local.rag-mcp` first, then run exactly one process.

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
curl -s 'http://127.0.0.1:7432/v1/backlinks?id=DOCUMENT_ID&wing=PROJECT' | head
```

### 3c. Snapshot (без HTTP)

Tool `export_graph_snapshot` → `rag-mcp-ui --snapshot ./graph.json --seed ...`  
Snapshot mode has graph only (no live wiki catalog / backlinks).

### 3d. Только UI (exclusive read-only DB inspection, MCP выключен)

This maintenance mode is allowed only after the gateway has stopped and no
process owns the canonical file:

```bash
./target/release/rag-mcp-ui \
  --db /Users/themoretheless/.local/share/rag-mcp/rag.duckdb \
  --seed "SEED"
```

Seed: title wiki/note, `node_id` или `document_id` (из `list_documents` / `list_wiki_pages` / `GET /v1/wiki`).
Do not use `--db` while another process holds the same DuckDB (single writer).
Direct `--db` Wiki is strictly read-only: edit/save and every other native
write require `--http` and go through the one-writer gateway.

---

## 4. Wiki смотреть

**В egui (рекомендуется с gateway):**

```bash
rag-mcp-ui --http http://127.0.0.1:7432
# exclusive read-only inspection only after stopping the gateway:
# rag-mcp-ui --db /Users/themoretheless/.local/share/rag-mcp/rag.duckdb
```

With `--http`, the app opens on **Home** and exposes Home, Library, Search,
Wiki, Connections and Operations. Exclusive `--db` and snapshot modes remain
limited read-only graph/wiki inspection paths. Details: [`EGUI_USAGE.md`](EGUI_USAGE.md).

В HTTP mode `/v1/projects` является authoritative catalog и грузится отдельно
от `/v1/graph`. Ошибка Connections не блокирует Home, Library, Search, History,
Wiki или Operations. Ошибка refresh каталога сохраняет последний успешный
список и selection; кнопка Retry повторяет только catalog request, не сбрасывая
graph/workspace state.

Search results сохраняют snapshot отправленного запроса и показывают stale
notice, если controls уже изменены. Library блокирует Previous/Next при
несовпадении draft и applied filters; Apply & refresh применяет фильтры со
сбросом cursor. Wiki backlinks ограничены выбранным проектом; transport/DB
failure показывается отдельно с Retry, а не как пустой список.

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
same read-then-CAS rule to MCP wiki writes. `update_wiki_page` requires an
existing page; omitted kind/category/summary retain their current values, as do
project placement, lifecycle/pin/boost state, source ownership and unrelated
metadata. Omitting CAS retains last-write-wins compatibility behavior only when
`RAG_WIKI_REQUIRE_IF_MATCH=false` (the default); when true, updates must supply
the revision/etag.

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
| 2 | Gateway: confirm the single `local.rag-mcp` service is ready on `127.0.0.1:7432` |
| 3 | Clients: Claude / Code / Zed → `http://127.0.0.1:7432/mcp` (mcp-remote or connector; see `docs/CONNECT.md`) |
| 4 | UI: `rag-mcp-ui --http http://127.0.0.1:7432` (Wiki + Graph) |
| 5 | Работа: ingest, wiki, search (MCP); browse via UI or `GET /v1/*` |
| 6 | Offline graph only if needed: export → `rag-mcp-ui --snapshot` |
| 7 | Бэкап: `POST /v1/operations/backup` с allowlisted path и `dry_run=false`; проверить `.sha256` и `.metadata.json` sidecars. Не копировать live DuckDB мимо gateway. Backup briefly checkpoints/clones under the Store mutex, then copies one pinned MVCC generation on the cloned connection while normal Store queries remain available. |

Portable recovery bundle публикуется иначе, чем raw copy: MCP `export_bundle`
и `recovery export-bundle` полностью stage+sync файл рядом с destination, затем
делают no-clobber publish по умолчанию или atomic replace только при явном
`overwrite=true`; временный файл очищается и parent directory синхронизируется.
Offline CLI откажется писать bundle поверх самой DB или её hard-link. При
`import_bundle conflict_policy=overwrite` совпадение id и URI с двумя разными
existing documents является conflict и откатывает всю import transaction.

Текущий recovery bundle format — **v2**: JSON/JSONL несёт canonical
`embedding_manifest`, а vector-bearing import проверяет provider/model/dims/base
URL/fingerprint и размерность chunks. Непустой target обязан иметь точно ту же
identity; пустой target принимает её только на apply. JSONL без единственного
manifest header перед document records (включая empty/headerless/duplicate или
unknown version) отклоняется.

Для legacy v1 без chunks допустим metadata-only import. V1 с chunks требует MCP
`import_bundle reembed_legacy=true` через запущенный single-writer gateway:
сначала dry-run и `embeddings_reembed_planned`, затем apply, который заменит все
vectors live provider и отчитается `embeddings_reembedded` и
`durable_mutation_committed`. Offline CLI такой bundle намеренно не импортирует,
потому что у него нет live embedding provider. Version/manifest вручную не
подделывать.

Portable JSON/JSONL — не путь для полного production corpus: input и encoded
output ограничены 64 MiB, один bundle — максимум 10,000 documents и 50,000
chunks. Export делает SQL preflight до materialization, import проверяет размер
файла и читает bounded reader до любой DB mutation. Для большего corpus
используй verified DuckDB `backup_db` / `recovery backup`, а не разбиение или
ручное редактирование vector bundle.

---

## 7. Одной строкой (copy-paste)

Foreground gateway + UI (shared DB; only after stopping `local.rag-mcp`):

```bash
cd /Users/themoretheless/Documents/Sources/rag
cargo build --release -p rag-mcp -p rag-mcp-ui
# terminal 1:
RAG_DB_PATH=/Users/themoretheless/.local/share/rag-mcp/rag.duckdb \
  RAG_HTTP_BIND=127.0.0.1:7432 RAG_HTTP_ONLY=true \
  RAG_TOOLS=spine RAG_EMBEDDING_PROVIDER=mock RAG_LLM_ENABLED=false \
  ./target/release/rag-mcp
# terminal 2:
./target/release/rag-mcp-ui --http http://127.0.0.1:7432 --seed "SEED"
```

Snapshot-only (no live DB):

```bash
./target/release/rag-mcp-ui export \
  --db /path/to/a/verified/offline-backup.duckdb \
  -o ./graph.json
./target/release/rag-mcp-ui --snapshot ./graph.json --seed "SEED"
```

Замени `SEED` на реальный title/id из базы.
