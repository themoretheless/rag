# rag-mcp Feature Research Report

**Product:** `rag-mcp` (Rust + DuckDB, stdio MCP)  
**Date:** 2026-07-29  
**Scope:** Karpathy LLM Wiki, MemPalace (Jovovich), retrieval/graph/ops/diff theme syntheses + ~100-repo pattern survey  
**Baseline:** current `SPEC.md` (vector chunks + Obsidian-like wikilink graph)

**Doc map:** [`docs/ARCHITECTURE_VISION.md`](docs/ARCHITECTURE_VISION.md) (north star) · [`docs/PRODUCT_PRINCIPLES.md`](docs/PRODUCT_PRINCIPLES.md) · [`docs/SYSTEM_MAP.md`](docs/SYSTEM_MAP.md) · [`SPEC.md`](SPEC.md) · [`docs/ROADMAP.md`](docs/ROADMAP.md) · [`docs/MCP_TOOL_MATRIX.md`](docs/MCP_TOOL_MATRIX.md) · [`docs/ARCHITECTURE_NOTES.md`](docs/ARCHITECTURE_NOTES.md) · [`docs/REPOS_SURVEYED.md`](docs/REPOS_SURVEYED.md)

Short bullets and tool tables live in ROADMAP / MCP_TOOL_MATRIX; this file is the full analysis + DDL + adversarial A1–A30.

---

## 1. Executive summary

rag-mcp already owns a strong **local dual substrate**: DuckDB documents/chunks with cosine embeddings, plus an Obsidian-like object graph (documents, tags, stubs, wikilinks, backlinks, neighbors). That combination is rare among Python MCP RAG demos (most are collection CRUD only) and is the right core for personal PKB / agent memory.

Research across Karpathy-style **compile-once wikis**, MemPalace-style **palace memory**, hybrid retrieval (BM25+vector+RRF), graph lints, and ops shells points to one product direction:

> **Do not only retrieve. Compile raw sources into a compounding knowledge layer (wiki + index + log + optional temporal KG), keep retrieval honest (hybrid when needed), and keep the store single-file DuckDB with deterministic graph extract.**

### Opinionated thesis for rag-mcp

| Keep (v1 identity) | Add next | Defer / non-goal |
|--------------------|----------|------------------|
| Single binary, stdio MCP, zero Python | FTS/BM25 + hybrid RRF search | External vector DBs, Chroma multi-backend |
| DuckDB single file, embeddings JSON | Wing/room scoped metadata columns | Mandatory LLM entity extract (cognify) |
| Wikilink + tag + stub graph | Immutable raw + LLM wiki compile layer | AAAK dialect, 36-tool palace rename |
| mock \| openai embeddings | Content-hash idempotent ingest | PDF parsers, local ONNX embed (later ops) |
| Graph tools (neighbors/backlinks/…) | Lint suite + resolve_stub | Multi-tenant auth, HTTP MCP (v1) |
| Brute-force cosine OK for personal corpus | Append-only ops/wiki log + index.md catalog | HNSW/VSS until scale hurts |

**Top conflict resolved:** Karpathy theme says *anti-RAG / compile-once* as P0 and demotes hybrid; retrieval theme says hybrid BM25 is P0. **Resolution for rag-mcp:** ship **hybrid search as P0 substrate quality** (cheap, DuckDB-native, does not fight compile-layer), and ship **wiki compile layer as co-P0 product differentiator**. Hybrid does not replace compile; it makes raw fallback and large index navigation work. Do not treat rerank/HyDE/VSS as P0.

---

## 2. Karpathy LLM Wiki method → implications for rag-mcp

### Method (concrete, not marketing)

1. **Immutable raw sources** — never mutate originals; everything else is derived.
2. **LLM-owned wiki markdown pages** — agent compiles sources into pages (entity/concept/source-summary).
3. **Schema file (`AGENTS.md` / schema)** — conventions the agent must follow when writing pages.
4. **Wiki page CRUD** — write/update/get/list as first-class tools.
5. **`index.md` content catalog** — navigation first, search second when catalog is small.
6. **`log.md` append-only timeline** — every significant op is logged with parseable prefixes.
7. **Ingest = source → multi-page compile** — one source can spawn summary + entity pages + index updates.
8. **Index-first query** — read index, open pages, answer with citations from wiki (not only raw chunks).
9. **File answer back to wiki** — good answers become durable pages (compounding memory).
10. **Lint umbrella** — orphans, missing cross-refs, missing concept pages, stubs, contradictions, stale claims, data gaps.
11. **Raw vs compiled enforcement** — store policy: raw layer immutable; wiki/index/log/schema mutable by agent.
12. **Optional hybrid BM25+vector only when index.md does not scale**; `search_raw` as fallback.

### Implications mapped to DuckDB + MCP

| Karpathy layer | rag-mcp mapping |
|----------------|-----------------|
| Raw sources | `documents` with `layer='raw'` or separate `raw_sources` table; no in-place content edits after ingest |
| Wiki pages | `documents` with `layer='wiki'` / `kind` in {wiki, entity, concept, source_summary} **or** `wiki_pages` table linked to graph nodes |
| Schema | Single document `uri=schema://agents` or `wiki_schema` row; tools `get_schema` / `update_schema` |
| index.md | `wiki_index` table or special doc + `rebuild_index` / `read_index` / `update_index_entry` |
| log.md | `ops_log` append-only table; tools `append_log` / `read_log` / `list_recent_ops` |
| Compile ingest | `ingest_source` / `ingest_raw` + agent-driven multi `write_wiki_page` (server can provide hooks; LLM does synthesis) |
| Query path | `query_with_index` → `search_wiki` → optional `search_raw` |
| File answer | `file_answer` → create/update wiki page + log entry + index touch |
| Lint | SQL + graph walks: `find_orphans`, `find_missing_backlinks`, `find_contradictions`, … |
| Wikilinks on wiki | Reuse existing extract pipeline on wiki content |

### Priority for this theme

- **P0:** raw immutability, wiki CRUD, schema, index catalog, append-only log, multi-page compile path, index-first query, file_answer, cited wiki answers, raw-vs-compiled policy.
- **P1:** entity/concept/source-summary page kinds, lint suite, list-by-category/tag, parseable log prefixes.
- **P2:** hybrid only when index fails, search_raw, frontmatter/wikilinks polish, git export, human-curates/LLM-maintains protocol notes.

**Do not demote existing SPEC tools.** `ingest_text`/`ingest_file`, `search`, graph nav remain substrate. Wiki layer sits **on top**.

---

## 3. MemPalace (Jovovich) method → implications

### Method (concrete)

