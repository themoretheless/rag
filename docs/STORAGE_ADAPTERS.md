# Storage adapters (multi-backend)

**Status:** DuckDB production adapter and a bounded Markdown document adapter are implemented; the full multi-backend contract remains a design boundary.

**Default backend:** DuckDB (single-file, bundled, local-first)

**Goal:** domain logic (chunk, embed, graph, wiki, MCP tools) must not depend on DuckDB types.

**Principles:** one logical store, explicit physical capabilities. See [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md) §1.2 · [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) · map: [`SYSTEM_MAP.md`](SYSTEM_MAP.md) §6.

Current implementation status: `RAG_STORAGE_BACKEND=duckdb` opens the only full application adapter. The backend-neutral factory also supports opt-in `markdown` document CRUD when an explicit `RAG_VAULT_PATH` is set. `sqlite`, `postgres`, and `memory` are recognized names that fail explicitly. Markdown files and frontmatter are the document source of truth, but search, chunks, graph, transactions, and vectors are not adapter capabilities. The Markdown adapter can rebuild a lightweight lexical JSONL sidecar or explicitly watch it without changing the default runtime.

A remote vector DB alone is **not** a primary store. Any future split must retain a metadata/graph source of truth and publish explicit capabilities.

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

Principle: **one domain contract per proven capability**, with explicit physical
backend metadata. Embeddings and FTS may eventually be native to a backend or
emulated in Rust, but the current application implementations remain DuckDB
specific.

**Markdown is a first-class document backend**, not a full application backend.
Its files can be edited and git-diffed; MCP search/link/ingest still require the
DuckDB runtime.

---

## 2. Layering

```
MCP / HTTP / wiki / graph / ingest ──→ Store ──→ DuckDB (full runtime)

document-contract consumers ──→ dyn Storage ──┬─→ DuckDB
                                               └─→ Markdown vault
```

The two paths are intentional. `Storage` is not yet the application façade and
does not claim search, graph, wiki, transaction or maintenance parity. A future
metadata/vector/full-text split is justified only by a measured workload and
must preserve one metadata/graph source of truth.

---

## 3. Implemented contract

```rust
pub trait Storage: Send + Sync {
    fn metadata(&self) -> BackendMetadata;
    fn upsert_document(&self, document: &Document) -> Result<(), AppError>;
    fn get_document(&self, id: &str) -> Result<Option<Document>, AppError>;
    fn list_documents(&self) -> Result<Vec<Document>, AppError>;
    fn delete_document(&self, id: &str) -> Result<bool, AppError>;
}
```

The contract is synchronous because both shipped implementations are local.
Capability metadata is explicit. Extend the interface only alongside a second
consumer and shared conformance tests; do not predeclare a full backend API.

---

## 4. Backend matrix

| Backend | Current capability | Product status | Advancement gate |
|---------|--------------------|----------------|------------------|
| **DuckDB** | Documents, chunks, exact vectors, generation-aware FTS, graph, wiki, transactions, maintenance | Full default; shipped | Add ANN/VSS only after the measured retrieval threshold in [`ROADMAP.md`](ROADMAP.md) is crossed |
| **Markdown vault** | Document CRUD plus deterministic lexical JSONL rebuild/watch | Bounded opt-in library adapter; shipped | Full app backend only after search/chunk/graph/transaction conformance tests pass |
| **SQLite / Postgres / Memory** | Name parsing and explicit refusal | Not implemented | Require a concrete deployment or test need and the same capability suite |
| **Qdrant / LanceDB / LibSQL / MotherDuck** | Design candidates only | Not scheduled | Require measured evidence that DuckDB cannot meet an accepted workload |

---

## 4b. Markdown document backend

### Goals

- **Source of truth = files on disk** (human-readable, git-diffable, open in Obsidian).
- Preserve the full document body and metadata in readable files.
- Rebuild a disposable lexical catalog without treating it as source of truth.
- Refuse search/graph/wiki/transaction claims until their contracts exist.

### Implemented layout

```
$RAG_VAULT_PATH/                    # explicit root when backend=markdown
  <encoded-layer>/                  # for example raw/ or wiki/
    <encoded-document-id>.md        # JSON/YAML-1.2 frontmatter + body
  .rag/                             # rebuildable machine sidecars
    documents.v1.jsonl              # deterministic lexical metadata; rebuildable
```

