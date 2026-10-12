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
- 3.2 partial: `ChatClient` no longer reads `RAG_LLM_*`. The remaining reads
  are one-time start-up settings of their subsystem (backup, sync, PID file).
- 3.4 done: `bootstrap::{open_offline, init_tracing}`; offline tools now also
  honour `RAG_STORAGE_BACKEND`.
- 3.5 partial: telemetry groups are explicit for every registered tool, with a
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
- 5.6 partial: `Document::metadata_field` for lenient reads.

- 5.5 done: `wiki/{schema,write,compile,consolidate,stale,file_answer,lint,
  index}.rs`; `wiki/mod.rs` keeps layer constants, shared DTOs and re-exports.

**Phase 4**

- 4.2 partial: recovery import writes chunks through `insert_chunks_locked`
  and formats timestamps like every other path (microsecond round-trip test).
  It still restores revision and timestamps verbatim on purpose.
- 4.5 partial: the 2.5k lines of store tests moved to `db/store/tests.rs`
  (same for wiki, recovery, graph, search, source_sync and maintain/apply).
- 4.6 done: `db/recovery/{bundle,import,publish}.rs`.

**Phase 6**

- 6.1 done: `crates/rag-mcp-core` (serde-only graph types and PKB defaults,
  re-exported by `rag_mcp::models`). The UI's DuckDB code sits behind the
  default `direct-db` feature; `--no-default-features` links no server crate,
  and CI keeps that build compiling.
- 6.2 partial: `SyncJobRequest`, `RestoreRevisionResult` and
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
- 4.3 partial: one lexical-hit row decoder for the BM25 and TF paths.
- 4.5: corpus lane, embedding manifest, ops log and wiki index live in
  `db/store/{corpus_lane,manifest,ops_log,wiki_index}.rs`; `store.rs` is 3.5k
  lines (from 4.2k plus 2.5k of tests).
- 5.4 done: `llm::ChatCompleter`; compile/consolidate prompts live in `wiki`;
  an in-process scripted LLM tests the maintenance planner.
- 5.6: severity is `IssueSeverity`; finding codes stay an open string
  vocabulary.
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
- 3.2: `RAG_HTTP_ALLOWED_HOSTS` and `RAG_AUTO_SYNC_*` are read by
  `Config::from_env`.
- 4.4 done: `Store::lock` is `pub(in crate::db)` outside tests. Former callers
  use `db::{payload_kv, analytics, knowledge, chunk_text, review,
  graph::resolve}` or `Store` methods (`reindex_fts`, `chunks_generation`,
  `set_document_tags`, `checkpoint`). `payload_kv::with_session` gives
  read-modify-write callers one lock without exposing the connection.
- 5.8 partial: `rag_mcp_core::Layer` names the layer values; constants and
  production comparisons use it. Stored values and JSON stay strings.
- 5.9 done: status/doctor/LLM-status reports live in `diagnostics`,
  `WakeUpReport` in `diary`; the crate root still re-exports them.
- 6.3 done as inherent methods: `LoadSource::{wiki_catalog, document,
  backlinks, save_wiki}` hold the HTTP-vs-direct-db choice once. A trait was
  not needed because there are exactly two closed variants.
- 6.4 done: `GraphApp` is split into `app/{mod,events,frame,graph,revisions,
  wiki}.rs`.
- 6.5 done as a contract test: `tests/it/web_types_contract_test.rs` round-trips
  every field of the TS `GraphNode`, `GraphEdge`, `SearchHit` and
  `SearchExplanation` through the Rust types. It found three `GraphNode` fields
  the server never sends and a dead `layer === 'wiki'` check in the inspector;
  both are fixed. No codegen dependency was added.

**Not done, with the reason**

| Item | Why it stops here |
|------|-------------------|
| 3.2 remaining env reads | Backup, PID file, WAL warning, sync node identity and call-log size are read once by the subsystem that owns them at start-up, several before `Config` exists. |
| 3.5 HTTP route roles | Unknown routes already default to Admin (fail-closed), so a missing entry cannot widen access. |
| 4.1 query helpers, rest of 4.3 | ~90 row loops would be rewritten mechanically with no behaviour gain; adopt a helper when a loop is touched for another reason. |
| 5.8 typed `kind`/`status` | `status` already has named constants; `kind` is an open vocabulary written by agents, so an enum would reject valid pages. |
| 6.2 remaining DTO pairs | They differ on purpose (the UI tolerates server drift and reads timestamps as strings); merging them is a wire-compatibility decision. |
| 6.6 more crates | `rag-mcp-core` already lets clients avoid the server crate. Splitting `db` or `wiki` into crates would need their `Config`/LLM seams to be crate-public first, and no second consumer exists. |

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
| `RAG_*` env reads outside `config.rs` | ~30 | 18 | 0 |
| Upward imports from `config`/domain into `mcp`/`http_api` | 5 | 0 | 0 |
| Largest production file (`db/store.rs` without tests) | ~4.2k lines | ~3.3k | < 2k |
| Largest UI file (`app.rs`) | 4.6k lines | 1.7k (`app/frame.rs`) | — |
| UI copies of server DTOs | 11 | 8 | 0 |