| MemPalace concept | Meaning | rag-mcp map |
|-------------------|---------|-------------|
| Drawer | Verbatim storage unit | `chunks` + full `documents.content` |
| Content-hash drawer id | Deterministic idempotent id | `content_hash` column + `check_duplicate` |
| Wing / room | Hierarchical scope for filtered search | `wing`, `room`, `source_file` columns on documents |
| Hall | Taxonomy / tags | Existing tag nodes + optional hall labels |
| Tunnel | Cross-wing link | `graph_edges.rel_type='tunnel'` + tunnel CRUD/follow |
| L0–L3 stack | Identity → essential story → facts → drawers | Special `memory://` URIs + `wake_up` |
| Temporal KG | Facts with validity windows | New `kg_facts` table + supersede/invalidate/timeline |
| Agent diaries | Per-agent chronological notes | documents `kind=diary`, wing `agents/<name>` |
| Checkpoint | Batch write + diary | `checkpoint` tool |
| Know-before-speaking | Soft protocol | Client-side; optional `wake_up` returns L0/L1 |
| Sweep / sync / repair | Idempotent ingest, prune orphans, reconnect stubs | Ops tools P1–P2 |
| Hybrid search | Vector + BM25 + RRF, scoped | Same as retrieval P0, filter by wing/room |
| No mandatory cognify | Deterministic store | Aligns with SPEC stance |

### What to copy vs reject

**Copy:** wing/room scoped columns, content_hash + duplicate check, hybrid RRF, provenance on hits (offsets, scores, wing/room), delete_by_source, tunnels as graph edge type (P0 via `link_nodes`; tunnel CRUD tools P1), wake_up + diaries + kg_* as P1 agent-memory, sync/repair as P2, DuckDB-single-backend philosophy.

**Reject:** Chroma multi-backend, 36-tool “palace” naming surface, AAAK dialect on by default, mandatory LLM cognify.

### Priority for this theme

- **P0:** wing/room/source_file columns, hybrid BM25+vector+RRF, `check_duplicate`, taxonomy list, `delete_by_source`, allow `rel_type=tunnel` on existing graph edges (neighbors/backlinks already return all rel types), first-class `graph_expand_search` (aligned with SPEC post-v1 P0).
- **P1:** `wake_up`, `kg_*`, diaries, `checkpoint`, tunnel CRUD + `follow_tunnels` (edge type already valid in P0).
- **P2:** hooks protocol, sync/repair, optional rerank/AAAK, read-only env gate.

---

## 4. Survey of ~100 related repos

### Methodology

- **Sources:** theme syntheses (Karpathy wiki, MemPalace, retrieval, graph, ops, diff), public MCP/RAG/memory server patterns (Chroma MCP, qmd-style hybrid, Graphiti/Cognee-style temporal KG, Obsidian graph tools, FastMCP demos, local DuckDB/SQLite RAG).
- **Unit of analysis:** shipping feature or tool capability, not star count.
- **Coverage estimate:** ~95–110 distinct open-source or documented systems/patterns across MCP servers, GraphRAG kits, personal knowledge bases, and agent memory stores.
- **Dedup:** features counted once per product class (e.g. “hybrid BM25+vector” across many repos counts as one high-frequency pattern).

### Coverage count

| Class | Approx. count | Role in survey |
|-------|---------------|----------------|
| MCP vector/collection servers (Chroma, etc.) | ~25 | Baseline: CRUD + search only |
| Local RAG (SQLite/DuckDB/Lance) | ~15 | Embed + chunk + search |
| GraphRAG / knowledge graph memory | ~20 | Entity extract, communities, temporal |
| Obsidian / markdown PKB tools | ~15 | Wikilinks, backlinks, stubs, vault export |
| Agent memory (palace, diary, wake-up) | ~10 | Hierarchical scope, L0–L3 |
| Hybrid retrieval / eval kits | ~15 | BM25, RRF, rerank, metrics |
| **Total patterns surveyed** | **~100** | |

### Top feature frequency table

Frequency = how often the capability appears as a first-class product feature across the surveyed set (High ≥40%, Med 15–40%, Low <15% but strategically important).

| Feature | Freq | In SPEC v1? | rag-mcp decision |
|---------|------|-------------|------------------|
| Vector / embedding search | High | **Have** | Keep; improve provenance |
| Document ingest text/file | High | **Have** | Keep |
| Chunking + metadata | High | **Have** | P1: section-aware MD |
| Hybrid BM25 + vector | High | Missing (non-goal v1) | **P0** promote |
| RRF fusion | Med | Missing | **P0** with hybrid |
| Collection/dataset CRUD | High | Missing | P2 multi-tenant only |
| Wikilink extraction | Med | **Have** | Keep |
| Backlinks / neighbors | Med | **Have** | Keep |
| Stub / unresolved links | Low–Med | **Have** | + `resolve_stub` tool P1 |
| Graph export JSON | Med | **Have** | Keep |
| Graph expand after search | Med | Partial (optional) | **P0** first-class (`graph_expand_search`) |
| LLM entity extract (cognify) | Med | Non-goal | Stay out of v1–P1 core |
| Temporal fact validity | Low–Med | Missing | **P1** if agent memory |
| Content-hash / dedupe | Med | Missing | **P0** |
| Scoped search (namespace/wing) | Med | Missing | **P0** wing/room |
| Delete by source / prune | Med | Partial (delete_document) | **P0** delete_by_source |
| File watch / sync | Med | Missing | **P1–P2** ops |
| Markdown vault export | Med | Missing | **P1** (Karpathy + Obsidian) |
| Wiki compile / index.md / log | Low | Missing | **P0** differentiator |
| Lint orphans / missing links | Low | Missing | **P1** |
| MCP resources + prompts | Med | Missing | **P1** ops |
| HTTP MCP transport | Med | Non-goal v1 | **P2** |
| HNSW / VSS ANN | Med | Non-goal v1 | **P2** when scale |
| Rerank / HyDE | Med | Missing | **P1 optional / P2** |
| Eval harness (MRR, recall@k) | Low–Med | Missing | **P2** ops |
| Single-binary local-first | Low among MCP | **Have** (design) | Protect as differentiator |
| Zero mandatory cloud LLM for graph | Low | **Have** | Protect |

### Competitive position (diff theme)

**Wins vs typical Python MCP RAG:** one binary, one DuckDB file, fast cold start, dual vector+Obsidian graph, air-gapped mock mode, SQL-joinable docs/chunks/graph.

**Losses if we stay SPEC-frozen:** no BM25 (keyword-heavy queries fail), no compile layer (agents re-discover every session), no wing/room scope, no dedupe hash, weak lint, no temporal agent memory.

---

## 5. Gap analysis vs current SPEC.md

Legend: **Have** | **Partial** | **Missing**

### Retrieval

| Capability | Status | Notes |
|------------|--------|-------|
| Cosine vector search | Have | Full-scan portable |
| `search` filters (doc/tag/uri) | Partial | Only `document_id` |
| BM25 / DuckDB FTS on chunks | Missing | Explicit non-goal v1 → promote P0 |
| Hybrid + RRF | Missing | P0 |
| Hit provenance (offsets, multi scores) | Partial | title/uri/score; no wing, no lex score |
| min_score / snippet mode | Missing | P0 |
| FTS reindex on ingest/delete | Missing | P0 with FTS |
| status / index health | Partial | `stats` only |
| graph_expand_search | Partial | SPEC “if time” → **P0** first-class (SPEC + this doc) |
| Adjacent chunk expansion | Missing | P1 |
| Section-aware MD chunking | Missing | P1 |
| multi_get | Missing | P1 |
| find_similar document | Missing | P1 |
| Rerank / HyDE / VSS | Missing | P2 (or P1 optional rerank) |

