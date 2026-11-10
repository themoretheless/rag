# Target architecture

Status: target for Stages 2–8 of [PLAN.md](PLAN.md). The code does not follow
it yet; the "Current violations" section lists where. The decision record is
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
  impls live in `store`.
- `store` does not call `intel` or `app`. Where a write must trigger work owned
  above it (today: `code_index::source::persist` inside the document upsert),
  `store` exposes a hook trait (`DocumentWriteHook`) and the composition root
  registers the implementation.
- `config` holds values only. Provider enums and defaults it needs are domain
  types; process-wide knobs (`max_links_per_doc`, call-log capacity, sync
  identity) are passed at construction instead of being pushed into globals by
  `Config::apply_process_limits`.
- Environment variables are read in one place (`Config::from_env`).
- An architecture test in `tests/it` (Stage 2) greps `use crate::` edges and
  fails on an edge that breaks the rule.

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

Boundaries are fixed inside the root crate first (Stages 2–4). At the end
(Stage 8) only two crates are split out, because they have the slowest builds
and the clearest seams:

- `crates/rag-store-duckdb` — the `store` layer;
- `crates/rag-code-intel` — the `intel` layer.

`rag-mcp-core` (wire types) and `rag-mcp-markup` (link parsing) stay as they
are. `rag-mcp-ui --features direct-db` will depend on the store crate only.

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
- `db → external_db`: `src/db/schema.rs:401` runs `external_db::DDL`;
  `external_db` implements `Store` methods.
- `code_index ↔ external_db`: `src/external_db/write_check.rs:540` ↔
  `src/code_index/sql.rs:1405`.
- ~~`config →` upper layers~~: resolved in Stage 2. Provider enums live in
  `config::providers`, defaults and caps in `config::limits` (re-exported by
  `llm`, `rerank`, `maintain`, `query_transform`, `telemetry`, `db::search`);
  `impl EmbeddingIdentity for Config` moved to `db`; `apply_process_limits` is
  implemented in `bootstrap` (composition). The three process globals it sets
  remain globals (see INVENTORY.md, config row).
- ~~`models`/`error → duckdb`~~: resolved in Stage 2; the impls live in `src/db/sql_types.rs`.
- `http_api → mcp`: `src/http_api/mod.rs:68`, `src/http_api/ops.rs:18-19`.

Leaks across layers:

- `Store::lock` is `pub(crate)` (`src/db/store.rs:294`); 19 `impl Store` blocks
  with 40 methods and 33 `self.lock()` call sites live outside `src/db`
  (code_index, external_db, analysis_tasks, gate, calibration).
- No `app` layer: MCP tools and HTTP handlers each rebuild commands, limits and
  defaults (see "Known MCP/HTTP drifts" in PLAN.md).
- Environment reads outside `config`: `src/db/store.rs:1545,1552`,
  `src/embeddings/mod.rs:197`, `src/watch.rs:42-44`, `src/search_pack.rs:104`,
  `src/ops.rs:585`, `src/mcp/analysis.rs:183`, `src/external_db/mod.rs` (6),
  `src/db/fts.rs` (3), `src/http_api/resources.rs` (3).
- Process-global state: the vector snapshot cache
  (`src/db/search.rs:982`, keyed by DB path) and the globals set by
  `apply_process_limits`.
