# Inventory (origin/main `facf533`)

Derived from the code, not from earlier docs. LOC and function lengths come
from `scripts/refactor_metrics.py --modules` (LOC includes in-file tests;
"fns >80" counts production functions only). `status` is updated as stages
land. Priority: P0 = blocks the layering, P1 = large SOLID/DRY debt, P2 =
local cleanup, P3 = leave.

## Headline findings (verified)

| finding | evidence | measured |
|---|---|---|
| `Store` is a god object | `impl Store` in 40 files | 258 production methods in 46 blocks; 19 blocks / 40 methods outside `src/db` (PLAN.md quotes 261 / ~59 from an earlier count that included test-only methods) |
| `Store::lock` leaks the connection | `src/db/store.rs:294` (`pub(crate)`) | 140 `self.lock()` sites in `src/db`, 33 outside |
| cycle `db → code_index` | `src/db/store.rs:1174` (`code_index::source::persist` in `upsert_document_cas_locked`) | 1 edge down, whole `code_index` up |
| cycle `db ↔ graph` | `src/graph/mod.rs:8` ↔ `src/db/graph/resolve.rs:16`, `src/db/store.rs:374,980` | `src/graph` is a 17-line re-export shim |
| cycle `db → external_db`, `code_index ↔ external_db` | `src/db/schema.rs:401`; `src/external_db/write_check.rs:540` ↔ `src/code_index/sql.rs:1405` | |
| `config →` llm / rerank / maintain / db | `src/config.rs:110,138,327,435-437,908` | `apply_process_limits` writes 3 process globals |
| `models` / `error → duckdb` | `src/models.rs:594-601`, `src/error.rs:65` | |
| `http_api → mcp` | `src/http_api/mod.rs:68`, `src/http_api/ops.rs:18-19` | |
| sync DuckDB on the async worker | `src/mcp/analysis.rs` | 50 tools, 13 `run_blocking` call lines; ~37 tools call the store inline |
| blocking helpers | `util::blocking_section` (200 uses), `util::run_blocking` (86), `spawn_blocking` (10), `block_in_place` (3), `RagServer::blocking`, `retain_mutation_guard_while_blocking`, `*_nonblocking` wrappers | 7 shapes |
| `OpsLogEntry` built by hand | `grep 'OpsLogEntry {'` | 31 struct literals in 22 non-test files (wiki 8, mcp 6, maintain 4, …) |
| maintain action list written 4 times | `src/maintain/plan.rs:40` (enum), `:82` (`ALL`), `:112` (`as_str`), `:194` (`ALLOWED_ACTIONS`); dispatch `src/maintain/apply.rs:322` | 26 actions |
| `storage` abstraction unused | `src/storage/mod.rs:100` (`trait Storage`), `src/storage/markdown/**` (757 lines) | production uses only `open_configured` and `duckdb_capability_names`; `open_configured_storage` has no caller outside `storage` |
| status literals | `'archived'` with `'tombstone'` in SQL strings | 26 lines |
| env reads outside `config` | `src/db/store.rs:1545,1552`, `src/embeddings/mod.rs:197`, `src/watch.rs:42-44`, `src/search_pack.rs:104`, `src/ops.rs:585`, `src/mcp/analysis.rs:183`, `src/external_db/mod.rs` | 24 reads in 12 files |
| fixture copies | `fn open_temp_store` ×9 in `tests/`, `fn test_config` ×15 in `src/` and `tests/` | removed in Stage 1 where bodies are equivalent |
| long functions | `scripts/refactor_metrics.py --long 30` | 255 production functions over 80 lines (218 in `src/`, 37 in `crates/`) |

## Per module