### Graph (Obsidian-like — do not drop from roadmap)

SPEC v1 substrate tools and behaviors stay required forever; P0 only **extends** them. Never demote or replace:

| Capability | Status | Notes |
|------------|--------|-------|
| Wikilink `[[...]]` + tag `#tag` extract | Have | Re-run on wiki page writes too (P0 policy) |
| Stub create + promote on ingest | Have | No explicit `resolve_stub` tool until P1 |
| `get_graph` / `get_neighbors` / `get_backlinks` | Have | Obsidian graph view / local graph / backlinks panel |
| `link_nodes` / `find_node` | Have | Explicit edges + node resolve |
| Stable graph node id on re-ingest by uri | Have (policy) | Enforce in impl; P0 re-ingest must keep |
| Graph cleanup on `delete_document` | Partial | P0 harden with `delete_by_source` |
| `graph_expand_search` | Partial | **P0** first-class (vector/hybrid hits → neighbors) |
| find_orphans / missing backlinks | Missing | P1 lint |
| link_health | Missing | P1 |
| get_subgraph | Partial | filter seed_ids in Store API sketch; tool P1 |
| Tunnels (`rel_type=tunnel`) | Missing | **P0** allow edge type via `link_nodes`; CRUD/`follow_tunnels` **P1** |
| Temporal edges / kg_facts | Missing | P1 |
| LLM cognify / communities | Missing | Out of core / P2 research |
| entity kind + mentions | Partial | Schema reserved, no pipeline |

### Karpathy wiki layer

| Capability | Status | Notes |
|------------|--------|-------|
| Immutable raw layer | Missing | Policy + schema |
| Wiki page CRUD | Missing | P0 |
| Schema file tools | Missing | P0 |
| index.md catalog | Missing | P0 |
| Append-only log | Missing | P0 |
| ingest_source multi-page compile | Missing | P0 (agent-assisted) |
| query_with_index / search_wiki | Missing | P0 |
| file_answer | Missing | P0 |
| Lint suite | Missing | P1 |
| Export git-friendly markdown | Missing | P1–P2 |

### MemPalace / ops

| Capability | Status | Notes |
|------------|--------|-------|
| content_hash + check_duplicate | Missing | P0 |
| wing / room columns | Missing | P0 |
| list taxonomy | Missing | P0 |
| delete_by_source | Missing | P0 |
| tunnel edge type (`rel_type=tunnel`) | Missing | P0 allow; tunnel CRUD tools P1 |
| graph_expand_search | Partial | **P0** (same as SPEC / Graph section) |
| wake_up / diaries / kg_* | Missing | P1 |
| sync_sources / repair | Missing | P2 |
| multi-format / watch | Missing | P1–P2 |
| export vault | Missing | P1 |
| MCP resources/prompts | Missing | P1 |
| read-only mode | Missing | P2 |
| HTTP transport | Missing (non-goal v1) | P2 |
| Local ONNX embed | Missing | P2 ops |

### Explicit non-goals still valid for early v1 ship

From SPEC + diff theme (do not block first green binary): PDF parsers, HNSW, multi-tenant auth, force-directed layout UI, LangChain clone, LightRAG-style mandatory NER. **Exception:** BM25 hybrid is lifted from non-goal into post-v1 P0 once baseline tools work.

---

## 6. Prioritized roadmap

### P0 — next sprint (after SPEC v1 tools green)

Goal: **honest search + scoped memory + compile substrate without abandoning the Obsidian dual graph.**

Cross-check vs `SPEC.md` § Research-derived backlog P0 (must match intent):

| SPEC P0 bullet | FEATURES P0 items |
|----------------|-------------------|
| FTS/BM25 + hybrid `search(mode=lex\|vec\|hybrid)` + RRF; filters, min_score, snippets | 1–3 |
| `content_hash`, `check_duplicate`, wing/room/source_file/layer columns, `get_taxonomy`, `delete_by_source` | 4–6 (`layer`/`kind` columns in 5; raw immutability policy in 7) |
| Immutable raw + wiki CRUD + schema + index catalog + append-only `ops_log` | 7–11 |
| `ingest_source`, `query_with_index`, `search_wiki`, `file_answer`, first-class `graph_expand_search`, extended `status` | 12–15 |
| (product identity) keep Obsidian graph tools / wikilink extract; tunnel edge type | 0, 14, 16, store policy |

**0. Preserve SPEC v1 graph/Obsidian surface (not optional):**  
   `get_graph`, `get_neighbors`, `get_backlinks`, `link_nodes`, `find_node`; wikilink/tag extract; stub create + promote; stable node id per uri; `stats` includes node/edge counts. Wiki and raw tools **sit on top**; do not rename or replace these.

1. **DuckDB FTS BM25 on chunks** + maintain on ingest/delete  
2. **Hybrid search** `mode=lex|vec|hybrid` + **RRF**  
3. **Search filters** (document_id, tag, uri, wing, room, metadata) + min_score + snippets + dual scores  
4. **content_hash** + `check_duplicate` + idempotent re-ingest (keep **stable graph node id** on uri upsert)  
5. **wing / room / source_file** (+ document `layer` / `kind` columns) + `list_wings` / `list_rooms` / `get_taxonomy`  
6. **delete_by_source** + harden `delete_document` graph cleanup (edges, orphaned stubs/tags policy)  
7. **Immutable raw layer policy** (`layer` or `raw_sources`) + `ingest_raw` / `list_sources` / `get_source`  
8. **Wiki CRUD** (`write_wiki_page`, `update_wiki_page`, `get_wiki_page`, `list_wiki_pages`) — each write re-runs wikilink/tag graph extract  
9. **Schema tools** (`get_schema`, `update_schema`)  
10. **Index catalog** (`read_index`, `update_index_entry`, `rebuild_index`)  
11. **Append-only log** (`append_log`, `read_log`, `list_recent_ops`) / `ops_log` table  
12. **ingest_source** path (raw ingest + hooks for multi-page wiki updates)  
13. **query_with_index** + **search_wiki** + **file_answer**  
14. **graph_expand_search** first-class (vector/hybrid hits → document nodes → `neighbors`)  
15. **status** extended (FTS healthy, embed dims, docs/chunks/nodes/edges, db path)  
16. **Tunnel edge type** allowed: `graph_edges.rel_type` includes `tunnel` (create via existing `link_nodes`); dedicated tunnel CRUD/`follow_tunnels` remain **P1**  
17. **Retrieval honesty (adversarial MUST):** citation-ready SearchHit; `max_context_tokens`; diversity MMR/collapse_by_document; filter DSL operators; FTS stemmer env; embed cache by hash; sync FTS before tool return  
18. **Integrity/safety (adversarial MUST):** `RAG_INGEST_ROOTS`; `embedding_manifest` + `reembed`; `schema_version` + minimal `doctor`; atomic ingest transaction  

