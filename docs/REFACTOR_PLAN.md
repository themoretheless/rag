# Refactor plan: structure, architecture, SOLID / DRY

**Status:** executed on this branch except the items listed under "Not done, with the reason" (see [Progress](#progress)) · **Written:** 2026-10-11 · **Baseline:** branch
`claude/optimistic-pascal-8x90fc` (MCP facade split, `AppError::code()`,
`util::{blocking,text,time}` already done; see [`SOLID_DRY_AUDIT.md`](SOLID_DRY_AUDIT.md)).

**Binding constraints** (from [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) and the
audit guardrails): HTTP/MCP wire shapes stay stable; `Store` stays the DuckDB
façade and transaction owner; the `Storage` trait stays document-lifecycle only;
no broad trait over `Store`. Every slice below is one PR that keeps
`cargo clippy --workspace --all-targets -- -D warnings`,
`scripts/check-fmt-changed.sh` and `cargo test --workspace` green.

Sizes: **S** < 200 lines touched · **M** 200–1000 · **L** > 1000.

---

## Phase 0 — Correctness defects found by the audit

Small, independent fixes. Each ships with a regression test that fails before the
fix. These go first because the later refactors would otherwise move the bugs.

| # | Defect | Evidence | Fix | Size |
|---|--------|----------|-----|------|
| 0.1 | `prune_orphans` runs `BEGIN…COMMIT` through `execute_batch` on the shared connection; a mid-batch failure leaves the transaction open for every later caller, and FTS is marked dirty before the work starts | `src/db/store.rs:2160-2185` | Use `conn.transaction()`; mark dirty inside it | S |
| 0.2 | HTTP embedding-manifest report says `match: true` without checking `base_url` or the fingerprint; the canonical predicate is fail-closed | `src/http_api/admin.rs:136-152` vs `src/db/store.rs:4157` | Call `embedding_manifest_matches_config` | S |
| 0.3 | Lexical and vector legs build scope filters differently: vector trims values, lexical does not, so a padded `wing` filters one leg of hybrid search only | `src/db/fts.rs:683-714` vs `src/db/search.rs:1096-1122` | One `LexFilters::where_sql(doc_id_col)`; delete the test-only `sanitize_stemmer`/`make_snippet`/`search_lex_tf` copies in `search.rs:1194-1394` so tests hit production code | S |
| 0.4 | `maintain` `set_tags` creates tag nodes with its own key rule (`tag://{label}`, no `#` strip, no `tag_key`), so `tag://#Alpha` and `tag://alpha` can coexist; edge delete/insert run under separate locks | `src/maintain/apply.rs:930-986` vs `src/graph/resolve.rs:413` | Reuse the resolver's tag upsert; one transaction | S |
| 0.5 | Schema document writes are `upsert_document` + `let _ = rebuild_document_graph` + `let _ = append_ops_log`: not atomic, errors swallowed | `src/wiki/mod.rs:307-320, 386-395` | Route through `write_document_atomic` | S |
| 0.6 | DuckDB `Storage::upsert_document` skips graph rebuild and wiki journal that `write_document_atomic_with_manifest` performs | `src/db/store.rs:548-565` | Delegate to the atomic path; add a two-backend conformance test for the trait | S |
| 0.7 | MCP `data.code` is attached only for `Busy`; a CAS conflict is indistinguishable from bad input (CLAUDE.md documents the workaround) | `src/mcp/error.rs` | Always attach `{"code": err.code()}` (additive) | S |
| 0.8 | `From<reqwest::Error>` always yields `Embeddings`, so LLM HTTP failures report `EMBEDDINGS_ERROR` | `src/error.rs:66` | Map at the call site (`llm` → `Llm`) | S |
| 0.9 | HTTP `find_similar` drops `wing`/`room` | `src/http_api/retrieval.rs:587` vs `src/mcp/search.rs:225` | Pass them through | S |
| 0.10 | Environment-dependent tests: nanosecond vs microsecond timestamp compare; 5 s child-process budget breaks under `RUST_BACKTRACE=1`; 2 s semaphore timeouts | `src/maintain/compress.rs:1158`; `tests/http_auth_startup_test.rs:36`; `src/wiki/mod.rs:2998,3019,3324,3338,3415,3434` | Compare the store round-trip; drop or raise wall-clock budgets to ≥ 30 s | S |

---

## Phase 1 — Build and test infrastructure

Cheap, no production behavior change, and it makes every later phase faster.

1. **Dev profile and CI cache (S).** Root `Cargo.toml` has no `[profile]`.
   Add `[profile.dev] debug = "line-tables-only"` and
   `[profile.dev.package."*"] debug = false`. Add `Swatinem/rust-cache` to the
   `rust` job (it rebuilds bundled DuckDB from scratch every run).
2. **One integration-test binary (S).** Eight `tests/*.rs` files each link
   DuckDB. Merge into `tests/it/main.rs` with modules; keep
   `recovery_cli_test` separate if it must spawn binaries.
3. **Shared test kit (M).** `fn test_config` exists 11×, each a ~30-field
   `Config` literal (`src/{diary,ingest}.rs`, `src/maintain/{analyze,apply,
   compress,organize,refresh}.rs`, `src/mcp/facade/tests.rs`,
   `tests/{wiki,mempalace_parity,maintain}_test.rs`); `seed_doc` 4×, `fixture`
   3×, `open_store` 2×. `Config::for_tests` already exists (`config.rs:143`).
   Add `#[cfg(any(test, feature = "test-util"))] pub mod testkit` with
   `temp_store()`, `seed_doc()`, and use `Config::for_tests` everywhere.
   Acceptance: adding a `Config` field touches one file.
4. **Single FTS fault hook (S).** Six `#[cfg(test)]` magic-stemmer strings in
   production code (`apply:97`, `compress:189/191`, `refresh:285`,
   `source_sync:30/32`, `diagnostics:36`, `facade:54`) become one test hook in
   `db::fts`.
5. **Box large `WorkerEvt` payloads (S)** in `crates/rag-mcp-ui/src/worker.rs`
   (`LoadedGraph`, `OperationsSnapshot`, `DocumentBody`).

---

## Phase 2 — Application layer shared by HTTP and MCP

Biggest DRY win. About 25 operations exist in both transports; ~10 already share
a service, ~8 duplicate validation/defaults/clamping and already disagree.

1. **Parity tests first (S).** `http_api/admin.rs` and `http_api/graph.rs` have
   no tests. Add HTTP↔MCP parity tests for every operation below before moving
   it.
2. **`AppContext` (M).** `HttpState` (`http_api/mod.rs:79`) and `RagServer`
   both hold `{store, embedder, config}` plus their own LLM client lifetimes.
   One `AppContext { store, embedder, config, llm, jobs }` owned by both.
3. **`app::` commands (M, S per op).** Each takes raw `Option` params and owns
   `non_blank → default → clamp → run_blocking`; transports only map fields and
   response shape. Order:
   - `pack_context` (`retrieval.rs:418` / `search.rs:248`; MCP runs DuckDB on the executor)
   - `SearchCommand::from_request` (~28-line copy; MCP skips `non_blank`)
   - ops-log and diary reads (HTTP clamps 500/200, MCP does not; defaults 20 vs 10)
   - graph node / seed resolution (3 implementations; only HTTP checks URI)
   - list documents / list wiki pages
   - wiki upsert policy (slug/uri/id resolution lives only in `http_api/wiki.rs:208`)
   - `llm_status` (HTTP builds a `ChatClient` per request)
   - `multi_query_search` inline logic (`mcp/search.rs:79-140`) → `retrieval`
4. **No synchronous DuckDB on the async executor (S–M).** Falls out of 3; the
   remaining direct calls (MCP ingest 18, graph 15, wiki 22, kg 5, collections 5;
   HTTP `admin.rs`) are wrapped in `run_blocking`.
5. **Move workflows out of the transport (M).** `JobRegistry` and job lifecycle
   (`http_api/jobs.rs:50-575`) and the replication worker
   (`http_api/sync.rs:117-375`) move to `app`/`source_sync`, so MCP
   `sync_sources` can use jobs and cancellation.

**Decision needed:** aligning MCP diary/ops-log limits with HTTP adds a clamp to
MCP. That is a visible behavior change; it is either accepted or each transport
keeps its default through an explicit policy parameter.

---

## Phase 3 — Layering, configuration, registries

1. **Remove upward edges (S each).**
   - `config.rs:91,220,226` imports `mcp::ToolSurface` and `http_api::parse_bind` → move both into `config`.
   - `mcp/facade.rs:468,495` calls `http_api::record_mcp_tool` → move `activity` beside `telemetry`.
   - `http_api/retrieval.rs:22` imports `mcp::tools::PackHitParams` → move to `models`.
   - `http_api/ops.rs:181` builds a whole `RagServer` per request to count tools → static count.
   - Acceptance: a test or `cargo-modules` check that `db`/domain never import `mcp`/`http_api`.
2. **Configuration read once (M).** ~30 `RAG_*` variables are read outside
   `Config` in 14 files (`ops.rs` 7, `http_api/sync.rs` 3, `mcp/facade.rs`,
   `llm/chat.rs`, `graph/resolve.rs:151`, `db/sync.rs:65` via
   `SyncIdentity::from_env()` inside `wiki::write_wiki_page_command`, …) and
   `main.rs:235 env_flag` duplicates `config.rs:415`. Add `OpsConfig`,
   `SyncConfig`, `HttpConfig` sub-structs parsed in `Config::from_env`.
3. **Narrow config slices (S–M).** ~55 parameters take the whole `&Config`;
   `Store` reads four embedding fields. Introduce `EmbeddingIdentity` and pass it
   to `db` so persistence no longer depends on `Config`.
4. **Binary bootstrap (S).** `ingest_file.rs:101`, `ingest_project.rs:159`,
   `eval.rs:572,761` repeat open-store → manifest → provider and the tracing
   init, and skip `main`'s `validate_runtime_paths`/`ensure_fts`/backend
   selection. Add `bootstrap::{open_offline, init_tracing}`.
5. **One tool/route registry (M).** Tool names are maintained in four places
   (`#[tool]` attrs, `access.rs:40` roles, `telemetry.rs:249` groups with no
   completeness guard, `mcp/surface.rs`) and HTTP roles in a 100-line table
   (`http_api/auth.rs:136-235`). Each domain router exports a const
   `(name, role, group)` table; a test asserts every registered tool and route
   has an entry.
6. **Split `mcp/tools.rs` (M).** 1,539 lines of ~95 `*Params` structs; move each
   next to its router and factor a `#[serde(flatten)] ScopeFilter`
   (wing/room/layer/source_file/top_k). Schema snapshot test guards wire shape.
7. **`AppError::Invalid` (M).** `AppError::config` is used for input validation
   at 309 call sites, so clients read "config error: slug required". Add
   `Invalid` with the same `INVALID_REQUEST` code; migrate transports first.
   Replace the 11 direct `McpError::*` constructions and 21 stringly
   `AppError::db(e.to_string())` sites.

---

## Phase 4 — Persistence

1. **Query and transaction helpers (M).** ~94 hand-written
   `while let Some(row) = rows.next()?` loops, 16 `SELECT {DOCUMENT_SELECT}`
   sites (plus a verbatim copy at `recovery.rs:1044`), ~21 open-tx/commit
   blocks. Add `Store::with_tx`, `query_vec`, `query_documents`.
2. **One chunk/document write path (M).** Embedding-JSON serialization and
   chunk ownership checks appear 4× (`store.rs:530,596,785,2432`), "mark FTS
   dirty + delete chunks" 3×. Recovery import inserts rows itself
   (`recovery.rs:1262-1270`) with `to_rfc3339()` instead of `format_ts`,
   `CURRENT_TIMESTAMP` chunk times and no layer/kind/status/hash normalization.
   Add `PreparedChunks` and shared `insert_document_row_locked` /
   `replace_chunks_locked`; add a timestamp round-trip test.
3. **Row decoders in `db::rows` (S).** Graph, KG, collections and store
   decoders are scattered; `SearchHit` is built field-by-field 3×.
4. **Stop handing out raw connections (L, stepwise).** `Store::lock()` has ~41
   production call sites in 12 files outside `db` (~70 raw SQL statements):
   `maintain/analyze` 14 statements, `apply`, `compress`, `refresh`, `review`,
   `http_api/{knowledge,feedback,labels,jobs}`, `retrieval.rs:345-445`
   (duplicates `count_chunks_for_document`), `graph/resolve.rs` (~500 lines of
   SQL). Move each into a `db::*` repository, then narrow `lock` to
   `pub(super)`. `payload_kv` keeps its SQL by design. Reverse edges
   (`store.rs:660 → review`, `store.rs:3230 → graph::resolve`,
   `search.rs:576 → search_pack`) go with it.
5. **Extract Store slices (M each).** 185 `impl Store` methods (116 in
   `store.rs`). Candidates with clean seams: `CorpusLane` (13 methods, tokio
   lock policy), `EmbeddingManifestRepo` (8, removes `db → Config`),
   `WikiIndexRepo` (12; scoring/markdown rendering at `store.rs:3717-3985` moves
   to `wiki::index`), `OpsLog` (9). `Store` keeps delegating methods. Move the
   2,400 lines of store tests to `db/store/tests.rs`.
6. **Split `db/recovery.rs` (M)** into `recovery/{bundle,import,publish}.rs`
   along existing boundaries (codec 203-900, store ops 901-1431, filesystem
   publication 1432-2090).

---

## Phase 5 — Domain modules

1. **One FTS-finalization-failure path (S).** `apply.rs:211-240` and
   `refresh.rs:919-935` hand-roll `mark_fts_dirty_for_retry` without an audit
   row; others use `store.record_fts_finalization_failure`.
2. **Maintenance run skeleton (S–M).** `apply`, `refresh`, `compress`,
   `organize` each repeat dry-run → guard → work → FTS finalize → report →
   ops-log → write back `ops_log_id`/`seq` (`apply.rs:203-282`,
   `refresh.rs:899-926`, `compress.rs:607-645`, `organize.rs:478-513`), with
   silently different audit policy. Add `OpsLogEntry` builder (19 literals
   today) and `maintain::finish_run(.., AuditPolicy::{Required, BestEffort})`.
3. **LLM output parsing (S).** Four JSON parsers, `strip_json_fences` 2× plus an
   inline copy, `truncate` 3× (`llm/chat.rs`, `maintain/organize.rs`,
   `maintain/plan.rs`). Add `llm::json::parse_model_json<T>`.
4. **LLM seam (M).** `ChatClient` is HTTP transport and also owns domain
   prompts and parsing (`chat.rs:585-700`); organize builds its prompt inline.
   Add `trait ChatCompleter` + `complete_json<T>`; move prompts to
   `wiki::compile` and `maintain/prompts.rs`. Enables in-process fake-LLM tests
   for organize and plan (today only wiki tests fake it via a raw TCP listener).
5. **Split `wiki/mod.rs` (M, mechanical).** 4,831 lines, 47 % tests. Target:
   `wiki/{schema,write,compile,consolidate,stale,file_answer,lint,index}.rs`
   with `mod.rs` re-exports; `llm` imports confined to compile/consolidate/stale.
   Break up `write_wiki_page_synced_command` (265 lines). Update the stale
   "~3.1k LOC" in `ARCHITECTURE_VISION.md:448`.
6. **Typed codes and metadata (S–M).** `LintIssue` and `AnalysisIssue` use
   string `code`/`severity` → enums with `serde(rename)` (wire unchanged).
   `metadata_json` is parsed ad hoc in 9 places → `Document::metadata()`.
7. **Split `SourceSyncService::sync_with_control` (M)** (`source_sync.rs:310-689`,
   ~380 lines) into scan / prepare / flush / prune / finalize methods.
8. **Typed document fields (L, later).** `Document.layer/kind/status` and
   `GraphNode.kind` are strings while `NodeKind`/`RelType` enums exist; 97
   literal status strings. Do after the repositories in Phase 4 so the change is
   local to `db::rows`.
9. **Report DTOs to their services (S).** `StatusReport`, `LlmStatusReport`,
   `DoctorReport`, `WakeUpReport` leave `models.rs`.

---

## Phase 6 — Clients and crate boundaries

1. **`rag-mcp-core` crate (M).** Wire DTOs and graph models, serde only. The UI
   currently depends on the whole server crate (DuckDB bundled, axum, rmcp,
   tokio full) for `GraphView`, `WikiPageMetaFilter` and `Store::open_read_only`.
   Make `--db` an optional `direct-db` UI feature.
2. **Delete UI DTO copies (M).** Same name in both crates: `SearchHit`,
   `JobSnapshot`, `SyncJobRequest`, `ActivityEvent`, `ProjectHome`,
   `RevisionLineChange`, `RestoreRevisionResult`; renamed twins:
   `RevisionItem/Page`, `RevisionDiff`, `JobReport`, `WikiPageMeta`. Their
   `serde(default)` leniency hides drift.
3. **`UiBackend` trait (M).** `load.rs` has parallel `_http` / `_db` functions
   (wiki list, document, load, neighbors, backlinks) and reopens the read-only
   store per request; the worker branches on `LoadSource` in 5 places.
4. **Decompose `GraphApp` (L, incremental).** ~118 fields; a 480-line
   `drain_worker_events` match; ~1,700-line `ui()`; `NoSourceAction::Connect`
   pasted 5×. Per-workspace state structs with `apply(evt)` and
   `show(ui) -> Action`.
5. **Generated web types (M).** `web-svelte/src/api/types.ts` hand-copies 17
   interfaces and ~25 inline types drift (`SyncView` declares both
   `document_count` and `documents`); 12 components bypass `api/client.ts`.
   Generate TypeScript from the server's schemars/serde types and route every
   call through the client.
6. **Further crate split (L, last).** The server is one ~75k-line crate; after
   1, carve out `db` and `http_api` crates for incremental build parallelism.

---

## Progress

Status of each item on this branch. "Done" means implemented with a test that
fails without the change, unless the item is a pure move.

**Phase 0 — all ten done.** Three more defects surfaced while fixing them and
are fixed too:

- HTTP-MCP built a new `RagServer` per request (manifest check, reqwest/TLS
  client, tool router). It now clones one prototype per request.
- MCP `compile_source` built its own `ChatClient` with provider `Custom` and
  env-derived limits (600 s / 2048 tokens instead of the configured values),
  which is the wrong protocol for Anthropic. It now uses the configured client.
- MCP `get_backlinks` resolved labels with "first match" instead of the
  document-first preference of `Store::resolve_node`; it now calls it.

**Phase 1**

- 1.1 done. Bundled DuckDB build output fell from 3.9 GB to 0.9 GB per profile.
- 1.2 done: `tests/it/main.rs` links DuckDB once instead of eight times.
- 1.3 done: every `test_config` copy collapses onto `Config::for_tests()`;
  `testkit::SeedDoc` backs the four `seed_doc` helpers.
- 1.4 done: `db::fts::test_fault` replaces six per-module magic stemmers.
- 1.5 dropped: clippy's `large_enum_variant` no longer fires, events move once
  through a channel, and boxing would touch the 480-line event match for no
  measurable gain.

**Phase 2**

- Shared use cases: `retrieval::pack_context`, `SearchRequest::into_command`,
  `Store::resolve_seed` / `resolve_node`, `retrieval::find_similar` scope.
- Ops-log pages are bounded by `Store` (`MAX_OPS_LOG_PAGE`) for every caller.
  MCP `diary_read` was already bounded by `diary::diary_read`, so the planned
  diary decision was unnecessary.
- `HttpState` caches the chat client; `llm_status` and the eval judge reuse it.
- Corpus-scale MCP calls run on the blocking pool via `RagServer::blocking`.
  A dispatcher-wide `block_in_place` was rejected: it would make every tool
  call uncancellable.
- Items still open at this point were finished later; see "Final round".
  Originally open: `AppContext` (2.2), list-documents/list-wiki and wiki-upsert policy,
  `multi_query_search` into `retrieval`, moving `JobRegistry` (2.5), parity
  tests for `admin.rs`/`graph.rs` (2.1).

**Phase 3**

- 3.1 done: no `config`/domain → transport imports remain (`ToolSurface`,
  `parse_bind` in `config`; `crate::activity`; `PackHitParams` in
  `search_pack`; static tool count).
- 3.2 (first pass): `ChatClient` no longer reads `RAG_LLM_*`. The remaining reads
  are one-time start-up settings of their subsystem (backup, sync, PID file).
- 3.4 done: `bootstrap::{open_offline, init_tracing}`; offline tools now also
  honour `RAG_STORAGE_BACKEND`.
- 3.5 done: HTTP roles are explicit for every catalogued route
  (`ADMIN_HTTP_ROUTES` plus read/write classification); a test fails when a
  route in `/v1/routes` falls through to the fail-closed default. Telemetry
  groups are explicit for every registered tool, with a
  completeness test. The HTTP route-role table is unchanged.
- 3.6 done without `ScopeFilter`: `mcp/tools/<domain>.rs` per router, re-exported
  from `mcp::tools`. Flattening shared filter fields would change the generated
  JSON schemas, so it waits for a wire-schema review.
- 3.3 and 3.7 were finished later; see below.

**Phase 5**

- 5.1 done (`record_fts_finalization_failure` everywhere).
- 5.2 done: `maintain::audit::record_run` + `OpsLogEntry::new` builder; the
  compress best-effort audit policy stays explicit at its call site.
- 5.3 done: `llm::json` owns fence stripping, embedded-object parsing and
  excerpt truncation for compile, consolidate, organize and plan.
- 5.6 done: `Document::{metadata, metadata_lenient, metadata_field}` replace
  ad hoc `metadata_json` parsing in revisions, memory lifecycle, wiki write and
  stale, the indexer and knowledge properties. Lint and analysis findings use
  `LintCode` / `AnalysisCode` (named codes plus `Other`, serialized as the same
  strings, so client-supplied reports with custom codes still parse).

- 5.5 done: `wiki/{schema,write,compile,consolidate,stale,file_answer,lint,
  index}.rs`; `wiki/mod.rs` keeps layer constants, shared DTOs and re-exports.

**Phase 4**

- 4.2 done: recovery import writes chunks through `insert_chunks_locked`
  and formats timestamps like every other path (microsecond round-trip test).
  Embedding JSON and chunk-ownership checks have one implementation
  (`store::{embedding_json, owned_embedding_json}`) used by atomic writes,
  sync writes, chunk replacement and recovery import. Import restores revision
  and timestamps verbatim on purpose.
- 4.5 done: `Store` methods are grouped in `db/store/{documents,chunks,health,
  revisions,schema_diary,corpus_lane,manifest,ops_log,wiki_index}.rs`, leaving
  `store.rs` with the write/transaction core (~1.7k production lines). The
  2.5k lines of store tests moved to `db/store/tests.rs`
  (same for wiki, recovery, graph, search, source_sync and maintain/apply).
- 4.6 done: `db/recovery/{bundle,import,publish}.rs`.

**Phase 6**

- 6.1 done: `crates/rag-mcp-core` (serde-only graph types and PKB defaults,
  re-exported by `rag_mcp::models`). The UI's DuckDB code sits behind the
  default `direct-db` feature; `--no-default-features` links no server crate,
  and CI keeps that build compiling.
- 6.2 done: every UI copy of a server DTO is gone. `ActivityEvent`,
  `SearchHit`, `ProjectHome`/`SourceRootSummary` and `JobSnapshot` with its
  progress/report types live in `rag-mcp-core`. Tolerance moved into the shared
  types: `JobStatus` and `SourceSyncPhase` have a `#[serde(other)] Unknown`
  variant and optional fields default, so an older UI still reads a newer
  server (core test `job_snapshot_tolerates_newer_servers`).
- Earlier: `SyncJobRequest`, `RestoreRevisionResult` and
  `RevisionLineChange` were identical on both sides and now live in the core
  crate. The remaining same-name pairs differ on purpose (the UI reads
  timestamps as strings and a subset of fields to tolerate server drift), so
  merging them is a wire-compatibility decision, not a refactor.
- 6.5: the audit overstated the web client; every component already goes
  through `api/client.ts` (auth, errors). Generated types remain open.

**Later additions**

- 2.1: handler tests for `http_api/admin.rs` (ops-log filters, 404) and
  `http_api/graph.rs` (unknown/blank seed), plus `Store::resolve_seed` order.
- 2.3: `retrieval::multi_query_search` owns the use case and embeds every
  rewrite in one provider call.
- 3.7 done: `AppError::Invalid` for caller-input validation in transports and
  retrieval (same `INVALID_REQUEST` code and HTTP 400).
- 4.1 done: `db::rows::{query_vec, for_each_row, visit_rows, try_visit_rows}`
  replace all 78 hand-written production row loops (`try_visit_rows` covers
  `break` / `continue` / early results via `ControlFlow`); `Store::with_tx`
  replaces 17 transaction blocks.
- 4.3 done: one lexical-hit row decoder for the BM25 and TF paths; the vector
  path builds hits from its snapshot, and the remaining builder is test-only.
- 4.5: corpus lane, embedding manifest, ops log and wiki index live in
  `db/store/{corpus_lane,manifest,ops_log,wiki_index}.rs`; `store.rs` is 3.5k
  lines (from 4.2k plus 2.5k of tests).
- 5.4 done: `llm::ChatCompleter`; compile/consolidate prompts live in `wiki`;
  an in-process scripted LLM tests the maintenance planner.
- 5.6: severity is `IssueSeverity`; finding codes were typed later (see above).
- 5.7: `sync_with_control` repeated flush/cancel branches collapse into two
  local macros (377 to about 280 lines).
- Tests no longer leak a DuckDB directory per call (`testkit::{scratch_dir,
  temp_store}`); a full run left about 110 directories and filled the disk.

**Final round**

- 2.2 done: `context::AppContext` (store, embedder, config, chat client) is
  built once in `main`; `RagServer::from_context`, `HttpState::from_context`
  and `http_api::serve` take it, so start-up no longer re-parses `Config` or
  builds a second embedder.
- 2.5 done: `JobRegistry`, lifecycle and persistence live in `src/jobs.rs`;
  `http_api/jobs.rs` keeps routes and handlers.
- 2.x wiki upsert done: `wiki::put_wiki_page` owns slug/uri/id resolution,
  the id-vs-URI check, create-only guards and the create-or-CAS choice; the
  HTTP handler is a thin adapter. MCP keeps its separate create/update tools.
- 3.3 done: `db::EmbeddingIdentity` replaces `Config` in the store's manifest
  functions; `db` no longer imports `config`.
- 3.2 done: credentials (`RAG_*_API_KEY`, `RAG_PRIMARY_TOKEN`) are
  `config::Secret`, whose `Debug` prints `***`, so a logged `Config` no longer
  reveals them. `RAG_HTTP_ALLOWED_HOSTS`, `RAG_AUTO_SYNC_*`, `RAG_EVAL_HISTORY`,
  `RAG_NODE_NAME`, `RAG_SYNC_INTERVAL_SECS`, `RAG_STORAGE_BACKEND`,
  `RAG_VAULT_PATH`, `RAG_PID_FILE`, `RAG_WAL_WARN_BYTES`,
  `RAG_STARTUP_WARN_MS`, `RAG_AUTO_BACKUP_*`, `RAG_MAX_LINKS_PER_DOC`,
  `RAG_CALL_LOG_CAPACITY`, `RAG_NODE_ID`, `RAG_PRIMARY_URL` and
  `RAG_INGEST_ROOTS` and `RAG_PRIMARY_TOKEN` are read by
  `Config::from_env`.
- 4.4 done: `Store::lock` is `pub(in crate::db)` outside tests. Former callers
  use `db::{payload_kv, analytics, knowledge, chunk_text, review,
  graph::resolve}` or `Store` methods (`reindex_fts`, `chunks_generation`,
  `set_document_tags`, `checkpoint`). `payload_kv::with_session` gives
  read-modify-write callers one lock without exposing the connection.
- 5.8 done: `Document::kind` is `models::DocumentKind` (named kinds plus
  `Other` for agent-written kinds, stored and sent as the same string, with
  DuckDB `ToSql`/`FromSql`), so no valid page is rejected.
  `rag_mcp_core::Layer` and `rag_mcp_core::NodeKind` name the layer and
  graph-node values; production comparisons in server and UI use them, and the
  server's former `models::NodeKind` copy is gone. Stored values and JSON stay
  strings.
- 5.9 done: status/doctor/LLM-status reports live in `diagnostics`,
  `WakeUpReport` in `diary`; the crate root still re-exports them.
- 6.3 done as inherent methods: `LoadSource::{wiki_catalog, document,
  backlinks, save_wiki}` hold the HTTP-vs-direct-db choice once. A trait was
  not needed because there are exactly two closed variants.
- 6.4 done: `GraphApp` is split into `app/{mod,events,frame,graph,revisions,
  wiki}.rs`.
- 6.6 done: `crates/rag-mcp-markup` holds the pure `[[wikilink]]`/`#tag`
  parser and label normalization (`graph::{extract, normalize}` re-export it).
  The native UI renders wiki links with it instead of its own naive `[[...]]`
  scan, so links inside code spans and `![[embeds]]` are no longer drawn as
  links and `[[Page#Heading]]` resolves to `Page`, as in the server graph.
- 6.5 done as a contract test: `tests/it/web_types_contract_test.rs` round-trips
  every field of the TS `GraphNode`, `GraphEdge`, `SearchHit` and
  `SearchExplanation` through the Rust types. It found three `GraphNode` fields
  the server never sends and a dead `layer === 'wiki'` check in the inspector;
  both are fixed. No codegen dependency was added.

**Independent audit after the plan**

A separate read-only audit of the code (not of this document) found further
items; all are fixed:

- Performance: search recency and field boosts did two full `get_document`
  loads per hit; one batched ranking-field query replaces them.
  `attach_context` reloads no document's chunks twice; MCP graph search
  resolves each hit document's node once.
- Duplication: `util::{first_line, first_prose_line}`, `embeddings::embed_exact`,
  `http_api::error::search_response_error`, `OPS_LOG_COLUMNS`,
  `prefer_document_node`, one `near_duplicate_pairs`, merged scoped/UI graph
  neighbor and edge queries (with the relation count derived from
  `pkb_rel_types`), and shared UI gateway helpers (`gateway::{client,
  join_url, send_json}`). The dead `wiki::render_index_markdown` copy is gone.
- Layering: `db::review` no longer calls into `wiki`
  (`wiki::create_review_proposal` builds the snapshots).
- Configuration: `RAG_CHECKPOINT_ON_START`, `RAG_HTTP_ONLY`, `RAG_MCP_HTTP`
  and the HTTP role tokens are read by `Config`.
- Vocabulary: `Layer::is` ignores ASCII case and replaces ten
  `eq_ignore_ascii_case` checks, so raw immutability has one rule. The UI's
  display lookups keep string matches because they map open vocabularies
  (`l0`, `code`, `concept`) with a fallback.
- Size: `db/graph.rs` → `graph/{export,tunnels}.rs`, `maintain/apply.rs` →
  `apply/{metadata,index,llm,dedupe}.rs`, store helpers →
  `store/{wiki_meta,wiki_index,ops_log,manifest}.rs`, UI `frame.rs` →
  `app/{panels,workspace}.rs`. No production file exceeds 1.5k lines.

**Verification re-audit**

A second independent audit confirmed layering, configuration, row loops, raw
locks and file sizes clean, and found the following; all are fixed:

- Layer case: known layers are stored lowercase (`Layer::canonicalize` on
  the one document insert path, and on meta updates), and `Layer::parse`
  ignores case like `Layer::is`, so SQL filters and Rust checks agree on a
  page written as `Wiki`.
- N+1 queries: `Store::content_hashes` (one query for retrieval source
  versions), `Store::find_by_id_or_uri` (the uri lookup runs only on an id
  miss), `Store::nodes_in_wing` (tunnel wing filter),
  `Store::document_pairs_by_relation` (stale-wiki `related` scan instead of a
  neighborhood walk per raw document), set lookups in wiki lint, counting
  instead of loading wiki pages in organize, and a per-run document cache in
  compress pair planning.
- Literals: graph resolve and maintenance use `Layer`/`NodeKind`/
  `DocumentKind` instead of `"wiki"`/`"raw"`/`"tag"` strings; the UI reads a
  node's layer from parsed metadata instead of a substring match.
- Duplicates: one sync kind check, shared UI `ui::text::{truncate,
  short_timestamp}` (the three copies disagreed on length and format).
- Kept as is: the four UI graph walks (stable-order cap, depth map, depth
  rings, depth limit with availability filter) share only the BFS skeleton;
  a common walker would need hooks for each variant and different key types.

**Third audit**

A third independent audit confirmed the earlier fixes and found, all fixed:
the stale-wiki source-version loop now resolves sources from the raw/wiki
pages already in memory (store lookup only on a miss); the UI's four URL
encoders are one `gateway::percent_encode`; `util::truncate_chars` replaces
the telemetry and search-pack copies and the UI excerpt/timestamp helpers
reuse `ui::text`; recovery import parses timestamps through
`util::parse_db_timestamp` (still falling back to now); the `graph_stats` doc
comment is back on its function. `llm::json` keeps its own excerpt rule
(max characters plus `…`) on purpose for error messages.

**Not done, with the reason**

Nothing: every planned item above is done.

## Performance round

A read-only hot-path audit after the refactor found 20 runtime issues. Each was
checked against the code, and then fixed or kept with a recorded reason.

**Search**
- Context expansion reads chunk bodies without embedding JSON. Plain searches
  fetch section labels for all hits in one metadata query. Previously every
  search loaded and parsed every embedding of every hit's document.
- Vector hits hydrate content in `IN (...)` batches, where they used one point
  query per candidate (pools reach 4096).
- Scoped vector search reuses a current global snapshot, and caches the last
  scoped snapshot, so refill rounds and multi-query fan-out stop re-parsing
  embeddings.
- Snippets are built only for returned hits. The TF fallback keeps a top-k heap.
  MMR is O(k·n) with deterministic ties. Field boosts tokenize the query once.
- Bug fixed: `make_snippet` panicked when lowercasing changed byte lengths
  (`İ`). A regression test covers it.

**Graph and wiki**
- `backlinks` is one query (was N+1). The HTTP backlinks handler reads once and
  groups through maps (was O(rows·edges·nodes)).
- `get_graph_view` filters and caps in SQL and joins edges to the selected nodes.
- `lint_wiki` reads document refs, nodes and edges once, and derives stub
  backlinks in memory (was one query per stub).
- Stale detection borrows the raw corpus instead of cloning it twice.
- A graph rebuild memoizes link resolution per document. The resolution
  lookups use cached prepared statements.
- The wiki index rebuild is one transaction: one commit, and no window in which
  the index is empty.

**Writes and ingest**
- Chunk and edge inserts use 64-row `VALUES` statements. This measured 3–4.5×
  faster than per-row execution (200–2000 chunks, 768-dim vectors).
- Ingest counts chunks with `COUNT(*)`.
- `reembed_all` embeds up to 128 chunks per provider call across documents. A
  failing group is retried per document.
- Source sync keeps its batch across unchanged files; it used to flush on every
  skipped file.
- The Ollama embedder remembers a working legacy endpoint.
- Bug fixed: `near_duplicate_pairs` returns the most similar pairs. It used to
  stop at the first 100 matches in scan order.
- The release profile uses thin LTO and one codegen unit. `cargo build --release`
  was verified with it.

**Measured** (debug build, mock embeddings, `repository-v1`, the same corpus
for the old and new binaries; recall and MRR are identical):

| Mode | Mean before → after | p95 before → after |
|------|---------------------|--------------------|
| lex | 213 → 140 ms (−34%) | 270 → 162 ms |
| hybrid | 399 → 172 ms (−57%) | 492 → 208 ms |

**Kept on purpose, with the reason**
- FTS rebuild timing: the BM25 index is already rebuilt lazily at the first
  search after any number of writes, so a burst of ingests pays once. Serving
  stale or TF results instead would break read-your-writes.
- `prepare_cached` everywhere: measured at about 10% per point query, because
  execution dominates. It is applied only to the per-link resolution lookups.
- Stored `content_hash` in stale detection: writers may supply their own hash
  (see `merge_exact_dup`), so the check keeps hashing the body.
- `load_vector_documents` is read live on each query, because title, status
  and placement changes do not advance the chunk generation. The cost of
  that read was later cut; see "Last item".

### Performance re-audit

An independent review checked the round's diff and looked for remaining hot
spots.

**Regression found and fixed:** the scoped vector-snapshot cache was keyed only
on the chunk generation. Metadata-only writes do not advance that generation:
placement, archive and restore. A document moved into a scope was therefore
missing from that scope's vector results until the next chunk write. The
cache now records which documents were in scope when it was built, and
rebuilds when a document joins the scope. A regression test fails without the
fix.

**Further fixes**
- Orphan checks (`find_orphans`, `prune_orphan_stubs`, `prune_orphan_tags`)
  used a correlated `source_id = n.id OR target_id = n.id`, which DuckDB runs
  as a nested loop. They now anti-join against `db::graph::INCIDENT_NODE_IDS`.
  Measured 77 s → 35 ms on 20k nodes / 50k edges (debug build).
- `Neighbors` context expansion reads only the `index ± radius` window.
- Wiki review resolves a page's cited sources on demand, where it loaded every
  raw document with its body.
- `reembed_all` reads only the revision as its CAS token.
- Source sync commits manifest refreshes for unchanged files in one transaction
  per 256 files, retrying per file if that fails.
- Organize suggestions skip document bodies and read first-chunk embeddings in
  one query.

**Closing pass:** the items held back above are now fixed too.
- `compiled_link_exists` was confirmed slow. It is the predicate behind status,
  lint and catalog `compiled=false` paging. It nested a `LIMIT 1` scalar
  subquery per raw document: more than 190 s, then spilled until the disk
  filled, on 15k raw documents / 60k edges. One correlated equality runs as a
  hash semi-join in 150 ms.
- Source-manifest root load, run at every sync start: `NOT EXISTS (… OR …)`
  became two equality anti-joins (exactly equivalent).
- Doctor repair embeds missing documents through
  `IngestService::ingest_many`: up to 64 chunks per provider call, each
  document committed on its own, with a per-document retry when a group fails.
- `find_exact_duplicates` is one query for the top groups and their members.
- `lint_wiki` reads nodes without metadata and edges without context text.

**No change needed**
- FTS rebuild timing is already the cheapest correct policy:
  - Source sync and doctor repair rebuild once when they finish.
  - Single ingests are batched into one rebuild at the next search.
  - Serving stale or TF results would break read-your-writes.
- Two predicates keep their `OR` because they are bounded: the export
  companion lookup runs for one node matched by equality, and the
  source-dedupe member counts run under a `LIMIT`.
- Behaviour notes from the review that are accepted:
  - Autocommit `insert_chunks` / `insert_graph_edges` now fail per 64-row batch
    rather than per row.
  - Compress plans from the strongest duplicate pairs.

### Final audit

A last independent audit of HEAD found no remaining per-item query loops,
problem subqueries or per-row commits in the SQL layer. It did find
synchronous DuckDB work running on the async executor, plus three smaller
costs. All are fixed:

- **Executor stalls.**
  - Search and multi-query retrieval, including lazy BM25 and vector
    rebuilds, run on the blocking pool. The corpus read guard moves into the
    task.
  - `IngestService` extracts, prepares and commits off the executor; only
    embedding stays async.
  - Local wiki writes and `ingest_raw` read and commit on the blocking pool.
  - The remaining handler-level store calls go through
    `util::blocking_section` (`block_in_place` on the multi-thread runtime,
    inline elsewhere). That covers HTTP status, doctor, health, ready,
    checkpoint, admin, sync, feedback and review, the synchronous MCP tools,
    and the store calls inside async MCP tools and diary writes.
- **Vector snapshot after a write.** The global snapshot is refreshed
  incrementally. A DuckDB-side `hash(embedding_json)` listing decides which
  vectors to reuse, and only new or changed rows are parsed: 4.39 s → 379 ms
  on 20k chunks × 768 dims after adding one document (debug build). A test
  checks it against a full rebuild.
- **Replication** embeds every page of a push or pull batch in one provider
  call (`PrecomputedEmbedder`) and still commits per event.
- **Stale detection** hashes each cited source once and borrows instead of
  cloning.
- **Native UI wiki view.** Link keys are cached per catalog change, and
  resolving a link is one hash probe instead of a per-frame
  O(links × pages) scan. Parsed `[[link]]` segments are cached per line in
  egui's frame cache, so a line is parsed once while it stays on screen.

### Verification audit

A further independent audit checked documented claims against the code and
looked for anything significant left. One claim was false: `GET /v1/agents`
still ran on the executor. It and every finding are now fixed:

- **BM25 refresh is incremental.**
  - The index (`fts_main_chunks.{docs,terms,dict,stats}`) is patched for the
    chunks added or removed since it was built, using the extension's own
    `tokenize` macro, stopword table and stemmer.
  - Chunk text is immutable, so the id diff between `chunks` and `docs` is
    the full change set.
  - It falls back to the full rebuild when half the corpus changed, the
    stemmer changed, or anything fails.
  - A test compares dict, document lengths, postings, stats and BM25 scores
    with a full rebuild, and checks that the patch path actually ran.
  - Measured 19 s → 0.35 s after a 15-chunk change on 20k chunks (debug
    build). This supersedes the earlier "FTS timing needs no change" note.
- **Re-ingest reuses vectors.**
  - Each chunk whose text is unchanged takes the document's stored vector,
    which is exact because the embedding input is the chunk text under one
    manifest.
  - An identical re-ingest makes no provider call; an edit embeds only the
    changed chunks.
- **Snapshot refresh lists digests.** Chunk writers store
  `chunks.embedding_digest`, existing rows are backfilled at migration, and
  the refresh lists that column instead of hashing all embedding JSON. The
  id-only shortcut was rejected because ids can be reused with new vectors.
- **Wiki review** reads the raw corpus without bodies and fetches only the
  cited ones.
- **Polled endpoints.**
  - `/health` integrity counts read a body only for documents that lack
    chunks.
  - `/v1/agents` runs in `blocking_section` and reads diary activity without
    bodies.
  - `/v1/kg?limit=` returns a page with the full count; the Agents view asks
    for 80.
- **Native UI sidebar.**
  - Search text and category groups are indexed once per catalog change.
  - Without summaries, rows are uniform and only visible rows are laid out.
  - Categories fold with a header row.

### Second verification audit

A second independent audit confirmed the incremental BM25 patch, the
embedding digests, the `/health` and `/v1/agents` changes and the sidebar
index. It found one UI bug and one remaining gap; both are fixed, along with
its smaller items:

- **Bug, native UI.** Saving a wiki page re-sorted `wiki_pages` without
  bumping its revision, so the sidebar index and link keys went stale after a
  rename. The save path now bumps it; every mutation site does.
- **Scoped vector search after a write.** Project-scoped searches used to
  re-parse every embedding in scope. They now refresh incrementally, reusing
  vectors by chunk id and digest. Sources are tried in this order: the same
  scope's last snapshot, the global snapshot, then another scope's.
  - A test checks the scoped refresh from both a scoped and a global
    snapshot against a scoped rebuild.
- **No buffer copy on refresh.** Snapshot vectors are shared per chunk
  (`Arc<[f32]>`), so a refresh never copies unchanged vectors: 327 → 234 ms on
  20k × 768 (debug), against 4.4 s for a full build.
- **Vector reuse everywhere a page is rewritten.**
  `document_indexer::reusing_document_vectors` covers wiki writes,
  `ingest_raw`, `update_document` and revision restore, as well as the ingest
  path.
- **BM25 query.** It scores chunk ids first and reads content and titles for
  the top k rows. A third audit questioned whether this saves anything; a
  benchmark found fetching the top rows by join and by `IN` cost the same
  (about 12 ms for 50 rows of 20k). DuckDB already avoids scanning every body
  here, so this is a clarity change, not a measured speedup.

Still left on purpose: the egui document and wiki readers lay out the open
document each repainted frame. That cost grows with one document's length,
not with the corpus, and is egui's normal immediate-mode cost.

### Third verification audit

A third independent audit found no correctness bugs in the new code. Its one
remaining item and its smaller notes are handled:

- **Representative and first-chunk embeddings** (organize, near-duplicate
  pairs). Each document's first chunk is now picked from narrow columns with
  `arg_min(id, {chunk_index, id})`, and only those chunks' `embedding_json` is
  joined in. Measured 123 → 63 ms for 1000 documents. The old correlated
  `ORDER BY … LIMIT 1` was decorrelated by DuckDB, not pathological as feared.
- **Recovery import** keeps bundle chunk ids. It now sets a force-rebuild
  flag, so the next FTS refresh rebuilds fully instead of patching by id. A
  test checks the flag on a same-id, new-text chunk and that it clears.
- A misplaced doc comment in `document_indexer` is restored to
  `PreparedDocumentChunks`.

### Closing verification

The last independent check confirmed that the first-chunk SQL and the FTS
force-rebuild flag are correct, and that no significant optimization remains
on hot or steady-state paths. It named one corpus-scaled admin path, now
fixed:

- **Recovery bundle export** reads all chunks in one ordered scan grouped in
  Rust. It used to run a query per document, up to the 10k-document cap.
- **Recovery import** looks up documents by id and by uri through two
  statements prepared once. It used to re-prepare an `id = ? OR uri = ?`
  query per item.
- The FTS force-rebuild flag is written back only while it is set.

Verdict of the independent verification: no significant optimization remains.

### Last item

A final independent audit checked every item kept on purpose against the
code, and found each one justified. It also flagged a cost the earlier
reasoning had not measured: an unscoped vector search listed the id, title and
uri of every active document on each query. That happens again on each refill
round and for each multi-query sub-query. It is fixed:

- Vector scoring checks scope by id only:
  - Searches filtered by a field (wing, room, layer, kind, uri, source file or
    document) read the ids in scope.
  - Unscoped searches read only the archived and tombstoned ids, and exclude
    them.
  - Searches that include archived documents read nothing.
- Titles and uris are read only for the returned hits, in the same batched
  `IN` lookup as their content.
- Measured on 83k documents, 10% of them archived (debug build): 390–520 ms →
  33–36 ms per query. A test covers the exclusion set and `include_archived`.

A further independent check of that change found one regression and two more
costs, all fixed:

- **Regression: orphan chunks.** An exclusion set admits chunks whose
  document row is missing. Such a chunk could take a top-k slot, and was then
  dropped at hydration, so the search returned fewer hits. Now, when a hit's
  document is missing, the search adds the orphan chunks' document ids to the
  exclusion set and ranks again. A test puts an orphan chunk above every real
  one.
- **Layer or kind filters only** (for example `layer=raw`) still listed every
  matching id. One count over the filtered columns now chooses the smaller
  side, the matching ids or their complement. A NULL column counts as outside
  the scope, as it does in SQL.
  - Measured on 83k documents: `layer=raw` 335–412 ms → 106–110 ms.
  - `layer=wiki` (5% of documents) stays about the same, at 90–110 ms.
  - A test covers both sides and a NULL layer.
- **`/v1/status` polling.** The web Sync view polls it every 10 s, and each
  poll ran the layer-health scan, including the compile-debt predicate, under
  the store lock. `GET /v1/status?summary=true` returns only the row counts,
  schema version and database path, and the Sync view uses it. Without
  `summary` the response is unchanged.

A last independent check of those fixes found no correctness bugs. It did
find one more polled cost, now fixed, and a refinement:

- **`/health` polling.** The web shell polls `/health` every 15 s per tab.
  Each poll ran the integrity anti-joins under the store lock: chunks,
  documents, nodes and edges, about 30–60 ms in a release build at 80k
  documents. The shell never reads those numbers.
  - `GET /health?summary=true` skips them, and the shell uses it.
  - Plain `/health`, external monitors and `/v1/doctor` still report
    integrity.
- **Orphan chunks** stay in the database-wide vector snapshot, and query-time
  filtering handles them. An attempt to drop them by joining `documents` at
  build time was reverted after review. That snapshot is cached by chunk
  generation, and creating a document row does not bump the generation, so
  chunks that gained a document later would have stayed invisible. A test
  covers that case. While orphan chunks persist, an unscoped search whose top
  results reach one pays one anti-join over chunks and a second ranking pass.
  Normal deletes remove chunks, so orphans come only from integrity damage,
  which `/health` and the doctor report and repair.
- `summary` accepts `1`, `true`, `yes`, `on` or a bare `?summary`, so a probe
  sending those forms does not get a 400.

### Completeness audit at the final head

An independent audit of the whole branch found no correctness bugs and one
gap in the executor work: the replication cycle still ran DuckDB on async
workers. That cycle runs every `RAG_SYNC_INTERVAL_SECS`, 5 s by default. It
is fixed:

- On the primary, `POST /v1/sync/register` and `GET /v1/sync/pull` run in
  `blocking_section`.
- The replica's `sync_once` reads only its own pull cursor, through the new
  `Store::sync_pull_cursor`, in `blocking_section`. It used to call the full
  `sync_status`: a `MAX` over `sync_changes`, an outbox count and a node
  listing.
- The same class on demand: `GET /v1/diary` and the MCP tools
  `collection_get`, `link_nodes`, `follow_tunnels` and `read_log`.
- A mechanical sweep of every async MCP tool and HTTP handler body found no
  store call outside a blocking section, except the non-blocking `try_write`
  corpus guards. That sweep did not follow helper functions; see "Helper
  functions" below.

Kept: `query_with_index` reads and scores the whole `wiki_index` catalog on
each call. That cost grows with the number of wiki pages, not with the raw
corpus, and the rows are short.

A further completeness audit found two more costs, plus executor gaps on the
admin paths. All are fixed:

- **Source-sync job progress.** Background sync jobs (`POST /v1/jobs/sync`)
  wrote their progress snapshot to DuckDB on every callback, about 2–3
  autocommits per file. That is 20–30k commits on an incremental re-sync of
  10k mostly unchanged files, and it undid the batching for skipped files.
  - The in-memory snapshot still updates on every callback.
  - The persisted copy, read only after a restart, is written at most once
    per second. Every status change is still written at once.
  - Job writes and deletes run in `blocking_section`. A test covers both
    rules.
  - Writers clone a snapshot, release the jobs lock, then persist, so a
    progress write cloned before a concurrent cancel could land after it.
    That race predates this branch. Writes are now serialized, and each
    stores the job's newest in-memory state, so the persisted row never goes
    back. A test persists a stale clone after a cancel. A write for a job already
    pruned from the map is skipped, so a deleted row is not brought back.
- **Manifest checks in search.** Vector and hybrid search, and the MCP and
  ingest write paths, ran the embedding-manifest checks on the async
  executor before handing off to the blocking pool. A lazy BM25 rebuild
  holding the store lock could therefore park every concurrent vector search
  on a worker. The checks now run in `blocking_section`.
- **Maintenance workflows.** `maintain_refresh`, `reembed_all`,
  `maintain_organize` and the synchronous plan steps run their store work in
  `blocking_section`. That covers the graph and wiki-index rebuilds, paged
  loads and commits, heuristic suggestions, refiles and status changes.
  Only embedding and LLM calls await.

**Helper functions.** The next audit followed handlers into the helpers they
call and found store work still running on the async workers. All of it now
runs in `blocking_section`, leaving only embedding and LLM awaits outside:

- `IngestService::update_document`, behind MCP `update_document_meta`, and
  `reembed_document`.
- `RevisionService::restore`, behind `POST /v1/revisions/restore`.
- The wiki CAS path:
  - `get_wiki_page` wraps its own lookups, so every caller is covered,
    including `update_wiki_page_cas`.
  - The `put_wiki_page` lookup.
  - The manifest checks in `ingest_raw`.
- `refresh_stale_wiki`: the corpus-wide `find_stale_wiki` read and its
  ops-log write.
- `compile_source` and `consolidate`: document resolution, schema, revision
  snapshot, source linking and ops-log writes.
- Maintenance plans:
  - the `MergeExactDup`, `DropTombstones` and `Vacuum`/`Checkpoint` steps;
  - the per-step `log_action` write;
  - `record_run`, which every maintenance workflow shares;
  - the generation reads and the FTS failure record.
- `DiagnosticsService::repair`: doctor snapshots, the missing-chunk listing,
  `prune_orphans` and `checkpoint`.

A call-graph-following audit of that head found no bugs, and one remaining
helper on a regular path: `Store::try_embedding_write_guard` read the
embedding manifest on the async worker. Every wiki write and revision
restore takes that guard. Its manifest reads now run in `blocking_section`.
The same pass closed the rare paths it listed as minor:

- The `db::payload_kv` store helpers run in `blocking_section`. These cover
  retrieval traces, eval runs, eval prompts and background jobs.
- The trace path's content-hash lookup.
- `record_fts_finalization_failure`, which wraps itself.
- The maintenance dry-run counts and inventory.
- The library-only `apply_consolidate_proposal` lookup.

What stays on async workers is in-memory work, non-blocking `try_*` lane
guards, and microsecond path canonicalization.

The final call-graph audit at this head found no bugs and no significant
remaining item (verdict: complete). It noted two things that are kept on
purpose:

- `/v1/agents` runs two `LIMIT 500` top-N reads: recent `ops_log` rows,
  sorted by sequence and time, and recent diary documents without their
  bodies.
- `/v1/sync/status` runs a `MAX` over `sync_changes`, a count of unsent
  outbox rows and a small node listing.

The web Sync view polls both every 10 s, and the web shell also polls
`/v1/agents` every 15 s in each tab. Both run in `blocking_section`, off
the async workers. The ops-log and replication reads grow with operation and
replication history.

The diary read filters `documents` on `kind` without an index, so it scans
the kind, status and creation-time columns of every document. It used to
load the full row projection for the 500 newest entries: 115–155 ms on 83k
documents with about 2,000 diary entries (debug build). It now reads only
`id`, room, wing, `metadata_json` and creation time, and resolves the agent
name with the same Rust code as the full entries. That takes 54–62 ms. A test checks that
it resolves agent names exactly as the full entries do. The `LIMIT` bounds
the rows returned. The Agents view also polls `/v1/calls` every 5 s; that endpoint is
an in-memory ring and does not touch the store.

A check of the final doc found one per-frame cost that does grow with the
data, now fixed. The native graph seed picker scanned the whole loaded graph
every frame while its field had focus. That graph holds up to
`UI_GRAPH_EXPORT_MAX_NODES` (100,000) nodes, or is uncapped for a snapshot
file. Its matches are now cached per graph revision and query, so the scan
runs once per keystroke.

The audit of that fix found two more per-frame scans of the same kind, now
cached:

- The empty-graph banner's topology stats counted node kinds and edge types
  over the whole loaded graph each frame. They are now computed once per
  graph load.
- The graph toolbar's "open wiki page" check scanned the wiki catalog each
  frame while a node was selected. It is now memoized per node document id,
  node uri and catalog revision.

Graph size by lens: Neighborhood and seeded views draw at most 300 nodes and
2000 edges. Without a seed, the other five lenses draw the whole loaded graph.

Verdict: no correctness bugs, and no remaining significant cost on hot or
polled paths. These costs remain, each bounded and kept on purpose:

- The live per-query read of a vector search's scope, measured above.
- The first search after a write burst pays the incremental refreshes, on
  the blocking pool:
  - The vector snapshot lists every chunk's identity and digest: 234 ms on
    20k chunks.
  - The BM25 patch anti-joins `chunks` against the index: 0.35 s on 20k
    chunks.
  - Both are measured above in a debug build, against 4.4 s and 19 s for
    the full rebuilds they replace. A chunk change log keyed by generation
    would make them delta-only, but that would add a write to every chunk
    commit.
- Native UI per-frame work:
  - egui lays out the open document, which grows with one document's
    length.
  - The activity grid draws up to 200 rows.
  - The Sync workspace repaints at about 30 fps while it is open.
  - Graph painting, and the selected node's detail panel (incident edges,
    sorted), visit every drawn node and edge. In the unseeded overview
    lenses that is the whole loaded graph, up to `UI_GRAPH_EXPORT_MAX_NODES`.
    Painting culls off-screen items and switches to a simpler mode above
    5,000 nodes. The detail panel's scan is the same order as painting, so
    caching it would save only a constant factor.

## Not planned

- A broad trait over `Store`, or full multi-backend `Storage` parity: the audit
  keeps both out on purpose; only narrow seams that enable fakes
  (`ChatCompleter`, ops-log sink) are added.
- Wire renames (`wing` → project, tool names) or new default tools.

## Baseline to track

| Measure | Start | Now | Target |
|---------|-------|-----|--------|
| `Store::lock()` call sites outside `db` (production) | ~41 | 0 | `payload_kv` only |
| Operations implemented in both transports without a shared service | ~8 | 0 | 0 |
| `fn test_config` bodies not built on `Config::for_tests()` | 11 | 0 | 0 |
| Integration-test binaries linking DuckDB | 8 | 1 | 1–2 |
| `RAG_*` env reads outside `config.rs` | ~30 | 0 | 0 |
| Upward imports from `config`/domain into `mcp`/`http_api` | 5 | 0 | 0 |
| Largest production file (without tests) | ~4.2k lines (`db/store.rs`) | ~1.5k | < 2k |
| Largest UI file (`app.rs`) | 4.6k lines | 1.3k (`load.rs`, `app/mod.rs`) | — |
| UI copies of server DTOs | 11 | 0 (all shared via `rag-mcp-core`) | 0 |
