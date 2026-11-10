# Plan: full refactor of rag (server, egui client, web client)

## Context
The user asked for an aggressive whole-repo refactor: SOLID, DRY, clean code, patterns, modularity, divide and conquer, project structure, system design, measured optimization. Three read-only inventories of `origin/main` (`facf533`, 149k LOC) found the debt below. Earlier docs (`docs/REFACTOR_PLAN.md`) claim several of these items are done; they are not.

Decisions from the user:
- **Crates:** fix boundaries inside the root crate first; at the end split out only `rag-store-duckdb` and `rag-code-intel`.
- **Allowed contract changes:** a DB schema migration; aligning MCP and HTTP behaviour; deleting dead code.
- **Clients:** structure and speed for egui and web. No mass i18n of strings.

Follows the `full-refactor` skill: behaviour stays the same except the three approved changes; green gates on every step; moves separate from logic edits; optimization only with before/after numbers.

## Main findings (evidence in the working files, see Stage 0)
- `Store` is a god object: 261 methods in 46 `impl Store` blocks, ~59 outside `db/` (code_index, analysis_tasks, gate, calibration, external_db) with raw SQL via `self.lock()`. `Store::lock` is `pub(crate)` (`src/db/store.rs:294`).
- Cycles: `db ↔ code_index` (`db/store.rs:1174`), `db ↔ graph`, `code_index ↔ external_db`, `config →` llm/rerank/maintain/db globals, `models`/`error → duckdb`, `http_api → mcp`.
- No shared use-case layer: 20+ MCP/HTTP pairs rebuild the same commands with drifted limits, defaults and error codes. Tool names live in 4–5 hand lists.
- Async: ~35 tools in `src/mcp/analysis.rs` run sync DuckDB/FS on the tokio worker. Seven different blocking helpers.
- Hot paths: the vector snapshot is built under the Store mutex (~4 s cold) in a process-global cache; embeddings are stored as JSON text; RRF clones every hit; the lex "fallback" re-runs the call that failed; `Config` (79 fields) is cloned per request; tool results are re-parsed from JSON up to 3 times.
- Write path: "mark changed + delete chunks" ×4, document INSERT ×2 (recovery import skips normalization). `layer`/`status` are strings; `'archived','tombstone'` ×26.
- maintain: action enum written 4 times, `serde_json::Value` params, 26-arm dispatch, 5 advertised actions not implemented. Ops-log entries are built by hand at 32 sites.
- egui: `GraphApp` ~125 fields, `draw_workspace` 799 lines, same `ViewMode` switch in 3 functions, per-frame string-keyed lookups per edge, HTTP helpers copied 2–4 times.
- web: full SVG and simulation rebuild on every change; endpoints fetched from 2–4 views with local types; formatters, debounce, polling duplicated; no lazy routes, no ESLint, loose tsconfig.
- Dead: `Storage` trait + Markdown backend (757 lines), `Minimap.svelte` (263), alias methods.

## Stage 0. Baseline and working files (no code changes)
Create `docs/refactor/`: `BASELINE.md`, `INVENTORY.md`, `ARCHITECTURE.md`, `PLAN.md`; ADRs in `docs/adr/`. Fill them from the three inventory reports.
- Baseline numbers: LOC per module, functions >80 lines (now 103+), `impl Store` count, `self.lock()` sites outside `db`, clean build time, test time, binary size, web bundle size.
- Add timing tests next to the existing ones in `src/db/search/tests.rs` for: cold/warm vector snapshot, hybrid search p50/p95 on a seeded corpus, document upsert, MCP `tools/call` overhead. Record "before".