### P1 — product depth

- Page kinds: entity, concept, source_summary; list_by_category / list_by_tag  
- Lint suite: orphans, missing backlinks/cross-refs, missing concepts, contradictions, stale claims, data gaps, link_health, resolve_stub  
- Temporal KG: `kg_add|query|invalidate|supersede|timeline|stats`  
- Agent diaries + `wake_up` (L0 identity + L1 essential story)  
- `checkpoint`, `follow_tunnels`, tunnel CRUD  
- Adjacent chunk expand, section-aware markdown chunking, multi_get, find_similar  
- Optional LLM/cross-encoder rerank top-N  
- Export markdown vault / wiki index+log (Obsidian/git-friendly)  
- MCP resources (doc/chunk/graph URIs) + prompts (cited answer, folder ingest)  
- Directory ingest, sync_sources / prune_orphans (start of watch era)  
- Recency weighting, multi-query expansion RRF  

### P2 — scale, polish, optional protocols

- DuckDB VSS/HNSW when full-scan hurts  
- HyDE mode, retrieval eval harness (MRR, recall@k, nDCG)  
- File watcher auto-reingest, multi-format (html/pdf/code)  
- HTTP+TLS MCP, read-only env gate  
- Local embedding provider (ONNX/MiniLM)  
- search_raw fallback explicit, git-backed export polish  
- Client auto-save hooks protocol, integrity repair  
- Optional AAAK off-by-default, cost/usage metrics, soft multi-dataset  

### Out of scope (do not build as product core)

- Pluggable external vector DB matrix (Chroma/Pinecone/…) as primary store  
- Mandatory LLM cognify / LightRAG entity pipeline for graph  
- LangChain-scale orchestration runtime  
- Force-directed graph layout / canvas UI server-side  
- Multi-tenant auth product (v1–P1)  
- AAAK dialect default-on  
- 36-tool palace rename of existing tools  
- Block refs `[[note#^block]]` until raw/wiki layers settle  

---

## 7. Proposed new MCP tools list

### Keep (SPEC substrate — Obsidian graph included; never drop)

| Tool | Role (SPEC v1) |
|------|----------------|
| `ingest_text` / `ingest_file` | Ingest + chunk + embed + **graph extract** (wikilinks, tags, stubs) |
| `search` | Semantic vector search (P0 extends with modes/filters) |
| `list_documents` / `get_document` / `delete_document` | Doc CRUD; delete must clean chunks + graph edges/nodes |
| `stats` | docs, chunks, **nodes**, **edges**, db path |
| `get_graph` | Full/filtered `{nodes,edges}` (Obsidian graph view) |
| `get_neighbors` | Local graph BFS (Obsidian local graph) |
| `get_backlinks` | Incoming edges (Obsidian backlinks panel) |
| `link_nodes` | Explicit edge (`related` default; P0 also `tunnel`) |
| `find_node` | Resolve by label / document_id / node_id |

### P0 additions

| Tool | Role |
|------|------|
| `search` (extend) | `mode=lex\|vec\|hybrid`, filters, min_score, snippet, top_k |
| `check_duplicate` | content_hash / uri idempotency probe |
| `list_wings` / `list_rooms` / `get_taxonomy` | MemPalace-style scope nav |
| `delete_by_source` | prune by source_file / uri prefix (+ graph cleanup) |
| `status` | health beyond counts (FTS, dims, path, node/edge counts) |
| `graph_expand_search` | hybrid/vector hits + neighbor subgraph (SPEC follow-up → P0) |
| `ingest_raw` | immutable raw source register |
| `list_sources` / `get_source` | raw layer read |
| `write_wiki_page` / `update_wiki_page` / `get_wiki_page` / `list_wiki_pages` | wiki CRUD + graph re-extract |
| `get_schema` / `update_schema` | agent conventions |
| `read_index` / `update_index_entry` / `rebuild_index` | content catalog |
| `append_log` / `read_log` / `list_recent_ops` | timeline / `ops_log` |
| `ingest_source` | raw + compile workflow entry |
| `query_with_index` | index-first navigation answer path |
| `search_wiki` | search compiled layer only |
| `file_answer` | persist cited answer into wiki |
| `get_embedding_manifest` | model/dims/provider fingerprint |
| `reembed` / `reembed_document` | migrate vectors after config change |
| `doctor` | integrity: schema, FTS, embed dims, allowlist (minimal P0) |
| `pack_context` | optional token-budgeted citation pack |

### P1 additions

| Tool | Role |
|------|------|
| `lint_wiki` | umbrella lint |
| `find_orphans` / `find_missing_backlinks` / `find_mentioned_without_page` | graph hygiene |
| `find_contradictions` / `find_stale_claims` / `find_data_gaps` | knowledge quality |
| `link_health` / `resolve_stub` | repair stubs and edges |
| `list_by_category` / `list_by_tag` | catalog filters |
| `create_tunnel` / `list_tunnels` / `delete_tunnel` / `follow_tunnels` | cross-wing |
| `wake_up` | L0+L1 memory bootstrap |
| `diary_write` / `diary_read` | per-agent diaries |
| `kg_add` / `kg_query` / `kg_invalidate` / `kg_supersede` / `kg_timeline` / `kg_stats` | temporal facts |
| `checkpoint` | batch write + diary |
| `multi_get` / `find_similar` / `expand_chunks` | retrieval UX |
| `export_vault` / `export_markdown` / `export_graph` | Obsidian/git |
| `ingest_directory` / `sync_sources` | bulk ops |
| `get_subgraph` | seeded graph export |
| `reindex` | rebuild FTS / embeddings subset |
| `forget` / `undelete` | soft-delete tombstone path |
| `backup_db` | consistent DuckDB snapshot |
| `consolidate_memory` | diary/episode → wiki/kg compression |
| `record_feedback` | label hits for eval (schema early) |

### P2 additions

| Tool | Role |
|------|------|
| `search_raw` | explicit raw fallback |
| `hyde_search` | HyDE mode |
| `evaluate_retrieval` / `doctor` | eval + integrity |
| `watch_path` / `unwatch_path` / `list_watches` | file watch |
| `repair` / `prune_orphans` | integrity |
| `hook_settings` | client auto-save protocol |
| Dataset tools if ever needed | multi-tenant later |

---

## 8. Proposed DuckDB schema additions

Compatible with existing `documents`, `chunks`, `graph_nodes`, `graph_edges`. Prefer additive migrations.

