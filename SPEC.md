# RAG MCP Server (Rust + DuckDB)

## Goal

Local stdio MCP server for Retrieval-Augmented Generation + Obsidian-like object graph:

- ingest text / files into a DuckDB-backed store
- chunk documents
- embed chunks (OpenAI-compatible API or deterministic mock for tests)
- semantic search over chunks
- **object graph** (nodes + edges): documents, tags, unresolved link stubs; wikilinks `[[...]]`, tags `#tag`, explicit links
- graph query tools (neighbors, subgraph, full graph export for UI)
- expose tools over MCP (`rmcp`, stdio transport)

Binary name: `rag-mcp`

### Product shape (Obsidian analogy)

| Obsidian | rag-mcp |
|----------|---------|
| Note | `GraphNode` kind=`document` (backed by `documents` row) |
| `[[wikilink]]` | directed edge `wikilink` source→target (resolve by title/uri; else stub node) |
| `#tag` | node kind=`tag` + edge `tagged` document→tag |
| Graph view | tool `get_graph` / `get_neighbors` returning `{nodes, edges}` JSON |
| Local graph | `get_neighbors(node_id, depth)` BFS up to depth |
| Unlinked mention | stub node `kind=stub`, `resolved=false` until a matching doc is ingested |

## Stack

| Piece | Choice |
|-------|--------|
| Language | Rust edition 2021 |
| MCP | `rmcp` 0.11+ with features `server`, `transport-io` (or latest stable 2.x/3.x that compiles) |
| DB | `duckdb` crate (bundled) |
| Async | `tokio` full |
| Serde | `serde`, `serde_json` |
| Schema for tools | `schemars` |
| HTTP embeddings | `reqwest` (json, rustls-tls) |
| Errors | `thiserror` + `anyhow` at boundaries |
| Logging | `tracing` + `tracing-subscriber` (stderr only; never stdout — MCP owns stdout) |
| IDs | `uuid` v4 |
| Time | `chrono` |

Prefer crates that compile on the machine. If `rmcp` 3.x API differs, adapt to whatever is on crates.io and builds.

## Config (env)

| Env | Default | Meaning |
|-----|---------|---------|
| `RAG_DB_PATH` | `./rag.duckdb` | DuckDB file path |
| `RAG_EMBEDDING_PROVIDER` | `mock` | `mock` \| `openai` |
| `RAG_EMBEDDING_BASE_URL` | `https://api.openai.com/v1` | OpenAI-compatible base URL |
| `RAG_EMBEDDING_API_KEY` | (empty) | Required when provider=openai |
| `RAG_EMBEDDING_MODEL` | `text-embedding-3-small` | Model name |
| `RAG_EMBEDDING_DIMS` | `1536` | Vector dimensions (must match model) |
| `RAG_CHUNK_SIZE` | `800` | Approx chars per chunk |
| `RAG_CHUNK_OVERLAP` | `120` | Overlap chars |
| `RAG_DEFAULT_TOP_K` | `5` | Default search limit |
| `RUST_LOG` | `info` | Tracing filter |

## Module map (file ownership)

```
src/
  main.rs                 # entry: config, open store, serve stdio
  lib.rs                  # re-exports for integration tests
  config.rs               # Config::from_env()
  error.rs                # AppError + Result alias
  models.rs               # Document, Chunk, SearchHit, IngestResult, Stats, GraphNode, GraphEdge, GraphView
  db/
    mod.rs
    schema.rs             # DDL, migrate/open (docs, chunks, graph_nodes, graph_edges)
    store.rs              # document/chunk CRUD
    search.rs             # vector similarity query
    graph.rs              # graph CRUD, resolve links, neighbors BFS, export
  chunking/
    mod.rs                # Chunker trait + factory
    fixed.rs              # fixed-size char chunker with overlap
  embeddings/
    mod.rs                # EmbeddingProvider trait + factory
    mock.rs               # deterministic hash embedding (tests / offline)
    openai.rs             # OpenAI-compatible /embeddings
  graph/
    mod.rs                # link extraction orchestration
    extract.rs            # parse [[wikilinks]] and #tags from markdown/text
    resolve.rs            # match targets to existing docs / create stubs
  mcp/
    mod.rs
    server.rs             # ServerHandler + tool_router
    tools.rs              # parameter structs (schemars)
tests/
  integration_test.rs     # ingest + search + graph link roundtrip
README.md
examples/mcp.client.json  # sample MCP client config
```

