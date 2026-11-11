# Inventory (origin/main `facf533`)

## Final status (Stage 9, 2026-11-11)

Every finding of the tables below, with its outcome. done = fixed and covered
by a test or an enforced check; partly = the main part done, the rest in
[BACKLOG.md](BACKLOG.md); not done = deliberately left, reason given. Paths
are the current ones (`crates/rag-store-duckdb/src/…` = store crate,
`crates/rag-code-intel/src/…` = intel crate). The tables after this section
are the historical record from the start of the refactor.

### Headline findings

| finding | status | outcome / reason |
|---|---|---|
| `Store` god object (258 methods, 46 blocks) | done | Stage 3: 14 repository types behind accessors; `Store` keeps open/lock/tx, accessors and 15 forwarders for rag-mcp-ui (`db/store/ui_compat.rs`, removal with the egui stage). 39 `fn` items on `Store`, 3 `impl Store` blocks, all in the store crate. |
| `Store::lock` leaks the connection (140 + 33 sites) | done | Stage 2: private to `db` outside `cfg(any(test, feature = "testkit"))`; 0 production `store.lock()` outside the store crate; the orphan rule keeps `impl Store` in it. |
| cycle `db → code_index` | done | Stage 2: `DocumentWriteHook`, registered at composition. |
| cycle `db ↔ graph` | done | Stage 2: `db` uses `rag_mcp_markup` directly. |
| cycle `db → external_db` | done | Stage 3: `SchemaExtension` registered at composition. |
| cycle `code_index ↔ external_db` | not done | Both are in `rag-code-intel` now, so it no longer blocks a split; untangling needs a shared `sql_check` module (BACKLOG). |
| `config →` llm / rerank / maintain / db | partly | Stage 2: provider enums and limits in `config`; `apply_process_limits` moved to `bootstrap`. Three process globals remain (BACKLOG). |
| `models` / `error → duckdb` | done | Stage 2: impls in `db/sql_types.rs`. |
| `http_api → mcp` | done | Stage 4: `main` builds the MCP service and passes it to `http_api`. |
| sync DuckDB on the async worker (`mcp/analysis.rs`) | done | Stage 4: every analysis tool on `run_blocking`. |
| 7 blocking helper shapes | partly | Edges use `RagServer::blocking*` / `HttpState::blocking*` over `run_blocking`; 86 `blocking_section` sites left where data is borrowed or callers are synchronous (BACKLOG). |
| `OpsLogEntry` built by hand (31 literals) | done | Stage 3: `OpsLogEntry::new` builder, `OpsLog::record` / `append`. |
| maintain action list written 4 times | done | Stage 4: declared once; 5 unimplemented actions deleted (CHANGES.md). |
| `storage` abstraction unused | done | Stage 3: `Storage` trait and Markdown backend deleted. |
| status literals (26 lines) | done | Stage 3: `db::sql::ACTIVE_DOCUMENT_SQL`. |
| env reads outside `config` (24 in 12 files) | partly | Moved into `Config` except four groups that run before or without a `Config` (BACKLOG, ARCHITECTURE.md). |
| fixture copies (`open_temp_store` ×9, `test_config` ×15) | done | Stage 1: `testkit`, `Config::for_tests`. |
| long functions (255 over 80 lines) | partly | The three worst maintenance functions split (451 → 119, 356 → 131, 263 → 146); 254 remain over 80 lines and 46 over 200 (BACKLOG). |
| vector snapshot built under the Store mutex, process-global cache | done | Stage 5 item 1: `VectorCache` owned by `Store`, built outside the mutex. |
| embeddings stored as JSON text | done | Stage 5 item 2: schema v26 `FLOAT[]`, quarantine for unconvertible rows. |
| RRF clones, lex fallback re-run, neighbour N+1 | done | Stage 5 item 3. |
| tool results parsed up to 3 times | done | Stage 5 item 4. |
| N+1 queries (tags, collections, kg_stats, health) | done | Stage 5 item 5; `kg_invalidate` batching measured and reverted (no gain). |
| `Config` cloned per request | done | Stage 4: `Arc<Config>`. |
| tool names in 4–5 hand lists | done | Stage 4: `tool_spec.rs` table plus a two-way registry test. |
| 3 extra test binaries linking DuckDB | done | Stage 1: one `it` binary. |

