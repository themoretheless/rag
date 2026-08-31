# Storage adapters (multi-backend)

**Status:** design for post-v1 / parallel to P0 graph+hybrid  
**Default backend:** DuckDB (single-file, bundled, local-first)  
**Goal:** domain logic (chunk, embed, graph, wiki, MCP tools) must not depend on DuckDB types.

**Principles:** one logical store, pluggable physical backend. See [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md) §1.2 · [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) · map: [`SYSTEM_MAP.md`](SYSTEM_MAP.md) §6.  

Current implementation status: `RAG_STORAGE_BACKEND=duckdb` opens the production adapter through the storage factory. `sqlite`, `postgres`, `markdown`, and `memory` are recognized migration targets but deliberately fail at startup until their adapters are implemented. `status` and `doctor` expose the active backend and capability list; `export_bundle` and `export_vault` provide migration boundaries.
Remote vector DB alone is **not** the primary source of truth; DuckDB (or markdown vault / SQL) remains SoT for documents and graph.

---

## 1. Why adapters

| Need | Without adapters | With adapters |
|------|------------------|---------------|
| Local laptop PKB | DuckDB file is ideal | keep DuckDB |
| **Obsidian / git wiki** | locked in binary DB | **Markdown vault** (files are source of truth) |
| Shared team / server | one process hits limits | Postgres + pgvector |
| Edge / embedded | want zero C++ deps | SQLite (+ optional sqlite-vec) |
| Huge ANN corpus | full-scan cosine hurts | Qdrant / Lance / Milvus for **vectors only** |
| Tests | heavy DuckDB link | in-memory mock / SQLite |

Principle: **one domain store API**, many physical backends.  
Embeddings and FTS may be **native to the backend** or **emulated in Rust** (same as current cosine-over-JSON).

**Markdown is a first-class backend**, not only an export format: Karpathy LLM Wiki + Obsidian graph live as real files you can edit, git-diff, and open in Obsidian while MCP tools still search/link/ingest.

---

## 2. Layering

```
MCP tools / wiki / graph / ingest pipeline
              │
              ▼
     Arc<dyn Storage>     ← async-friendly facade (or sync + spawn_blocking)
              │
     ┌────────┼────────────┬──────────────┬─────────────┐
     ▼        ▼            ▼              ▼             ▼
  DuckDb   SqliteStore  PostgresStore  MarkdownVault  Composite
  Store                  (pgvector)    (md + sidecar) (SQL + Vector)
```

Optional split (P1+):

- `MetaStore` — documents, graph, wiki, ops_log (SQL)
- `VectorStore` — ANN / embed index (may be same DB or external)
- `FullTextStore` — BM25/FTS

`Storage` is the default façade that implements all three for simple backends.

---

## 3. Trait sketch (Rust)

```rust
/// Backend identifier for config + doctor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    DuckDb,
    Sqlite,
    Postgres,
    /// Git-friendly Obsidian/Karpathy vault: markdown files are source of truth
    Markdown,
    Memory,
    // later: Qdrant, Lance (as vector half of Composite)
}

/// Open options from env / CLI.
pub struct StorageConfig {
    pub kind: BackendKind,
    /// duckdb/sqlite path, or postgres DSN
    pub url_or_path: String,
    pub embed_dims: usize,
    pub pool_size: u32,
    // backend-specific JSON blob optional
    pub extra: serde_json::Value,
}

#[async_trait]
pub trait Storage: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn location(&self) -> &str; // path or redacted DSN

    // --- documents ---
    async fn upsert_document(&self, doc: &Document) -> Result<()>;
    async fn get_document(&self, id: &str) -> Result<Option<Document>>;
    async fn find_by_uri(&self, uri: &str) -> Result<Option<Document>>;
    async fn find_by_content_hash(&self, hash: &str) -> Result<Option<Document>>;
    async fn list_documents(&self, filter: DocumentFilter) -> Result<Vec<Document>>;
    async fn delete_document(&self, id: &str) -> Result<bool>;
    async fn delete_by_source(&self, source_file: &str) -> Result<u64>;

    // --- chunks + embeddings ---
    async fn insert_chunks(&self, chunks: &[Chunk]) -> Result<()>;
    async fn list_chunks_for_document(&self, doc_id: &str) -> Result<Vec<Chunk>>;
    async fn delete_chunks_for_document(&self, doc_id: &str) -> Result<()>;

    // --- search (backend may implement hybrid natively) ---
    async fn search(&self, req: SearchRequest) -> Result<Vec<SearchHit>>;

    // --- graph ---
    async fn upsert_graph_node(&self, node: &GraphNode) -> Result<()>;
    async fn insert_graph_edges(&self, edges: &[GraphEdge]) -> Result<()>;
    async fn delete_edges_from(&self, source_id: &str) -> Result<()>;
    async fn get_graph_view(&self, filter: GraphFilter) -> Result<GraphView>;
    async fn neighbors(&self, node_id: &str, depth: u32, max_nodes: u32) -> Result<GraphView>;
    async fn backlinks(&self, node_id: &str) -> Result<GraphView>;
    async fn find_nodes(&self, q: NodeQuery) -> Result<Vec<GraphNode>>;
    async fn link_nodes(&self, edge: &GraphEdge) -> Result<()>;

    // --- wiki / ops (may map to documents layer=wiki) ---
    async fn append_ops_log(&self, entry: &OpsLogEntry) -> Result<()>;
    async fn read_ops_log(&self, limit: u32) -> Result<Vec<OpsLogEntry>>;
    async fn wiki_index_rebuild(&self) -> Result<()>;
    async fn wiki_index_read(&self) -> Result<Vec<WikiIndexEntry>>;

    // --- health ---
    async fn stats(&self) -> Result<Stats>;
    async fn doctor(&self) -> Result<DoctorReport>;
    async fn migrate(&self) -> Result<()>;
}

pub fn open_storage(cfg: &StorageConfig) -> Result<Arc<dyn Storage>> {
    match cfg.kind {
        BackendKind::DuckDb => Ok(Arc::new(DuckDbStorage::open(&cfg.url_or_path)?)),
        BackendKind::Sqlite => Ok(Arc::new(SqliteStorage::open(&cfg.url_or_path)?)),
        BackendKind::Postgres => Ok(Arc::new(PostgresStorage::connect(&cfg.url_or_path)?)),
        BackendKind::Markdown => Ok(Arc::new(MarkdownVaultStorage::open(&cfg.url_or_path)?)),
        BackendKind::Memory => Ok(Arc::new(MemoryStorage::new())),
    }
}
```