## Data model (DuckDB)

```sql
CREATE TABLE IF NOT EXISTS documents (
  id VARCHAR PRIMARY KEY,
  uri VARCHAR NOT NULL,
  title VARCHAR NOT NULL,
  content TEXT NOT NULL,
  metadata_json VARCHAR NOT NULL DEFAULT '{}',
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS chunks (
  id VARCHAR PRIMARY KEY,
  document_id VARCHAR NOT NULL,
  chunk_index INTEGER NOT NULL,
  content TEXT NOT NULL,
  -- embedding stored as JSON array of f32 for portability across duckdb versions
  embedding_json VARCHAR NOT NULL,
  char_start INTEGER NOT NULL,
  char_end INTEGER NOT NULL,
  created_at TIMESTAMP NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_chunks_document_id ON chunks(document_id);

-- Object graph (Obsidian-like)
CREATE TABLE IF NOT EXISTS graph_nodes (
  id VARCHAR PRIMARY KEY,
  kind VARCHAR NOT NULL,           -- document | tag | stub | entity
  label VARCHAR NOT NULL,          -- display title / tag name
  document_id VARCHAR,             -- set when kind=document
  uri VARCHAR,                     -- optional stable key
  resolved BOOLEAN NOT NULL DEFAULT true,
  metadata_json VARCHAR NOT NULL DEFAULT '{}',
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS graph_edges (
  id VARCHAR PRIMARY KEY,
  source_id VARCHAR NOT NULL,
  target_id VARCHAR NOT NULL,
  rel_type VARCHAR NOT NULL,       -- wikilink | tagged | related | mentions
  weight DOUBLE NOT NULL DEFAULT 1.0,
  context VARCHAR,                 -- optional snippet around the link
  created_at TIMESTAMP NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_graph_edges_source ON graph_edges(source_id);
CREATE INDEX IF NOT EXISTS idx_graph_edges_target ON graph_edges(target_id);
CREATE INDEX IF NOT EXISTS idx_graph_nodes_label ON graph_nodes(label);
CREATE INDEX IF NOT EXISTS idx_graph_nodes_document_id ON graph_nodes(document_id);
```

Notes:

- Store embeddings as JSON float arrays in `embedding_json`. Rank in Rust with cosine similarity (simple, portable, no VSS extension required).
- For small/medium corpora this is fine. Document future VSS path in README only.
- Connection: open once, wrap in `Arc<Mutex<Connection>>` or `r2d2` if needed. DuckDB connections are not Sync; use `Mutex`.
- Graph is first-class: rebuild edges for a document on every re-ingest (delete old edges from that doc node, re-extract).

## Domain types

```rust
pub struct Document {
    pub id: String,
    pub uri: String,
    pub title: String,
    pub content: String,
    pub metadata_json: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct Chunk {
    pub id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub content: String,
    pub embedding: Vec<f32>,
    pub char_start: i32,
    pub char_end: i32,
}

pub struct SearchHit {
    pub chunk_id: String,
    pub document_id: String,
    pub document_title: String,
    pub document_uri: String,
    pub chunk_index: i32,
    pub content: String,
    pub score: f32, // cosine similarity in [-1, 1]
}

pub struct IngestResult {
    pub document_id: String,
    pub chunk_count: usize,
}

pub struct Stats {
    pub document_count: u64,
    pub chunk_count: u64,
    pub node_count: u64,
    pub edge_count: u64,
    pub db_path: String,
}

pub struct GraphNode {
    pub id: String,
    pub kind: String,       // document | tag | stub | entity
    pub label: String,
    pub document_id: Option<String>,
    pub uri: Option<String>,
    pub resolved: bool,
    pub metadata_json: String,
}

pub struct GraphEdge {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub rel_type: String,   // wikilink | tagged | related | mentions
    pub weight: f64,
    pub context: Option<String>,
}

/// Payload shaped for Obsidian-like graph UIs / LLM consumption
pub struct GraphView {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}
```

## Chunking

- Split on character windows of `chunk_size` with `overlap`.
- Prefer break at nearest newline/space within last 20% of window when possible.
- Empty input → zero chunks.
- Preserve `char_start` / `char_end` relative to original content.