## Stage 1. Foundation and safety net
- Move `tests/{file_ingest,query_transform,rerank}_test.rs` into `tests/it` (one DuckDB link).
- Expose `testkit` behind a `testkit` feature; delete the 8 `open_temp_store` and 15 `test_config` copies.
- Add MCP-vs-HTTP parity tests for search, pack_context, multi_get, wiki write, get_neighbors (they pin today's behaviour before Stage 4 aligns it).
- `[workspace.dependencies]`; `opt-level = 1` for deps in dev/test.
- Web: add ESLint (typescript-eslint strict + svelte), Prettier, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, `noUnusedLocals`; fix what they report; add to CI. Run Rust tests on Linux in CI too.

## Stage 2. Boundaries inside the root crate (moves, no logic change)
Target layering, one direction only:

```
domain   models, error, config types, util, chunking, graph::extract, search_pack
store    db/** (DuckDB)            → domain
intel    code_index, external_db, gate, calibration, analysis_tasks → store, domain
app      use cases: retrieval, ingest, source_sync, wiki, maintain, eval, llm, embeddings → store, intel, domain
edge     mcp, http_api             → app
bins     main, src/bin/*           → edge, app
```

- Break the cycles:
  - `db → code_index`: replace `code_index::source::persist` call in `upsert_document_cas_locked` with a `DocumentWriteHook` registered at composition.
  - `db ↔ graph`: `db/graph/resolve.rs` imports `rag_mcp_markup` directly; `graph/mod.rs` stops re-exporting `db`.
  - `config`: provider enums and defaults move to domain; `apply_process_limits` globals become fields passed at construction.
  - `models`/`error`: move the duckdb `ToSql`/`FromSql` and `From<duckdb::Error>` impls into `db`.
  - `http_api → mcp`: tool registry moves to app (Stage 4); the bin mounts `/mcp`.
- `impl Store` outside `db`: convert the ~59 methods to repository types that take a connection handle from `Store::with_conn`/`with_tx`. Then make `Store::lock` private to `db`.
- Move env reads at `db/store.rs:1545`, `embeddings/mod.rs:197`, `watch.rs:42`, `search_pack.rs:104`, `ops.rs:585`, `mcp/analysis.rs:183` into `Config`.
- Add an architecture test (a `tests/it` module that greps `use crate::` edges) to keep the layering.

## Stage 3. Store and write path (SOLID, DRY)
- Split `Store` by concern into repository types over the shared connection: `Documents`, `Chunks`, `GraphRepo`, `KgRepo`, `OpsLog`, `EmbeddingManifestRepo`, `WikiIndexRepo`, `SyncRepo`, `CorpusLane`, `Collections`, `ReembedProgress`. `Store` keeps open/lock/tx and accessors (`store.ops_log()`); old methods delegate during migration, then go.
- One `replace_chunks_locked` and one `insert_document_row_locked`, used by recovery import too (fixes the skipped normalization — a bug fix, separate commit with a test).
- Typed `Layer` and `DocStatus` on `Document`; one `ACTIVE_DOCUMENT_SQL`; an `in_placeholders(n)` helper (15 sites); shared row decoders in `db::rows`; one WHERE builder.
- `OpsLog::record(op, entity, payload)` builder replaces the 32 hand-built entries, with one failure policy (warn, never fail the mutation).
- `AppError::Timeout` and `AppError::Invalid` used instead of `AppError::config` for request errors.
- Delete: `Storage` trait and `storage/markdown/**`, `find_all_by_content_hash`, `all_chunks_with_embeddings`, duplicate `upsert_meta`/`make_snippet`/`tokenize`.
- Move inline test modules >300 lines into sibling `tests.rs`.

## Stage 4. Application layer and edges
- `src/app/` use-case modules with typed commands and results (`SearchUseCase`, `WikiUseCase`, `GraphUseCase`, `IngestUseCase`, `AdminUseCase`): parameter normalization, limit clamps, defaults live here once. MCP tools and HTTP handlers become thin adapters. Behaviour is aligned to one rule per use case (approved); every changed limit/default/error code is listed in `PLAN.md`.
- One wiki write entrypoint (`WikiWriteCommand { mode: Create|Upsert|Cas }`) instead of six.
- One `ToolSpec` table (name, access class, telemetry class, spine flag) that generates the lists in `access.rs`, `telemetry.rs`, `mcp/surface.rs`, `analysis_tasks.rs`.
- Blocking: two helpers only (`run_blocking` for DB/FS, `blocking_section` nowhere in new code); wrap all `mcp/analysis.rs` tools; macro/table for the six `db_*` tools.
- `Arc<Config>` in `HttpState` and `RagServer`; build the tool router once and share it; drop the legacy constructors.
- `http_api/feedback.rs` and `eval_ops.rs` call the search use case directly (no handler → bytes → JSON round trip); eval logic moves to `src/eval`; `bin/eval.rs` reuses production search.
- Background loops (auto-backup, FTS maintenance, auto-sync, watcher, sync worker) start from one `BackgroundTasks` in the composition root with a shared `CancellationToken`.
- maintain: `MaintenanceAction` as one serde-tagged enum with typed params; `trait MaintenanceStep`; rule table for `heuristic_plan`; remove the 5 unimplemented actions (approved); split the 448/346/313-line monoliths into phases. Prompts in one `prompts` module; one LLM trait.
- Bins: `clap` for argument parsing; all use `bootstrap::open_offline` or state why not.

## Stage 5. Optimization (each with before/after in BASELINE.md)
- Vector snapshot: cache owned by `Store`, built outside the mutex, `document_id` interned to `u32`, metadata fetched only for returned hits.
- Schema migration (approved): `chunks.embedding FLOAT[]` replaces `embedding_json`; backfill on open, gated by schema version; `migrate` runs DDL only when the version changes.
- Search: remove the duplicate lex fallback; RRF over indices instead of cloned hits; batch Neighbors context reads; batch query-variant embeddings; fold `ranking_fields` into hydration.
- Facade: parse each tool result once.
- N+1 fixes: `set_document_tags`, collections, KG writes, `health` chunkless count.
- Release profile: measure `lto = "fat"` vs thin; keep the faster/smaller one.

## Stage 6. egui client
- `GatewayApi { client, base }` replaces the copied HTTP helpers; `trait DocumentSource` with `HttpSource` and cfg `DbSource` replaces the `#[path]` swap and the disabled stub.
- Split `load.rs` into `cli`, `dto`, `http/*`, `snapshot`, `topology`. Move colour mapping out of `adapter` into `ui/theme`. Move identical DTOs (`RevisionItem` etc.) into `rag-mcp-core`; split core `lib.rs` by domain; add `RelType` enum.
- `GraphApp`: `Pending<T>` slots; per-workspace state structs with `apply(evt)` and `show(ui) -> Action`; `Workspace` dispatch replaces the three `ViewMode` switches; single `select_node`.
- Graph speed: positions in `Vec<Pos2>` by node index, edges carry indices, batched edge mesh, grid index for culling and hit testing, cached legend/tooltip/incident edges, `Arc<GraphView>` for Expand, parse placement metadata once. Measure frame time at 5000 nodes / 20000 edges before and after.

## Stage 7. web client
- `api/endpoints/*.ts` typed methods; all response types in `api/types.ts`; shared resources (`statusResource`, `agentsResource`) with deduplicated polling.
- `lib/format.ts`, `lib/async.ts` (`errorMessage`, `debounce`, `createRequestGate` with `AbortController`, `usePoll` that pauses on hidden tab); one route table.
- Split `SearchView`, `AgentsView`, `CorpusView` into store + child components; split `wiki.svelte.ts` into catalog, page/editor, prefs.
- `GraphCanvas`: d3 keyed data join, one simulation, debounced resize, no per-node blur filter; `$state.raw` for large arrays.
- Lazy routes, `manualChunks` for d3; delete `Minimap.svelte`; view tests for Search, Corpus, Agents. Record bundle size before/after.

## Stage 8. Crate split
After Stage 2–4 make it mechanical: extract `crates/rag-store-duckdb` (db) and `crates/rag-code-intel` (code_index, external_db, gate, calibration, analysis_tasks). `rag-mcp-ui --features direct-db` depends on the store crate only. Record build times.

## Stage 9. Wrap-up
Update `ARCHITECTURE.md`, ADRs, `INVENTORY.md` statuses; merge the three overlapping architecture docs; move code-intelligence contract docs under `docs/code-intelligence/`; fix the stale claims in `REFACTOR_PLAN.md`. Before/after table; residual backlog. Re-ingest edited docs via `ingest_file` and update the wiki page with CAS, per `CLAUDE.md`.

## Execution
- Branch `claude/optimistic-pascal-8x90fc`, restarted from `origin/main`. One PR per stage (draft), one intention per commit, merge only on the user's word.
- Stages 0–5 and 8 are sequential on the server. Stages 6 and 7 touch other directories and run in parallel worktree agents. Inside a stage, independent modules go to parallel agents; merges are sequential.
- Stop and ask when: a step needs a contract change beyond the three approved, a step touches more than ~30 files with logic edits, or tests fail for a reason outside the change.

## Verification
- Every commit: `cargo clippy --workspace --all-targets --locked -- -D warnings`, UI clippy with `--no-default-features`, `scripts/check-fmt-changed.sh origin/main`, `cargo test --workspace --locked` (env: `RAG_EMBEDDING_PROVIDER=mock RAG_EMBEDDING_DIMS=32 RAG_LLM_ENABLED=false`), eval gates (recall ≥ 0.9, MRR ≥ 0.75); web: `npm run check && npm run lint && npm test && npm run build`.
- Schema migration: test that opens a v-current fixture DB, migrates, and gets identical search results; recovery round trip.
- Parity tests (Stage 1) pass after Stage 4 with the listed intentional changes.
- End to end: start `rag-mcp` on a temp DB, ingest this repo's docs, run search over MCP and HTTP, open the web UI graph and wiki.
- An independent review agent after Stages 3, 4, 5 and at the end.

## Known MCP/HTTP drifts (to align in Stage 4)

Found and pinned by `tests/it/parity_test.rs` (each has a `// DRIFT:` comment
there). Where they agree today the test compares the two sides directly.
When Stage 4 aligns one on purpose, the matching assertion changes in the same
commit and the item moves to the list of intentional changes.

| # | operation | MCP today | HTTP today |
|---|---|---|---|
| 1 | all success responses | bare result (array / object) | wrapped: `ok: true` plus, for search, `mode`, `count`, `timings`, `items` |
| 2 | domain errors | JSON-RPC error: `-32602` invalid_params for Invalid/Config/Conflict/Forbidden, `-32002` for NotFound; wire code in `data.code` | HTTP status (400/404/409/403/503) and body `{ok:false, code, error}` |
| 3 | error message text | bare message (`invalid search mode 'bogus': …`) | `AppError` display form with a variant prefix (`config error: …`, `invalid request: …`) |
| 4 | request decode errors | `-32602` with no `data.code` | axum `422` with a plain-text body, no JSON envelope |
| 5 | CAS conflict on wiki update | `-32602` with `data.code = "CONFLICT"` (no distinct class) | `409` with `code: "CONFLICT"` |
| 6 | float scores in hits | f32 shortest form (`0.03226646`) | widened through `json!` to f64 (`0.03226646035909653`); same value as f32 |
| 7 | `multi_get` envelope | `{documents, missing}` | `{ok, items, missing}` |
| 8 | `multi_get` document shape | flat 7-field `DocumentDetail` (`id, uri, title, content, metadata_json, created_at, updated_at`) plus chunk fields | `{document: <full Document row>, chunks, chunks_total, chunks_truncated}` |
| 9 | wiki create on an existing slug | `write_wiki_page` overwrites (revision 2); there is no create-only tool | `POST /v1/wiki` is create-only → `409 CONFLICT`; `PUT /v1/wiki` upserts |
| 10 | wiki CAS update entrypoint | separate `update_wiki_page` tool (omitted kind/category/summary preserved) | same `PUT /v1/wiki` as create, `if_match_revision` / `if_match_etag` in the body |
| 11 | neighbors: parameter name | `node_id` | `seed` |
| 12 | neighbors: depth cap | clamped to 5 | clamped to 3 |
| 13 | neighbors: seed resolution | node id only | node id, then document id, then exact label |
| 14 | neighbors: unknown seed | success with an empty `{nodes:[], edges:[]}` | `404 NOT_FOUND` |
| 15 | neighbors: blank seed | success with the empty view | `400 INVALID_REQUEST` |
| 16 | `pack_context` | no `ok`; `trace` not available | `ok: true`; optional `trace` / `persist_trace` / `prior_trace` |

Agreements pinned by the same tests: search ranking, scores (as f32), hit
fields, default mode and default `top_k`, `top_k = 0` → no hits; pack budget
default (`max_context_tokens`), `max_tokens = 0` → empty pack; `multi_get`
order and `missing`; wiki create fields, revision 1 / `W/"1"`, CAS success,
stale write leaves the page unchanged, etag form of If-Match; neighbors default
depth 1 and the same nodes/edges; wire codes `INVALID_REQUEST`, `CONFLICT`,
`NOT_FOUND` on both sides.

## Stage 4 intentional changes

Envelopes stay transport-specific and are not drift: MCP returns the bare
use-case result (JSON-RPC error with `data.code` on failure); HTTP wraps it as
`{ok: true, …}` (`{ok: false, code, error}` with a status on failure). The same
holds for drifts 1, 2, 4 and 6 above and for the `multi_get` envelope key
(`documents` on MCP, `items` on HTTP). Error message text follows the same
convention (drift 3): MCP sends the bare message, HTTP the `AppError` display
form with its variant prefix.

1. **Error text (Stage 3, recorded here).** About 61 HTTP validation sites now
   answer `invalid request: …` or `timeout: …` instead of `config error: …`.
   Status codes and wire codes (`INVALID_REQUEST`) are unchanged.
2. **`get_neighbors` / `GET /v1/neighbors` (drifts 11–15)**, one rule in
   `app::GraphUseCase::neighbors`:
   - `node_id` and `seed` are aliases on both edges; MCP also accepts
     `project` (alias `wing`), HTTP also accepts `rel_types` (comma-separated).
   - Depth cap is 3 on both (MCP was 5).
   - The seed resolves as node id, document id, node URI, then exact label on
     both (MCP took a node id only).
   - Unknown seed → `NOT_FOUND` on both (MCP returned an empty view).
   - Blank seed → `INVALID_REQUEST` on both (MCP returned an empty view).
   - Without `rel_types` both walk the PKB node-kind/relation policy of the UI
     graph (MCP walked relations only); with `rel_types` both follow exactly
     those relations.
3. **`multi_get` document shape (drift 8).** One item shape on both:
   `{document: <full Document row>, chunks, chunks_total, chunks_truncated}`
   plus the earlier flat MCP fields `id, uri, title, content, metadata_json,
   created_at, updated_at` (and optional `source_file`,
   `source_download_path`), kept for MCP clients that read them. HTTP items
   gain those flat fields; MCP items gain `document`.
4. **`pack_context` (drift 16).** MCP accepts `include_trace`,
   `persist_trace` and `prior_trace` like HTTP and returns `trace` when asked.
5. **Wiki writes (drifts 5, 9, 10).** One entrypoint,
   `app::WikiUseCase::write(WikiWriteCommand { mode: Create | Upsert | Cas })`
   over `wiki::write_page`. MCP `write_wiki_page` → `Upsert` (new optional
   `create_only=true` → `Create`), MCP `update_wiki_page` → `Cas` (now also
   accepts a `wiki://` uri or document id in `slug`), HTTP `POST /v1/wiki` →
   `Create`, `PUT /v1/wiki` → `Upsert`. Changes: MCP `write_wiki_page` on an
   existing page now keeps omitted `kind`/`category`/`summary` (it reset kind
   to `wiki` and cleared category/summary); `update_wiki_page` and `PUT` on an
   existing page now apply a given `wing`/`room` (they ignored it). CAS
   conflict has one meaning on both: a stale `if_match_*` (or `Create` on an
   occupied slug) is `CONFLICT` and leaves the page unchanged. The removed
   transport entrypoints: `put_wiki_page`/`WikiPutCommand` (replaced by
   `write_page`); `update_wiki_page_cas` is now crate-internal.
   `write_wiki_page`/`write_wiki_page_command` stay as internal writers for
   maintenance, compile and review jobs (ops-log options), not transports.
   A blank `title` on an existing page: MCP `write_wiki_page` (Upsert) sets
   it to the slug, as before Stage 4 (`WikiPageRequest::blank_title_is_slug`);
   HTTP `PUT` and MCP `update_wiki_page` keep the current title, as before.
   `Cas` mode (MCP `update_wiki_page`) now applies the Upsert id/slug rule:
   an `id` that names another page than `slug` is `CONFLICT` and changes
   nothing (it used to take the id and ignore the slug). `update_wiki_page`
   rejects `create_only=true` with `INVALID_REQUEST` (it was silently
   ignored); `create_only` belongs to `write_wiki_page`.
6. **Eval / feedback replays.** `POST /v1/feedback/*` replays and online eval
   call the search use case in process instead of the `/v1/search` handler, so
   these internal searches no longer appear as `http search` rows in the call
   log, and a failed internal search surfaces the search error itself instead
   of `invalid request: search failed (<status>): <body>`.
7. **Maintenance whitelist.** The five advertised-but-unimplemented actions
   `consolidate_propose`, `resolve_stub`, `lint_and_fix_links`,
   `build_outline` and `merge_near_dup` are deleted (approved). A plan that
   names one is now rejected like any unknown action (LLM plans: moved to
   `rejected`; `apply_maintenance_plan`: `FORBIDDEN`) instead of being
   accepted and skipped at apply time. `heuristic_plan` no longer proposes
   `merge_near_dup` for near duplicates or `resolve_stub` for unresolved stubs
   (both still appear in `analyze_corpus`). The prompt lists the remaining 21
   names. `maintain_compress` keeps its own `merge_near_dup` report rows (a
   compress result label, not a plan action).
8. **Binary argument errors.** `ingest_file`, `db_repair`, `rag_gate` and
   `promote_dodo_projects` parse arguments with clap: `--help` prints on
   stdout, and unknown or missing arguments use clap's wording (for example
   `unexpected argument '--bogus' found`) with exit code 2. Flags and their
   meaning are unchanged; `rag_gate --mode` still defaults to `RAG_GATE_MODE`.
