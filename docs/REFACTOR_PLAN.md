# Refactor plan: structure, architecture, SOLID / DRY

**Status:** proposed · **Written:** 2026-10-11 · **Baseline:** branch
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

## Not planned

- A broad trait over `Store`, or full multi-backend `Storage` parity: the audit
  keeps both out on purpose; only narrow seams that enable fakes
  (`ChatCompleter`, ops-log sink) are added.
- Wire renames (`wing` → project, tool names) or new default tools.

## Baseline to track

| Measure | Now | Target |
|---------|-----|--------|
| `Store::lock()` call sites outside `db` | ~41 | `payload_kv` only |
| Operations implemented in both transports without a shared service | ~8 | 0 |
| `fn test_config` copies | 11 | 0 |
| Integration-test binaries linking DuckDB | 8 | 1–2 |
| `RAG_*` env reads outside `config.rs` | ~30 | 0 |
| Upward imports from `config`/domain into `mcp`/`http_api` | 5 | 0 |
| Largest production file (`db/store.rs` without tests) | ~4.2k lines | < 2k |
| UI copies of server DTOs | 11 | 0 |