### Per module

| module | status | outcome / reason |
|---|---|---|
| code_index (now `rag-code-intel`) | partly | Boundary done (repository over `with_conn`, write hook, own crate). Per-language `walk` monoliths (30 functions over 200 lines) left: L-size split with no behaviour gain (BACKLOG). |
| db (now `rag-store-duckdb`) | done | Repositories, private `lock`, write hook and schema extensions, `insert_document_row_locked` / `replace_chunks_locked`, schema v26, owned vector cache. Left: `Documents` still 60 methods; `RAG_DB_THREADS` / `RAG_DB_MEMORY_LIMIT` read at open (BACKLOG). |
| http_api | partly | Use cases, no `mcp` edge, `Arc<Config>`, eval/feedback call the use case. `mod.rs` size left (S, BACKLOG). |
| maintain | partly | Whitelist once, typed params, `MaintenanceStep`, phases for compress/organize/reembed_all. `maintain_refresh` 226 and `analyze_corpus` 222 lines left. |
| mcp | done | Analysis tools on the blocking pool; tool names from `tool_spec`; `Arc<Config>`; shared router; ingest/admin/search/graph/wiki tools over `src/app`. |
| wiki | partly | One write entrypoint (`app::WikiUseCase::write` → `wiki::write_page`); ops-log entries through the builder. `lint_wiki` 306 and `write_wiki_page_synced_command` 297 lines left; `tests.rs` size left. |
| external_db (now `rag-code-intel`) | partly | Repository and DDL registration done; cycle with `code_index` and its own env reads (credentials resolved at use) left. |
| source_sync | not done | `sync_with_control` (317 lines) left: orchestration with many cancellation and audit points; split into phases is M (BACKLOG). |
| bin | partly | clap in every binary (CHANGES.md); `bin/eval.rs` still calls `db::search` directly to time legs separately (BACKLOG). |
| eval | not done | Logic still split between `src/eval`, `bin/eval.rs` and `http_api/eval_ops.rs`; eval/feedback replays now call the search use case. No drift left that a client sees. |
| ingest | done | MCP ingest tools and HTTP URL ingest go through `IngestUseCase`; ops-log builder. |
| models | done | Typed `DocumentLayer`, `DocStatus`, `DocumentKind`; duckdb impls moved out. |
| config | partly | Values only, enums in `config::providers`, limits in `config::limits`; 3 process globals and `from_env` (306 lines) left. |
| retrieval | done | Behind `SearchUseCase`; both edges call it. |
| storage | done | Deleted; `src/storage/mod.rs` is store construction only. |
| gate, calibration, analysis_tasks (now `rag-code-intel`) | done | Repositories, DDL registered at composition. |
| chunking, embeddings, llm, rerank, query_transform | partly | One LLM seam (`ChatCompleter`); `RAG_EMBED_CONCURRENCY` read outside `Config` (BACKLOG). |
| diagnostics, ops, jobs, watch, telemetry, access, revisions, diary | done | Tool-name lists from `tool_spec`; every background loop in `BackgroundTasks`; `RAG_WATCH_*` in `Config`. `RAG_WORKER_THREADS` read before the runtime exists (BACKLOG). |
| file_ingest, ooxml, web_ingest, document_indexer, source_scan, search_pack, util, memory_lifecycle | partly | `decode_text` moved to `util::decode`; `RAG_TOKENIZER_JSON` read lazily in `search_pack` (BACKLOG). P3 otherwise, left as is. |
| main, lib, bootstrap, context | done | One composition root (`main.rs`, `bootstrap`, `AppContext`, `BackgroundTasks`). |
| tests | done | One `it` binary, shared `testkit`, parity and architecture tests. |
| crates/rag-mcp-ui | not done | Egui stage 6 is not part of this branch; its direct-db build now depends on the store crate only (Stage 8). |
| crates/rag-mcp-core, crates/rag-mcp-markup | not done | P3: small and cohesive. |
| web-svelte/src | not done | Web stage 7 is not part of this branch. |