```sql
-- Documents: scope, identity, layering
ALTER TABLE documents ADD COLUMN IF NOT EXISTS content_hash VARCHAR;
ALTER TABLE documents ADD COLUMN IF NOT EXISTS wing VARCHAR;
ALTER TABLE documents ADD COLUMN IF NOT EXISTS room VARCHAR;
ALTER TABLE documents ADD COLUMN IF NOT EXISTS source_file VARCHAR;
ALTER TABLE documents ADD COLUMN IF NOT EXISTS layer VARCHAR DEFAULT 'raw';
  -- raw | wiki | diary | schema | index | log
ALTER TABLE documents ADD COLUMN IF NOT EXISTS kind VARCHAR DEFAULT 'document';
  -- document | wiki | entity | concept | source_summary | diary | schema
ALTER TABLE documents ADD COLUMN IF NOT EXISTS category VARCHAR;
ALTER TABLE documents ADD COLUMN IF NOT EXISTS immutable BOOLEAN DEFAULT false;

CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_uri ON documents(uri);
CREATE INDEX IF NOT EXISTS idx_documents_content_hash ON documents(content_hash);
CREATE INDEX IF NOT EXISTS idx_documents_wing_room ON documents(wing, room);
CREATE INDEX IF NOT EXISTS idx_documents_layer ON documents(layer);

-- Optional dedicated raw registry (if split from documents)
CREATE TABLE IF NOT EXISTS raw_sources (
  id VARCHAR PRIMARY KEY,
  uri VARCHAR NOT NULL UNIQUE,
  title VARCHAR NOT NULL,
  content TEXT NOT NULL,
  content_hash VARCHAR NOT NULL,
  source_file VARCHAR,
  wing VARCHAR,
  room VARCHAR,
  metadata_json VARCHAR NOT NULL DEFAULT '{}',
  created_at TIMESTAMP NOT NULL
);

-- Wiki pages (if not only documents.layer='wiki')
CREATE TABLE IF NOT EXISTS wiki_pages (
  id VARCHAR PRIMARY KEY,
  slug VARCHAR NOT NULL UNIQUE,
  title VARCHAR NOT NULL,
  content TEXT NOT NULL,
  kind VARCHAR NOT NULL DEFAULT 'wiki', -- wiki | entity | concept | source_summary
  category VARCHAR,
  tags_json VARCHAR NOT NULL DEFAULT '[]',
  source_ids_json VARCHAR NOT NULL DEFAULT '[]', -- links to raw_sources / documents
  frontmatter_json VARCHAR NOT NULL DEFAULT '{}',
  graph_node_id VARCHAR,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS wiki_index_entries (
  id VARCHAR PRIMARY KEY,
  slug VARCHAR NOT NULL UNIQUE,
  title VARCHAR NOT NULL,
  kind VARCHAR NOT NULL,
  category VARCHAR,
  summary VARCHAR,
  page_id VARCHAR,
  updated_at TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS wiki_schema (
  id VARCHAR PRIMARY KEY,           -- singleton 'default'
  content TEXT NOT NULL,            -- AGENTS.md / conventions
  updated_at TIMESTAMP NOT NULL
);

-- Append-only ops / wiki log
CREATE TABLE IF NOT EXISTS ops_log (
  id VARCHAR PRIMARY KEY,
  seq BIGINT NOT NULL,             -- monotonic
  ts TIMESTAMP NOT NULL,
  op VARCHAR NOT NULL,              -- ingest | wiki_write | file_answer | lint | delete | ...
  prefix VARCHAR,                   -- parseable short code e.g. INGEST/WIKI/LINT
  message TEXT NOT NULL,
  entity_id VARCHAR,
  entity_kind VARCHAR,
  payload_json VARCHAR NOT NULL DEFAULT '{}',
  agent_name VARCHAR
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_ops_log_seq ON ops_log(seq);

-- Temporal knowledge facts (MemPalace / Graphiti-lite)
CREATE TABLE IF NOT EXISTS kg_facts (
  id VARCHAR PRIMARY KEY,
  subject VARCHAR NOT NULL,
  predicate VARCHAR NOT NULL,
  object VARCHAR NOT NULL,
  valid_from TIMESTAMP,
  valid_to TIMESTAMP,               -- null = open
  status VARCHAR NOT NULL DEFAULT 'active', -- active | invalidated | superseded
  superseded_by VARCHAR,
  source_document_id VARCHAR,
  confidence DOUBLE,
  metadata_json VARCHAR NOT NULL DEFAULT '{}',
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_kg_facts_subject ON kg_facts(subject);
CREATE INDEX IF NOT EXISTS idx_kg_facts_status ON kg_facts(status);

-- Graph: allow tunnel (+ keep wikilink | tagged | related | mentions)
-- graph_edges.rel_type includes: tunnel
-- graph_nodes.kind includes: entity (reserved), diary-backed document nodes

-- FTS: DuckDB FTS extension / pragma on chunks.content (implementation-specific)
-- CREATE ... FTS INDEX on chunks(content)  -- exact DDL depends on duckdb FTS API in use

-- Optional: store separate lex rank cache is not required; BM25 at query time
```

### Store policy (enforcement)

1. `layer='raw'` or `raw_sources`: **no content UPDATE**; only insert or supersede-by-new-uri.  
2. Wiki/index/schema/log: mutable via dedicated tools; each mutation appends `ops_log`.  
3. Re-ingest by uri keeps **stable graph node id**.  
4. Wiki page writes re-run wikilink/tag extract into graph.  
5. Hybrid search default filters can restrict `layer` or search all with provenance field `layer`.

---

## 9. Synthesis: single coherent product story

```
┌─────────────────────────────────────────────────────────────┐
│ MCP host (Claude / Cursor / agent)                          │
│  wake_up → query_with_index → search_wiki / search(hybrid)  │
│  file_answer → lint_wiki → diary / kg_*                     │
└───────────────────────────┬─────────────────────────────────┘
                            │ stdio MCP
┌───────────────────────────▼─────────────────────────────────┐
│ rag-mcp (Rust)                                              │
│  Tools: ingest* | search* | graph* | wiki* | lint* | kg*    │
├─────────────────────────────────────────────────────────────┤
│ Compile layer: wiki_pages + index + schema + ops_log        │
│ Graph layer:   nodes/edges (wikilink, tag, tunnel, stub)    │
│ Raw layer:     immutable sources + chunks + embeddings      │
│ Retrieval:     vec cosine | BM25 FTS | hybrid RRF           │
├─────────────────────────────────────────────────────────────┤
│ DuckDB single file (RAG_DB_PATH)                            │
└─────────────────────────────────────────────────────────────┘
```

**v1 ship:** SPEC as written (vector + Obsidian graph tools: wikilinks, tags, stubs, get_graph/neighbors/backlinks).  
**Immediate post-v1 (P0):** hybrid + scope/dedupe + wiki compile stack + first-class `graph_expand_search` + tunnel *edge type* (CRUD later) **plus** adversarial integrity: path allowlist, embedding_manifest/reembed, citation hits, diversity, token pack, atomic ingest, FTS sync, filter DSL, minimal doctor - **without dropping** v1 graph tools.  
**Differentiator:** local Rust binary that compounds knowledge (Karpathy) with palace scope/tunnels (MemPalace) on an Obsidian graph (SPEC), without Python or external vector DB.

---

## 10. Top 10 must-build features (ranked)

