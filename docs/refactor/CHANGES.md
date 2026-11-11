# Changes visible to clients

Every behaviour change of the refactor (PR #3, from `origin/main` `facf533`)
that an MCP, HTTP or CLI client, or an operator, can notice. Everything not
listed here behaves as before; `tests/it/parity_test.rs` pins the MCP/HTTP
behaviour. Engineering detail and the drift table are in
[PLAN.md](PLAN.md) ("Known MCP/HTTP drifts", "Stage 4 intentional changes").

Envelopes stay transport-specific and did not change: MCP returns the bare
result and a JSON-RPC error with the wire code in `data.code`; HTTP returns
`{ok: true, …}` or `{ok: false, code, error}` with a status. Wire codes and
HTTP statuses did not change anywhere below unless stated.

## Upgrade note: schema v26 (no downgrade)

- The first start of this build migrates the database from schema v25 to
  **v26**: `chunks.embedding_json` (JSON text) becomes `chunks.embedding`
  (`FLOAT[]`). The migration runs in 5 000-row transactions and resumes if
  interrupted; search results and scores are bit-identical.
- **A v26 database cannot be opened by an older build.** Older builds do not
  check the version and fail with SQL errors on the missing column. Take a
  verified backup (`backup_db` / `recovery backup`) **before** upgrading if
  you may need to roll back; restore that backup to downgrade.
- A database whose schema is newer than the running build is now refused
  before any DDL: `database schema vN is newer than this build supports (vM)`.
- Legacy vectors that cannot be converted (unparseable JSON, a null element,
  NaN or a value beyond `f32`) are not dropped: the raw text is kept in the
  new table `chunk_embedding_quarantine`, the chunk keeps a NULL embedding
  (skipped by vector search until re-embedded) and its document is marked for
  re-embedding, so the next source sync or `reembed_all` repairs it.
- The vector snapshot skips a stored vector that does not decode or is
  non-finite (with a warning) instead of failing every vector search.
- `chunks.embedding` is nullable in fresh v26 databases too (it was
  `NOT NULL` in earlier, unreleased commits of this branch); a NULL vector
  always means a chunk recorded in `chunk_embedding_quarantine`.
- Recovery bundles: a quarantined chunk is exported with its text, offsets
  and metadata, `"embedding": []` and a `quarantine` object
  (`reason`, `raw_embedding_json`, `quarantined_at`) copied from its
  quarantine row. Import restores the chunk row with a NULL embedding (its
  text stays lexically searchable), writes the quarantine row verbatim and
  clears the document's `content_hash` so it is re-embedded. The import
  report gains `chunks_quarantined`.
- Backup verification (`recovery verify`, the backup sidecar
  `verification` object) gains `quarantined_chunks`; NULL vectors of
  quarantined chunks no longer fail `embedding_contract_ok`. Sidecars written
  by older builds still verify.

## MCP and HTTP

1. **Error text.** About 61 HTTP validation errors read `invalid request: …`
   (or `timeout: …` for exhausted search budgets) instead of
   `config error: …`. Code `INVALID_REQUEST` and status 400 unchanged.
2. **`get_neighbors` / `GET /v1/neighbors`** follow one rule:
   - `node_id` and `seed` are aliases on both; MCP also accepts `project`
     (alias `wing`), HTTP also `rel_types` (comma-separated).
   - Depth is capped at 3 on both (MCP allowed 5).
   - The seed resolves as node id, document id, node URI, then exact label on
     both (MCP took a node id only).
   - Unknown seed → `NOT_FOUND`, blank seed → `INVALID_REQUEST` on both (MCP
     returned an empty view).
   - Without `rel_types` both walk the UI graph's PKB node-kind/relation
     policy (MCP walked relations only); with `rel_types`, exactly those
     relations. `rel_types` together with `project` is `INVALID_REQUEST`
     (it was silently ignored).
3. **`multi_get`**: one item shape on both transports,
   `{document, chunks, chunks_total, chunks_truncated}` plus the flat fields
   `id, uri, title, content, metadata_json, created_at, updated_at` (and
   optional `source_file`, `source_download_path`). HTTP items gain the flat
   fields, MCP items gain `document`. `document.content` is omitted on both
   (read the flat `content`). Envelope keys unchanged (`documents` on MCP,
   `items` on HTTP).
4. **`pack_context` (MCP)** accepts `include_trace`, `persist_trace` and
   `prior_trace` like HTTP and returns `trace` when asked.
5. **Wiki writes** go through one entrypoint:
   - MCP `write_wiki_page` gains optional `create_only=true` (create-only,
     `CONFLICT` on an existing slug, like `POST /v1/wiki`).
   - MCP `write_wiki_page` on an existing page keeps omitted
     `kind` / `category` / `summary` (it reset kind to `wiki` and cleared the
     other two). A blank `title` still becomes the slug.
   - MCP `update_wiki_page` also accepts a `wiki://` URI or a document id in
     `slug`; an `id` that names a different page than `slug` is `CONFLICT`
     and changes nothing (the id used to win); `create_only=true` is
     `INVALID_REQUEST` (it was ignored).
   - `update_wiki_page` and `PUT /v1/wiki` on an existing page apply a given
     `wing` / `room` (they ignored it).
   - A stale `if_match_revision` / `if_match_etag` is `CONFLICT` on both and
     leaves the page unchanged (MCP still reports it as `invalid_params` with
     `data.code = "CONFLICT"`).
6. **Feedback replays and online eval** (`POST /v1/feedback/*`, eval
   endpoints) call search in process: these internal searches no longer appear
   as `http search` rows in the call log, and a failed internal search returns
   the search error itself instead of
   `invalid request: search failed (<status>): <body>`.
7. **Maintenance plans**: the actions `consolidate_propose`, `resolve_stub`,
   `lint_and_fix_links`, `build_outline` and `merge_near_dup` (advertised but
   never implemented) are removed. A plan naming one is rejected like any
   unknown action (LLM plans: moved to `rejected`; `apply_maintenance_plan`:
   `FORBIDDEN`) instead of being accepted and skipped. `heuristic_plan` no
   longer proposes them; `analyze_corpus` still reports near duplicates and
   unresolved stubs, and `maintain_compress` keeps its `merge_near_dup` result
   rows. The planner prompt lists the remaining 21 actions.
8. **`list_rooms` / `GET /v1/rooms`**: the `wing` filter is trimmed on both
   (HTTP matched nothing for `" alpha "`).
9. **Lexical search errors**: `mode=lex` returns the first FTS error instead
   of retrying the same failing call; `hybrid` still returns the vector hits
   when the lexical leg fails.
10. **HTTP-MCP and admin reads**: MCP admin reads (`get_taxonomy`,
    `list_wings`, `list_rooms`, `kg_*`, `lint_wiki`, `reembed_status`,
    `llm_status`) and their HTTP routes return the same data as before; only
    the `list_rooms` trim above differs.

## CLI binaries

All binaries parse arguments with clap. Flags, defaults and meanings are
unchanged (`ingest_project --wing projects --room default --max-bytes
524288`; `eval --root . --top-k 5 --modes lex,vec,hybrid --warmup 0
--repeat 1`, `--golden` alias of `--dataset`; `recovery retention --keep 7`;
`rag_gate --mode` still defaults to `RAG_GATE_MODE`).

- `--help` prints on stdout and exits 0 (some printed on stderr).
- Unknown arguments, missing values and unparsable numbers use clap's wording
  (`unexpected argument '--bogus' found`, `invalid value 'x' for '--top-k
  <N>'`) and exit 2 (`recovery`, `ingest_project`, `eval` exited 1).
  Semantic checks keep their messages and exit 1.
- `recovery`: subcommands are clap subcommands. A missing
  `--db` / `--out` / `--in` / `--dir` / `--backup` is clap's "required
  arguments were not provided" (exit 2) instead of `required --db PATH`
  (exit 1); an unknown subcommand is `unrecognized subcommand` (exit 2);
  `recovery` with no subcommand prints help on stderr and exits 2 (it printed
  usage and exited 0); arguments a subcommand does not define are rejected
  (they were ignored), except `inventory --keep`, still accepted and ignored.
- Affected: `ingest_file`, `db_repair`, `rag_gate`, `promote_dodo_projects`,
  `recovery`, `ingest_project`, `eval`.

## Configuration and operations

- `RAG_STORAGE_BACKEND`: only `duckdb` runs. `markdown`, `sqlite`,
  `postgres` and `memory` are recognized and fail at start-up with
  "recognized but not implemented" (unchanged for the server); the library-only
  Markdown vault backend is deleted.
- Recovery import normalizes document rows like every other write (layer
  lowercased and trimmed, blank kind/status defaulted, missing
  `content_hash` computed, non-positive boost reset), keeping the bundle's
  revision and timestamps.
- Opening a current-schema store runs a few guarded repairs (about 7 ms at
  40 000 chunks).
- Shutdown: every exit path stops the background loops (auto-backup, FTS
  maintenance, auto-sync, file watcher, sync worker) and waits for them.
- `code_read` on a corrupt cached span returns `invalid cached code span;
  rebuild index_code` instead of panicking.

## Rust library (`rag_mcp` crate)

For code that links the crate rather than talking to the server:

- New crates `rag-store-duckdb` and `rag-code-intel`; `rag_mcp` re-exports
  the moved modules under their old paths.
- Removed: `wiki::put_wiki_page` / `WikiPutCommand` (use
  `app::WikiUseCase::write` with `WikiWriteMode`), `storage::Storage`,
  `BackendMetadata`, `open_configured_storage`, the Markdown backend;
  `update_wiki_page_cas` is crate-internal.
- `impl From<AppError> for rmcp::ErrorData` is now `mcp::error::mcp_error`;
  `Config::apply_process_limits` is the `bootstrap::ApplyProcessLimits`
  trait (import it).
- `Store` methods moved to repository accessors (`store.documents()`,
  `store.chunks()`, …); open stores with `storage::open_store`.