## Historical record (start of the refactor)

Derived from the code, not from earlier docs. LOC and function lengths come
from `scripts/refactor_metrics.py --modules` (LOC includes in-file tests;
"fns >80" counts production functions only). `status` is updated as stages
land. Priority: P0 = blocks the layering, P1 = large SOLID/DRY debt, P2 =
local cleanup, P3 = leave.

## Headline findings (verified)

| finding | evidence | measured |
|---|---|---|
| ~~`Store` is a god object~~ (Stage 3: 14 repository types, Store keeps open/lock/tx, accessors and 15 methods for rag-mcp-ui) | `impl Store` in 40 files | 258 production methods in 46 blocks; 19 blocks / 40 methods outside `src/db` (PLAN.md quotes 261 / ~59 from an earlier count that included test-only methods) |
| `Store::lock` leaks the connection | `src/db/store.rs:294` (`pub(crate)`) | 140 `self.lock()` sites in `src/db`, 33 outside |
| cycle `db → code_index` | `src/db/store.rs:1174` (`code_index::source::persist` in `upsert_document_cas_locked`) | 1 edge down, whole `code_index` up |
| cycle `db ↔ graph` | `src/graph/mod.rs:8` ↔ `src/db/graph/resolve.rs:16`, `src/db/store.rs:374,980` | `src/graph` is a 17-line re-export shim |
| cycle ~~`db → external_db`~~ (Stage 3: intel DDL registered at composition), `code_index ↔ external_db` | `src/db/schema.rs:401`; `src/external_db/write_check.rs:540` ↔ `src/code_index/sql.rs:1405` | |
| `config →` llm / rerank / maintain / db | `src/config.rs:110,138,327,435-437,908` | `apply_process_limits` writes 3 process globals |
| `models` / `error → duckdb` | `src/models.rs:594-601`, `src/error.rs:65` | |
| `http_api → mcp` | `src/http_api/mod.rs:68`, `src/http_api/ops.rs:18-19` | done (Stage 4) |
| sync DuckDB on the async worker | `src/mcp/analysis.rs` | 50 tools, 13 `run_blocking` call lines; ~37 tools call the store inline |
| blocking helpers | `util::blocking_section` (200 uses), `util::run_blocking` (86), `spawn_blocking` (10), `block_in_place` (3), `RagServer::blocking`, `retain_mutation_guard_while_blocking`, `*_nonblocking` wrappers | 7 shapes |
| ~~`OpsLogEntry` built by hand~~ (Stage 3: `OpsLogEntry::new` builder + `OpsLog::record`/`append`) | `grep 'OpsLogEntry {'` | 31 struct literals in 22 non-test files (wiki 8, mcp 6, maintain 4, …) |
| maintain action list written 4 times | `src/maintain/plan.rs:40` (enum), `:82` (`ALL`), `:112` (`as_str`), `:194` (`ALLOWED_ACTIONS`); dispatch `src/maintain/apply.rs:322` | 26 actions |
| ~~`storage` abstraction unused~~ (Stage 3: deleted) | `src/storage/mod.rs:100` (`trait Storage`), `src/storage/markdown/**` (757 lines) | production uses only `open_configured` and `duckdb_capability_names`; `open_configured_storage` has no caller outside `storage` |
| ~~status literals~~ (Stage 3: `db::sql::ACTIVE_DOCUMENT_SQL`; the code_index rule that also hides `consolidated` stays separate) | `'archived'` with `'tombstone'` in SQL strings | 26 lines |
| env reads outside `config` | `src/db/store.rs:1545,1552`, `src/embeddings/mod.rs:197`, `src/watch.rs:42-44`, `src/search_pack.rs:104`, `src/ops.rs:585`, `src/mcp/analysis.rs:183`, `src/external_db/mod.rs` | 24 reads in 12 files |
| fixture copies | `fn open_temp_store` ×9 in `tests/`, `fn test_config` ×15 in `src/` and `tests/` | removed in Stage 1 where bodies are equivalent |
| long functions | `scripts/refactor_metrics.py --long 30` | 255 production functions over 80 lines (218 in `src/`, 37 in `crates/`) |