Sprint labels: **P0** = next after SPEC v1 green; **P1** = product depth. Rank is product importance, not only sprint order.

1. **Hybrid search (BM25 + vector + RRF)** — **P0** — keyword + semantic; default quality bar  
2. **Immutable raw + wiki page CRUD + schema** — **P0** — compile-once compounding memory  
3. **index.md catalog + append-only ops/wiki log** — **P0** — index-first nav + audit timeline  
4. **ingest_source / multi-page compile + file_answer** — **P0** — close the agent learning loop  
5. **content_hash + check_duplicate + upsert-by-uri + stable graph node id** — **P0** — safe re-ingest without breaking Obsidian edges  
6. **wing/room scoped metadata + taxonomy list + filtered search** — **P0** — MemPalace without second store  
7. **graph_expand_search + keep SPEC graph tools** — **P0** — hits + local graph; never drop `get_graph` / neighbors / backlinks / wikilink extract  
8. **delete_by_source + status/health + graph cleanup** — **P0** — operable corpus hygiene  
9. **Wiki/graph lint suite + resolve_stub** — **P1** — orphans, missing backlinks, stubs  
10. **Temporal kg_facts + tunnel CRUD + wake_up + diaries** — **P1** — agent memory depth; tunnel *edge type* already allowed in P0 via `link_nodes`

---

## 11. Notes for implementers

- Prefer **one DuckDB file**, additive migrations, no second store.  
- Keep **deterministic markdown graph extract**; LLM synthesis happens in the host agent writing wiki pages via tools, not inside a mandatory cognify pipeline.  
- Extend `search` rather than inventing parallel tool names for every mode.  
- Karpathy tools can share `documents` with `layer` discriminators if `wiki_pages` feels heavy; either schema is fine if tools stay stable.  
- Do not rename SPEC tools to palace vocabulary.  
- Russian/English docs OK; code identifiers English.  
- **P0 is not only hybrid + wiki:** absorb §12 adversarial MUST items (A1–A5, A8–A9, A13–A14, A17, A29). Canonical short list: `docs/ROADMAP.md`. Tool names: `docs/MCP_TOOL_MATRIX.md`. Layers: `docs/ARCHITECTURE_NOTES.md`.

---

## 12. Adversarial addenda

Adversarial pass against hybrid RAG + agent-memory MCP practice (Mem0/Graphiti/Letta patterns, production hybrid retrieval, MCP filesystem risk). Sections 1–11 cover the big product bets (hybrid RRF, wiki compile, wing/room, lint, kg_facts). These are **must-haves or near-must-haves that are under-specified, demoted too far, or missing** for a serious local dual-store. Promotions below are reflected in `docs/ROADMAP.md` and `docs/MCP_TOOL_MATRIX.md`.

### Severity legend

| Tag | Meaning |
|-----|---------|
| **MUST** | Ship in P0 or hard-block quality/safety |
| **SHOULD** | Promote to early P1; production agents hit this wall |
| **NICE** | Common elsewhere; keep P2 unless cheap |

### Missing / under-specified must-haves

| # | Feature (concrete name) | Why it matters | Suggested tier |
|---|-------------------------|----------------|----------------|
| A1 | **`ingest_path_allowlist` / sandbox root** | `ingest_file` + future `ingest_directory`/`watch_path` can read arbitrary FS paths via MCP. Memory MCP servers without path allowlist are a local RCE-adjacent footgun. Env e.g. `RAG_INGEST_ROOTS`. | **MUST** P0 |
| A2 | **`embedding_manifest` + `reembed`** | Store `model`, `dims`, `provider`, `base_url` fingerprint on corpus. Refuse hybrid/vec search on mismatch; tool `reembed` / `reembed_document` migrates vectors. Silent dim/model drift corrupts ranking. | **MUST** P0 |
| A3 | **Citation-ready `SearchHit` contract** | Stable fields: `chunk_id`, `document_id`, `uri`, `title`, `char_start`/`char_end`, `quote`, optional `heading_path`, `layer`, `wing`/`room`, `score_vec`, `score_lex`, `score_rrf`. Agents need packable citations without re-fetch heuristics. Partial in SPEC; not locked as API. | **MUST** P0 |
| A4 | **Token-budget pack (`max_context_tokens`)** | `top_k` alone overflows context windows. Search/tool flag packs hits under token budget (tiktoken-approx or char/4), prefers diversity. Standard agent-memory UX. | **MUST** P0 with hybrid |
| A5 | **Result diversity: MMR + same-doc collapse** | Hybrid often returns 5 near-duplicate adjacent chunks. `diversity=mmr\|collapse_by_document` + `max_chunks_per_document`. Without this, hybrid quality gains are illusory. | **MUST** P0 |
| A6 | **Hierarchical parent-child chunks** | Retrieve small child; return parent section / window for generation. Columns `parent_chunk_id` or `section_id`. Section-aware MD chunking alone is insufficient without parent expand. | **SHOULD** P1 (couple with `expand_chunks`) |
| A7 | **Contextual chunk prefix (title/heading before embed)** | Prepend `title` + heading breadcrumb into embed text (not necessarily stored body). Large quality win vs raw window embeds; cheap vs HyDE/rerank. | **SHOULD** P0/P1 |
| A8 | **Embedding cache by `content_hash`** | Skip re-API on unchanged chunk text across re-ingest. Couples with A2 and content_hash. Cost control is ops-critical once openai provider is default. | **MUST** P0 |
| A9 | **FTS tokenizer / language config** | DuckDB FTS stemmer/stopwords are English-default. Env `RAG_FTS_STEMMER` / `none` for CJK/code. Multilingual personal wikis fail BM25 silently. | **MUST** P0 with FTS |
| A10 | **Soft-delete / forget + memory update** | Hard `delete_document` only. Agent memory needs `forget` (tombstone), `update_memory` / patch wiki page without losing graph node id, optional TTL. Mem0-class CRUD is expected. | **SHOULD** P1 |
| A11 | **Session / user / agent memory scopes** | `wing`/`room` are spatial taxonomy, not episodic scopes. Columns or filters: `agent_id`, `session_id`, `user_id` (or `scope_kind`+`scope_id`) on diaries/ops_log/kg_facts. Cross-session recall without this is ad hoc. | **SHOULD** P1 with diaries/wake_up |
| A12 | **Memory consolidation pass** | Letta-style sleep/consolidate: summarize diaries → wiki/kg facts, decay raw episodes. `consolidate_memory` / scheduled checkpoint sibling. wake_up without consolidate grows noise forever. | **SHOULD** P1–P2 |
| A13 | **Schema version + `doctor` embed/FTS checks** | `schema_version` table; `doctor` reports migration lag, FTS index present, embed dims, orphan edges, immutable-layer violations. `status` counts are not integrity. | **MUST** P0 (minimal) / P1 full |
| A14 | **Atomic ingest transaction** | Multi-step ingest (doc + chunks + graph rebuild + ops_log) must commit/rollback as one unit. Partial failure leaves ghost graph edges and half chunks. | **MUST** P0 |
| A15 | **MCP tool safety annotations** | Mark `delete_*`, `update_schema`, `reembed`, `repair` as destructive; search/list as read-only. Hosts (Cursor/Claude) use annotations for confirm UX. | **SHOULD** P0 when rmcp supports |
| A16 | **Group-by-document search mode** | `group_by=document` returns best chunk + sibling count / optional merge. Agents cite notes, not arbitrary windows. | **SHOULD** P1 |
| A17 | **Metadata filter DSL** | JSON `filters`: eq/in/exists on `metadata_json`, wing, room, layer, kind, tags, time range on `created_at`/`updated_at`. “filters” in P0 list is vague without operators. | **MUST** P0 (define operators) |
| A18 | **Recency / time-decay ranking flag** | `recency_half_life_days` or boost on `updated_at`. Memory MCP without time bias fails “what did we decide last week”. Listed under P1 multi-query; should be explicit search param. | **SHOULD** P1 |
| A19 | **Backup / snapshot** | `backup_db(path)` or export consistent snapshot before reembed/migration. Single-file DuckDB is the product; no backup tool is ops malpractice. | **SHOULD** P1 |
| A20 | **Write-lock / busy error surface** | Document single-writer Mutex; return structured `STORE_BUSY` / timeout rather than hang MCP. Multi-host stdio is rare but dual-client happens. | **SHOULD** P0 docs + error |
| A21 | **Query rewrite / multi-query default path** | Single query embedding misses keyword variants. `search(expand=multi)` generates N rewrites (host LLM or rule-based) + RRF. In FEATURES as P1 “multi-query RRF”; treat as hybrid quality default option. | **SHOULD** P1 |
| A22 | **Relevance feedback store** | `record_feedback(query, hit_id, label)` + export qrels for `evaluate_retrieval`. Eval harness without labeled feedback never leaves toy status. | **NICE** P2 but schema cheap in P1 |
| A23 | **Pin / boost / demote** | Manual `boost_document` weight for canonical notes (schema, AGENTS). Index-first helps; explicit boost is how humans fix ranker failures. | **NICE** P1–P2 |
| A24 | **Heading path on chunks** | `heading_path` / `section_title` columns from MD ATX headers for citations and contextual embed (A7). | **SHOULD** P1 with section chunker |
| A25 | **Idempotent tool keys / dry_run** | `dry_run=true` on ingest/delete/reembed; optional `idempotency_key` on write tools for agent retries. | **NICE** P1 |
| A26 | **Progress events for bulk ops** | Long `ingest_directory` / `reembed` needs partial progress (log lines or MCP progress if available). Otherwise hosts look hung. | **SHOULD** P1 with directory ingest |
| A27 | **Conflict policy on concurrent wiki write** | `updated_at` / etag check on `update_wiki_page` (If-Match). Last-write-wins silently loses agent work. | **SHOULD** P1 with wiki CRUD |
| A28 | **Graph edge provenance link to chunk** | `graph_edges.chunk_id` or `char_start` beyond free-text `context`. Enables “why is this wikilink here” and safe edge rebuild diffs. | **SHOULD** P1 |
| A29 | **Read-your-writes search consistency** | After ingest in same session, search must see new chunks without manual reindex lag. FTS maintain-on-write is listed; call out **sync FTS refresh before tool return**. | **MUST** P0 with FTS |
| A30 | **Empty / tiny corpus behavior** | Documented tool errors vs empty lists; `status.ready_for_search`; avoid embed API call on empty store optional short-circuit. | **SHOULD** P0 polish |