9. **Ingest and admin reads (`app::IngestUseCase`, `app::AdminUseCase`).**
   MCP `ingest_text` / `ingest_raw` / `add_drawer` / `ingest_file` /
   `ingest_url` and HTTP `POST /v1/ingest/url` call `IngestUseCase` (the URL
   fetch timeout is `RAG_INGEST_URL_TIMEOUT_SECS` on both, as before). MCP
   `get_taxonomy`, `list_wings`, `list_rooms`, `kg_query`, `kg_timeline`,
   `kg_stats`, `lint_wiki`, `reembed_status`, `llm_status` and the matching
   HTTP `GET /v1/taxonomy`, `/v1/wings`, `/v1/rooms`, `/v1/kg`,
   `/v1/kg/timeline`, `/v1/kg/stats`, `/v1/lint-wiki`, `/v1/reembed-status`,
   `/v1/llm-status` call `AdminUseCase`, whose store reads run on the
   blocking pool. One drift aligned: the `wing` filter of `list_rooms` /
   `GET /v1/rooms` is trimmed on both (HTTP passed `" alpha "` through and
   matched nothing). Blank KG filters already meant "no filter" on both (the
   store trims them); `GET /v1/kg` keeps its `limit`, MCP `kg_query` stays
   unpaged. Pinned by the admin tests at the end of `tests/it/parity_test.rs`.