## Per module

| module | LOC | fns >80 | largest production functions | main smells | prio | status |
|---|---:|---:|---|---|---|---|
| src/code_index | 49823 | 107 | `walk` 425 (ruby.rs:274); `walk` 402 (csharp.rs:300); `code_read` 382 (code_read.rs:85); `parse_unit` 339 (sql.rs:671); `docker` 339 (infrastructure/docker.rs:190) | ~~13 `impl Store` blocks with raw SQL via `self.lock()`~~; ~~called from inside the store write path (`db/store.rs:1174`)~~ (Stage 2: `DocumentWriteHook`); cycle with `external_db` (sql.rs:1405); per-language `walk` monoliths; ~~13 `impl Store` blocks~~ (Stage 2: `CodeIndex` repository over `Store::with_conn`) | P0 (boundary), P2 (walkers) | Stage 2: boundary done |
| src/db | 29239 | 27 | `migrate` 234 (schema.rs:397); `import_recovery_bundle` 198 (recovery/import.rs:251); `upsert_document_cas_locked` 171 (store.rs:1169) | ~~god object `Store` (218 methods here)~~ (Stage 3: repositories `Documents`, `Chunks`, `GraphRepo`, `KgRepo`, `OpsLog`, `EmbeddingManifestRepo`, `WikiIndexRepo`, `SyncRepo`, `CorpusLane`, `Collections`, `ReembedProgress`, `Maintenance`, `SourceManifestRepo`, `Recovery`); ~~`lock` is `pub(crate)`~~ (Stage 2: private to `db` outside tests); ~~calls up into `external_db`~~ (Stage 3: `SchemaExtension` registration); process-global vector snapshot cache (search.rs:982) built under the Store mutex; embeddings stored as JSON text; ~~"mark changed + delete chunks" repeated~~ (Stage 3: `replace_chunks_locked`); ~~recovery import has its own document INSERT~~ (Stage 3: `insert_document_row_locked`, normalization bug fixed); env reads (store.rs `apply_performance_pragmas`); `Documents` is still the largest repository (60 methods) | P0 | Stage 3: done (Stage 5: schema/vector cache) |
| src/http_api | 9962 | 5 | `sync_once` 100 (sync.rs:317); `non_admin_http_role` 98 (auth.rs:175); `run_one` 91 (feedback.rs:328) | handlers rebuild commands that MCP tools also build (drift list in PLAN.md); depends on `mcp` (mod.rs:68, ops.rs:18; kept for Stage 4, no neutral home before the app-layer tool registry); `Config` cloned per request (retrieval.rs pack/revision handlers); `feedback`/`eval_ops` call the search handler and re-parse its bytes; `mod.rs` is 1445 lines | P1 | partly done (Stage 4: use cases, no `mcp` edge, `Arc<Config>`, eval/feedback call the use case; `mod.rs` size open) |
| src/maintain | 9340 | 14 | `maintain_compress` 448 (compress.rs:187); `maintain_organize` 346 (organize.rs:175); `reembed_all` 256 (refresh.rs:475) | action list ×4 (plan.rs:40,82,112,194); 26-arm dispatch (apply.rs:322); `serde_json::Value` params; monolithic phases; 5 `test_config` copies | P1 | done (Stage 4: whitelist declared once, typed params per action, `trait MaintenanceStep`, compress/organize/reembed_all split into phases — now 119/131/146 lines; `maintain_refresh` 226 and `analyze_corpus` 222 open) |
| src/mcp | 9146 | 6 | `call_tool` 118 (facade.rs:420); `prepare_recovery_bundle_for_import` 109 (recovery.rs:247); `search_wiki` 96 (wiki.rs:406) | ~37 tools in analysis.rs run sync DuckDB inline; tool names in several hand lists (`access.rs:40` 150-line match, `telemetry.rs:253`, `surface.rs:13`, `analysis_tasks.rs`); tool methods are `pub(super)`, so nothing outside can call a use case without the transport; `Config` owned by value in `RagServer` | P1 | todo |
| src/wiki | 5394 | 9 | `lint_wiki` 302 (lint.rs:6); `write_wiki_page_synced_command` 298 (write.rs:147); `consolidate` 203 (consolidate.rs:36) | several write entrypoints (`write_wiki_page`, `write_wiki_page_command`, `update_wiki_page_cas`, `put_wiki_page`, …) with different defaults; 8 hand-built ops-log entries; tests.rs 2388 lines | P1 | todo |
| src/external_db | 4520 | 9 | `check_write_plan` 246 (write_check.rs:524); `eval` 211 (check_expr.rs:88); `infer` 142 (relations.rs:163) | ~~3 `impl Store` blocks~~ (Stage 2: `ExternalDb` repository); ~~its DDL is run by `db/schema.rs:401`~~ (Stage 3: `external_db::SCHEMA`, registered at composition); cycle with `code_index`; 6 env reads in mod.rs | P0 (boundary) | Stage 3: DDL done |
| src/source_sync | 2851 | 3 | `sync_with_control` 313 (source_sync.rs:263); `prepare_source_file` 106 (:577) | one 313-line orchestration function; tests build `Config` literals 17 times | P2 | todo |
| src/bin | 2680 | 7 | `parse_args_from` 192 (eval.rs:180); `evaluate` 141 (eval.rs:604); `run` 130 (ingest_project.rs:119) | hand-written argument parsing; `eval` re-implements search wiring | P2 | partly done (Stage 4: clap in every bin; `eval` still calls `db::search` directly) |
| src/eval | 1899 | 0 | `compare_query_metrics` 76 (compare.rs:50) | logic split between here, `bin/eval.rs` and `http_api/eval_ops.rs` | P2 | todo |
| src/ingest | 1568 | 3 | `ingest_url` 111 (ingest.rs:258); `update_document` 99 (:770); `prepare_ingest` 95 (:627) | hand-built ops-log entries; own `test_config` | P2 | todo |
| src/models | 1442 | 0 | `from_document` 39 (models.rs:1019) | ~~`duckdb` trait impls in a domain file~~ (moved to `db/sql_types.rs`); ~~`layer`/`status` are strings~~ (Stage 3: `DocumentLayer`, `DocStatus`, open enums with the same wire/DB strings) | P0 (small) | Stage 3: done |
| src/config | 1215 | 2 | `from_env` 303 (config.rs:461); `validate` 187 (:765) | ~79 fields; ~~depends on llm/rerank/maintain/db~~ (Stage 2: enums in `config::providers`, constants in `config::limits`); process globals still set by `apply_process_limits`, now implemented in `bootstrap.rs`. Left as globals on purpose: `max_links_per_doc` is read by `db::graph::resolve::extract_options` on every document write, reached through `Store` (no config at `Store::open`, ~1180 call sites); call-log capacity sizes the process-wide `telemetry` ring buffer shared by MCP and HTTP; sync identity is read by `db::sync::SyncIdentity::current` from store sync paths. Passing them at construction needs a `Store` options type (Stage 3/4) | P0 | Stage 2: partial |
| src/retrieval | 1184 | 3 | `execute_search_with_llm` 128 (retrieval.rs:619); `run_query_transforms` 111 (:782) | closest thing to a use-case module today; transports still wrap it differently | P1 | todo |
| src/storage | 1021 | 2 | `refresh_batch` 84 (markdown/watcher.rs:85); `upsert_document` 82 (markdown/mod.rs:310) | ~~`Storage` trait + Markdown backend not used by production paths~~ | P1 (delete) | Stage 3: deleted |
| src/gate, src/calibration, src/analysis_tasks | 1009 / 442 / 675 | 4 / 1 / 0 | `shell_commands` 112 (gate.rs:142); `freshness` 112 (gate.rs:390); `validate` 94 (calibration.rs:102) | ~~`impl Store` blocks with raw SQL~~ (Stage 2: `Gate`, `Calibrations`, `AnalysisTasks` repositories over `Store::with_conn`); DDL owned here and registered at composition (Stage 3) | P0 (boundary) | Stage 3: done |
| src/chunking, src/embeddings, src/llm, src/rerank, src/query_transform | 1311 / 1205 / 1295 / 505 / 458 | 2 / 0 / 1 / 0 / 0 | `mask_brace_family` 119 (chunking/code_structure.rs:262); `complete_anthropic` 94 (llm/chat.rs:383) | env read `embeddings/mod.rs:197`; provider enums imported by `config` | P2 | todo |
| src/diagnostics, src/ops, src/jobs, src/watch, src/telemetry, src/access, src/revisions, src/diary | 1063 / 790 / 721 / 587 / 614 / 248 / 753 / 675 | 1 / 0 / 1 / 0 / 1 / 1 / 1 / 2 | `restore` 159 (revisions.rs:69); `required_tool_role` 150 (access.rs:40); `repair` 130 (diagnostics.rs:263); `diary_write` 112 (diary/mod.rs:137) | tool-name lists by hand (`access.rs`, `telemetry.rs`); background loops started from several places (`ops.rs:381`, `watch.rs:299`, `mcp/facade.rs:115,657`, `http_api/sync.rs`); env reads `ops.rs:585`, `watch.rs:42` | P1 (lists, loops), P2 | todo |
| src/file_ingest, src/ooxml, src/web_ingest, src/document_indexer, src/source_scan, src/search_pack, src/util, src/memory_lifecycle | 1007 / 707 / 279 / 571 / 365 / 514 / 657 / 416 | 1 / 2 / 0 / 1 / 0 / 0 / 0 / 0 | `extract_docx` 125 (ooxml.rs:198); `extract_file` 117 (file_ingest.rs:69) | env read `search_pack.rs:104`; otherwise cohesive | P3 | todo |
| src/main, src/lib, src/bootstrap, src/context | 288 / 136 / 52 / 45 | 2 / 0 / 0 / 0 | `run_serve_modes` 123 (main.rs:152); `async_main` 107 (main.rs:43) | composition spread between `main.rs`, `bootstrap`, `http_api::serve`, `RagServer::spawn_*` | P1 | todo |
| tests | 5162 | — | — | 3 extra test binaries each linking DuckDB; 9 `open_temp_store` copies | P1 | todo (Stage 1) |
| crates/rag-mcp-ui | 20047 | 37 | `draw_workspace` 799 (app/workspace.rs:6); `draw_project_home` 552 (ui/home.rs:37); `draw_workspace_toolbar` 533 (app/panels.rs:6); `drain_worker_events` 478 (app/events.rs:7) | owned by the Stage 6 agent | P1 | todo (Stage 6) |
| crates/rag-mcp-core, crates/rag-mcp-markup | 726 / 847 | 0 / 0 | `mask_code_regions` 69 (markup extract.rs:204) | core `lib.rs` is one file | P3 | todo |
| web-svelte/src | 11023 (ts + svelte) | — | — | owned by the Stage 7 agent | P1 | todo (Stage 7) |

