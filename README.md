# rag-mcp

Local **MCP knowledge gateway** for Retrieval-Augmented Generation with an
Obsidian-like object graph and a Karpathy-style wiki compile layer. One Rust
process serves stdio and/or streamable HTTP over a single-writer DuckDB store;
no Python runtime.

**Binary:** `rag-mcp`  
**License:** MIT

## What it is

`rag-mcp` ingests text and supported files into a local DuckDB database, chunks
them, embeds chunks (mock / OpenAI-compatible / Ollama), and exposes search plus
document CRUD over the [Model Context Protocol](https://modelcontextprotocol.io/)
and a loopback HTTP product API. Both transports share the same `Store`.

Beyond plain RAG it maintains:

1. **Document / tag / stub graph** (Obsidian-like wikilinks and tags)
2. **Hybrid retrieval** (`lex` | `vec` | `hybrid` with RRF; DuckDB FTS when available, term-frequency fallback otherwise)
3. **Wiki compile layer** (immutable raw sources, wiki pages, schema, index catalog, ops log, optional local LLM compile)

| Obsidian | rag-mcp |
|----------|---------|
| Note | Graph node `kind=document` |
| `[[wikilink]]` | Directed edge `wikilink` (stub if target missing) |
| `#tag` | Node `kind=tag` + edge `tagged` |
| Graph / local graph | `get_graph` / `get_neighbors` |
| Backlinks | `get_backlinks` |

Layout is not computed server-side: tools return pure `{nodes, edges}` JSON for clients or LLMs.

## Features (implemented)

- **Ingest** raw text, UTF-8 files (`RAG_INGEST_ROOTS` allowlist), immutable `ingest_raw`, MemPalace-style `add_drawer` (wing + room required)
- **Palace placement**: wings / rooms taxonomy (`list_wings`, `list_rooms`, `get_taxonomy`); scoped search by wing/room/layer/source
- **Fixed-size chunking** with overlap (`RAG_CHUNK_SIZE` / `RAG_CHUNK_OVERLAP`); Markdown chunks retain `heading_path` and leaf `section` metadata
- **Embeddings**: `mock` (deterministic), `openai` / `openai_compat`, `ollama` (native or OpenAI-compatible base)
- **Search modes**: `lex` (BM25 or TF fallback), `vec` (cosine over stored vectors), `hybrid` (RRF fusion); selective vector scopes are materialized in SQL before exact scoring instead of copying the global snapshot
- **Search filters**: document_id, wing, room, layer, source_file, include_archived; min_score; diversity (`mmr` | `collapse_by_document`); token packing. Project/document/source scopes neither consult nor populate the database-wide vector cache
- **Citation-oriented hits**: scores (`score`, `score_vec`, `score_lex`, `score_rrf`), snippet, char offsets, and Markdown section metadata when available
- **Opt-in context expansion**: `search` and `pack_context` accept `context_expansion=neighbors|parent_section`; `neighbor_chunks` controls the neighbor radius (default 1)
- **Object graph**: wikilinks, tags, stubs, neighbors BFS, backlinks, `link_nodes`, dedicated tunnel tools, `graph_expand_search`, `graph_stats`
- **Temporal knowledge graph**: `kg_add` / `kg_query` / `kg_invalidate` / `kg_supersede` / `kg_timeline` / `kg_stats`
- **Agent diary + session**: `diary_write` / `diary_read`, `wake_up`, `checkpoint`, `memories_filed_away`
- **Wiki layer**: write/get/list wiki pages, schema, index catalog, ops log, `query_with_index`, `search_wiki`, `file_answer`, `lint_wiki`, `consolidate`
- **Local LLM** (optional): Ollama / LM Studio OpenAI-compatible chat + embeddings (`RAG_LLM_*`, `RAG_EMBEDDING_PROVIDER=ollama`)
- **Maintenance loop**: `analyze_corpus` → `plan_maintenance` → `apply_maintenance_plan` (dry_run default) → `maintain_organize` / `maintain_compress` / `maintain_refresh`; wiki `compile_source`, `consolidate`, `refresh_stale_wiki`
- **Incremental sources**: root manifest preflight skips healthy unchanged files; changed files share bounded cross-document embedding batches while retaining ordered per-document atomic commits; parent/subdirectory root rebinding is preserved; generated `.cache`, Storybook output, and Python virtual environments are excluded; oversized files become explicit per-file errors; deleted or newly policy-excluded source state is pruned transactionally. A process-local read/write lane keeps a full sync exclusive while concurrent `lex`/`hybrid` searches either finish before it starts or fail fast with retryable `STORE_BUSY`
- **Product API**: Project Home, lean Unified Library, search, SQL-scoped project graph, paginated revision timeline with lazy snapshots/diff/restore, background jobs, checkpoint and verified backup
- **HTTP safety and observability**: all `/v1/*` and mounted `/mcp` request bodies are capped at 1 MiB even without `Content-Length`; bounded Activity exposes anonymous client IDs and operation lineage without raw IP/UA, request bodies, source paths or titles, and records job resources as `/v1/jobs/{id}` rather than retaining their identifiers
- **Integrity**: atomic document/chunk/graph writes, indexed document/graph-node URI lookup with single URI ownership enforced by CAS, immutable-raw revision restore guard, content_hash / `check_duplicate`, `embedding_manifest`, generation-aware FTS/vector caches; `status` aggregates layer health in SQL; `doctor`, `vacuum_store`, ops_log
- **Single-file DuckDB** (bundled crate); embeddings stored as JSON float arrays (portable, no VSS required)
- **Optional native client**: Home, Library, Search, Wiki, project Connections,
  Operations and document History over the live HTTP gateway; transport failures
  are sanitized before display, and project changes cannot discard an in-flight
  wiki save, revision restore or Operations mutation
- **Logging to stderr only** (stdout is reserved for MCP)

## MemPalace-inspired model

Capabilities inspired by [MemPalace](https://github.com/MemPalace/mempalace), implemented on DuckDB with **our tool names** (no `mempalace_*` rename, no AAAK dialect, no Chroma). Design notes: [`docs/MEMPALACE_PARITY.md`](docs/MEMPALACE_PARITY.md).

| MemPalace idea | rag-mcp |
|----------------|---------|
| Drawer (verbatim unit) | Document body + chunks; prefer `add_drawer` / `ingest_raw` (no silent summarize on ingest) |
| Wing | `documents.wing` (top shelf: project, area, person, …) |
| Room | `documents.room` (sub-area under a wing) |
| Closet (summary) | Wiki page `kind=source_summary` via compile / wiki tools |
| Tunnel | Graph edge `rel_type=tunnel` + tunnel CRUD / follow tools |
| Content-hash id | `content_hash` + `check_duplicate` |
| Temporal KG | `kg_facts` + `kg_*` tools (half-open validity windows) |
| Diary | Documents `layer=diary`, `kind=diary`, wing `agents/<name>` |
| wake-up | `wake_up`: status + recent diary + pinned docs + schema snippet (if present) |
| Checkpoint | `checkpoint`: ops_log savepoint + optional diary note |

**Wings / rooms.** Placement is first-class metadata, not free-text only. `add_drawer` requires `wing` and `room`. Inventory: `list_wings`, `list_rooms`, `get_taxonomy`. Filters on `search`, `list_documents`, `list_sources`, and related tools take `wing` / `room` / `layer` / `source_file`.

**Verbatim storage.** Ingest and diary write store content as-is. Summaries and synthesis happen only through explicit wiki / consolidate / compile tools, never silently on `ingest_*` / `add_drawer` / `diary_write`.

**Temporal KG.** Subject–predicate–object facts with optional `valid_from` / `valid_to` (half-open). Query active facts or at a point in time; invalidate or supersede when reality changes; timeline per subject; `kg_stats` aggregates.

**Diary and session bootstrap.** Agents append notes with `diary_write` (searchable, embedded). `wake_up` is the session entrypoint (health + memory surface without seeding a default schema). Prefer `checkpoint` at session boundaries over ad-hoc `append_log` + `diary_write`.

**Tool naming honesty.** Tables under [MCP tools](#mcp-tools-implemented) list
tools that exist on the binary today. We do **not** expose MemPalace-prefixed
aliases. Remaining gaps (generic event watch, hallways, AAAK, full multi-backend
parity) stay in [Limitations](#limitations-honest) and the parity doc.

## Environment variables

| Env | Default | Meaning |
|-----|---------|---------|
| `RAG_DB_PATH` | `./rag.duckdb` | DuckDB file path |
| `RAG_HTTP_BIND` | (empty/off) | Loopback gateway bind such as `127.0.0.1:7432`; streamable MCP is `/mcp` and product routes are `/v1/*` |
| `RAG_HTTP_ONLY` | `false` | Run the HTTP gateway without stdio; requires `RAG_HTTP_BIND` |
| `RAG_HTTP_ALLOW_REMOTE` | `false` | Required for non-loopback bind; the gateway has no built-in authentication |
| `RAG_HTTP_ALLOWED_HOSTS` | loopback names/addresses; concrete bind IP | Comma-separated extra Host names/IPs accepted by mounted `/mcp`; wildcard binds require every remote MCP authority explicitly |
| `RAG_EMBEDDING_PROVIDER` | `mock` | `mock` \| `openai` \| `openai_compat` \| `ollama` |
| `RAG_EMBEDDING_BASE_URL` | OpenAI `https://api.openai.com/v1`; Ollama `http://127.0.0.1:11434` | API root (native Ollama or OpenAI-compatible `/v1`) |
| `RAG_EMBEDDING_API_KEY` | (empty; local defaults to `ollama`) | Required when `provider=openai` against non-local hosts |
| `RAG_EMBEDDING_MODEL` | `text-embedding-3-small` (Ollama: `nomic-embed-text`) | Model name |
| `RAG_EMBEDDING_DIMS` | `1536` (Ollama: `768`) | Vector dimensions (must match model and corpus manifest) |
| `RAG_CHUNK_SIZE` | `800` | Approx chars per chunk |
| `RAG_CHUNK_OVERLAP` | `120` | Overlap chars (must be &lt; chunk size) |
| `RAG_DEFAULT_TOP_K` | `5` | Default search limit |
| `RAG_STORAGE_BACKEND` | `duckdb` | Full `rag-mcp` runtime accepts DuckDB; the backend-neutral library document factory also supports opt-in `markdown` vault CRUD |
| `RAG_VAULT_PATH` | — | Explicit vault root required by the Markdown storage adapter; never inferred from `RAG_DB_PATH` |
| `RAG_STARTUP_WARN_MS` | `30000` | Warn when a measured startup phase exceeds this duration |
| `RAG_PID_FILE` | `<RAG_DB_PATH with .pid>` | Optional PID metadata path used to flag stale crash metadata |
| `RAG_DEFAULT_SEARCH_MODE` | `vec` | `vec` \| `lex` \| `hybrid` |
| `RAG_INGEST_ROOTS` | (empty) | Comma-separated path allowlist for `ingest_file`. **Empty refuses all file paths** |
| `RAG_CHECKPOINT_ON_START` | `true` | Flush and validate DuckDB WAL before serving |
| `RAG_WAL_WARN_BYTES` | `268435456` | Doctor warning threshold for an uncheckpointed WAL |
| `RAG_AUTO_BACKUP_DIR` | (empty/off) | Directory for periodic checkpointed snapshots |
| `RAG_AUTO_BACKUP_INTERVAL_SECS` | `86400` | Minimum age before the next automatic snapshot |
| `RAG_AUTO_BACKUP_KEEP` | `7` | Number of automatic snapshots retained |
| `RAG_AUTO_SYNC_ROOTS` | (empty/off) | `;`-separated allowlisted directories for incremental background sync |
| `RAG_AUTO_SYNC_INTERVAL_SECS` | `3600` | Background sync interval; the first pass runs at startup |
| `RAG_MAX_CONTEXT_TOKENS` | `4096` | Default token budget when packing search hits (~4 chars/token) |
| `RAG_MAX_CHUNKS_PER_DOC` | `3` | Max chunks retained per document under diversity collapse |
| `RAG_FTS_STEMMER` | `porter` | DuckDB FTS stemmer; use `none` for CJK/code |
| `RAG_TOOLS` | `spine` | MCP tool surface: `spine` (currently 33 compile-first tools, count-bounded by test) or `full` (all tools). See vision §5. |
| `RAG_LLM_PROVIDER` | `ollama` | Chat preset: `ollama` \| `openai` \| `codex` \| `claude` \| `kimi` \| `deepseek` \| `custom` — see [`docs/LLM_PROVIDERS.md`](docs/LLM_PROVIDERS.md) |
| `RAG_LLM_ENABLED` | `true` | When false, chat/compile tools refuse |
| `RAG_LLM_BASE_URL` | provider default | Chat API root (override preset) |
| `RAG_LLM_MODEL` | provider default | Chat model name |
| `RAG_LLM_API_KEY` | provider fallbacks | Key; or `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / `DEEPSEEK_API_KEY` / `MOONSHOT_API_KEY` |
| `RAG_LLM_TIMEOUT_SECS` | `120` | HTTP timeout for chat completions |
| `RAG_LLM_MAX_TOKENS` | `4096` | Max completion tokens per chat request |
| `RAG_MAINT_MAX_DOCS` | `50` | Cap for maintenance reembed / graph rebuild / stale wiki recompile |
| `RAG_MAINT_NEAR_DUP_THRESHOLD` | `0.92` | Cosine threshold for near-duplicate detection in `analyze_corpus` |
| `RUST_LOG` | `info` | Tracing filter (`tracing-subscriber`, stderr only) |

The built-in `RAG_DB_PATH` fallback is useful for isolated development only. On
this workstation the production source of truth is
`/Users/themoretheless/.local/share/rag-mcp/rag.duckdb`, owned by the single
gateway at `http://127.0.0.1:7432`. Clients connect to that gateway and do not
start their own `rag-mcp` process.

## Build and run

Requirements: Rust stable (edition 2021), network for first build (DuckDB bundled C++).

```bash
cargo build --release
```

Deploy/restart the one installed gateway with [`docs/PROD_RUN.md`](docs/PROD_RUN.md).
Running `./target/release/rag-mcp` in the foreground is an exclusive replacement,
not a second production process: stop `local.rag-mcp` first and pass the
canonical `RAG_DB_PATH` explicitly.

Debug:

```bash
# isolated disposable smoke only, with the production gateway stopped
RAG_DB_PATH=/tmp/rag-mcp-debug.duckdb cargo run
```

Without `RAG_HTTP_BIND`, the process serves MCP on **stdio**. With a bind it
also serves HTTP, and `RAG_HTTP_ONLY=true` disables stdio. Application logs
always stay on stderr.

Default workspace members build **only** `rag-mcp` (no egui). The optional
native client is a separate package; see [Optional native client](#optional-native-client-rag-mcp-ui).

### Offline smoke (mock embeddings)

This intentionally uses a disposable database. Stop the production gateway
first so the smoke cannot be mistaken for, or race, the live service.

```bash
export RAG_EMBEDDING_PROVIDER=mock
export RAG_DB_PATH=/tmp/rag-mcp-offline-smoke.duckdb
export RAG_DEFAULT_SEARCH_MODE=hybrid
cargo run
```

### OpenAI-compatible embeddings

These settings belong to the one gateway. Changing embedding dimensions
requires a fresh compatible corpus or a full re-index; do not start a second
`cargo run` process against either the canonical DB or an implicit
`./rag.duckdb`.

```bash
export RAG_EMBEDDING_PROVIDER=openai
export RAG_EMBEDDING_API_KEY=sk-...
export RAG_EMBEDDING_MODEL=text-embedding-3-small
export RAG_EMBEDDING_DIMS=1536
# optional: RAG_EMBEDDING_BASE_URL=https://api.openai.com/v1
# Apply to local.rag-mcp and restart the single gateway via docs/PROD_RUN.md.
```

### Local Ollama (quick start)

See **[Local LLM + maintenance](#local-llm--maintenance)** below and full guides:
[`docs/LOCAL_LLM_MAINTENANCE.md`](docs/LOCAL_LLM_MAINTENANCE.md),
[`docs/LOCAL_LLM_WIKI.md`](docs/LOCAL_LLM_WIKI.md),
[`docs/ORGANIZE.md`](docs/ORGANIZE.md).

```bash
# terminal 1
ollama serve
ollama pull llama3.2
ollama pull nomic-embed-text

# Configure/restart the one gateway; do not start a second process beside it.
export RAG_DB_PATH=/Users/themoretheless/.local/share/rag-mcp/rag.duckdb
export RAG_EMBEDDING_PROVIDER=ollama
export RAG_EMBEDDING_BASE_URL=http://127.0.0.1:11434
export RAG_EMBEDDING_API_KEY=ollama
export RAG_EMBEDDING_MODEL=nomic-embed-text
export RAG_EMBEDDING_DIMS=768
export RAG_LLM_ENABLED=true
export RAG_LLM_BASE_URL=http://127.0.0.1:11434/v1
export RAG_LLM_MODEL=llama3.2
export RAG_LLM_API_KEY=ollama
export RAG_INGEST_ROOTS=/absolute/path/to/vault
export RAG_HTTP_BIND=127.0.0.1:7432
export RAG_HTTP_ONLY=true
# Foreground replacement only after stopping local.rag-mcp.
./target/release/rag-mcp
```

## Local LLM + maintenance

Chat providers: **Ollama, OpenAI, Codex, Claude, Kimi, DeepSeek** (and any OpenAI-compatible custom URL). Full table: [`docs/LLM_PROVIDERS.md`](docs/LLM_PROVIDERS.md).

```bash
export RAG_LLM_PROVIDER=claude          # or ollama | openai | codex | kimi | deepseek | custom
export ANTHROPIC_API_KEY=sk-ant-...     # provider-specific key env also works
```

Run **embeddings and chat** (local Ollama or cloud), then **analyze → plan → dry_run apply → compress → refresh** wiki and DuckDB.

### Ollama setup

```bash
# Install from https://ollama.com then:
ollama serve
ollama pull llama3.2           # chat / compile / plan
ollama pull nomic-embed-text   # embeddings (768 dims)
# alternatives: qwen2.5, mistral, mxbai-embed-large, …
```

Also works with:

| Provider | Chat base URL | Embeddings |
|----------|---------------|------------|
| **Ollama** (recommended) | `http://127.0.0.1:11434/v1` | `RAG_EMBEDDING_PROVIDER=ollama`, base `http://127.0.0.1:11434` (native) or `…/v1` |
| LM Studio | `http://127.0.0.1:1234/v1` | `openai` / `openai_compat` + same base |
| llama.cpp server | `http://127.0.0.1:8080/v1` | same OpenAI-compatible path |

Chat API: `POST {RAG_LLM_BASE_URL}/chat/completions`.  
Embeddings: native Ollama `/api/embed` (or `/api/embeddings`), or OpenAI-compatible `/embeddings` when base ends with `/v1`.

On model/dim change: `vec` / `hybrid` refuse until you reembed (`reembed_document` or `maintain_refresh` with `reembed_all`).

### Env (LLM + maintenance)

| Env | Default | Meaning |
|-----|---------|---------|
| `RAG_EMBEDDING_PROVIDER` | `mock` | Use `ollama` for local vectors |
| `RAG_EMBEDDING_BASE_URL` | Ollama: `http://127.0.0.1:11434` | Embed API root |
| `RAG_EMBEDDING_MODEL` | Ollama: `nomic-embed-text` | Embed model tag |
| `RAG_EMBEDDING_DIMS` | Ollama: `768` | Must match model and corpus manifest |
| `RAG_EMBEDDING_API_KEY` | local → `ollama` | Dummy key for local hosts |
| `RAG_LLM_ENABLED` | `true` | When false, chat/compile/plan-LLM tools refuse |
| `RAG_LLM_BASE_URL` | `http://127.0.0.1:11434/v1` | OpenAI-compatible chat base |
| `RAG_LLM_MODEL` | `llama3.2` | Chat model name |
| `RAG_LLM_API_KEY` | `ollama` | Chat Bearer (Ollama accepts dummy) |
| `RAG_LLM_TIMEOUT_SECS` | `120` | HTTP timeout for completions |
| `RAG_LLM_MAX_TOKENS` | `4096` | Max completion tokens per request |
| `RAG_MAINT_MAX_DOCS` | `50` | Cap docs touched per maintain / reembed / recompile |
| `RAG_MAINT_NEAR_DUP_THRESHOLD` | `0.92` | Cosine θ for near-dups in analyze / compress L2 |

Full env table: [Environment variables](#environment-variables).

### Pipeline

```
llm_status / analyze_corpus
        │
        ▼
plan_maintenance          (LLM or heuristic JSON action list)
        │
        ▼
apply_maintenance_plan    (dry_run=true by default)
        │
        ▼
maintain_organize / maintain_compress / maintain_refresh
        │
        ▼
compile_source · consolidate · refresh_stale_wiki · vacuum_store
```

Every apply path logs to **ops_log** (`append_log` / `read_log` / `list_recent_ops`).

### Maintenance and LLM tools

| Tool | Params (high level) | Role |
|------|---------------------|------|
| `llm_status` | (none) | Chat + embed config and reachability |
| `analyze_corpus` | `stub_age_days?`, `include_near_dups?`, `near_dup_threshold?`, `archive_min_age_days?`, `log_ops?` | Deterministic health report (no LLM) |
| `plan_maintenance` | `analysis?`, `max_actions?`, `force_heuristic?`, `log_ops?` | Analysis → validated whitelist action plan (LLM or heuristic) |
| `apply_maintenance_plan` | `actions[]`, `dry_run?` (**default true**), `max_docs?`, `agent?` | Execute plan; preview unless `dry_run=false` |
| `maintain_organize` | `dry_run?` (**default true**), `mode?`, `max_docs?`, `min_confidence?`, `rebuild_index?`, `agent?` | Suggest/apply refiles for docs missing wing |
| `maintain_compress` | `level?` (0–2), `dry_run?` (**default true**), `confirm?`, `allow_raw_delete?`, `near_dup_threshold?`, `max_docs?` | L0 CHECKPOINT+FTS; L1 exact hash merge; L2 near-dup (merge needs `confirm`) |
| `maintain_refresh` | `reindex_fts?`, `rebuild_graph?`, `graph_dirty_only?`, `rebuild_wiki_index?`, `reembed_all?`, `dry_run?`, `max_docs?` | Actualize FTS / graph / wiki index / reembed |
| `vacuum_store` | (none) | DuckDB `CHECKPOINT` + size delta; ops_log |
| `compile_source` | `source_id_or_uri`, `dry_run?`, `agent?` | Local LLM: raw → wiki pages |
| `consolidate` | `document_ids`, `apply?` (**default false**), `max_docs?`, slug/title/… | Local LLM merge docs → one wiki proposal (or write) |
| `list_memory_lifecycle_candidates` | `status?`, `layer?`, `kind?`, `limit?` | Deterministic candidate list (defaults to `active`) |
| `consolidate_memory_items` | `document_ids`, `output_document_id`, `agent?` | Mark selected items `consolidated` and attach source/output provenance; idempotent, no LLM |
| `archive_memory_items` | `document_ids`, `agent?` | Mark selected items `archived`; idempotent, no LLM |
| `refresh_stale_wiki` | `dry_run?` (**default true**), `max_docs?`, `agent?` | List or recompile wiki older than linked raw |

Related wiki/search tools: `search_wiki`, `query_with_index`, `file_answer`, `lint_wiki`, `rebuild_index`, `read_index`.

Example agent session after Ollama is up:

1. `llm_status` → ok  
2. `analyze_corpus` → stubs / dups / stale wiki  
3. `plan_maintenance` → JSON actions  
4. `apply_maintenance_plan` with `dry_run=true` → preview  
5. `apply_maintenance_plan` with `dry_run=false` → apply  
6. `maintain_compress` `level=1` (still dry_run first)  
7. `vacuum_store` / `maintain_refresh`  

### Safety (dry_run + whitelist)

1. **Whitelist only:** LLM and plan tools cannot invent SQL or shell; only named actions (refile, pin, archive, set_tags, rebuild_*, reembed, compile_source, merge_exact_dup, vacuum, …).
2. **`dry_run` defaults true** for multi-action / bulk paths: `apply_maintenance_plan`, `maintain_organize`, `maintain_compress`, `refresh_stale_wiki`. Preview + ops_log, no mutation until you set `dry_run=false`.
3. **`confirm=true`** required for compress L2 near-dup merges (with `dry_run=false`).
4. **Budget:** `RAG_MAINT_MAX_DOCS` (default 50) caps docs touched per run; override per-call with `max_docs` where exposed.
5. **ops_log** on every apply (and most dry_runs with `*_dry_run` op names).
6. **No silent raw mutation:** `layer=raw` bodies stay immutable; hard-delete of raw needs explicit `allow_raw_delete` (compress/apply). Prefer tombstones.
7. Offline-first: unreachable LLM → clear error (timeouts via `RAG_LLM_TIMEOUT_SECS`), not an infinite hang.

Design depth: [`docs/LOCAL_LLM_MAINTENANCE.md`](docs/LOCAL_LLM_MAINTENANCE.md) (pipeline, compress levels, phases), [`docs/ORGANIZE.md`](docs/ORGANIZE.md) (place / rank / structure / compile / hygiene).

## MCP client config examples

The normal configuration is one already-running HTTP gateway for every client.
The gateway owns the canonical DuckDB file; client processes never open it.

**Zed** (`~/.config/zed/settings.json`, or Settings → AI → MCP Servers):

```json
{
  "context_servers": {
    "rag-mcp": {
      "url": "http://127.0.0.1:7432/mcp"
    }
  }
}
```

Ready-made snippet: [`examples/zed.settings.json`](examples/zed.settings.json). After edit: open **Settings → AI → MCP Servers** and confirm green indicator for `rag-mcp`. In Agent Panel mention `rag-mcp` or enable its tools.

**Claude Code / HTTP-capable clients:**

```json
{
  "mcpServers": {
    "rag-mcp": {
      "type": "http",
      "url": "http://127.0.0.1:7432/mcp"
    }
  }
}
```

**Claude Desktop:** use the `mcp-remote` stdio bridge to the same gateway:

```json
{
  "mcpServers": {
    "rag-mcp": {
      "command": "npx",
      "args": ["-y", "mcp-remote@0.1.38", "http://127.0.0.1:7432/mcp"]
    }
  }
}
```

Ready-made gateway examples live under [`examples/`](examples). Direct stdio is
an exclusive offline/legacy mode only: stop the gateway first and use an
explicit disposable database or the canonical database with no other owner.

## MCP tools (implemented)

All tools return JSON text content via MCP `CallToolResult`.

### Ingest and raw layer

| Tool | Params | Behavior |
|------|--------|----------|
| `ingest_text` | `text`, `title?`, `uri?`, `metadata_json?` | Chunk, embed, store; upsert by uri; graph extract |
| `ingest_file` | `path`, `title?`, `uri?`, `metadata_json?` | Ingest text, Markdown, HTML, PDF, or common source code under `RAG_INGEST_ROOTS` |
| `sync_sources` | `path`, `remove_deleted?`, `wing?`, `room?`, `max_file_bytes?` | Manifest-aware recursive sync; skip healthy unchanged files and generated trees, repair missing chunks, optionally prune deleted and explicitly excluded sources |

HTML ingest strips tags plus script/style content. Source files retain `format`, `language`, and
`source_path` metadata and prefer code block/line chunk boundaries. PDF ingest invokes Poppler's
`pdftotext`; a missing tool or extraction failure is reported for that file, and PDF bytes are never
treated as text. Existing plain-text and Markdown behavior remains unchanged.
| `ingest_raw` | `text`, `title?`, `uri?`, `metadata_json?`, `wing?`, `room?`, `source_file?` | Immutable raw layer (`layer=raw`); same uri+hash no-op; content change refused |
| `add_drawer` | `content`, `wing`, `room`, `source_file?`, `title?`, `uri?`, `metadata_json?` | Verbatim drawer with required placement |
| `list_sources` | `wing?`, `room?` | List raw-layer sources |
| `get_source` | `document_id?` \| `uri?`, `include_chunks?` | Fetch one raw source |

### Search and packing

| Tool | Params | Behavior |
|------|--------|----------|
| `search` | `query`, `top_k?`, `mode?`, `document_id?`, `wing?`, `room?`, `layer?`, `source_file?`, `include_archived?`, `min_score?`, `diversity?`, `group_by?`, `recency_half_life_days?`, `max_context_tokens?`, `max_chunks_per_document?` | `lex` \| `vec` \| `hybrid` search over chunks; optional document grouping and recency boost |
| `multi_get` | `document_ids`, `include_chunks?` | Fetch up to 100 documents in one call and report missing IDs |
| `expand_chunks` | `document_id`, `chunk_index`, `radius?` | Read the neighboring chunk window around a known hit |
| `find_similar` | `document_id`, `chunk_indices?`, `top_k?`, filters… | Find chunks nearest to an existing document or selected chunks |
| `search_wiki` | `query`, `top_k?`, `mode?`, `min_score?`, `wing?`, `room?`, `diversity?`, … | Search restricted to `layer=wiki` |
| `graph_expand_search` | `query`, `top_k?`, `document_id?`, `depth?`, `max_nodes?` | Vector search then neighbor expand from hit document nodes |
| `pack_context` | `hits`, `max_tokens?` | Pack ranked hits under a token budget into a citation block |
| `query_with_index` | `query`, `top_k?`, `include_content?` | Index-first catalog match (wiki index) |

### Documents, taxonomy, integrity

| Tool | Params | Behavior |
|------|--------|----------|
| `list_documents` | `wing?`, `room?`, `source_file?`, `layer?`, `kind?`, `include_archived?`, `limit?` | Inventory without full body (default skips archived) |
| `get_document` | `document_id`, `include_chunks?` | Metadata + optional chunk texts (no embeddings) |
| `update_document_meta` | `document_id`, `wing?`, `room?`, `title?`, `metadata_json?`, `pinned?`, `boost?`, `status?`, `layer?`, `kind?`, `source_file?`, `content?` | Meta update without re-embed unless `content` changes; refused for immutable raw body |
| `delete_document` | `document_id` | Delete document + chunks + graph cleanup |
| `delete_by_source` | `source_file` (alias `source`), `dry_run?` | Bulk delete by exact `source_file` |
| `cleanup_source_duplicates` | `dry_run?`, `confirm?`, `max_candidates?` | Preview by default; remove a bounded deterministic batch of legacy raw duplicates only when one exact `file://<source_file>` survivor exists; rewires safe references and reports protected groups without changing them |
| `check_duplicate` | `content?`, `content_hash?`/`hash?`, `uri?` | content_hash / uri idempotency probe |
| `list_wings` | (none) | Distinct wings with document counts |
| `list_rooms` | `wing?` | Distinct rooms with counts |
| `get_taxonomy` | (none) | Wing → room tree + counts |
| `stats` | (none) | Docs, chunks, nodes, edges + `db_path` |
| `status` | (none) | Health: counts, FTS readiness, embed dims, `ready_for_search`; layer/index compilation health is aggregated in SQL without loading bodies |
| `doctor` | (none) | Schema / FTS / embedding manifest, WAL size, orphan references, missing chunks, and scope integrity |
| `get_embedding_manifest` | (none) | Corpus embedding fingerprint (provider, model, dims) |
| `multi_query_search` | `queries`, `top_k?`, `mode?`, filters | Caller-supplied query rewrites fused with deterministic RRF; no implicit LLM call |
| `reembed_document` | `document_id` | Re-embed all chunks for one document with live config |
| `llm_status` | (none) | Chat + embed config and reachability |
| `vacuum_store` | (none) | DuckDB `CHECKPOINT` + file size delta; ops_log |

### Backup and recovery

| Tool | Params | Behavior |
|------|--------|----------|
| `backup_db` | `path`, `dry_run?`, `overwrite?` | Consistent checkpointed DuckDB copy; no overwrite by default |
| `export_bundle` | `path`, `format?`, `dry_run?`, `overwrite?` | Portable JSON/JSONL documents, metadata, and chunks; fully staged and synced before atomic publication, with no-clobber default |
| `export_vault` | `path`, `dry_run?`, `overwrite?` | Git-friendly Markdown grouped by wing/room/layer plus `.rag` graph, ops log, and manifest; dry-run by default |
| `import_bundle` | `path`, `format?`, `dry_run?`, `conflict_policy?` | Transactional import; `error`, `skip`, or explicit `overwrite`; an id/URI cross-collision conflicts and rolls back the whole bundle |

All paths must be under `RAG_INGEST_ROOTS`. Imports default to dry-run. See [Backup and recovery](docs/RECOVERY.md).

### Graph (Obsidian-like)

| Tool | Params | Behavior |
|------|--------|----------|
| `get_graph` | `kinds?`, `rel_types?`, `seed_ids?`, `max_nodes?` | Filtered graph export `{nodes, edges}` |
| `get_neighbors` | `node_id`, `depth?`, `max_nodes?` | Local BFS (undirected traversal) |
| `get_backlinks` | `node_id` \| `document_id` \| `label` | Incoming edges + source nodes |
| `link_nodes` | `source_id`, `target_id`, `rel_type?`, `weight?` | Explicit edge (`related` default; `tunnel` allowed) |
| `create_tunnel` | `source_id`, `target_id`, `weight?`, `context?`/`label?` | Tunnel edge between two nodes (upsert by pair) |
| `list_tunnels` | `node_id?` | List `rel_type=tunnel` edges |
| `delete_tunnel` | `tunnel_id` | Delete one tunnel edge by id |
| `follow_tunnels` | `node_id`, `depth?`, `max_nodes?` | Multi-hop BFS on tunnel edges only |
| `find_tunnels` | `node_id?`, `other_node_id?`, `wing?`, `limit?` | Filter tunnels by endpoints / wing |
| `find_node` | `label?`, `document_id?`, `node_id?` | Resolve node metadata |
| `graph_stats` | (none) | Totals + counts by kind / rel_type |

**Note text conventions:** `[[Target]]` / `[[Target|alias]]` become wikilink edges; `#tag` / `#multi/level` become tag nodes. Images `![[...]]` are not edges. Unresolved wikilink targets become `kind=stub` until a matching document title/uri is ingested. Edge types include `wikilink`, `tagged`, `related`, `mentions`, `tunnel`.

### Temporal knowledge graph

| Tool | Params | Behavior |
|------|--------|----------|
| `kg_add` | `subject`, `predicate`, `object`, `valid_from?`, `valid_to?`, `source_document_id?`, `confidence?`, `metadata_json?` | Add fact; idempotent for open active SPO |
| `kg_query` | `subject?`, `predicate?`, `object?`, `at_time?` | Active facts, or facts valid at `at_time` |
| `kg_invalidate` | `subject`, `predicate`, `object`, `ended?` | Close matching open active fact(s) |
| `kg_supersede` | `subject`, `predicate`, `old_object`, `new_object`, `at?`, `source_document_id?`, `confidence?` | Close old object, open successor |
| `kg_timeline` | `subject` | All statuses for subject, by `valid_from` |
| `kg_stats` | (none) | Counts by status + distinct subjects/predicates |

### Diary, wake-up, checkpoint

| Tool | Params | Behavior |
|------|--------|----------|
| `diary_write` | `agent_name` (alias `agent`), `content`, `wing?`, `topic?`, `title?` | Verbatim diary note (`layer=diary`, wing `agents/<name>` by default); chunks + embeds |
| `diary_read` | `agent_name`, `last_n?` (alias `limit`) | Recent diary entries for one agent (newest first) |
| `wake_up` | `agent_name?`, `diary_limit?`, `pinned_limit?` | Status + diary slice + pinned docs + existing schema snippet only (does not seed default schema) |
| `checkpoint` | `summary` (alias `message`), `diary?`, `agent_name?` | Always append `ops_log` (`op=checkpoint`); optional diary body writes a diary entry |
| `memories_filed_away` | `limit?` | Recent memory-filing ops from ops_log (ingest/drawer/wiki/diary/checkpoint) |
| `reconnect` | (none) | DuckDB single-process no-op success (parity with MemPalace reconnect) |

Memory documents use the existing durable `status` field (`active`, `consolidated`,
`archived`, plus legacy statuses). Lifecycle consolidation links selected sources to an
existing output under structured `metadata_json.memory_lifecycle` provenance; it does
not synthesize or rewrite content.

### Wiki compile layer

| Tool | Params | Behavior |
|------|--------|----------|
| `write_wiki_page` | `slug`, `title`, `content`, `kind?`, `category?`, `summary?`, `agent?` | Create/overwrite wiki page + graph extract + index touch |
| `update_wiki_page` | (same as write) | Alias of `write_wiki_page` (upsert by slug) |
| `get_wiki_page` | `id_or_slug` | Fetch by slug, `wiki://…`, or document id |
| `list_wiki_pages` | (none) | List `layer=wiki` documents |
| `get_schema` | `no_default?` | Read agent conventions at `schema://agents` |
| `update_schema` | `content`, `title?`, `agent?` | Create/replace schema document |
| `read_index` | `format?` (`json` \| `markdown`) | Wiki content catalog |
| `update_index_entry` | `slug`, `title?`, `kind?`, `category?`, `summary?`, `page_id?` | Create/merge one catalog row |
| `rebuild_index` | (none) | Rebuild catalog from all wiki documents |
| `append_log` | `op`, `message?`, `prefix?`, `entity_id?`, `entity_kind?`, `payload_json?`, `agent_name?` | Append ops_log entry |
| `read_log` | `id?` \| `seq?`, `limit?` | Read one entry or list recent |
| `list_recent_ops` | `limit?` | Newest ops_log rows |
| `file_answer` | `title`, `body`, `slug?`, `citations?`, `agent?` | Persist cited answer as wiki page |
| `compile_source` | `source_id_or_uri`, `dry_run?`, `agent?` | Local LLM compiles a raw source into wiki pages |
| `lint_wiki` | (none) | Structured wiki/link health: index gaps, broken/duplicate/self wikilinks, unresolved stubs, orphan pages/documents, uncompiled raw, aggregate counts |
| `refresh_stale_wiki` | `dry_run?` (default true), `max_docs?`, `agent?` | List (or recompile) wiki older than linked raw |

### Maintenance (see [Local LLM + maintenance](#local-llm--maintenance))

| Tool | Params | Behavior |
|------|--------|----------|
| `llm_status` | (none) | Chat + embed config and reachability |
| `analyze_corpus` | `stub_age_days?`, `include_near_dups?`, `near_dup_threshold?`, `archive_min_age_days?`, `log_ops?` | Deterministic health report (no LLM) |
| `plan_maintenance` | `analysis?`, `max_actions?`, `force_heuristic?`, `log_ops?` | Analysis → whitelist action plan (LLM or heuristic) |
| `apply_maintenance_plan` | `actions[]`, `dry_run?` (default **true**), `max_docs?`, `agent?` | Execute plan; dry_run previews without mutation |
| `maintain_organize` | `dry_run?` (default **true**), `mode?`, `max_docs?`, `min_confidence?`, `rebuild_index?`, `agent?` | Suggest/apply refiles for missing wing |
| `maintain_compress` | `level?`, `dry_run?` (default **true**), `confirm?`, `allow_raw_delete?`, `near_dup_threshold?`, `max_docs?` | L0 FTS+CHECKPOINT; L1 exact merge; L2 near-dup (needs confirm) |
| `maintain_refresh` | `reindex_fts?`, `rebuild_graph?`, `graph_dirty_only?`, `rebuild_wiki_index?`, `reembed_all?`, `dry_run?`, `max_docs?` | Whitelisted refresh; safe defaults when no flags set |
| `vacuum_store` | (none) | DuckDB `CHECKPOINT` + file size delta; ops_log |
| `consolidate` | `document_ids`, `apply?` (default false), `max_docs?`, … | LLM merge → wiki proposal or write |
| `refresh_stale_wiki` | `dry_run?` (default **true**), `max_docs?`, `agent?` | List or recompile wiki older than linked raw |

## Embeddings providers

| | `mock` | `openai` / `openai_compat` | `ollama` |
|--|--------|----------------------------|----------|
| When | Default; tests; offline | Cloud or any OpenAI-compatible `/embeddings` | Local Ollama |
| How | Deterministic hash → `dims` floats, L2-normalized | `POST {base}/embeddings` with Bearer key | Native Ollama embed API (or OpenAI path if base ends with `/v1`) |
| Same text | Same vector always | Model-dependent | Model-dependent |
| API key | Not required | Required except local hosts | Dummy key filled when empty |
| Use for | Dev, CI, wiring clients | Production / remote | Local semantic quality |

Keep `RAG_EMBEDDING_DIMS` consistent with the model and with vectors already stored. The corpus records an **embedding manifest**; `vec` / `hybrid` refuse when dims mismatch. Use `reembed_document` or `maintain_refresh` with `reembed_all` after model/dim changes.

Compatible servers (LiteLLM, Azure OpenAI, LM Studio, local proxies) work by setting `RAG_EMBEDDING_BASE_URL` and matching model/dims.

## Lexical search (FTS)

Preferred path: DuckDB `fts` extension (`INSTALL`/`LOAD fts` +
`PRAGMA create_fts_index`). Mutations advance the chunk generation and mark FTS
stale; the next lexical/hybrid read performs one single-flight refresh, so the
reader observes preceding writes without rebuilding on every request.

If the FTS extension is unavailable (offline CI, locked-down hosts), the same `lex` / `hybrid` API falls back to a **term-frequency scorer in Rust**. Scores are not full Okapi BM25; `status` / `doctor` report the active backend (`duckdb_bm25` vs `term_frequency`).

## Limitations (honest)

Tool tables above match the current MCP surface (see also Features and MemPalace-inspired model). Still **not** shipped:

- No MemPalace `mempalace_*` tool renames or AAAK dialect
- No generic OS watcher for DuckDB source roots; `RAG_AUTO_SYNC_ROOTS` and HTTP jobs run manifest-aware scans. The limited Markdown document adapter has its own sidecar watcher
- No hallways (named multi-hop path objects) as first-class tools
- Dedicated CLI `maintain` subcommand and compress L3+ remain open; backup and source-sync schedules are built into the gateway
- No HNSW / DuckDB VSS ANN. A recorded local benchmark observed 133.98 ms
  hybrid p95 at 100,111 chunks on the cached exact path, below the current
  300 ms gate; rollout decisions require a representative rerun
- Versioned retrieval evaluation is available through the `eval` CLI; see
  [`docs/RETRIEVAL_EVALUATION.md`](docs/RETRIEVAL_EVALUATION.md) for dataset,
  metrics, diagnostics, and threshold-based future ANN guidance.
- Binary formats other than PDF are not parsed
- No multi-tenant auth
- No force-directed layout server-side (clients own visualization; optional `rag-mcp-ui` is RadialLocal only)
- No block refs `[[note#^block]]` (full-note link only)
- No YAML property bi-directional sync with Obsidian vault
- DuckDB is the only full application backend. The opt-in Markdown adapter implements document CRUD, frontmatter, sidecar rebuild and watching, but not search/graph/wiki/transaction parity; SQLite/Postgres/Memory still fail explicitly at startup
- `reconnect` is an intentional no-op success on single-process DuckDB (no Chroma-style client cache)

Architecture north star: [`docs/ARCHITECTURE_VISION.md`](docs/ARCHITECTURE_VISION.md), principles [`docs/PRODUCT_PRINCIPLES.md`](docs/PRODUCT_PRINCIPLES.md), map [`docs/SYSTEM_MAP.md`](docs/SYSTEM_MAP.md).

Current release gate: [`docs/ROADMAP.md`](docs/ROADMAP.md). Historical contract
and research: [`SPEC.md`](SPEC.md), [`FEATURES.md`](FEATURES.md),
[`docs/MEMPALACE_PARITY.md`](docs/MEMPALACE_PARITY.md),
[`docs/MCP_TOOL_MATRIX.md`](docs/MCP_TOOL_MATRIX.md).

Optional UI: [`docs/EGUI_USAGE.md`](docs/EGUI_USAGE.md), design [`docs/EGUI_GRAPH_VIEW.md`](docs/EGUI_GRAPH_VIEW.md).

## Optional native client (`rag-mcp-ui`)

The egui client in **`crates/rag-mcp-ui`** is a separate workspace binary, so
the headless `rag-mcp` package has zero egui/eframe dependencies and plain
`cargo build` stays lean.

Live `--http` mode provides project Home, server-filtered Unified Library,
lex/vec/hybrid Search, Wiki reading/editing, project-scoped Connections,
Activity, background jobs, health/backup operations and document History with
bounded diff and CAS restore. Wiki writes and revision restores use CAS through
the gateway. HTTP transport errors are reduced to safe timeout/connect/generic
messages instead of exposing request URLs or query details. The project selector
is locked while a wiki save, restore, sync/cancel, checkpoint or backup request is
in flight; the submitted edit/restore state remains owned until its response.
Verified backups have a dedicated 30-minute client timeout. Snapshot mode is topology-only; `--db` is an exclusive
development/maintenance mode and must not run beside the gateway.

Short usage: [`docs/EGUI_USAGE.md`](docs/EGUI_USAGE.md). Design: [`docs/EGUI_GRAPH_VIEW.md`](docs/EGUI_GRAPH_VIEW.md).

### Build and run

```bash
cargo build -p rag-mcp-ui
cargo run -p rag-mcp-ui -- --http http://127.0.0.1:7432
cargo run -p rag-mcp-ui -- --snapshot ./graph.json --seed "Note title"
# exclusive live DB only when MCP is not holding the file:
cargo run -p rag-mcp-ui -- \
  --db /Users/themoretheless/.local/share/rag-mcp/rag.duckdb \
  --seed some-node-id
```

Use exactly one of `--http`, `--snapshot`, or `--db`. Optional graph flags are
`--seed`, `--depth` (default 1), and `--max-nodes` (default 100, hard layout cap
300). Logs stay on stderr. Connections use bounded RadialLocal layout; the
server never persists canvas coordinates.

### Export snapshot from DuckDB

Mode C dump (topology only, same shape as MCP `get_graph`). Exclusive open; path printed on stdout:

```bash
# Prefer a verified offline backup. Stop the gateway before opening live DB.
cargo run -p rag-mcp-ui -- export \
  --db /path/to/a/verified/offline-backup.duckdb -o graph.json
cargo run -p rag-mcp-ui -- export \
  --db /path/to/a/verified/offline-backup.duckdb --pkb -o pkb-graph.json
cargo run -p rag-mcp-ui -- --snapshot graph.json --seed "Note title"
```

### Coexistence with MCP (single writer)

| Mode | Writer | UI | Use |
|------|--------|-----|-----|
| **A** Exclusive live | UI `--db` | Live Store | Dev; **MCP off** |
| **B** Gateway + HTTP client | gateway | `--http` | Normal agent and native use |
| **C** Snapshot | gateway (or export) | `--snapshot` read-only | Offline review / portable topology |
| **D** Dual-live write | - | - | **Forbidden** |

One process owns DuckDB writes. Never open UI `--db` on the same file the
gateway already holds. Prefer `--http` for live refresh; snapshot refresh means
re-exporting JSON, not opening a second live writer.

## License

MIT