10. **Binary arguments: `recovery`, `ingest_project`, `eval`.** All three
    parse with clap, flags and defaults unchanged (`ingest_project --wing
    projects --room default --max-bytes 524288`; `eval --root . --top-k 5
    --modes lex,vec,hybrid --warmup 0 --repeat 1`, `--golden` alias of
    `--dataset`; `recovery retention --keep 7`). `--help` prints on stdout
    with exit 0 (it printed on stderr). Unknown arguments, missing values and
    unparsable numbers use clap's wording (`unexpected argument '--bogus'
    found`, `invalid value 'x' for '--top-k <N>'`) with exit code 2 (it was
    1). Semantic checks keep their messages and exit 1 (`required: --root
    DIR`, `--top-k must be greater than zero`, `--checkpoint requires
    --settings-a and --settings-b`, …). `recovery`: subcommands are clap
    subcommands; a missing `--db`/`--out`/`--in`/`--dir`/`--backup` is
    clap's "required arguments were not provided" (exit 2) instead of
    `required --db PATH` (exit 1); an unknown subcommand is `unrecognized
    subcommand` (exit 2); `recovery` with no subcommand prints the help on
    stderr and exits 2 (it printed the usage and exited 0); arguments a
    subcommand does not define are rejected (they were ignored), except
    `inventory --keep`, still accepted and ignored.