## Embeddings

### Trait

```rust
#[async_trait]
trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
}
```

### Mock

- Deterministic: hash text (e.g. FNV or blake3 of normalized text) into `dims` floats in [-1,1], L2-normalize.
- Same text → same vector. Used when `RAG_EMBEDDING_PROVIDER=mock` and in tests.

### OpenAI-compatible

- POST `{base}/embeddings` with `{"model","input":[...]}`.
- Header `Authorization: Bearer {key}`.
- Batch all inputs in one request when reasonable (chunk batches of ≤64).
- Validate returned length == dims.

## Store API

```rust
impl Store {
    pub fn open(path: &Path) -> Result<Self>;
    pub fn upsert_document(&self, doc: &Document) -> Result<()>;
    pub fn insert_chunks(&self, chunks: &[Chunk]) -> Result<()>;
    pub fn delete_document(&self, id: &str) -> Result<bool>; // also delete chunks
    pub fn get_document(&self, id: &str) -> Result<Option<Document>>;
    pub fn list_documents(&self) -> Result<Vec<Document>>;
    pub fn list_chunks_for_document(&self, doc_id: &str) -> Result<Vec<Chunk>>;
    pub fn all_chunks_with_embeddings(&self) -> Result<Vec<Chunk>>; // for search
    pub fn stats(&self) -> Result<(u64, u64, u64, u64)>; // docs, chunks, nodes, edges

    // Graph (see also db/graph.rs helpers)
    pub fn upsert_graph_node(&self, node: &GraphNode) -> Result<()>;
    pub fn insert_graph_edges(&self, edges: &[GraphEdge]) -> Result<()>;
    pub fn delete_edges_from(&self, source_id: &str) -> Result<()>;
    pub fn find_node_by_document_id(&self, doc_id: &str) -> Result<Option<GraphNode>>;
    pub fn find_nodes_by_label(&self, label: &str) -> Result<Vec<GraphNode>>;
    pub fn get_graph_view(&self, filter: GraphFilter) -> Result<GraphView>;
    pub fn neighbors(&self, node_id: &str, depth: u32, max_nodes: u32) -> Result<GraphView>;
    pub fn link_nodes(&self, source_id: &str, target_id: &str, rel_type: &str, weight: f64) -> Result<GraphEdge>;
}

pub struct GraphFilter {
    pub kinds: Option<Vec<String>>,
    pub rel_types: Option<Vec<String>>,
    pub seed_ids: Option<Vec<String>>,
    pub max_nodes: Option<u32>,
}
```

## Object graph (Obsidian-like)

### Link extraction (`graph/extract.rs`)

From document `content` (markdown-friendly text):

1. **Wikilinks**: `[[Target]]` and `[[Target|alias]]` → target label = `Target` (trim). Ignore images `![[...]]` as edges for v1 (optional: edge type `embeds` later).
2. **Tags**: `#tag` and `#multi/level` (Unicode letters/digits/`_/-`/`/`). Do not match inside code fences ` ``` ` or inline `` `code` `` if easy; best-effort regex OK for v1.
3. Return `Vec<ExtractedLink { target_label, rel_type, context, alias? }>`.

### Resolve + write (`graph/resolve.rs` + ingest)

On ingest of document D:

1. Ensure graph node for D: `kind=document`, `label=title`, `document_id=D.id`, `uri=D.uri`, `resolved=true`.
2. Delete existing edges where `source_id = D's node`.
3. For each extracted wikilink label L:
   - Find node with same label (prefer `kind=document` resolved, else any).
   - If none: create `kind=stub`, `label=L`, `resolved=false`.
   - Insert edge `wikilink` D→target with optional context snippet (±40 chars).
4. For each tag T:
   - Upsert node `kind=tag`, `label=T`, `resolved=true`.
   - Edge `tagged` D→tag.
5. When a new document is ingested whose **title** or **uri basename** matches an existing stub label, promote stub: set `kind=document`, `document_id`, `uri`, `resolved=true` (keep same node id so edges remain valid).

### Explicit links

Tool `link_documents` / `link_nodes` creates `rel_type=related` (or caller-supplied) without parsing content.

### Graph query semantics