## Stage 2: environment reads still outside `Config`

`RAG_GATE_MODE` (`mcp/analysis.rs`) and `RAG_WATCH_*` (`watch.rs`) moved into
`Config` fields in Stage 2. These stay, because no `Config` reaches the read
without a construction change that belongs to Stage 3/4:

| env | read at | why it stays |
|---|---|---|
| `RAG_DB_THREADS`, `RAG_DB_MEMORY_LIMIT` | `db/store.rs` `apply_performance_pragmas` | applied by `Store::open` / `open_read_only`, which take only a path (~1180 call sites, plus the UI crate). Needs a `StoreOptions` built at composition. |
| `RAG_EMBED_CONCURRENCY` | `embeddings/mod.rs` `embed_concurrency_limit` | sizes the process-wide `EMBED_GATE` semaphore in a `LazyLock`, also reported by `embedder_stats()`; no `Config` at first use. |
| `RAG_TOKENIZER_JSON` | `search_pack.rs` `tokenizer_count` | lazy process-wide tokenizer behind the pure `estimate_tokens`, called from domain code with no `Config`. |
| `RAG_WORKER_THREADS` | `ops.rs` `worker_threads` | read by `main` to size the tokio runtime before `Config::from_env` runs inside it, and by the resource snapshot. |

## Residual backlog (outside this refactor)