**Sync note:** current code is sync `Store` + `Mutex`. Adapters may stay sync initially:

```rust
pub trait Storage: Send + Sync { /* sync methods */ }
```

MCP tools call `storage.*` the same way they call `store.*` today. Migrate to async later if Postgres pool needs it (`spawn_blocking` is fine for DuckDB/SQLite).

---

## 4. Backend matrix

| Backend | Meta (docs/graph/wiki) | Vectors | FTS/BM25 | Deploy | Priority |
|---------|------------------------|---------|----------|--------|----------|
| **DuckDB** | SQL tables | `embedding_json` + Rust cosine; later VSS | DuckDB FTS ext or Rust lex | single file | **P0 default** (shipped) |
| **Markdown vault** | `.md` files + YAML frontmatter; graph from `[[links]]` | sidecar (see below) | ripgrep/tantivy or sidecar FTS | vault directory | **P1 (high)** |
| **SQLite** | SQL tables | JSON blob + cosine; optional `sqlite-vec` | FTS5 | single file, lighter link | **P1** |
| **Postgres** | SQL | **pgvector** | native FTS / tsvector | server DSN | **P1** |
| **Qdrant** | no (pair with SQL) | ANN collections | payload filter only | server | **P2** composite |
| **LanceDB** | partial | ANN | limited | local/dir | **P2** |
| **Memory** | HashMap | Vec cosine | simple | tests only | **P1** for unit tests |
| **LibSQL / Turso** | SQL | like SQLite | FTS | edge/remote | **P2** |
| **MotherDuck** | DuckDB protocol | same as DuckDB | same | cloud DuckDB | **P2** (DSN variant of DuckDB) |

---

## 4b. Markdown vault backend (Obsidian / Karpathy-native)

### Goals

- **Source of truth = files on disk** (human-readable, git-diffable, open in Obsidian).
- MCP tools still: ingest, search, graph neighbors/backlinks, wiki compile, file_answer.
- No requirement that the user run a SQL DB to browse knowledge.

### Vault layout (default)

```
$RAG_VAULT_PATH/                    # or RAG_DB_PATH when backend=markdown
  raw/                              # immutable sources (Karpathy raw layer)
    articles/
      2026-04-llm-wiki.md
  wiki/                             # compiled / agent-written pages
    index.md                        # catalog (also rebuilt via tool)
    log.md                          # append-only ops timeline
    schema.md                       # AGENTS/schema conventions
    entities/
    concepts/
  notes/                            # general notes (layer flexible)
  .rag/                             # machine sidecars (gitoptional)
    manifest.json                   # embed model/dims, schema_version
    embeddings.duckdb               # optional: vectors only (hybrid mode)
    # OR embeddings.jsonl           # portable: {chunk_id, path, start, end, vec}
    fts/                            # optional tantivy or simple index dir
    cache/
```