| module | LOC | fns >80 | largest production functions | main smells | prio | status |
|---|---:|---:|---|---|---|---|
| src/code_index | 49823 | 107 | `walk` 425 (ruby.rs:274); `walk` 402 (csharp.rs:300); `code_read` 382 (code_read.rs:85); `parse_unit` 339 (sql.rs:671); `docker` 339 (infrastructure/docker.rs:190) | 13 `impl Store` blocks with raw SQL via `self.lock()`; called from inside the store write path (`db/store.rs:1174`); cycle with `external_db` (sql.rs:1405); per-language `walk` monoliths | P0 (boundary), P2 (walkers) | todo |
| src/db | 29239 | 27 | `migrate` 234 (schema.rs:397); `import_recovery_bundle` 198 (recovery/import.rs:251); `upsert_document_cas_locked` 171 (store.rs:1169) | god object `Store` (218 methods here); `lock` is `pub(crate)` (store.rs:294); calls up into `code_index`, `graph`, `external_db`; process-global vector snapshot cache (search.rs:982) built under the Store mutex; embeddings stored as JSON text; "mark changed + delete chunks" repeated; recovery import has its own document INSERT; env reads (store.rs:1545) | P0 | todo |
| src/http_api | 9962 | 5 | `sync_once` 100 (sync.rs:317); `non_admin_http_role` 98 (auth.rs:175); `run_one` 91 (feedback.rs:328) | handlers rebuild commands that MCP tools also build (drift list in PLAN.md); depends on `mcp` (mod.rs:68, ops.rs:18); `Config` cloned per request (retrieval.rs pack/revision handlers); `feedback`/`eval_ops` call the search handler and re-parse its bytes; `mod.rs` is 1445 lines | P1 | todo |
| src/maintain | 9340 | 14 | `maintain_compress` 448 (compress.rs:187); `maintain_organize` 346 (organize.rs:175); `reembed_all` 256 (refresh.rs:475) | action list ×4 (plan.rs:40,82,112,194); 26-arm dispatch (apply.rs:322); `serde_json::Value` params; monolithic phases; 5 `test_config` copies | P1 | todo |
| src/mcp | 9146 | 6 | `call_tool` 118 (facade.rs:420); `prepare_recovery_bundle_for_import` 109 (recovery.rs:247); `search_wiki` 96 (wiki.rs:406) | ~37 tools in analysis.rs run sync DuckDB inline; tool names in several hand lists (`access.rs:40` 150-line match, `telemetry.rs:253`, `surface.rs:13`, `analysis_tasks.rs`); tool methods are `pub(super)`, so nothing outside can call a use case without the transport; `Config` owned by value in `RagServer` | P1 | todo |
| src/wiki | 5394 | 9 | `lint_wiki` 302 (lint.rs:6); `write_wiki_page_synced_command` 298 (write.rs:147); `consolidate` 203 (consolidate.rs:36) | several write entrypoints (`write_wiki_page`, `write_wiki_page_command`, `update_wiki_page_cas`, `put_wiki_page`, …) with different defaults; 8 hand-built ops-log entries; tests.rs 2388 lines | P1 | todo |
| src/external_db | 4520 | 9 | `check_write_plan` 246 (write_check.rs:524); `eval` 211 (check_expr.rs:88); `infer` 142 (relations.rs:163) | 3 `impl Store` blocks; its DDL is run by `db/schema.rs:401`; cycle with `code_index`; 6 env reads in mod.rs | P0 (boundary) | todo |
| src/source_sync | 2851 | 3 | `sync_with_control` 313 (source_sync.rs:263); `prepare_source_file` 106 (:577) | one 313-line orchestration function; tests build `Config` literals 17 times | P2 | todo |
| src/bin | 2680 | 7 | `parse_args_from` 192 (eval.rs:180); `evaluate` 141 (eval.rs:604); `run` 130 (ingest_project.rs:119) | hand-written argument parsing; `eval` re-implements search wiring | P2 | todo |
| src/eval | 1899 | 0 | `compare_query_metrics` 76 (compare.rs:50) | logic split between here, `bin/eval.rs` and `http_api/eval_ops.rs` | P2 | todo |
| src/ingest | 1568 | 3 | `ingest_url` 111 (ingest.rs:258); `update_document` 99 (:770); `prepare_ingest` 95 (:627) | hand-built ops-log entries; own `test_config` | P2 | todo |
| src/models | 1442 | 0 | `from_document` 39 (models.rs:1019) | `duckdb` trait impls in a domain file (:594); `layer`/`status` are strings | P0 (small) | todo |
| src/config | 1215 | 2 | `from_env` 303 (config.rs:461); `validate` 187 (:765) | ~79 fields; depends on llm/rerank/maintain/db; sets process globals (:435) | P0 | todo |
| src/retrieval | 1184 | 3 | `execute_search_with_llm` 128 (retrieval.rs:619); `run_query_transforms` 111 (:782) | closest thing to a use-case module today; transports still wrap it differently | P1 | todo |
| src/storage | 1021 | 2 | `refresh_batch` 84 (markdown/watcher.rs:85); `upsert_document` 82 (markdown/mod.rs:310) | `Storage` trait + Markdown backend not used by production paths | P1 (delete) | todo |
| src/gate, src/calibration, src/analysis_tasks | 1009 / 442 / 675 | 4 / 1 / 0 | `shell_commands` 112 (gate.rs:142); `freshness` 112 (gate.rs:390); `validate` 94 (calibration.rs:102) | `impl Store` blocks with raw SQL (1 / 1 / 1; 9 methods, 10 `self.lock()` sites) | P0 (boundary) | todo |
| src/chunking, src/embeddings, src/llm, src/rerank, src/query_transform | 1311 / 1205 / 1295 / 505 / 458 | 2 / 0 / 1 / 0 / 0 | `mask_brace_family` 119 (chunking/code_structure.rs:262); `complete_anthropic` 94 (llm/chat.rs:383) | env read `embeddings/mod.rs:197`; provider enums imported by `config` | P2 | todo |
| src/diagnostics, src/ops, src/jobs, src/watch, src/telemetry, src/access, src/revisions, src/diary | 1063 / 790 / 721 / 587 / 614 / 248 / 753 / 675 | 1 / 0 / 1 / 0 / 1 / 1 / 1 / 2 | `restore` 159 (revisions.rs:69); `required_tool_role` 150 (access.rs:40); `repair` 130 (diagnostics.rs:263); `diary_write` 112 (diary/mod.rs:137) | tool-name lists by hand (`access.rs`, `telemetry.rs`); background loops started from several places (`ops.rs:381`, `watch.rs:299`, `mcp/facade.rs:115,657`, `http_api/sync.rs`); env reads `ops.rs:585`, `watch.rs:42` | P1 (lists, loops), P2 | todo |
| src/file_ingest, src/ooxml, src/web_ingest, src/document_indexer, src/source_scan, src/search_pack, src/util, src/memory_lifecycle | 1007 / 707 / 279 / 571 / 365 / 514 / 657 / 416 | 1 / 2 / 0 / 1 / 0 / 0 / 0 / 0 | `extract_docx` 125 (ooxml.rs:198); `extract_file` 117 (file_ingest.rs:69) | env read `search_pack.rs:104`; otherwise cohesive | P3 | todo |
| src/main, src/lib, src/bootstrap, src/context | 288 / 136 / 52 / 45 | 2 / 0 / 0 / 0 | `run_serve_modes` 123 (main.rs:152); `async_main` 107 (main.rs:43) | composition spread between `main.rs`, `bootstrap`, `http_api::serve`, `RagServer::spawn_*` | P1 | todo |
| tests | 5162 | — | — | 3 extra test binaries each linking DuckDB; 9 `open_temp_store` copies | P1 | todo (Stage 1) |
| crates/rag-mcp-ui | 20047 | 37 | `draw_workspace` 799 (app/workspace.rs:6); `draw_project_home` 552 (ui/home.rs:37); `draw_workspace_toolbar` 533 (app/panels.rs:6); `drain_worker_events` 478 (app/events.rs:7) | owned by the Stage 6 agent | P1 | todo (Stage 6) |
| crates/rag-mcp-core, crates/rag-mcp-markup | 726 / 847 | 0 / 0 | `mask_code_regions` 69 (markup extract.rs:204) | core `lib.rs` is one file | P3 | todo |
| web-svelte/src | 11023 (ts + svelte) | — | — | owned by the Stage 7 agent | P1 | todo (Stage 7) |
