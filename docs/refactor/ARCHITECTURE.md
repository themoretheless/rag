# Architecture

Status: final, after the refactor (Stages 0–5 and 8 done; the egui and web
client stages 6 and 7 are not part of this branch). This is the single source
of truth for the **code** architecture: crates, layers, where each concern
lives, composition, background work and the schema. Product intent stays in
[`../ARCHITECTURE_VISION.md`](../ARCHITECTURE_VISION.md) and
[`../PRODUCT_PRINCIPLES.md`](../PRODUCT_PRINCIPLES.md); data-plane mechanics
(layers L0–L4, invariants) in
[`../ARCHITECTURE_NOTES.md`](../ARCHITECTURE_NOTES.md). The decision record is
[ADR 0001](../adr/0001-layering-and-crate-split.md); the history of the work is
[PLAN.md](PLAN.md), [INVENTORY.md](INVENTORY.md) and [BASELINE.md](BASELINE.md).

## Crate graph

```
rag-mcp-core      rag-mcp-markup          serde-only wire types; [[link]]/#tag parsing
      ↑                 ↑
rag-store-duckdb                          domain + store
      ↑
rag-code-intel                            code intelligence
      ↑
rag-mcp (root, src/)                      app, edges, composition, binaries

rag-mcp-ui (default feature direct-db) → rag-store-duckdb, rag-mcp-core, rag-mcp-markup
rag-mcp-ui --no-default-features       → rag-mcp-core, rag-mcp-markup (HTTP gateway only)
```

| crate | path | holds |
|---|---|---|
| `rag-mcp-core` | `crates/rag-mcp-core` | wire types shared with clients: graph view, search hits, project home, jobs, activity events, `Layer` / `NodeKind` |
| `rag-mcp-markup` | `crates/rag-mcp-markup` | pure wikilink / tag extraction and label normalization (server graph and UI renderer) |
| `rag-store-duckdb` | `crates/rag-store-duckdb` | domain modules (`models`, `error`, `config`, `util`, `graph`, `search_pack`, `tool_spec`) and the store (`db/**`) |
| `rag-code-intel` | `crates/rag-code-intel` | `code_index`, `external_db`, `gate`, `calibration`, `analysis_tasks` |
| `rag-mcp` | `.` (`src/`) | `app` use cases, domain workflows (ingest, wiki, maintain, retrieval, …), MCP and HTTP edges, composition, `src/bin/*` |
| `rag-mcp-ui` | `crates/rag-mcp-ui` | native egui client |

The root crate re-exports every moved module under its old path
(`rag_mcp::db`, `rag_mcp::models`, `rag_mcp::code_index`, …), so callers and
docs that name `crate::db::…` still resolve.

The domain modules live in the store crate, not in a crate of their own: the
store implements DuckDB traits for domain types (`ToSql` / `FromSql` for the
string-coded enums, `From<duckdb::Error> for AppError`, in
`crates/rag-store-duckdb/src/db/sql_types.rs`), and the orphan rule allows that
only in the crate that owns the types.

## Layers

```
domain   models, error, config (values), util, graph, search_pack, tool_spec   [rag-store-duckdb]
store    db/** (DuckDB)                                       → domain          [rag-store-duckdb]
intel    code_index, external_db, gate, calibration,
         analysis_tasks                                      → store, domain   [rag-code-intel]
app      src/app (use cases) and the workflows they call:
         retrieval, ingest, source_sync, wiki, maintain, eval,
         llm, embeddings, chunking, revisions, diary, jobs, …  → intel, store    [rag-mcp]
edge     src/mcp, src/http_api                               → app             [rag-mcp]
root     src/main.rs, src/bin/*, src/bootstrap.rs,
         src/storage, src/context.rs, src/background.rs      → everything      [rag-mcp]
```