Frontmatter on every note:

```yaml
---
id: 550e8400-e29b-41d4-a716-446655440000
title: LLM Wiki
uri: vault://wiki/concepts/llm-wiki
layer: wiki          # raw | wiki | diary
kind: concept        # document | entity | concept | source_summary | diary
wing: research
room: architecture
tags: [rag, karpathy]
content_hash: sha256:abc...
created_at: 2026-07-29T12:00:00Z
updated_at: 2026-07-29T12:00:00Z
---

Body with [[wikilinks]] and #tags …

## Related
- [[Hybrid Search]]
```

### Mapping domain ↔ files

| Domain | Markdown representation |
|--------|-------------------------|
| Document | one `.md` file; body = content; meta = frontmatter |
| uri | relative path (`wiki/concepts/foo.md`) or `vault://...` |
| Chunks | derived on read/ingest: split body (after frontmatter); not separate files by default |
| Embeddings | **not** in `.md` (keeps diffs clean) → `.rag/embeddings.*` sidecar |
| Graph nodes | document/tag/stub; stubs = optional `stubs/*.md` or only in sidecar until created |
| Graph edges | **primary:** parse `[[wikilinks]]` + `#tags` live from files; **cache:** `.rag/graph.json` rebuildable |
| Wiki index | `wiki/index.md` (+ optional structured `.rag/index.json`) |
| Ops log | `wiki/log.md` lines `## [ISO] op \| title` (Karpathy-parseable) |
| Schema | `wiki/schema.md` |

### Two operating modes

| Mode | Env | Behavior |
|------|-----|----------|
| **Pure markdown** | `RAG_MARKDOWN_INDEX=none` | Search = lex over files (grep/tantivy); vectors optional slow path or off |
| **Markdown + sidecar index** | `RAG_MARKDOWN_INDEX=duckdb` \| `sqlite` \| `jsonl` (default **duckdb** sidecar in `.rag/`) | Files remain SoT; hybrid vec+FTS from sidecar; reindex on file change / `doctor` / `reindex` tool |

Recommended default: **Markdown + small DuckDB/SQLite sidecar** under `.rag/` so hybrid search stays fast, while humans only touch `.md`.

### Write policy

1. **Raw files** (`raw/**`): MCP may create on ingest; **no in-place body mutation** except full replace via re-ingest (same as SQL raw layer).
2. **Wiki files**: agent tools may write/update; always rewrite frontmatter + body atomically (temp file + rename).
3. **Sidecar**: never hand-edited; rebuildable via `reindex` / `rebuild_index` / `doctor --repair`.
4. **External Obsidian edits**: detect mtime/content_hash mismatch → invalidate chunk embeddings for that file; optional file watcher (P2).

### Graph

- On `get_backlinks` / `get_neighbors`: prefer live parse of vault (correct after manual edits); optionally use `.rag/graph.json` if fresh.
- Stub targets: `[[Missing Page]]` with no file → stub node; creating `Missing Page.md` promotes stub (same semantics as SQL backend).

### Config

| Env | Meaning |
|-----|---------|
| `RAG_STORAGE_BACKEND=markdown` | enable vault adapter |
| `RAG_VAULT_PATH` | vault root (falls back to `RAG_DB_PATH` if unset) |
| `RAG_MARKDOWN_INDEX` | `duckdb` (default) \| `sqlite` \| `jsonl` \| `none` |
| `RAG_MARKDOWN_WATCH` | `0` \| `1` file watch reindex (P2) |
| `RAG_MARKDOWN_GITIGNORE_SIDECAR` | if `1`, ensure `.rag/` in vault `.gitignore` (default recommend true for embeddings) |

### MCP extras (markdown-specific, optional)

| Tool | Purpose |
|------|---------|
| `vault_status` | path, file counts, sidecar freshness (or fold into `status`) |
| `reindex` | rebuild embeddings + graph cache from all `.md` |
| `export_vault` / already files | no-op or sync check |
| `import_folder` | mine a directory of md into vault layout |

`export_vault` from SQL backends **writes this same layout**, so DuckDB → Markdown is a first-class migration path.

### Priority

- **P1:** read/write documents as md + frontmatter; wikilink graph from files; lex search; optional duckdb sidecar for vectors  
- **P1:** `export_vault` from DuckDB producing this layout  
- **P2:** file watcher, pure tantivy, multi-vault, conflict UI for concurrent Obsidian+MCP writes

### Composite pattern (P2)

```
PostgresMetaStore  +  QdrantVectorStore  =>  CompositeStorage
```

Document/chunk metadata and graph live in SQL; vectors in ANN engine; search does RRF in the façade.

---

## 5. Config (env)

