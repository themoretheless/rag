# Residual backlog

Everything the refactor found and deliberately left, with the reason and a
rough size (S < 200 lines touched, M 200–1000, L > 1000). Status of every
original finding is in [INVENTORY.md](INVENTORY.md); measurements in
[BASELINE.md](BASELINE.md).

## Dependencies

| item | why it is not done | size | trigger |
|---|---|---|---|
| rmcp 1.4 → 3.x (RUSTSEC-2026-0462, ignored in `deny.toml`) | Two-major SDK migration; `rmcp = "=1.4.0"` is pinned because macro and runtime must match. The advisory is in the Streamable HTTP *client* transports, which are not compiled (`server`, `transport-io`, `transport-streamable-http-server` only). Touches every `#[tool]` router, `mcp::http` and the facade. | L | Before enabling any rmcp client feature, or when 1.x stops receiving fixes. |
| DuckDB 2.0 crates (`duckdb` / `libduckdb-sys` 1.20000.x) | Storage-engine major upgrade; needed to drop `zip 6.0.0` (build-dependency of `libduckdb-sys 1.10505.0`, RUSTSEC-2026-0367/0368/0369/0429, all in `ZipArchive::extract`, used only to unpack DuckDB's own bundled archive). `duckdb = "1"` accepts 1.20000.x, so an unqualified `cargo update` performs the upgrade silently: pin or review before updating. Needs a compat test that opens an existing v26 database and a recovery round trip. | M | Next storage change; never through a blanket `cargo update`. |

## Store and performance

| item | why it is not done | size |
|---|---|---|
| Insert speed after the `FLOAT[]` cast | Document upsert fell from 93 to 86 docs/s (−7 %, update 61 → 56) because writers bind each vector as JSON text and `CAST(? AS FLOAT[])`. duckdb-rs 1.x rejects `Value::List` parameters; binding a borrowed Arrow `ListArray` (`ValueRef::List`) or an appender might remove the cast. Needs a bit-exactness test like the existing one. | S–M |
| Process globals set by `apply_process_limits` (3) | `MAX_LINKS` (`db/graph/resolve.rs`, read on every document write through `Store`), sync identity (`db/sync.rs`), telemetry call-log capacity (`src/telemetry.rs`, sizes the process-wide ring buffer shared by MCP and HTTP). Removing them needs a `StoreOptions` passed to `Store::open` (≈1 180 call sites plus the UI) and a telemetry handle in `AppContext`. | M |
| Environment reads outside `Config::from_env` (4 groups) | `RAG_DB_THREADS` / `RAG_DB_MEMORY_LIMIT` (`db/store.rs`, applied at open: same `StoreOptions` change), `RAG_EMBED_CONCURRENCY` (`src/embeddings/mod.rs`, sizes a lazy process-wide semaphore), `RAG_TOKENIZER_JSON` (`search_pack.rs`, lazy tokenizer behind the pure `estimate_tokens`), `RAG_WORKER_THREADS` (`src/ops.rs`, read before the tokio runtime and `Config` exist). `src/http_api/resources.rs` re-reads three of them for the resource report. Not counted: `RAG_EXTERNAL_DATABASES` and per-target URL variables in `external_db` (credentials resolved at use), test-only FTS crash variables, `HOSTNAME`, `GIT_*` scrubbing. | S each |
| `Documents` is the largest repository (60 methods) | Splitting by sub-concern (revisions, wiki meta, dedupe) is mechanical but touches most callers. | M |
| `ui_compat.rs` forwarders on `Store` (15) | Kept for `rag-mcp-ui`'s direct-db reader; removal belongs to the egui stage (6). | S |
| `code_index ↔ external_db` module cycle inside `rag-code-intel` | `external_db` asks `CodeIndex` for revisions and SQL write checks; `code_index::sql` evaluates check expressions from `external_db`. Both live in one crate now, so the cycle no longer blocks a split; untangling needs a small shared `sql_check` module. | S–M |

## Application layer and edges

| item | why it is not done | size |
|---|---|---|
| 86 `util::blocking_section` call sites | `block_in_place` kept where `run_blocking` cannot work: synchronous domain APIs taking `&Store` that are called from both async and blocking contexts (`db/payload_kv`, `diagnostics`, `revisions`, `diary`, `db/store/corpus_lane`, `maintain/audit`); async workflows that borrow `&Store` / `&Config` / `&dyn EmbeddingProvider` or a held corpus guard across the call (`ingest`, `maintain/*`, `wiki/*`, `retrieval`, `jobs`, `document_indexer`); MCP `graph_expand_search` and `search_wiki` (held corpus guard); `require_vec_compatible`, `http_api::sync::spawn_worker` (synchronous); `previously_applied_sync` (borrows the request). Each needs owned (`Arc`) inputs. | M–L |
| `bin/eval.rs` calls `db::search` directly | It times embedding and search separately per leg; `SearchUseCase` folds them together. Needs a timing hook on the use case. | S |
| Most of the `RAG_TOOLS=full` surface, recovery and maintenance call domain modules from the edge | Only operations with an MCP/HTTP pair were moved behind `src/app` use cases; the rest have one transport and no drift to fix. | M |
| `http_api/mod.rs` size | Router, state and auth glue in one file. | S |
| Wiki compile/consolidate and the answer judge keep their prompts in their own modules | Only the maintenance prompts moved to `maintain/prompts.rs`. | S |

## Long functions still over 200 lines (46)

`scripts/refactor_metrics.py --long 400`. Production functions over 80 lines:
255 before, 254 after (the splits moved code into phases just over the line;
see BASELINE.md).

| where | count | largest | why left |
|---|---:|---|---|
| `rag-code-intel` | 30 | `ruby::walk` 425, `csharp::walk` 402, `code_read` 389, `sql::parse_unit` 339, `docker` 339, `powershell::walk` 311, `code_dead_files` 311 | Per-language syntax walkers are one `match` over node kinds; splitting them without a visitor framework only moves arms. Each language has golden tests, so a split is safe but large. L |
| `rag-mcp-ui` | 9 | `draw_workspace` 799, `draw_project_home` 552, `draw_workspace_toolbar` 533, `drain_worker_events` 478, `draw_canvas` 333 | Owned by the egui stage (6), not part of this branch. L |
| root `src/` | 5 | `source_sync::sync_with_control` 317, `wiki::lint::lint_wiki` 306, `wiki::write::write_wiki_page_synced_command` 297, `maintain::refresh::maintain_refresh` 226, `maintain::analyze::analyze_corpus` 222 | Orchestration functions with many early exits and audit points; split like `maintain_compress` (phases), M each |
| `rag-store-duckdb` | 2 | `Config::from_env` 306, `schema::migrate_core` 242 | `from_env` is a flat list of variable reads (split by sub-config); `migrate_core` is ordered DDL that must stay readable as one sequence. S each |

## Clients

| item | why it is not done | size |
|---|---|---|
| Stage 6 (egui): `GatewayApi`, `DocumentSource` trait, `GraphApp` per-workspace state, graph render speed | Not part of this branch. | L |
| Stage 7 (web): typed endpoints, shared resources, lazy routes, `GraphCanvas` keyed join, delete `Minimap.svelte`, bundle size | Not part of this branch. | L |
| Web bundle size and MCP `tools/call` end-to-end overhead | Never measured (planned in Stage 0). | S |