- `get_graph`: export nodes+edges with optional filters (kinds, rel_types, max_nodes). Default cap 500 nodes.
- `get_neighbors`: BFS undirected for traversal UI (follow edge both ways), depth default 1, max_nodes default 100.
- `get_backlinks`: edges where target = node (who links here) — Obsidian backlinks panel.
- Layout is **not** computed server-side; return pure topology JSON for client/LLM.

### Hybrid RAG + graph (nice for agents)

- `search` remains pure vector.
- Optional tool `graph_expand_search`: run vector search → take top hit document nodes → `neighbors(depth=1)` → return hits + subgraph. Implement if time; otherwise document as follow-up.

## Search

1. Embed query via provider.
2. Load all chunk embeddings (or filter by document_id if provided).
3. Cosine similarity in Rust.
4. Sort desc, take top_k.
5. Return SearchHit list.

## Ingest pipeline

1. Build Document (uuid, uri, title, content, metadata, timestamps).
2. Upsert document (replace: delete old chunks for same id if re-ingest by uri — for v1: always new uuid unless `document_id` provided).
3. Chunk content.
4. Embed all chunk texts.
5. Insert chunks.
6. **Rebuild graph for document**: node upsert, extract links/tags, resolve stubs, write edges.
7. Return IngestResult `{ document_id, chunk_count, node_id, edge_count }`.

Re-ingest policy: if `uri` already exists, delete previous document+chunks+graph edges for that uri's node, then insert fresh (upsert by uri). Prefer **stable graph node id** per uri when re-ingesting so external references do not break.

`IngestResult` extended:

```rust
pub struct IngestResult {
    pub document_id: String,
    pub chunk_count: usize,
    pub node_id: String,
    pub edge_count: usize,
}
```

## MCP tools

| Tool | Params | Behavior |
|------|--------|----------|
| `ingest_text` | `text`, `title?`, `uri?`, `metadata_json?` | Ingest raw text + graph extract |
| `ingest_file` | `path`, `title?`, `uri?`, `metadata_json?` | Read UTF-8 file, ingest |
| `search` | `query`, `top_k?`, `document_id?` | Semantic search |
| `list_documents` | (none) | List docs (id, title, uri, created_at) — no full content |
| `get_document` | `document_id`, `include_chunks?` | Metadata + optional chunk texts (no embeddings) |
| `delete_document` | `document_id` | Delete doc + chunks + graph node edges; drop node if no remaining edges require it |
| `stats` | (none) | Counts (docs, chunks, nodes, edges) + db path |
| `get_graph` | `kinds?`, `rel_types?`, `max_nodes?` | Full/filtered graph `{nodes,edges}` |
| `get_neighbors` | `node_id`, `depth?`, `max_nodes?` | Local graph BFS (Obsidian local graph) |
| `get_backlinks` | `node_id` \| `document_id` \| `label` | Incoming edges + source nodes |
| `link_nodes` | `source_id`, `target_id`, `rel_type?`, `weight?` | Explicit edge (`related` default) |
| `find_node` | `label?`, `document_id?`, `node_id?` | Resolve node metadata |

All tools return JSON text content via `CallToolResult`.

Server instructions string must list every tool briefly, including graph tools and wikilink syntax.

Logging: write only to stderr.

## main.rs flow

1. Init tracing on stderr.
2. `Config::from_env()`.
3. `Store::open(&config.db_path)`.
4. Build embedding provider from config.
5. Build `RagServer { store, embedder, config, tool_router }`.
6. `serve(stdio()).await` + `waiting().await`.

## Tests

- Unit: chunker boundaries, mock embed deterministic, cosine similarity, wikilink/tag extraction, stub promotion.
- Integration (`#[tokio::test]`): temp dir duckdb, mock embedder, ingest two docs, search finds relevant chunk, delete removes, stats update.
- Graph integration: ingest note A with `[[B]]` and `#idea` before B exists → stub B + tag; ingest B with title `B` → stub promoted; `get_backlinks(B)` shows A; `get_neighbors(A, depth=1)` includes B and `#idea`.

## Local LLM and maintenance

Multi-provider chat via `RAG_LLM_PROVIDER` (see [`docs/LLM_PROVIDERS.md`](docs/LLM_PROVIDERS.md)):