Each adapter-written note uses JSON frontmatter (a YAML 1.2 subset) containing
the complete `Document` metadata needed for round-trip document CRUD:

```text
---
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "uri": "vault://wiki/concepts/llm-wiki",
  "title": "LLM Wiki",
  "metadata_json": "{\"tags\":[\"rag\",\"karpathy\"]}",
  "created_at": "2026-07-29T12:00:00Z",
  "updated_at": "2026-07-29T12:00:00Z",
  "wing": "research",
  "room": "architecture",
  "layer": "wiki",
  "kind": "concept",
  "content_hash": "abc...",
  "status": "active",
  "pinned": false,
  "boost": 1.0,
  "revision": 1
}
---

Body with [[wikilinks]] and #tags …

## Related
- [[Hybrid Search]]
```

### Current and future mapping

| Domain | Markdown representation |
|--------|-------------------------|
| Document | one `.md` file; body = content; meta = frontmatter |
| uri | stored in frontmatter; filesystem path is derived from encoded layer and document id |
| Chunks, embeddings | not implemented by the Markdown adapter |
| Graph, wiki, ops log | not implemented by the Markdown adapter |
| Lexical catalog | `.rag/documents.v1.jsonl`; rebuilt from Markdown and never read as body source |

### Target operating modes beyond the document slice

| Mode | Env | Behavior |
|------|-----|----------|
| **Pure markdown** | Proposed `RAG_MARKDOWN_INDEX=none` | Search = lexical scan over files; vectors off |
| **Markdown + retrieval sidecar** | Proposed explicit index setting | Files remain SoT; the sidecar provides hybrid retrieval and is rebuildable |

Neither mode is a full application backend today. The shipped `.rag/documents.v1.jsonl` sidecar stores deterministic lexical metadata only; it does not provide hybrid search or embeddings.

### Implemented write and index policy

- Document upsert writes frontmatter and body through a temporary file and
  rename, and removes the prior path when the document layer changes.
- The adapter itself does not enforce raw immutability; application policies
  must not be assumed when using the document contract directly.
- The sidecar is never hand-edited. `reindex` and the explicit watcher rebuild
  it from Markdown; a future full backend must additionally invalidate derived
  chunks and embeddings.

`MarkdownVaultStorage::reindex` scans regular `.md` files in sorted path order, skips symlinks and every `.rag` subtree, rejects canonical paths outside the vault, and atomically replaces `.rag/documents.v1.jsonl`. Each versioned JSONL record contains the frontmatter document ID, relative path, a BLAKE3 body hash, and small lexical fields (title/layer/kind, counts, and sorted unique terms). The sidecar contains no vectors and is always disposable.

`MarkdownVaultStorage::watch_sidecar` is an explicit blocking library API with a caller-owned stop callback; opening any backend never starts it. It debounces and coalesces filesystem events, ignores `.rag`, symlinks, editor/temporary files, and unsafe paths, then atomically replaces the JSONL sidecar after updating or removing only affected paths. Missing/corrupt sidecars, ambiguous events, duplicate IDs, and watcher overflow/errors conservatively trigger a full `reindex`. There is no built-in daemon lifecycle.

Graph and wikilink resolution are intentionally absent from this adapter. Live
vault parsing versus a derived graph cache is a future conformance decision,
not current behavior.

### Implemented config

| Env | Meaning |
|-----|---------|
| `RAG_STORAGE_BACKEND=markdown` | select the bounded vault adapter through the library `open_configured_storage`; the production `rag-mcp` binary rejects this incomplete application backend |
| `RAG_VAULT_PATH` | explicit vault root (required; no `RAG_DB_PATH` fallback) |

`RAG_MARKDOWN_INDEX`, `RAG_MARKDOWN_WATCH`, and
`RAG_MARKDOWN_GITIGNORE_SIDECAR` remain design placeholders and are not parsed.
Sidecar rebuild/watch is an explicit library call.

The adapter's `reindex` and watcher are library APIs, not MCP tools. The shipped
DuckDB `export_vault` command produces a human/Git-oriented export, but it is
not claimed as a lossless round-trip into this adapter layout.

### Completion gate

