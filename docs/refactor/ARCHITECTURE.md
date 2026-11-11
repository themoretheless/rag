# Target architecture

Status: target for Stages 2–8 of [PLAN.md](PLAN.md); Stage 8 (crate split) is
done, see "Crates". The "Current violations" section is the historical list
from the start of the refactor. The decision record is
[ADR 0001](../adr/0001-layering-and-crate-split.md).

## Layers

```
domain   models, error, config types, util, chunking, graph::extract, search_pack
store    db/** (DuckDB)                                              → domain
intel    code_index, external_db, gate, calibration, analysis_tasks  → store, domain
app      use cases: retrieval, ingest, source_sync, wiki, maintain,
         eval, llm, embeddings                                       → store, intel, domain
edge     mcp, http_api                                               → app
bins     main, src/bin/*                                             → edge, app
```

| layer | owns | must not know about |
|---|---|---|
| domain | plain types (`Document`, `Chunk`, `SearchHit`), `AppError`, config value types, pure functions (chunking, link extraction, token packing) | DuckDB, tokio runtime, HTTP, MCP, env vars |
| store | schema, migrations, SQL, the connection mutex, repositories (`Documents`, `Chunks`, `GraphRepo`, `OpsLog`, …), vector/FTS search | code intelligence, use cases, transports |
| intel | code index, external DB checks, gate, calibration, analysis tasks; talks to the store through repository types, never `Store::lock` | use cases, transports |
| app | one use case per operation with typed commands/results; parameter normalization, limit clamps and defaults live here once | rmcp, axum |
| edge | MCP tools and HTTP handlers as thin adapters: decode → call a use case → encode | SQL, each other |
| bins | argument parsing and the composition root | anything below `app` directly, except through `bootstrap` |

## Dependency rule

A module may depend only on modules in its own layer or a layer listed to the
right of its arrow. Never upwards, never sideways between `mcp` and `http_api`.
No cycles between modules of one layer either.

Consequences that follow from the rule:

- `domain` has no `duckdb` import. `ToSql`/`FromSql` and `From<duckdb::Error>`
  impls live in `store` (`db/sql_types.rs`, same crate since Stage 8).
- `store` does not call `intel` or `app`. Where a write must trigger work owned
  above it (today: `code_index::source::persist` inside the document upsert),
  `store` exposes a hook trait (`DocumentWriteHook`) and the composition root
  registers the implementation.
- `config` holds values only. Provider enums and defaults it needs are domain
  types; process-wide knobs (`max_links_per_doc`, call-log capacity, sync
  identity) are passed at construction instead of being pushed into globals by
  `Config::apply_process_limits`.
- Environment variables are read in one place (`Config::from_env`).
- An architecture test (`tests/it/architecture_test.rs`, Stage 2) parses
  `crate::` edges between top-level modules and fails on an edge that breaks
  the rule. Its allow-list is empty since Stage 8 (`decode_text` moved to `util::decode`). The
  `db → analysis_tasks|gate|calibration|external_db` edges went in Stage 3:
  those modules export a `db::schema::SchemaExtension` that composition
  registers (`storage::register_schema_extensions`) and their repositories
  ensure (`Store::ensure_schema_extensions`).

## Application layer (Stage 4)

- `src/app/`: `SearchUseCase` (search, `pack_context`, `multi_get`),
  `GraphUseCase` (neighbors), `WikiUseCase` (one write entrypoint with
  `WikiWriteMode::{Create, Upsert, Cas}`), `IngestUseCase` (text, file and
  URL ingest), `AdminUseCase` (taxonomy, wings, rooms, KG reads, wiki lint,
  reembed and LLM status). MCP tools and HTTP handlers decode their
  parameters into these typed commands and wrap the result in their own
  envelope.
- Edges run store work on the blocking pool: `RagServer::blocking_tool` /
  `RagServer::blocking` and `HttpState::blocking_response` /
  `HttpState::blocking` (`util::run_blocking`). `util::blocking_section`
  (`block_in_place`) stays only where the data is borrowed or the caller is
  synchronous (INVENTORY.md, Stage 4 status).