| Env | Example | Meaning |
|-----|---------|---------|
| `RAG_STORAGE_BACKEND` | `duckdb` (default) \| `markdown` \| `sqlite` \| `postgres` \| `memory` | which adapter |
| `RAG_DB_PATH` | `./rag.duckdb` | path for duckdb/sqlite |
| `RAG_VAULT_PATH` | `./vault` | root for `markdown` backend (fallback: `RAG_DB_PATH`) |
| `RAG_MARKDOWN_INDEX` | `duckdb` \| `sqlite` \| `jsonl` \| `none` | sidecar search index for vault |
| `RAG_DATABASE_URL` | `postgres://user:pass@host/db` | DSN for postgres (and future remote) |
| `RAG_VECTOR_BACKEND` | `inline` \| `qdrant` | optional split |
| `RAG_QDRANT_URL` | `http://127.0.0.1:6333` | when vector backend = qdrant |
| `RAG_PG_POOL_SIZE` | `5` | postgres pool |

`doctor` must report: backend kind, location (redacted), vector mode, FTS mode, schema version, embed dims.

---

## 6. Capability flags

Not every backend supports every feature on day one. Expose:

```rust
pub struct StorageCaps {
    pub hybrid_fts: bool,
    pub native_ann: bool,
    pub transactions: bool,
    pub concurrent_writers: bool,
    pub json_metadata_filter: bool,
}
```

MCP `status` / `doctor` returns caps so agents do not call unsupported modes.  
`search(mode=hybrid)` on a backend without FTS → degrade to `vec` + warning field in result, or hard error (prefer **hard error with clear message** for honesty).

---

## 7. Migration path (from current code)

1. **Now (docs):** this file + SPEC/ROADMAP entries.  
2. **After graph-p0 stabilizes:** introduce `storage` module:
   - `Storage` trait = current `Store` method set (+ graph/search)
   - `DuckDbStorage` = rename/wrap existing `Store`
   - `RagServer` holds `Arc<dyn Storage>` instead of `Arc<Store>`
3. **P1:** `MarkdownVaultStorage` (files SoT + optional `.rag/` sidecar) + `export_vault` from DuckDB.  
4. **P1:** `SqliteStorage`, `MemoryStorage` for tests.  
5. **P1:** `PostgresStorage` with `sqlx` or `tokio-postgres` + pgvector.  
6. **P2:** `CompositeStorage`, Qdrant, Lance, MotherDuck DSN, vault file watcher.

SQL DDL should live as **backend-specific** migrations (`storage/duckdb/schema.rs`, `storage/sqlite/schema.rs`, …), not one shared DuckDB-only string forever. Shared **logical schema** documented once; dialects differ (`TIMESTAMP`, `INSERT OR REPLACE` vs `ON CONFLICT`, boolean types).

---

## 8. What stays backend-agnostic

- Chunking
- Embedding providers (mock/openai/ollama)
- Wikilink/tag extract + resolve (pure logic)
- RRF / diversity / token pack (if scores come from backend, fusion can stay in façade)
- Wiki compile policies, MCP tool surface
- content_hash, allowlist paths

---

## 9. Non-goals

- Supporting every vector DB on day one
- Lowest-common-denominator SQL that cripples DuckDB analytics
- Silent data loss when switching backends (export/import tool required before cross-backend migrate)
- Multi-master replication

**Cross-backend migrate (P2 tool):** `export_bundle` / `import_bundle` JSONL or parquet of documents+chunks+graph+wiki.

---

## 10. Suggested module layout

```
src/storage/
  mod.rs           # trait Storage, StorageConfig, open_storage
  caps.rs
  request.rs       # SearchRequest, DocumentFilter, NodeQuery
  duckdb/
    mod.rs
    schema.rs
    store.rs       # current Store moved here
    search.rs
    graph.rs
    fts.rs
  markdown/        # P1 — vault SoT
    mod.rs
    layout.rs      # paths: raw/, wiki/, .rag/
    frontmatter.rs
    vault_store.rs
    sidecar.rs     # embeddings + optional graph cache
    walk.rs        # discover .md files
  sqlite/          # P1
  postgres/        # P1
  memory/          # P1 tests
  composite/       # P2
```

Until the move, keep `src/db/*` as the DuckDB implementation and treat `docs/STORAGE_ADAPTERS.md` as the contract for the refactor.

---

## 11. MCP tools (backend-aware)

| Tool | Behavior |
|------|----------|
| `status` / `doctor` | include `backend`, `caps`, `location` |
| `search` | mode limited by caps |
| `export_bundle` | P2 portable dump |
| `import_bundle` | P2 load into current backend |

No separate tool per backend; config selects adapter at process start.