Document CRUD, deterministic JSONL rebuild/watch and DuckDB `export_vault` are
shipped as separate capabilities. A Markdown application backend is complete
only when it passes the
same document, chunk, retrieval, graph, transaction, repair, and concurrency
contracts as DuckDB. Until then the factory must refuse unsupported full-runtime
combinations rather than degrade silently.

---

## 5. Configuration boundary

| Env | Example | Meaning |
|-----|---------|---------|
| `RAG_STORAGE_BACKEND` | `duckdb` (default) \| `markdown` \| `sqlite` \| `postgres` \| `memory` | selects a recognized adapter name; the production binary currently accepts only DuckDB, while the library document factory also accepts Markdown |
| `RAG_DB_PATH` | `./rag.duckdb` | path for DuckDB |
| `RAG_VAULT_PATH` | `./vault` | required root for the bounded `markdown` adapter; never inferred from `RAG_DB_PATH` |

`RAG_DATABASE_URL`, `RAG_VECTOR_BACKEND`, `RAG_QDRANT_URL`, and
`RAG_PG_POOL_SIZE` are design names only; the binary does not parse them.

The full DuckDB runtime's current `status` / `doctor` responses report backend
identity, capability names, the database path, schema version, FTS readiness,
embedding dimensions and integrity counts. The path is not redacted, and there
is no separate `vector_mode` field; callers must treat these local diagnostics
accordingly.

---

## 6. Capability declarations

The implemented metadata uses one closed capability enum:

```rust
pub enum StorageCapability {
    Documents,
    FullTextSearch,
    VectorSearch,
    Transactions,
    Graph,
    TemporalKnowledgeGraph,
}
```

DuckDB advertises its full set through status/doctor. Markdown advertises only
`Documents`. Unsupported full-runtime configurations fail explicitly; they do
not silently degrade hybrid search or alias another backend.

---

## 7. Migration state and next boundary

Implemented:

- `Storage` owns the small document lifecycle contract.
- DuckDB and Markdown implement that contract.
- DuckDB remains the concrete full-runtime store.
- Markdown provides document CRUD, deterministic lexical sidecar rebuild/watch,
  and a DuckDB-to-vault export path.
- Portable `export_bundle` / `import_bundle` are shipped recovery tools. Bundle
  v2 carries the canonical embedding manifest with chunk vectors; import checks
  exact target identity and dimensions. Vector-bearing v1 requires explicit
  live-provider re-embedding through the gateway and is refused by the offline
  CLI; metadata-only v1 is safe to upgrade. JSON/JSONL is deliberately bounded
  to 64 MiB, 10,000 documents and 50,000 chunks; larger corpora use a verified
  DuckDB backup rather than portable in-memory materialization.

Future work is not backend-count driven. Extend the contract only for a real
consumer, specify capability and failure semantics first, and require a shared
conformance suite before routing the application through another adapter.

If a second SQL backend is justified, its dialect-specific migrations stay in
that adapter. Do not force current DuckDB DDL into a premature lowest-common-
denominator schema.

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

**Cross-backend migration:** `export_bundle` / `import_bundle` provide the
shipped portable document-and-chunk path. Graph/index data remains derived and
must be rebuilt after import. Portability does not erase vector provenance:
bundle v2 includes the canonical provider/model/dimensions/base-URL manifest,
and a target with existing chunks must match it exactly. This path is limited to
64 MiB / 10,000 documents / 50,000 chunks; it is not the full-corpus backup path.

---

## 10. Current ownership

```
src/storage/
  mod.rs                 # identity, capabilities, document trait and factories
  duckdb/mod.rs          # Store adapter for the document trait
  markdown/mod.rs        # Markdown CRUD and deterministic JSONL rebuild
  markdown/watcher.rs    # explicit blocking sidecar watcher
src/db/                  # full DuckDB repositories and transaction owner
```

Keep `src/db/*` as the full DuckDB implementation until an extraction removes
more coupling than it adds and passes the shared behavior suite.

---

## 11. MCP tools (backend-aware)

| Tool | Behavior |
|------|----------|
| `status` / `doctor` | report DuckDB backend identity and capabilities for the full runtime |
| `search` | DuckDB lex/vec/hybrid; no Markdown runtime fallback |
| `export_bundle` | shipped portable dump |
| `import_bundle` | shipped transactional load into DuckDB |

No separate tool per backend; config selects adapter at process start.