Moved to [BACKLOG.md](BACKLOG.md) (rmcp 3.x, DuckDB 2.0 and everything else
deferred, with reasons and sizes).

## Stage 4 status

Done: app use cases (search, pack_context, multi_get, neighbors, wiki write);
one wiki write entrypoint; `ToolSpec` table plus a two-way registry test;
`http_api → mcp` removed; every `mcp/analysis.rs` tool on `run_blocking`, one
body for the six `db_*` tools; `Arc<Config>`, one shared tool router, legacy
constructors gone; eval/feedback call the search use case; `BackgroundTasks`;
maintain whitelist declared once, five unimplemented actions deleted, rule
table for `heuristic_plan`; clap in four bins; `storage::open_store`.

Done in the second pass: `IngestUseCase` and `AdminUseCase` (MCP ingest and
admin reads, HTTP ingest/url and the console reads; rooms wing filter
aligned); typed params per maintenance action read once from the unchanged
wire object, `trait MaintenanceStep` with one implementation per action;
`maintain_compress` (451 → phases ≤119 lines), `maintain_organize`
(356 → 131) and `reembed_all` (263 → 146) split into named phases; one LLM
trait (`TransformLlm` folded into `ChatCompleter`) and the maintenance
prompts in `maintain/prompts.rs`; clap in `recovery`, `ingest_project` and
`eval`; 80 `blocking_section` sites in `mcp/*` and `http_api/*` moved to the
blocking pool (`RagServer::blocking_tool`, `HttpState::blocking_response`,
`HttpState::blocking`).