### Priority promotions (adversarial overrides)

FEATURES currently parks some items too late relative to hybrid/memory MCP norms:

1. **Promote A1–A5, A8–A9, A13–A14, A17, A29 into P0** alongside hybrid + wiki stack (safety, ranking honesty, corpus integrity).
2. **Do not ship hybrid without diversity + citation pack + token budget** — otherwise P0 hybrid is a leaderboard feature, not an agent feature.
3. **Couple `expand_chunks` with parent-child (A6) and heading_path (A24)**; otherwise adjacent expand is a band-aid.
4. **Treat path allowlist as non-negotiable** even if directory ingest stays P1.
5. **Embedding manifest/reembed is not optional ops polish** — any `RAG_EMBEDDING_MODEL` change without it bricks semantic search.

### Still correctly deferred (do not promote)

- External multi-backend vector DBs, mandatory cognify, AAAK default-on, palace tool rename, server-side graph layout, multi-tenant auth product.
- HyDE, VSS/HNSW, HTTP MCP, local ONNX: keep P2 until measured pain.
- Full Letta OS memory runtime: out of product scope; expose tools only.

### Minimal additive schema (adversarial)

```sql
-- Corpus / embed integrity
CREATE TABLE IF NOT EXISTS embedding_manifest (
  id VARCHAR PRIMARY KEY,          -- singleton 'default'
  provider VARCHAR NOT NULL,
  model VARCHAR NOT NULL,
  dims INTEGER NOT NULL,
  base_url VARCHAR,
  content_fingerprint VARCHAR,     -- optional config hash
  updated_at TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS schema_version (
  version INTEGER NOT NULL,
  applied_at TIMESTAMP NOT NULL,
  note VARCHAR
);

-- Chunk retrieval quality
ALTER TABLE chunks ADD COLUMN IF NOT EXISTS parent_chunk_id VARCHAR;
ALTER TABLE chunks ADD COLUMN IF NOT EXISTS section_id VARCHAR;
ALTER TABLE chunks ADD COLUMN IF NOT EXISTS heading_path VARCHAR;
ALTER TABLE chunks ADD COLUMN IF NOT EXISTS embed_text_hash VARCHAR; -- cache key for embed body

-- Soft forget
ALTER TABLE documents ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMP;
ALTER TABLE documents ADD COLUMN IF NOT EXISTS boost DOUBLE DEFAULT 1.0;

-- Episodic scopes (diaries / ops / optional on documents)
ALTER TABLE documents ADD COLUMN IF NOT EXISTS agent_id VARCHAR;
ALTER TABLE documents ADD COLUMN IF NOT EXISTS session_id VARCHAR;

-- Edge provenance
ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS chunk_id VARCHAR;
ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS char_start INTEGER;
ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS char_end INTEGER;

-- Feedback for eval
CREATE TABLE IF NOT EXISTS retrieval_feedback (
  id VARCHAR PRIMARY KEY,
  query TEXT NOT NULL,
  hit_chunk_id VARCHAR,
  label VARCHAR NOT NULL,          -- relevant | irrelevant | partial
  created_at TIMESTAMP NOT NULL,
  agent_name VARCHAR
);
```

### Tools to add (names only)

| Tool | Role |
|------|------|
| `reembed` / `reembed_document` | Migrate vectors after model/dim change |
| `get_embedding_manifest` | Show active model/dims fingerprint |
| `forget` / `undelete` | Soft-delete tombstone path |
| `backup_db` | Consistent file snapshot |
| `doctor` | Integrity: schema, FTS, embed dims, orphans, allowlist |
| `record_feedback` | Label search hits for eval |
| `consolidate_memory` | Diary/episode → wiki/kg compression |
| `pack_context` | Optional: turn hits into token-budgeted citation block |

