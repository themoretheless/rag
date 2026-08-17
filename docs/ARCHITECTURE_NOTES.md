# rag-mcp Target Architecture

**Product:** `rag-mcp` (Rust + DuckDB, stdio MCP)  
**Status:** design notes (target after SPEC v1 substrate)  
**Sources blended:** Karpathy LLM Wiki (compile-once), MemPalace (verbatim + palace scope), rag-mcp SPEC (vector chunks + Obsidian object graph)

**North star:** [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) · **Principles:** [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md) · **Map:** [`SYSTEM_MAP.md`](SYSTEM_MAP.md)

This document is the **layer mechanics** companion (data plane, workflows, invariants detail). It does not replace `SPEC.md` (v1 contract), vision/principles (product intent), or `FEATURES.md` / `docs/ROADMAP.md` (research / sequencing). It explains **how the three ideas fit together** in one logical store (DuckDB default adapter).

---

## 1. Problem the architecture solves

Three retrieval philosophies pull in different directions:

| Source | Core move | Failure mode if used alone |
|--------|-----------|----------------------------|
| **Classic RAG** (our v1) | Chunk + embed + cosine at query time | No accumulation: every answer re-derives synthesis from fragments |
| **Karpathy LLM Wiki** | Immutable raw → LLM compiles interlinked wiki (index, log, schema) | Without good search/scope, large wikis still need fallback retrieval |
| **MemPalace** | Verbatim drawers, wing/room hierarchy, hybrid search, optional temporal KG | Flat palace without compile: no compounding entity/concept pages |

**Target thesis for rag-mcp:**

> Keep **verbatim raw** (MemPalace drawers) and **vector + FTS + graph** as honest substrate. Sit a **compiled wiki layer** (Karpathy) on top, agent-written, index-first, logged. Scope everything with **wing/room** and Obsidian-like **nodes/edges**. One binary, one DuckDB file, no external vector DB as primary store.

Hybrid BM25+vector does **not** replace compile; it makes raw fallback and large catalogs work. Compile does **not** delete raw; raw remains source of truth.

---

## 2. Design principles (ordered)