Open (carried to the backlog): `bin/eval.rs` still calls `db::search`
directly (it measures embedding and search time per leg, which the
production use case folds together). 86 `blocking_section` sites remain,
kept on purpose: synchronous domain APIs that take `&Store` and are called
from both async and blocking contexts (`db/payload_kv`, `diagnostics`,
`revisions`, `diary`, `db/store/corpus_lane`, `maintain/audit`); async
domain workflows that borrow `&Store` / `&Config` / `&dyn EmbeddingProvider`
or a held corpus guard across the call (`ingest`, `maintain/*`, `wiki/*`,
`retrieval`, `jobs`, `document_indexer`; `run_blocking` needs `'static`
data, `block_in_place` keeps the borrow); `mcp` `graph_expand_search` and
`search_wiki` (borrow the held corpus guard); `require_vec_compatible` and
`http_api::sync::spawn_worker` (synchronous); `previously_applied_sync`
(its `WikiSyncWrite` borrows the request). `maintain_refresh` (226 lines)
and `analyze_corpus` (222) are still long. Wiki compile/consolidate and the
answer judge keep their prompts in their own modules.


## Stage 8: crate split

Done: `crates/rag-store-duckdb` holds `db/**` plus the domain modules
(`models`, `error`, `config`, `util`, `graph`, `search_pack`, `tool_spec`);
`crates/rag-code-intel` holds `code_index`, `external_db`, `gate`,
`calibration`, `analysis_tasks`. Paths in the module table above are the
pre-split ones: `src/db/…`, `src/models.rs`, … now live under
`crates/rag-store-duckdb/src/`, and `src/code_index/…`, `src/external_db/…`,
`src/gate*`, `src/calibration.rs`, `src/analysis_tasks.rs` under
`crates/rag-code-intel/src/`. The root crate re-exports all of them under
their old paths. `file_ingest::decode_text` moved to `util::decode` (the last
allow-listed layering edge). `rag-mcp-ui --features direct-db` depends on
`rag-store-duckdb` only. Build times in BASELINE.md.