- `maintain`: the plan step wire shape `{action, reason?, target_id?,
  params}` is fixed; `maintain/apply/params.rs` reads one typed struct per
  action from `params`, `maintain/apply/steps.rs` holds `trait
  MaintenanceStep` with one implementation per action. One LLM seam,
  `llm::ChatCompleter` (query transforms included); maintenance prompt text
  lives in `maintain/prompts.rs`.
- `src/tool_spec.rs` (domain): one `ToolSpec` row per MCP tool (role,
  telemetry group, spine flag, index-evidence flag) read by `access`,
  `telemetry`, `mcp::surface` and `analysis_tasks`.
- `src/background.rs`: `BackgroundTasks`, built in `main`, owns every loop and
  their shared `CancellationToken`.
- `storage::open_store`: the one store constructor for bins and test helpers;
  it installs the write hook and schema extensions.

## Composition root

`src/main.rs` (and each `src/bin/*` through `bootstrap::open_offline`) is the
only place that:

1. reads the environment into `Config` and validates it;
2. opens the `Store` and builds the embedder and chat client;
3. registers store hooks (`DocumentWriteHook`);
4. builds `AppContext { store, embedder, config: Arc<Config>, llm }` and the use
   cases over it;
5. builds the edges (`RagServer`, the axum router) from the use cases and mounts
   `/mcp` on the HTTP router;
6. starts background work (auto-backup, FTS maintenance, auto-sync, watcher,
   sync worker) from one `BackgroundTasks` with a shared `CancellationToken`.

Nothing below the root constructs its own dependencies or reads globals.

## Crates

Boundaries were fixed inside the root crate first (Stages 2–4). Stage 8 split
out the two layers with the slowest builds and the clearest seams:

```
rag-mcp-core   rag-mcp-markup                     (wire types, link parsing)
      ↑              ↑
rag-store-duckdb      domain (models, error, config, util, graph,
                      search_pack, tool_spec) + store (db/**)
      ↑
rag-code-intel        intel (code_index, external_db, gate, calibration,
                      analysis_tasks)
      ↑
rag-mcp (root)        app, edge, bins; re-exports every moved module under
                      its old path (rag_mcp::db, rag_mcp::models,
                      rag_mcp::code_index, ...)

rag-mcp-ui --features direct-db → rag-store-duckdb (+ core, markup) only
```

- The domain layer lives in `rag-store-duckdb`, not in a crate of its own:
  the store implements DuckDB traits for domain types (`ToSql`/`FromSql` for
  the string-coded enums, `From<duckdb::Error> for AppError`,
  `db/sql_types.rs`), which the orphan rule allows only in the crate that
  owns the types. A separate domain crate would have needed newtypes or a
  `duckdb` feature on the domain crate. `rag-mcp-core` stays the serde-only
  wire crate the gateway-only UI build uses without DuckDB.
- For the same reason two impls in the root crate changed shape:
  `impl From<AppError> for rmcp::ErrorData` is now `mcp::error::mcp_error`,
  and `Config::apply_process_limits` is the `bootstrap::ApplyProcessLimits`
  extension trait (import it to keep the method call).
- `rag-code-intel` registers its four tables with
  `rag_code_intel::register_schema_extensions` (called by
  `storage::register_schema_extensions`). A few of its tests drive the
  composed pipeline through a dev-dependency on `rag-mcp`; only `Store` from
  the shared store crate crosses that cycle.
- Test hooks of the store (`Store::lock`, `fts::test_fault`, FTS crash points,
  wiki write fault ops) are compiled under `cfg(any(test, feature =
  "testkit"))`; the root and code-intel `testkit` features enable the store's.
- `tests/it/architecture_test.rs` now checks the rules left inside a crate
  (domain modules do not reach `db`; app/edge/composition in the root crate;
  `mcp` and `http_api` apart). The crate graph enforces the rest, and the
  orphan rule keeps `impl Store` inside the store crate.

## Current violations (origin/main `facf533`)