| layer | owns | must not know about |
|---|---|---|
| domain | plain types (`Document`, `Chunk`, `SearchHit`), `AppError`, config value types, pure functions (link extraction, token packing), the `ToolSpec` table | transports, use cases |
| store | schema and migrations, SQL, the connection mutex, repositories, vector and FTS search, recovery/backup | code intelligence, use cases, transports |
| intel | code index, external DB metadata, gate, calibration, analysis tasks; reaches the store through repository types over `Store::with_conn` / `with_tx` | use cases, transports |
| app | one use case per operation with typed commands; normalization, limit clamps and defaults live here once | rmcp, axum |
| edge | MCP tools and HTTP handlers: decode → use case → encode in their own envelope | SQL, each other |
| root | argument parsing, configuration, wiring, background tasks | — |

Rules: depend only on your own layer or a lower one; never upward; `mcp` and
`http_api` never import each other. The crate graph enforces store → intel →
root. Inside a crate, `tests/it/architecture_test.rs` checks the rest (domain
modules do not reach `db`; app, edge and composition stay in the root crate;
`mcp` and `http_api` stay apart); its allow-list is empty. The orphan rule
keeps every `impl Store` inside the store crate.

Where the store must trigger work owned above it, it exposes a seam that the
root registers:

- `db::DocumentWriteHook` (`crates/rag-store-duckdb/src/db/write_hook.rs`): run
  by the document upsert; `code_index::source` registers the hook that moves
  `code_source_text` into the source-blob table.
- `db::schema::SchemaExtension` (`crates/rag-store-duckdb/src/db/schema.rs`):
  tables owned by higher layers (code intelligence, gate, calibration,
  analysis tasks, external DB) are registered by
  `rag_code_intel::register_schema_extensions` and ensured by their
  repositories (`Store::ensure_schema_extensions`).
- `db::EmbeddingIdentity` (`crates/rag-store-duckdb/src/db/embedding_identity.rs`):
  the store checks the embedding manifest without importing the provider
  configuration.

## Where each concern lives

| concern | location |
|---|---|
| store handle, open, lock, transactions | `crates/rag-store-duckdb/src/db/store.rs` (`Store::open`, `with_conn`, `with_tx`; `Store::lock` is `pub(in crate::db)` outside `cfg(any(test, feature = "testkit"))`) |
| repositories | `store.documents()`, `chunks()`, `graph()`, `kg()`, `ops_log()`, `embedding_manifest()`, `wiki_index()`, `sync()`, `corpus_lane()`, `collections()`, `reembed_progress()`, `maintenance()`, `source_manifest()`, `recovery()`; implementations under `crates/rag-store-duckdb/src/db/store/` and `db/*.rs` |
| methods kept for the UI's direct-db reader | `crates/rag-store-duckdb/src/db/store/ui_compat.rs` (forwarders) |
| schema, migrations, open-time repairs | `crates/rag-store-duckdb/src/db/schema.rs` (`SCHEMA_VERSION` 26) |
| vector snapshot cache | `VectorCache` in `crates/rag-store-duckdb/src/db/search.rs`, a field of `Store` shared by clones |
| lexical search, FTS state | `crates/rag-store-duckdb/src/db/fts.rs` |
| backup, verification, recovery bundles | `crates/rag-store-duckdb/src/db/recovery.rs`, `db/recovery/{bundle,import,publish}.rs` |
| SQL helpers, active-document filter | `crates/rag-store-duckdb/src/db/rows.rs`, `db/sql.rs` |
| configuration | `crates/rag-store-duckdb/src/config.rs` (`Config::from_env`, `validate`), `config/` (providers, limits) |
| MCP tool table (role, telemetry group, spine flag) | `crates/rag-store-duckdb/src/tool_spec.rs` |
| blocking helpers | `crates/rag-store-duckdb/src/util/blocking.rs` (`run_blocking`, `blocking_section`) |
| use cases | `src/app/{search,graph,wiki,ingest,admin}.rs`: `SearchUseCase` (search, `pack_context`, `multi_get`), `GraphUseCase` (neighbors), `WikiUseCase` (`WikiWriteMode::{Create, Upsert, Cas}`), `IngestUseCase`, `AdminUseCase` |
| retrieval pipeline | `src/retrieval.rs` (query transforms, multi-query, LLM-assisted search), `src/rerank`, `src/query_transform.rs` |
| ingest | `src/ingest.rs`, `src/document_indexer.rs`, `src/file_ingest.rs`, `src/web_ingest.rs`, `src/source_scan.rs`, `src/source_sync.rs` |
| wiki | `src/wiki/` (`write.rs` holds `write_page`, the one write path) |
| maintenance | `src/maintain/`; action whitelist declared once, typed params in `maintain/apply/params.rs`, `trait MaintenanceStep` in `maintain/apply/steps.rs`, prompts in `maintain/prompts.rs` |
| LLM seam | `src/llm/` (`ChatCompleter`, `ChatClient`) |
| embeddings | `src/embeddings/` |
| jobs | `src/jobs.rs` (registry, lifecycle, persistence) |
| status / doctor / health | `src/diagnostics.rs`, `src/ops.rs` |
| MCP edge | `src/mcp/` (`facade.rs` `RagServer`, one router per domain, `http.rs` streamable service, `error.rs` `mcp_error`) |
| HTTP edge | `src/http_api/` (`mod.rs` router and `HttpState`, one module per route cluster) |
| roles and auth | `src/access.rs`, `src/http_api/auth.rs` |
| telemetry, activity | `src/telemetry.rs`, `src/activity.rs` |
| store construction | `src/storage/mod.rs` (`open_store`, `open_configured`, hook and schema-extension registration) |
| composition | `src/main.rs`, `src/bootstrap.rs`, `src/context.rs`, `src/background.rs` |