Visibility widened for the split (all `pub(crate)` → `pub` unless noted):

- store, used by the root crate: `Store::with_conn`,
  `Store::ensure_schema_extensions`; repository methods on `Documents`
  (`document_revision`, `document_id_and_uri`, `list_document_refs`,
  `count_documents`, `count_documents_by_layer`,
  `count_graph_rebuild_candidates`, `list_graph_rebuild_candidates_after`,
  `list_document_ids_after`, `count_chunks_for_document_limit`,
  `exact_duplicate_source_ids`, `list_active_unscoped_documents`,
  `list_documents_missing_chunks`, `list_documents_by_layer_without_body`,
  `document_contents`, `prepare_document_meta_update`,
  `write_document_atomic`, `write_unchunked_document_with_audit`,
  `write_source_document_atomic`, `write_wiki_document_atomic`,
  `apply_document_dispositions_atomic`, `merge_exact_duplicates_atomic`,
  `delete_source_state`), `Chunks` (`chunk_vectors_by_content`,
  `representative_document_embeddings`, `first_chunk_embeddings`,
  `update_chunk_embeddings_atomic`, `count_chunks_for_document`),
  `CorpusLane` (`corpus_mutation_lane`, `try_corpus_mutation_guard`,
  `with_corpus_mutation_guard`, `try_corpus_mutation_scope`,
  `try_embedding_write_guard`, `record_fts_finalization_failure`),
  `Maintenance` (`layer_health_counts`, `uncompiled_raw_shelves`),
  `ReembedProgress` (all five methods), `list_diary_activity`, `SyncRepo`
  (`previously_applied_sync`, `preflight_sync_batch`); types
  `CorpusMutationGuard`, `EmbeddingWriteGuard`, `FtsFinalizationFailure`,
  `DocumentDerivedWrite`, `AtomicDocumentWriteResult`,
  `ExactDuplicateMergeResult`, `LayerHealthCounts`, `UncompiledShelfQueue`,
  `DocumentEmbeddingSample`, `WikiDocumentWrite` (and its fields),
  `DocumentRef`, `CorpusSearchGuard`, `sync::{SyncIdentity, SyncIdentity::current,
  SyncIdentity::role, WikiSyncWrite, WikiSyncPayload, validate_wiki_change}`;
  functions `store::{wiki_backlink_rows, embedding_migration_manifest,
  get_embedding_manifest_locked}`, `search::{acquire_corpus_search_guard,
  search_with_corpus_guard}`, `review::accept_locked`, `analytics::*` (11
  helpers and `REPORT_LIST_CAP`); modules `db::payload_kv` and `db::sql`
  (`#[doc(hidden)]`); macro `models::finding_codes` (`#[macro_export]`,
  `#[doc(hidden)]`).
- store test hooks, public only under `cfg(any(test, feature = "testkit"))`
  and `#[doc(hidden)]`: `Store::lock`, `fts::test_fault::{fail, panic,
  check}`, `fts::test_hooks::FTS_CRASH_ARMED`,
  `store::TEST_FAIL_WIKI_WRITE_AFTER_INDEX_OP`,
  `Chunks::all_chunks_with_embeddings`,
  `Documents::update_document_meta_for_sync_test`.
- code-intel, used by the root crate: module `code_index::source` (its
  `persist`, `install_document_write_hook`, `SourceBlobHook` were already
  `pub` inside it) plus `source::{read, is_preserved_html, same_attachment}`;
  `CodeIndex::code_snapshot_in_project`; `analysis_tasks::{index_tool,
  evidence_count}`; `AnalysisTasks::{analysis_call_start,
  analysis_call_finish}`.