Config env additions: `RAG_INGEST_ROOTS`, `RAG_FTS_STEMMER`, `RAG_MAX_CONTEXT_TOKENS` (default pack), `RAG_MAX_CHUNKS_PER_DOC`.

### Bottom line

FEATURES.md is strong on **product differentiation** (wiki compile, palace scope, lint, temporal KG) and correctly promotes **hybrid BM25+RRF**. It is weaker on **retrieval honesty details** (diversity, citations, token pack, contextual embed), **corpus integrity** (embed manifest/reembed, atomic ingest, FTS sync, doctor), **MCP safety** (path allowlist, destructive annotations), and **memory lifecycle** (soft forget, session scopes, consolidation). Ship those with or immediately after hybrid; otherwise the dual substrate stays a demo graph+vector kit rather than a durable agent memory MCP.

---

*End of FEATURES.md — derived from SPEC.md + theme syntheses (karpathy, mempalace, retrieval, graph, ops, diff) + adversarial pass. Executable priority lists: `docs/ROADMAP.md`, `docs/MCP_TOOL_MATRIX.md`, `docs/ARCHITECTURE_NOTES.md`.*

---

## DuckDB-native retrieval

Primary sources (DuckDB docs, current as of 2025–2026): [FTS extension](https://duckdb.org/docs/current/core_extensions/full_text_search.html), [VSS extension](https://duckdb.org/docs/current/core_extensions/vss.html). Implications for rag-mcp hybrid RAG on a single-file store.

### FTS (full-text / BM25) — P0 substrate

- **Extension:** `fts` (core; `INSTALL fts; LOAD fts;` or autoload). SQLite FTS5-like inverted index implemented as SQL schema under `fts_main_<table>`.
- **Index lifecycle:** `PRAGMA create_fts_index(input_table, input_id, *input_values, …)` / `PRAGMA drop_fts_index(input_table)`. Index id column + one or more VARCHAR fields (or `*` for all VARCHAR columns).
- **Ranking:** `fts_main_<table>.match_bm25(id, query, fields := …, k := 1.2, b := 0.75, conjunctive := 0)` → Okapi BM25 score; non-matches return `NULL` (filter with `WHERE score IS NOT NULL`, `ORDER BY score DESC`).
- **Tokenizer knobs:** `stemmer` (porter default; many languages + `none`), `stopwords` (`english` default, custom table, or `none`), `ignore` regex (default strips non-`[a-z]`), `strip_accents`, `lower`, `overwrite`.
- **Field-scoped lex search:** multi-column index; `fields := 'content,title'` restricts BM25 to named columns (maps to chunk body + optional title/heading).
- **Conjunctive queries:** `conjunctive := 1` requires all query terms present (AND-style).
- **Stem helper:** scalar `stem(input_string, stemmer)` for debugging / custom pipelines.
- **Critical limit — no automatic reindex:** FTS index does **not** update when the base table changes. Must drop/recreate (or `overwrite=1`) after ingest/delete; app layer owns **sync FTS refresh before search tool return** (read-your-writes).
- **Hybrid composition (app-side):** run BM25 over `chunks.content` (+ optional title/heading columns), run vector cosine separately, fuse ranks with **RRF** in Rust (DuckDB does not ship a built-in hybrid/RRF operator). Join scores back to `documents` for wing/room/layer/tag filters via normal SQL.
- **rag-mcp mapping:** `search(mode=lex|vec|hybrid)`; env `RAG_FTS_STEMMER` / stopwords for CJK/code; `doctor` checks `fts_main_chunks` presence; `reindex` rebuilds FTS after bulk mutate.

### VSS (vector / HNSW) — P2 when full-scan hurts

- **Extension:** `vss` (**experimental**). Accelerates nearest-neighbor over fixed-size `FLOAT[n]` `ARRAY` columns via **HNSW** (`CREATE INDEX … USING HNSW (col)`).
- **Distance metrics (index + query must match):**
  - `l2sq` → `array_distance` (default)
  - `cosine` → `array_cosine_distance` (rag-mcp default similarity path)
  - `ip` → `array_negative_inner_product`
- **Query shapes accelerated:** `ORDER BY <metric>(vec, query) LIMIT k`; also `min_by(table, distance_expr, k)` one-shot top-k. Verify plan has `HNSW_INDEX_SCAN`.
- **Hyperparams:** `ef_construction` (default 128), `ef_search` (64), `M` (16), `M0` (2×M); runtime override `SET hnsw_ef_search = N` / `RESET hnsw_ef_search`.
- **Multiple indexes:** one column per index; multiple HNSW indexes allowed on same table (different columns or different metrics).
- **Mutations:** inserts/updates/deletes supported; deletes are **soft-marked** (index can go stale). Remedy: `PRAGMA hnsw_compact_index('index_name')` or rebuild. Prefer bulk-load then create index for build speed/parallelism.
- **Persistence caveats (disk-backed DuckDB):** HNSW only on **in-memory** DBs by default; disk requires `SET hnsw_enable_experimental_persistence = true`. WAL recovery for custom indexes incomplete → crash risk of **index corruption / data loss**. Checkpoint rewrites entire index; load deserializes whole index into RAM (deferred until first access). **Not production-safe for rag-mcp’s single-file default without explicit risk acceptance.**
- **Hard limits:** `FLOAT` 32-bit only; index must fit in RAM; HNSW memory **not** counted toward DuckDB `memory_limit`; brute-force helpers `vss_join` / `vss_match` do **not** use HNSW.
- **Schema migration cost:** SPEC stores `embedding_json` VARCHAR + cosine in Rust. VSS needs `FLOAT[dims]` ARRAY column (or dual-write). Dim must match `RAG_EMBEDDING_DIMS` / embedding_manifest; reembed path required on model change.

### Hybrid RAG recipe (DuckDB-native, 2025–2026)

- **P0 path (ship first):** FTS BM25 on `chunks` + portable Rust cosine over `embedding_json` + RRF fusion + SQL metadata filters. No VSS dependency; works on disk-backed single file; full-scan vector is fine for personal/medium corpora.
- **P0 ops around FTS:** reindex on every successful ingest/delete transaction; surface FTS health in `status`/`doctor`; configurable stemmer/stopwords; citation fields carry `score_lex`, `score_vec`, `score_rrf`.
- **P2 path (scale):** migrate embeddings to `FLOAT[n]`, optional HNSW with `metric='cosine'`, keep FTS+RRF hybrid; gate persistence flag; compact after heavy deletes; document experimental risk in README.
- **Do not rely on DuckDB for:** RRF, MMR diversity, token packing, or rerank — keep those in the MCP tool layer.
- **Non-goals vs extensions:** no external vector DB required; do not block v1 or hybrid P0 on VSS; FTS is the keyword half of hybrid, not optional polish.