| Provider | Value | Notes |
|----------|-------|--------|
| Ollama | `ollama` (default) | local OpenAI-compat |
| OpenAI | `openai` | `OPENAI_API_KEY` |
| Codex | `codex` | OpenAI host, code-oriented default model |
| Claude | `claude` | Anthropic Messages API |
| Kimi | `kimi` | Moonshot OpenAI-compat |
| DeepSeek | `deepseek` | OpenAI-compat |
| Custom | `custom` | any OpenAI-compat base URL |

| Env | Default | Role |
|-----|---------|------|
| `RAG_LLM_PROVIDER` | `ollama` | preset |
| `RAG_LLM_ENABLED` | `true` | enable chat tools |
| `RAG_LLM_BASE_URL` | provider default | override host |
| `RAG_LLM_MODEL` | provider default | chat model |
| `RAG_LLM_API_KEY` | provider fallbacks | key |
| `RAG_EMBEDDING_*` | mock/openai/ollama | independent of chat provider |

Maintenance loop (see [`docs/LOCAL_LLM_MAINTENANCE.md`](docs/LOCAL_LLM_MAINTENANCE.md)):

1. **analyze_corpus** — orphans, stubs, dups, stale wiki, size, doctor signals  
2. **plan_maintenance** — local LLM proposes JSON actions (whitelist only)  
3. **apply** (dry_run first) — refile, merge, consolidate, archive  
4. **compress** — dedupe, checkpoint/vacuum, optional near-dup merge  
5. **refresh** — reembed, FTS reindex, rebuild graph/index, stale wiki recompile  

Raw layer stays immutable unless `allow_raw_delete`. All applies go to `ops_log`.

## Information organization (упорядочивание)

Beyond ingest/search/graph, the product must support **ordering knowledge** so the corpus compounds and stays navigable. Full design: [`docs/ORGANIZE.md`](docs/ORGANIZE.md).

Five families:

| Family | Purpose | Example tools |
|--------|---------|----------------|
| **Place** | taxonomy / shelf | `refile`, `list_taxonomy`, `list_by_placement` |
| **Rank** | importance order | `pin`, `set_boost`, sort on list/search |
| **Structure** | outlines, trees, ordered packs | collections, `build_outline`, `topo_order` |
| **Compile** | fragments → durable pages | `consolidate`, `file_answer`, index rebuild |
| **Hygiene** | dedupe / merge / lint | `merge_documents`, `find_near_duplicates`, `lint_organization` |

Document fields (logical): `wing`, `room`, `layer`, `kind`, `pinned`, `boost`, `sort_key`, `status` (`active`\|`archived`), optional `parent_id`.

Collections: ordered `(collection_id, document_id, position)` for curated packs and stable `pack_context`.

**Default list order:** pinned → boost desc → sort_key asc → updated_at desc.

Raw layer remains immutable; organization changes metadata, wiki, collections, and graph — not silent raw body rewrites.

Priority: **P0.5** place+rank+archive+dedupe; **P1** collections, outlines, topo, merge, consolidate; **P2** cluster/refile suggestions.

## Storage adapters (multi-backend)

**Default:** DuckDB (current implementation in `src/db`).

**Direction:** domain code talks to a `Storage` trait; DuckDB is one adapter. See [`docs/STORAGE_ADAPTERS.md`](docs/STORAGE_ADAPTERS.md).

| Backend | Role | Priority |
|---------|------|----------|
| DuckDB | default local single-file | **shipped / P0** |
| **Markdown vault** | Obsidian/Karpathy: `.md` files = source of truth; optional `.rag/` search sidecar | **P1 (high)** |
| SQLite | lighter local / tests | **P1** |
| Postgres + pgvector | shared server | **P1** |
| In-memory | unit tests | **P1** |
| Qdrant / Lance (vectors) + SQL meta | scale ANN | **P2** composite |
| MotherDuck / LibSQL | remote DuckDB/SQLite flavors | **P2** |

Config (planned):

| Env | Meaning |
|-----|---------|
| `RAG_STORAGE_BACKEND` | `duckdb` \| `markdown` \| `sqlite` \| `postgres` \| `memory` |
| `RAG_DB_PATH` | path for duckdb/sqlite |
| `RAG_VAULT_PATH` | vault root when `markdown` (fallback `RAG_DB_PATH`) |
| `RAG_MARKDOWN_INDEX` | `duckdb` \| `sqlite` \| `jsonl` \| `none` — sidecar under `.rag/` |
| `RAG_DATABASE_URL` | postgres (and future DSN backends) |
| `RAG_VECTOR_BACKEND` | `inline` \| `qdrant` (P2) |