### Caller-guarded writes

The atomic store writes `Documents::write_document_atomic`,
`write_source_document_atomic`, `merge_exact_duplicates_atomic`,
`apply_document_dispositions_atomic` and `Chunks::update_chunk_embeddings_atomic`
do not take the corpus mutation lane themselves; each carries a
`# Caller-guarded` doc section. Embedding writes run under
`corpus_lane().try_embedding_write_guard` (or inside a mutation scope); bulk
maintenance runs on a store from `try_corpus_mutation_scope` /
`with_corpus_mutation_guard`. Their callers use several guard kinds (shared
embedding guard, exclusive lease, sync lane), so the guard is not a
parameter.

## Composition root

`src/main.rs` (and each `src/bin/*` through `bootstrap::open_offline`):

1. reads the environment into `Config` (`Config::from_env`) and validates it;
   `bootstrap::ApplyProcessLimits::apply_process_limits` sets the three
   remaining process globals (below);
2. opens the store with `storage::open_configured` / `storage::open_store`,
   which installs the document write hook and the schema extensions before the
   first write;
3. builds the embedder and the chat client;
4. builds `context::AppContext { store, embedder, config: Arc<Config>, llm }`;
5. builds the edges from it (`RagServer::from_context`,
   `HttpState::from_context`); `main` builds the MCP streamable service
   (`src/mcp/http.rs`) and hands it to `http_api::serve_with_shutdown`, which
   mounts it at `/mcp` (so `http_api` never imports `mcp`);
6. starts background work from one `background::BackgroundTasks`.

## Background tasks

`BackgroundTasks` (`src/background.rs`) is built once in `main`. It owns every
long-running loop, their shared `CancellationToken` and the join handles;
`main` calls `shutdown` on every exit path, and loops wait with
`BackgroundTasks::pause` so cancellation wakes them.

| task name | started by |
|---|---|
| `auto-backup` | `ops::spawn_auto_backup` (`src/ops.rs`) |
| `fts-maintenance` | `diagnostics::spawn_fts_maintenance` (`src/diagnostics.rs`) |
| `auto-sync` | `RagServer::spawn_auto_sync` (`src/mcp/facade.rs`) |
| `rag-file-watch` (thread) | `RagServer::spawn_file_watcher` (`src/mcp/facade.rs`, `src/watch.rs`) |
| `sync-worker` | `http_api::sync` (`src/http_api/sync.rs`), when replication is configured |