Cycles:

- ~~`db → code_index`~~: resolved in Stage 2. `upsert_document_cas_locked` runs
  the registered `db::DocumentWriteHook`s; `code_index::source` registers its
  source-blob hook from `storage::install_store_hooks` (composition) and from
  the producers of `code_source_text` metadata.
- ~~`db ↔ graph`~~: resolved in Stage 2. `db/**` imports link parsing from
  `rag_mcp_markup` directly; `graph` no longer re-exports `db::graph::resolve`
  (callers use `db::graph::resolve::rebuild_document_graph`; the crate-root
  `rag_mcp::rebuild_document_graph` re-export is kept).
- ~~`db → external_db`~~: resolved in Stage 3 (DDL registered at composition;
  `ExternalDb` repository since Stage 2).
- `code_index ↔ external_db`: `src/external_db/write_check.rs:540` ↔
  `src/code_index/sql.rs:1405`.
- ~~`config →` upper layers~~: resolved in Stage 2. Provider enums live in
  `config::providers`, defaults and caps in `config::limits` (re-exported by
  `llm`, `rerank`, `maintain`, `query_transform`, `telemetry`, `db::search`);
  `impl EmbeddingIdentity for Config` moved to `db`; `apply_process_limits` is
  implemented in `bootstrap` (composition). The three process globals it sets
  remain globals (see INVENTORY.md, config row).
- ~~`models`/`error → duckdb`~~: resolved in Stage 2; the impls live in `src/db/sql_types.rs`.
- ~~`http_api → mcp`~~: removed in Stage 4 (MCP service in `mcp::http`, mounted by `main`; tool counts from `tool_spec`).
  Known remaining edge after Stage 2: `mod.rs` builds the `/mcp` streamable
  service from `RagServer`, and `ops.rs` reports `RagServer::registered_tool_count`
  and `mcp::surface::spine_tool_names`. Both name the MCP tool registry itself,
  so there is no neutral module to move them to until the Stage 4 app layer owns
  the registry and the bin mounts `/mcp`. Allow-listed in the architecture test.

Leaks across layers:

- ~~`Store` god object~~: resolved in Stage 3. Store keeps open/lock/tx and
  repository accessors (`store.documents()`, `store.chunks()`,
  `store.graph()`, `store.kg()`, `store.ops_log()`,
  `store.embedding_manifest()`, `store.wiki_index()`, `store.sync()`,
  `store.corpus_lane()`, `store.collections()`, `store.reembed_progress()`,
  `store.maintenance()`, `store.source_manifest()`, `store.recovery()`).
  15 forwarding methods stay in `db/store/ui_compat.rs` for rag-mcp-ui's
  direct-db reader until the UI crate moves (Stage 6).

- ~~`Store::lock` is `pub(crate)`; 19 `impl Store` blocks outside `src/db`~~:
  resolved in Stage 2. calibration, gate, analysis_tasks, external_db and
  code_index use repository types (`Calibrations`, `Gate`, `AnalysisTasks`,
  `ExternalDb`, `CodeIndex`) over `Store::with_conn` / `with_tx`; `Store::lock`
  is private to `db` in non-test builds (tests keep crate visibility to seed
  fixtures). `tests/it/architecture_test.rs` fails on a new `impl Store`
  outside `src/db`.
- No `app` layer: MCP tools and HTTP handlers each rebuild commands, limits and
  defaults (see "Known MCP/HTTP drifts" in PLAN.md).
- Environment reads outside `config`: `src/db/store.rs:1545,1552`,
  `src/embeddings/mod.rs:197`, `src/search_pack.rs:104`, `src/ops.rs:585`
  (left in Stage 2, reasons in INVENTORY.md; `watch.rs` and `mcp/analysis.rs`
  now read `Config`), `src/external_db/mod.rs` (6),
  `src/db/fts.rs` (3), `src/http_api/resources.rs` (3).
- Process-global state: the vector snapshot cache
  (`src/db/search.rs:982`, keyed by DB path) and the globals set by
  `apply_process_limits`.