Markdown vault layout (summary): `raw/`, `wiki/` (`index.md`, `log.md`, `schema.md`), notes with YAML frontmatter; embeddings **not** inlined in md (sidecar). Graph edges from live `[[wikilinks]]` / `#tags`. Full design: [`docs/STORAGE_ADAPTERS.md`](docs/STORAGE_ADAPTERS.md) §4b.

`doctor` / `status` must report backend kind, caps (fts, native ANN, transactions, vault sidecar freshness), and redacted location.

Refactor steps: wrap current `Store` as `DuckDbStorage` implementing `Storage`; point `RagServer` at `Arc<dyn Storage>`; add **Markdown vault**, SQLite, Postgres without changing MCP tool names. `export_vault` from SQL backends writes the same md layout.

## Non-goals (v1)

- Hybrid BM25 (promoted to P0 in research roadmap; not all adapters day-one)
- HNSW/VSS index (P2; backend-specific)
- Multi-tenant auth
- Shipping every vector DB as primary store without SQL meta
- Force-directed layout / canvas rendering (clients own visualization)
- Block references `[[note#^block]]` (store full note link only)
- Bidirectional auto-sync of typed properties YAML (optional later)

## Implementation rules for agents

1. Own only your assigned files. Do not rewrite other modules beyond necessary `mod` / `use` / trait imports.
2. Prefer compiling code over comments.
3. Never log to stdout.
4. Use `Result` with `AppError`; map to MCP errors at tool boundary.
5. After your work, ensure `cargo check` would succeed for your module in isolation assumptions (public APIs match SPEC).
6. Russian comments only if existing style uses them; default English identifiers and docs.

## Research-derived backlog

Product north star (idea-level, supersedes backlog on identity conflicts): [`docs/ARCHITECTURE_VISION.md`](docs/ARCHITECTURE_VISION.md), [`docs/PRODUCT_PRINCIPLES.md`](docs/PRODUCT_PRINCIPLES.md), [`docs/SYSTEM_MAP.md`](docs/SYSTEM_MAP.md).

Post-v1 priorities from `FEATURES.md` / `docs/ROADMAP.md` (Karpathy LLM Wiki, MemPalace, hybrid retrieval, graph lint, ops). v1 non-goals above still apply to the first ship; hybrid BM25 is **promoted for the sprint after v1**, not for blocking the initial binary.

### P0 (next sprint after v1 tools green)

- FTS/BM25 on chunks + hybrid `search(mode=lex|vec|hybrid)` + RRF; filter DSL, min_score, snippets; diversity + `max_context_tokens`; citation-ready scores/offsets
- `content_hash`, `check_duplicate`, wing/room/source_file/layer columns, `get_taxonomy`, `delete_by_source`
- Immutable raw layer + wiki page CRUD + schema tools + index catalog + append-only `ops_log`
- `ingest_source`, `query_with_index`, `search_wiki`, `file_answer`, first-class `graph_expand_search`, extended `status`
- Integrity/safety: `RAG_INGEST_ROOTS`, `embedding_manifest` + `reembed`, atomic ingest, FTS sync before return, minimal `doctor`

### P1

- Entity/concept/source_summary pages; lint suite (orphans, missing backlinks, resolve_stub, contradictions, stale claims, data gaps)
- Temporal `kg_facts` + supersede/invalidate/timeline; `wake_up`, diaries, checkpoint, tunnels
- Section-aware chunking, multi_get, find_similar, and expand_chunks shipped; optional rerank remains
- Export vault/markdown; source sync/prune shipped; MCP resources/prompts remain

### P2

- VSS/HNSW when needed; HyDE; file watch; local embedder; read-only mode; repair/hooks. Eval harness, multi-format ingest, and HTTP MCP are shipped.

### Explicit non-builds (research)

- External multi-backend vector DB as primary store; mandatory LLM cognify; palace rename of tools; AAAK default-on; multi-tenant auth in early releases

Full analysis, tool list, and proposed DDL: see `FEATURES.md` (incl. adversarial integrity items). Short bullets: `docs/ROADMAP.md`. Tool matrix: `docs/MCP_TOOL_MATRIX.md`. Architecture: `docs/ARCHITECTURE_NOTES.md`.