1. **Single logical store, pluggable physical backend.** Domain code depends on a `Storage` trait; **DuckDB is the default adapter** (single file, local-first). **Markdown vault** (`.md` SoT, Obsidian/Karpathy), SQLite, Postgres+pgvector, composite (SQL + Qdrant) are first-class — see [`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md). No mandatory Chroma/Pinecone primary path for v1.
2. **Layers, not mutually exclusive modes.** Raw is immutable; wiki/schema/index/log are agent-mutable; graph spans both.
3. **Verbatim first, synthesis second.** Ingest stores full text and chunks without mandatory LLM extract. Compilation is an explicit agent workflow (`ingest_source` + `write_wiki_page` / `file_answer`), not a silent rewrite of source rows.
4. **Deterministic graph extract for structure.** `[[wikilink]]`, `#tag`, stubs, promote-on-title: keep SPEC behavior. No mandatory LLM cognify / LightRAG NER for core graph.
5. **Index-first when small, hybrid when large.** Prefer `query_with_index` / catalog navigation; fall back to `search_wiki` then scoped `search` / `search_raw`.
6. **Scope before flat scan.** `wing`, `room`, `source_file`, `layer`, tag filters on search and graph export.
7. **Compounding memory.** Good answers are filed (`file_answer`); ops are append-only (`ops_log`); re-ingest is content-hash / uri idempotent.
8. **MCP surface stays substrate-honest.** Do not rename existing tools into a 36-tool “palace” dialect. Add tools; do not rebrand `search` as drawers-only jargon by default.
9. **Portable embeddings path.** Cosine in Rust over `embedding_json` until VSS/HNSW is justified (P2). FTS/BM25 is P0.
10. **Agent owns synthesis; server owns storage, search, graph hygiene, policy.**

---

## 3. Conceptual model: five layers

```
┌─────────────────────────────────────────────────────────────┐
│  L4  Agent / MCP client  (compile, query, lint, file_answer) │
├─────────────────────────────────────────────────────────────┤
│  L3  Compiled knowledge   wiki pages · index · schema · log  │
├─────────────────────────────────────────────────────────────┤
│  L2  Object graph         nodes · edges · stubs · tunnels    │
├─────────────────────────────────────────────────────────────┤
│  L1  Retrieval substrate  chunks · embeddings · FTS · RRF    │
├─────────────────────────────────────────────────────────────┤
│  L0  Verbatim corpus      raw documents · wing/room · hash   │
└─────────────────────────────────────────────────────────────┘
     Storage trait → DuckDB | Markdown vault | SQLite | Postgres | …
```

| Layer | Karpathy analogue | MemPalace analogue | rag-mcp tables / tools |
|-------|-------------------|--------------------|------------------------|
| **L0 Verbatim** | Raw sources (immutable) | Drawers + full text | `documents` (`layer=raw`, `immutable`), `raw_sources` optional; `ingest_raw`, `get_source` |
| **L1 Retrieval** | Optional qmd-style hybrid later | Hybrid BM25+vector, scoped search | `chunks.embedding_json` + FTS; `search(mode=lex\|vec\|hybrid)` |
| **L2 Graph** | Wikilinks in markdown / Obsidian view | Halls/tags; tunnels | `graph_nodes`, `graph_edges`; extract/resolve; `get_neighbors`, `get_backlinks` |
| **L3 Compile** | Wiki + schema + index.md + log.md | Closets/summaries (optional); L0–L1 wake-up later | `wiki_pages` / `layer=wiki`, `wiki_index_entries`, `wiki_schema`, `ops_log` |
| **L4 Agent** | Human+LLM workflows | wake_up, diaries, checkpoint | MCP tools; client protocols (hooks P2) |

**Mapping vocabulary (use product names, not marketing):**

| External term | rag-mcp name |
|---------------|--------------|
| Drawer | chunk (+ full document content) |
| Wing / room | `documents.wing` / `documents.room` |
| Hall | tag nodes + optional category |
| Tunnel | `graph_edges.rel_type = 'tunnel'` |
| Closet (summary) | wiki page `kind=source_summary` (compiled, not a rewrite of raw) |
| Wiki page | `wiki_pages` or `documents.layer='wiki'` |
| index.md | `wiki_index_entries` + `read_index` / `rebuild_index` |
| log.md | `ops_log` + `append_log` / `read_log` |
| Schema / AGENTS.md | `wiki_schema` + `get_schema` / `update_schema` |

---

## 4. Data plane (DuckDB)

### 4.1 Keep (SPEC v1 substrate)

- `documents` — full text, uri, title, metadata, timestamps  
- `chunks` — content, `embedding_json`, char offsets, document_id  
- `graph_nodes` — kinds: `document | tag | stub | entity`  
- `graph_edges` — types: `wikilink | tagged | related | mentions` (+ later `tunnel`)

Embeddings: JSON float arrays; rank cosine in Rust (portable). Connection: single open, `Mutex` (DuckDB not Sync).

### 4.2 Additive columns / tables (target)

**Documents (scope + layering):**

- `content_hash`, `wing`, `room`, `source_file`
- `layer` ∈ `raw | wiki | diary | schema | index | log` (default `raw`)
- `kind` ∈ `document | wiki | entity | concept | source_summary | diary | schema`
- `category`, `immutable` (raw sources: true after ingest)
- Unique index on `uri`; indexes on hash, wing/room, layer

**Retrieval:**

- DuckDB FTS index on chunk content (maintain on ingest/delete)
- Search hit provenance: vec score, lex score, fused score, snippet, wing/room, offsets

**Compile layer (preferred split for policy clarity):**

- `wiki_pages` — slug, title, content, kind, category, tags, source_ids, frontmatter, graph_node_id  
- `wiki_index_entries` — catalog row per page (title, kind, category, one-line summary)  
- `wiki_schema` — singleton conventions document  
- `ops_log` — append-only: seq, ts, op, prefix, message, entity_id, payload_json  

**Optional (if not folded into documents):**

- `raw_sources` — strict immutable registry when `documents` is too mixed  

**Corpus integrity (P0):**

- `embedding_manifest` — singleton fingerprint: provider, model, dims, base_url; vec/hybrid refuse mismatch  
- `schema_version` — migration version + applied_at  
- Chunks: `embed_text_hash` for embed cache; optional `parent_chunk_id`, `section_id`, `heading_path` (P1 expand)

**Agent memory (P1):**

- `kg_facts` — subject, predicate, object, valid_from/to, superseded_by, source_ids  
- diary documents: `kind=diary`, wing `agents/<name>`  
- Soft-delete: `documents.deleted_at`; optional `agent_id` / `session_id` scopes  
- Edge provenance: `graph_edges.chunk_id` / char offsets (P1)

### 4.3 Graph policy across layers

- Every raw document and every wiki page gets a graph node (stable id by uri/slug).
- Wikilink/tag extract runs on **both** raw and wiki content on write/update.
- Stub promotion: matching title/uri basename upgrades `stub` → `document`/`wiki` without breaking edge ids.
- Tunnels: explicit cross-wing edges (`rel_type=tunnel`), not inferred by default.
- Re-ingest by uri: prefer stable graph node id; rebuild edges from that source; respect `immutable` for raw content (new version = new source id or explicit supersede policy).

---

## 5. Control plane: workflows

### 5.1 Ingest (verbatim path)

```
ingest_text / ingest_file / ingest_raw
  → path allowlist check (ingest_file: RAG_INGEST_ROOTS)
  → content_hash, check_duplicate
  → begin atomic transaction
  → upsert document (layer=raw, immutable=true when raw policy)
  → chunk (fixed window v1; section-aware MD later)
  → embed (mock | openai-compatible); cache by embed_text_hash
  → insert chunks + FTS maintain (sync before tool return)
  → graph: upsert node, delete old edges from node, extract [[ ]] / #tags, resolve stubs
  → ops_log: INGEST
  → commit (or full rollback)
```

Server does **not** invent entity pages here. That is compile.  
`ingest_file` must refuse paths outside `RAG_INGEST_ROOTS` (empty list = refuse all absolute paths, or require explicit config; implementers document default).

### 5.2 Compile (Karpathy path)

```
ingest_source (or human: "process this source")
  → ensure raw registered (L0)
  → agent reads schema (get_schema)
  → agent: write_wiki_page / update_wiki_page (summary, entities, concepts)
  → server: chunk+embed wiki pages (searchable), graph extract on wiki markdown
  → update_index_entry / rebuild_index
  → append_log (INGEST | WIKI)
```

Rules:

- One source may touch many wiki pages (entity, concept, source_summary).
- Contradictions and supersessions: prefer explicit lint / `kg_supersede` later; flag in page body or log in P0.
- Schema co-evolves via `update_schema`; server does not hardcode domain page templates beyond kinds.

### 5.3 Query (ordered cascade)

1. **`query_with_index`** — read catalog, pick slugs, load pages, answer with wiki citations.  
2. **`search_wiki`** — hybrid/vector restricted to `layer=wiki` (and related kinds).  
3. **`search`** — `mode=lex|vec|hybrid`, filter DSL, min_score, snippets; **diversity** (MMR / collapse_by_document); **token pack** (`max_context_tokens`); citation-ready hit fields (`score_vec` / `score_lex` / `score_rrf`, offsets, quote).  
4. **`graph_expand_search`** — top hits → document/wiki nodes → `neighbors(depth)` subgraph.  
5. **`search_raw`** (P2 explicit) — force L0-only fallback.  
6. **`file_answer`** — persist high-value answer as wiki page + index touch + log (compounding).

Before vec/hybrid: load `embedding_manifest`; if config model/dims differ from corpus, return structured error pointing to `reembed`.

### 5.4 Lint and health

- **P0 ops:** `status` (counts, FTS healthy, embed dims, db path, `ready_for_search`); minimal `doctor` (schema_version, FTS present, embed mismatch, allowlist configured); `get_embedding_manifest` / `reembed`  
- **P1 graph/knowledge lint:** orphans, missing backlinks, resolve_stub, link_health, contradictions, stale claims, data gaps, mentioned-without-page  
- **P1–P2 ops:** full `doctor`, `backup_db`, later `repair`

### 5.5 Agent memory (P1, MemPalace-aligned)

- `wake_up` — return L0 identity + L1 essential story docs (special uris / wings)  
- `diary_write` / `diary_read`, `checkpoint`  
- `kg_add` / `kg_query` / `kg_invalidate` / `kg_supersede` / `kg_timeline`  
- Tunnels: create / list / delete / follow  

Not required for wiki compile to work; orthogonal product depth for multi-session agents.

---

## 6. Module map (target layout)

Build toward this ownership (v1 may still be flatter under `db/mod.rs`):

```
src/
  main.rs, lib.rs, config.rs, error.rs, models.rs
  chunking/          # fixed; later section-aware markdown
  embeddings/        # mock | openai; later local
  graph/
    extract.rs       # [[wikilink]], #tag
    resolve.rs       # stubs, promote, tunnels helpers
  db/
    schema.rs        # migrations: layers, FTS, wiki_*, ops_log, kg_facts
    store.rs         # document/chunk CRUD, hash, wing/room
    search.rs        # vec, lex, hybrid RRF, filters
    graph.rs         # nodes/edges, BFS, backlinks, expand
    wiki.rs          # pages, index, schema
    log.rs           # ops_log append/read
    kg.rs            # temporal facts (P1)
  compile/           # optional orchestration helpers (not LLM calls)
    ingest_source.rs # hooks: raw ensure + checklist for agent
    file_answer.rs
  mcp/
    server.rs, tools.rs
```

**Important:** the server does **not** call an LLM to “cognify” the graph (no mandatory NER). Default compilation is tool-driven: the **MCP client LLM** writes pages using wiki tools. **Optional** server-side chat (`RAG_LLM_*`, Ollama/LM Studio) may power `compile_*` / maintenance helpers when enabled; see [`LOCAL_LLM_MAINTENANCE.md`](LOCAL_LLM_MAINTENANCE.md). Server always owns storage, search, graph hygiene, and immutability policy.

---

## 7. MCP tool surface (architecture view)

### Substrate (keep / extend)

`ingest_text`, `ingest_file`, `search` (extend modes/filters), `list_documents`, `get_document`, `delete_document`, `stats` → `status`, `get_graph`, `get_neighbors`, `get_backlinks`, `link_nodes`, `find_node`

### Compile + palace + integrity (P0 cluster)

| Cluster | Tools |
|---------|--------|
| Scope | `list_wings`, `list_rooms`, `get_taxonomy`, `delete_by_source`, `check_duplicate` |
| Integrity | `status`, `doctor` (minimal), `get_embedding_manifest`, `reembed` / `reembed_document` |
| Raw | `ingest_raw`, `list_sources`, `get_source` |
| Wiki | `write_wiki_page`, `update_wiki_page`, `get_wiki_page`, `list_wiki_pages` |
| Catalog | `read_index`, `update_index_entry`, `rebuild_index` |
| Schema / log | `get_schema`, `update_schema`, `append_log`, `read_log`, `list_recent_ops` |
| Workflows | `ingest_source`, `query_with_index`, `search_wiki`, `file_answer`, `graph_expand_search` |
| Optional pack | `pack_context` (token-budgeted citation block) |

### Depth (P1+)

Lint suite, kg_*, wake_up, diaries, checkpoint, tunnels, forget/undelete, consolidate_memory, backup_db, expand_chunks (parent-child), multi_get, find_similar, export_vault / export_markdown, ingest_directory / sync_sources, MCP resources + prompts.

### Explicit non-builds (architecture constraints)

- External multi-backend vector DB as primary store  
- Mandatory LLM entity extraction for graph  
- AAAK dialect default-on  
- Rename existing tools to palace names  
- Server-side force-directed layout  
- Multi-tenant auth in early releases  

---

## 8. Target architecture diagram

```mermaid
flowchart TB
  subgraph Client["L4 MCP Client / Agent"]
    Human["Human: sources, questions"]
    Agent["LLM agent: compile, query, lint"]
  end

  subgraph MCP["rag-mcp stdio tools"]
    IngestT["ingest_text / ingest_file / ingest_raw"]
    IngestS["ingest_source"]
    WikiT["write/update/get/list wiki_page"]
    IndexT["read_index / rebuild_index / schema / ops_log"]
    SearchT["search hybrid · search_wiki · query_with_index"]
    GraphT["get_graph · neighbors · backlinks · graph_expand_search"]
    FileAns["file_answer"]
  end

  subgraph DuckDB["DuckDB single file"]
    subgraph L0["L0 Verbatim"]
      Raw["documents layer=raw / raw_sources\ncontent_hash · wing · room · immutable"]
    end
    subgraph L1["L1 Retrieval"]
      Chunks["chunks + embedding_json"]
      FTS["FTS / BM25"]
      RRF["RRF fusion"]
    end
    subgraph L2["L2 Object graph"]
      Nodes["graph_nodes\ndocument · tag · stub · entity · wiki"]
      Edges["graph_edges\nwikilink · tagged · related · tunnel"]
    end
    subgraph L3["L3 Compiled"]
      Wiki["wiki_pages\nentity · concept · source_summary"]
      WIndex["wiki_index_entries"]
      Schema["wiki_schema"]
      Log["ops_log append-only"]
    end
    subgraph P1["P1 Agent memory"]
      KG["kg_facts temporal"]
      Diary["kind=diary wings"]
    end
  end

  subgraph Embed["Embedding provider"]
    Mock["mock"]
    OAI["openai-compatible"]
  end

  Human --> Agent
  Agent --> IngestT
  Agent --> IngestS
  Agent --> WikiT
  Agent --> IndexT
  Agent --> SearchT
  Agent --> GraphT
  Agent --> FileAns

  IngestT --> Raw
  IngestT --> Chunks
  IngestT --> Nodes
  IngestT --> Edges
  IngestT --> Log
  Chunks --> Embed
  Chunks --> FTS

  IngestS --> Raw
  IngestS --> WikiT
  WikiT --> Wiki
  WikiT --> Chunks
  WikiT --> Nodes
  WikiT --> Edges
  WikiT --> WIndex
  WikiT --> Log

  IndexT --> WIndex
  IndexT --> Schema
  IndexT --> Log

  SearchT --> RRF
  RRF --> Chunks
  RRF --> FTS
  SearchT --> Wiki
  SearchT --> WIndex

  GraphT --> Nodes
  GraphT --> Edges
  GraphT --> RRF

  FileAns --> Wiki
  FileAns --> WIndex
  FileAns --> Log

  Wiki -.->|source_ids cite| Raw
  Nodes --> Raw
  Nodes --> Wiki
  KG --> Raw
  Diary --> Raw
```

### Query cascade (sequence)

```mermaid
sequenceDiagram
  participant A as Agent
  participant Q as query_with_index
  participant I as wiki_index_entries
  participant W as search_wiki / wiki_pages
  participant S as search hybrid
  participant G as graph_expand_search
  participant F as file_answer

  A->>Q: question
  Q->>I: read catalog
  alt index sufficient
    Q-->>A: page ids + citations
  else need content search
    A->>W: search_wiki
    W-->>A: wiki hits
  else need raw / hybrid
    A->>S: search mode=hybrid filters wing/room
    S-->>A: chunks + scores
    A->>G: expand top docs
    G-->>A: hits + subgraph
  end
  opt durable answer
    A->>F: file_answer
    F-->>A: new/updated wiki page
  end
```

---

## 9. Conflict resolutions (research → product)

| Tension | Resolution |
|---------|------------|
| Karpathy “anti-RAG / index only” vs hybrid BM25 | **Both:** index-first for compiled layer; hybrid is substrate quality and raw fallback (P0 co-priority). |
| MemPalace “never summarize” vs wiki summaries | **Split layers:** raw drawers never paraphrased; wiki summaries live only in L3 and cite raw ids. |
| Palace rename of tools vs SPEC names | **Keep SPEC names;** add wing/room as data, not as mandatory tool vocabulary. |
| LLM graph extract vs deterministic wikilinks | **Deterministic first;** entity kind reserved; cognify stays out of core. |
| Chroma multi-backend vs single DuckDB | **One logical store;** DuckDB is **default adapter** (not “never another backend”). Markdown vault / SQL adapters are first-class via `Storage` ([`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md)). Remote vector DB alone is not SoT; composite ANN half is optional scale (P2), not identity. |
| Closets vs wiki | Wiki `source_summary` / entity pages **are** the closet; no second parallel summary store. |
| “Server never calls LLM” vs local maintain | **No LLM for graph NER.** Optional local chat for compile/maintain tools; client remains default author of wiki knowledge. |
| Monolithic tool surface vs modular code | Many MCP tools OK; split `server.rs` by cluster without renaming the public surface. |

---

## 10. Phased arrival

| Phase | Architecture deliverable |
|-------|---------------------------|
| **v1 (SPEC)** | L0 partial + L1 vector only + L2 Obsidian graph + basic ingest/search tools |
| **P0** | L0 immutability + wing/room/hash; L1 FTS+hybrid RRF + diversity + citations + token pack; embedding_manifest/reembed; path allowlist; atomic ingest; L3 wiki/schema/index/log; graph_expand_search; query cascade |
| **P1** | Lint; tunnels; kg_facts; wake_up/diaries; forget; section/parent chunks; backup_db; export vault; MCP resources |
| **P2** | VSS/HNSW; HyDE; eval harness; watch/sync; local embedder; HTTP MCP; repair/hooks |

v1 must remain shippable without L3. Architecture is **additive migrations**, not a rewrite.

---

## 11. Invariants (definition of architectural success)

1. Deleting or corrupting wiki rows never loses raw source text.  
2. Re-ingest of identical `content_hash` is a no-op or explicit replace, never silent duplicate corpus bloat.  
3. Graph edge endpoints remain valid across re-ingest when uri/slug is stable.  
4. `search` can always fall back to raw chunks even if wiki is empty.  
5. `query_with_index` works with zero embeddings if catalog + pages exist (compile path offline-friendly with mock or empty L1).  
6. All significant mutations are visible in `ops_log`.  
7. Stdout remains MCP-only; logging on stderr.  
8. One process owns one active store path for writes (default: `RAG_DB_PATH` DuckDB); stdio MCP binary `rag-mcp`.  
9. `ingest_file` never reads outside `RAG_INGEST_ROOTS`.  
10. Vec/hybrid search never silently ranks with wrong embedding model/dims (manifest check or explicit reembed).  
11. Ingest is atomic: partial failure leaves neither half-chunks nor ghost edges.  
12. FTS is consistent with writes before the MCP tool returns (read-your-writes).  
13. Hybrid results are agent-usable: citation fields, diversity, and token budget are first-class, not afterthoughts.

---

## 12. Related docs

| Doc | Role |
|-----|------|
| [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) | **North star**: what the product is |
| [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md) | ordered laws + conflict resolutions |
| [`SYSTEM_MAP.md`](SYSTEM_MAP.md) | layers, modules, tool clusters |
| [`SPEC.md`](../SPEC.md) | v1 implementation contract |
| [`FEATURES.md`](../FEATURES.md) | research, tool list, proposed DDL, adversarial A1–A30 |
| [`docs/ROADMAP.md`](ROADMAP.md) | P0–P2 short bullets (includes integrity/safety) |
| [`docs/MCP_TOOL_MATRIX.md`](MCP_TOOL_MATRIX.md) | tool × Karpathy × MemPalace × priority |
| [`docs/STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md) | backend strategy |
| [`docs/REPOS_SURVEYED.md`](REPOS_SURVEYED.md) | survey evidence (~105 sources) |

**External primary patterns:** Karpathy LLM Wiki gist (raw / wiki / schema, index.md, log.md, ingest-query-lint); MemPalace (verbatim drawers, wing/room, hybrid retrieval, temporal KG, wake-up, no mandatory cognify).

---

*End of architecture notes.*