## MCP and HTTP over `src/app`

Both edges decode their parameters into the same typed commands and call the
same use case; only the envelope differs. MCP returns the bare result and maps
errors with `mcp::error::mcp_error` (JSON-RPC `-32602` / `-32002` with the wire
code in `data.code`). HTTP wraps success as `{ok: true, …}` and errors as
`{ok: false, code, error}` with a status. Store work runs on the blocking pool
(`RagServer::blocking_tool` / `blocking`, `HttpState::blocking_response` /
`blocking`, all over `util::run_blocking`); `util::blocking_section`
(`block_in_place`) remains only where data is borrowed across the call or the
caller is synchronous ([BACKLOG.md](BACKLOG.md)). `tests/it/parity_test.rs`
pins the shared behaviour; [CHANGES.md](CHANGES.md) lists every behaviour that
changed.

Operations not yet behind a use case (most of the `RAG_TOOLS=full` surface,
recovery, maintenance) call their domain module directly from the edge; the
rule still holds because those modules are in the app layer.

## Schema v26

- `SCHEMA_VERSION` is 26. `chunks.embedding` is a native, nullable
  `FLOAT[]` in fresh and migrated databases alike (DuckDB cannot drop a
  `NOT NULL` while indexes exist, so one shape for both keeps quarantined
  chunks restorable from a recovery bundle); `embedding_json` is gone. Writers still serialise a
  vector to JSON for `embedding_digest` and bind it as `CAST(? AS FLOAT[])`;
  readers decode the Arrow list directly.
- Migration from v25 runs on the first `Store::open`: add a nullable
  `embedding FLOAT[]`, backfill from `embedding_json` in batches of 5 000 rows
  (one transaction each), drop and re-create the chunk indexes around the
  column drop, record the version last. An interrupted migration resumes on
  the next open. Core DDL runs only when the stored version differs; schema
  extensions only when the digest of their DDL is not yet recorded in `meta`.
  Cheap repairs run behind an `EXISTS` guard on every open.
- Quarantine: a row whose `embedding_json` cannot become a `FLOAT[]` is copied
  verbatim into `chunk_embedding_quarantine` (`chunk_id`, `document_id`,
  `raw_embedding_json`, `reason`, `quarantined_at`); the chunk keeps a NULL
  embedding, which vector readers skip, and its document is marked for
  re-embedding. Backup verification (`verify_embedding_contract` in
  `db/recovery/publish.rs`) accepts those NULLs and reports them as
  `quarantined_chunks`; any other NULL or wrong-length vector fails the
  embedding contract. Recovery bundles carry a quarantined chunk with
  `"embedding": []` plus a `quarantine` object (its quarantine row), and
  import restores both.
- Downgrade: a v26 database cannot be opened by a pre-v26 binary. Keep a
  verified backup taken before the upgrade if a rollback may be needed
  ([CHANGES.md](CHANGES.md)).
- Tests: `crates/rag-store-duckdb/src/db/schema/embedding_migration_tests.rs`
  (v25 fixture → v26 with bit-identical vectors and search results,
  interrupted backfill, quarantine).

## Process globals and environment reads left

Kept on purpose; each is in [BACKLOG.md](BACKLOG.md) with the change that
would remove it.

- Globals set by `apply_process_limits`: `MAX_LINKS`
  (`db/graph/resolve.rs`), the sync identity (`db/sync.rs`) and the telemetry
  call-log capacity (`src/telemetry.rs`).
- Environment reads outside `Config::from_env`: `RAG_DB_THREADS` /
  `RAG_DB_MEMORY_LIMIT` (`db/store.rs`, applied at open),
  `RAG_EMBED_CONCURRENCY` (`src/embeddings/mod.rs`), `RAG_TOKENIZER_JSON`
  (`search_pack.rs`), `RAG_WORKER_THREADS` (`src/ops.rs`, read before the
  runtime exists).
