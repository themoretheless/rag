# System Map

**Product:** `rag-mcp`  
**Role:** living map of layers, code ownership, tool clusters, and documentation.  
**North star:** [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) · **Laws:** [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md)

Update this file when modules or major tool clusters move. Prefer truth over aspirational layout when they diverge; mark target vs current.

---

## 1. Conceptual layers

```
┌─────────────────────────────────────────────────────────────┐
│  L4  Agent / MCP client  (+ optional local chat LLM tools)  │
├─────────────────────────────────────────────────────────────┤
│  L3  Compiled knowledge   wiki · index · schema · ops_log   │
├─────────────────────────────────────────────────────────────┤
│  L2  Object graph         nodes · edges · stubs · tunnels   │
├─────────────────────────────────────────────────────────────┤
│  L1  Retrieval            chunks · embeddings · FTS · RRF   │
├─────────────────────────────────────────────────────────────┤
│  L0  Verbatim corpus      documents · wing/room · hash      │
└─────────────────────────────────────────────────────────────┘
              Storage (default: DuckDB single file)
```

| Layer | Tables / artifacts (logical) | Primary tools (examples) |
|-------|------------------------------|---------------------------|
| **L0** | `documents` (raw/diary/…), content_hash, wing, room, immutable | `ingest_*`, `add_drawer`, `get_document`, `list_*`, `delete_by_source` |
| **L1** | `chunks`, embedding_json, FTS, embedding_manifest | `search`, diversity/token pack, `reembed*`, `pack_context` |
| **L2** | `graph_nodes`, `graph_edges` | `get_graph`, `get_neighbors`, `get_backlinks`, `link_nodes`, tunnels, `graph_expand_search` |
| **L3** | wiki pages, index entries, schema, ops_log | wiki CRUD, `query_with_index`, `search_wiki`, `file_answer`, `lint_wiki`, `consolidate` |
| **L4** | client agent; optional server chat | `wake_up`, diary, `checkpoint`, `analyze_corpus`, maintain_*, `compile_source`, `llm_status` |
| **KG** | `kg_facts` (spans L2/L4 memory) | `kg_add/query/invalidate/supersede/timeline/stats` |

Vocabulary map (external → product): drawers→docs/chunks; wing/room→columns; tunnel→edge type; closet→wiki `source_summary`; index.md→index tools; log.md→ops_log. See [`ARCHITECTURE_NOTES.md`](ARCHITECTURE_NOTES.md) §3.

---

## 2. Runtime shape

```
MCP client (Claude / Zed / …)
        │ stdio JSON-RPC
        ▼
   rag-mcp binary
        │
        ├── mcp/server.rs   tool_router façade (today: large single file)
        ├── domain modules  chunk · embed · graph · search · wiki · maintain · diary · kg
        ├── db/*            DuckDB schema + store (target: storage/* adapters)
        └── optional HTTP   embeddings + chat (OpenAI-compatible / Ollama)
```

**Process rules:** one writer process per store path; UI snapshot/export or exclusive `--db` mode for inspector; never dual-live MCP + UI writers.

---

## 3. Code map (current ≈ target)

**Current tree (repo root `src/`):**

| Path | Owns |
|------|------|
| `main.rs`, `lib.rs`, `config.rs`, `error.rs`, `models.rs` | entry, config, domain DTOs |
| `chunking/` | fixed-window chunker |
| `embeddings/` | mock, openai, ollama providers |
| `llm/` | chat client + provider presets |
| `graph/` | wikilink/tag extract + resolve |
| `db/` | schema, store, search, graph, fts, kg |
| `wiki/` | wiki compile helpers |
| `diary/` | agent diary helpers |
| `maintain/` | analyze, plan, apply, organize, compress, refresh |
| `search_pack.rs` | token packing / hit packing |
| `mcp/server.rs` | **all** MCP tool handlers (concentration risk) |
| `mcp/tools.rs` | parameter structs (schemars) |
| `util/` | hashing helpers |

**Target pressure (not a rewrite mandate):**

- Extract tool clusters from `server.rs` into `mcp/tools_{ingest,search,graph,wiki,kg,maintain}.rs` or thin handlers over domain services.
- Introduce `storage/` + `trait Storage`; move `db/*` DuckDB code behind `storage/duckdb` ([`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md)).
- Keep public MCP tool **names** stable while internal modules split.

### SOLID seams (current boundaries)

Intentional boundaries after the modularity pass. Prefer extend-at-seam over growing façades. S/O pressure below is direction, not a rewrite mandate.

| Seam | Owns | Does not own | Open pressure (S / O) |
|------|------|--------------|------------------------|
| **`Store` (boundary)** | Document/chunk/graph/wiki/ops I/O; single-writer DuckDB façade | Ranking policy, pack/token budget, MCP/HTTP wire shapes | Split mixed methods (CRUD vs graph cascade, wiki index score, CAS pure helpers); target `Storage` trait so domain holds `dyn Storage` ([`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md)) |
| **`EmbeddingProvider` trait** | Embed text → vectors; mock / openai / ollama impls | Chunking, search fuse, config env parse | Co-locate kind policy on `EmbeddingProviderKind` (defaults, URL/key, dialect); `build_provider` = kind→ctor only; vector metrics in one module |
| **HTTP vs MCP** | Ports only: `http_api` routes + `mcp/server` tools map params → domain → JSON/status | Business rules, compile policy, embed choice | Keep both thin adapters; shared free fns for CAS if_match, search options, ingest pipeline; bind parse/validate out of handlers; spine membership data-driven |
| **Wiki compile layer (L3)** | Agent-authored pages, index, schema, ops_log; `compile_source` / `file_answer` / lint helpers | Raw body rewrite (L0 immutable); graph NER | Split write vs pure query paths; one owner for parent→wiki provenance links; kinds/mutability policy as registry so new page kinds do not scatter |

**Rules of thumb:** domain free functions and traits get algorithms; façades stay param-to-domain-to-JSON; new backends/modes/actions register at the seam (kind arm, route merge, action table) instead of editing god matches. See §9 for concentration debt; principles §1.15 and vision thin-MCP / Storage notes for product stance.

---

## 4. MCP tool clusters

Counts fluctuate; README and `#[tool` on `server.rs` are ground truth for “exists today.” Clusters for navigation:

| Cluster | Purpose | Design docs |
|---------|---------|-------------|
| **Ingest / raw** | text, file, raw, drawer, duplicates | SPEC, MEMPALACE_PARITY |
| **Search / pack** | lex/vec/hybrid, filters, diversity, expand | FEATURES, ARCHITECTURE_NOTES |
| **Documents / taxonomy** | list/get/delete, wings/rooms, refile, pin, archive | ORGANIZE |
| **Graph** | nodes, edges, stubs, tunnels, expand, stats | GRAPH_DESIGN |
| **Wiki / compile** | pages, schema, index, log, file_answer, lint | FEATURES Karpathy map |
| **KG** | temporal facts | MEMPALACE_PARITY |
| **Diary / session** | diary, wake_up, checkpoint | MEMPALACE_PARITY |
| **Integrity / ops** | status, doctor, manifest, reembed, vacuum, ops_log | ARCHITECTURE_NOTES |
| **LLM / maintain** | llm_status, analyze/plan/apply, organize/compress/refresh | LOCAL_LLM_MAINTENANCE, ORGANIZE |

Full planned × theme matrix: [`MCP_TOOL_MATRIX.md`](MCP_TOOL_MATRIX.md).  
Priority sequencing: [`ROADMAP.md`](ROADMAP.md).

---

## 5. Control-plane workflows (where to look)

| Workflow | Entry tools | Detail |
|----------|-------------|--------|
| Ingest verbatim | `ingest_text` / `ingest_file` / `ingest_raw` / `add_drawer` | ARCHITECTURE_NOTES §5.1 |
| Compile | `compile_source` / wiki write / `ingest_source` | §5.2; LOCAL_LLM_WIKI |
| Query cascade | `query_with_index` → `search_wiki` → `search` → `graph_expand_search` | §5.3 |
| Session memory | `wake_up`, diary, `checkpoint` | MEMPALACE_PARITY |
| Organize | `refile`, pin/boost, archive, collections (P1) | ORGANIZE |
| Maintain | `analyze_corpus` → plan → apply → compress/refresh | LOCAL_LLM_MAINTENANCE |
| Graph inspect | MCP topology tools; optional `rag-mcp-ui` | GRAPH_DESIGN, EGUI_GRAPH_VIEW |

---

## 6. Storage adapters (target)

```
Domain (ingest, search, graph, wiki)
            │
            ▼
     dyn Storage (capability flags)
            │
   ┌────────┼──────────┬────────────┐
   ▼        ▼          ▼            ▼
 DuckDB   Markdown   SQLite     Postgres
(default)  vault               +pgvector
```

- **Default path today:** DuckDB file via `RAG_DB_PATH` (no trait yet; code in `db/`).
- **Markdown vault:** files as SoT + sidecar vectors; Obsidian/git friendly.
- **Capability flags:** hybrid FTS, native ANN, transactions, vault_live_parse: degrade or hard-error honestly.
- **Non-goal:** remote vector DB as sole primary SoT.

See [`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md). Principles: [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md) §1.2.

---

## 7. Documentation map

```
ARCHITECTURE_VISION.md     ← north star (what / why)
        │
        ├── PRODUCT_PRINCIPLES.md   laws + conflict resolutions
        ├── SYSTEM_MAP.md           this file (layers + §8 recent surface truth)
        └── ARCHITECTURE_NOTES.md   layer mechanics + workflows

ROADMAP.md                 sequencing (points to vision)
SPEC.md                    v1 implementation contract
FEATURES.md                research depth + adversarial A1–A30
MCP_TOOL_MATRIX.md         tools × Karpathy × MemPalace
README.md                  operator-facing truth of shipped tools
CONNECT.md · PROD_RUN.md   multi-client HTTP gateway (may lag §8)

Topic deep-dives:
  ORGANIZE.md · MEMPALACE_PARITY.md · STORAGE_ADAPTERS.md
  GRAPH_DESIGN.md · EGUI_GRAPH_VIEW.md · GRAPH_EGUI_DECISIONS.md
  LOCAL_LLM_MAINTENANCE.md · LOCAL_LLM_WIKI.md · LLM_PROVIDERS.md
  REPOS_SURVEYED.md · EGUI_USAGE.md
```

### Authority on conflict

1. **Intent / identity:** ARCHITECTURE_VISION + PRODUCT_PRINCIPLES  
2. **v1 wire contracts:** SPEC (until revised)  
3. **Sequencing:** ROADMAP (must not violate principles)  
4. **Shipped surface:** README + code + this map §8 for recent wiki/HTTP/CAS/UI  
5. **Research / history:** FEATURES, REPOS_SURVEYED (may lag priorities)

---

## 8. Recent surface inventory (truth, not backlog)

Honest status of wiki catalog / HTTP bind / CAS / UI / hooks work that landed in code but is uneven in docs and tests. Prefer this section over aspirational ROADMAP labels for “exists today.” Codes: **shipped** = usable in production path; **partial** = core path works with documented gaps; **docs-stale** = code ahead of docs.

| Area | Status | Shipped | Incomplete / gaps |
|------|--------|---------|-------------------|
| **store-wiki-metas** | shipped | `list_wiki_page_metas` / filtered; `wiki_backlinks_for_document`; HTTP + UI; unit tests | Deeper q/limit edge cases; dual catalog parity with `list_wiki_index` |
| **http-wiki-bind** | shipped | GET `/v1/wiki` (+q/limit/offset/filters), PUT `/v1/wiki` (CAS), GET `/v1/backlinks`, `parse_bind` + remote guard + unit tests | No axum handler integration tests; backlinks only by `id` |
| **cas-revision** | shipped | `upsert_document_cas`, wiki if_match, `RAG_WIKI_REQUIRE_IF_MATCH`, PUT 409; chunks deleted **after** successful CAS | MCP Conflict → `invalid_params`; `--db` UI save uses check-then-meta (not full re-embed) |
| **mcp-ingest-wing** | shipped | `ingest_file` wing/room + CLI `ingest_file` bin | MCP tool description/matrix still thin; `ingest_text` has no wing/room |
| **ui-wiki** | shipped | Wiki browser + dual-pane + Edit/Save (PUT or `--db`) + backlinks + link colors + bold/code/*italic* | Snapshot mode no wiki catalog; no tables/images; no forward history |
| **graph-wiki-bridge** | shipped | Open as wiki / Show in graph by document id | No canvas double-click; snapshot/vault limited |
| **hooks-claude** | shipped | Queue/flush hooks; `.rag/` gitignored; CLAUDE.md CAS + re-ingest | No auto-flush; agent still runs `ingest_file` |
| **cli-ingest** | shipped | `ingest_file` + `ingest_project` bins | Paths differ slightly from MCP (`file://` pipeline vs raw helpers) |
| **tests-existing** | partial | parse_bind, metas, backlinks, document_cas, require-if-match wiki test | HTTP handler tests thin; more q/limit matrix |

### Operator implications (short)

- **HTTP gateway:** graph read + wiki catalog/backlinks + **PUT wiki** with optional CAS.
- **CAS:** optional per call; set `RAG_WIKI_REQUIRE_IF_MATCH=true` for multi-agent enforce on updates.
- **UI:** Wiki browser **and** editor (Edit/Save); prefer MCP for bulk agent writes.
- **Placement:** MCP/CLI `ingest_file` accept `wing`/`room`.

---

## 9. Known concentration / debt (map, not backlog)

| Debt | Why it matters | Direction |
|------|----------------|-----------|
| Large `mcp/server.rs` (~70+ tools) | Hard to review; risk of god-object | Split handlers by cluster; keep names |
| DuckDB types leak into domain | Blocks vault/SQLite adapters | `Storage` trait + adapters |
| Doc priority labels vs shipped MemPalace tools | ROADMAP/MCP matrix still say P1 for some shipped kg/diary tools | Prefer README/code; refresh labels when editing those docs |
| Optional UI dual-writer | DB corruption risk | Exclusive mode / snapshot only (EGUI decisions) |
| HTTP wiki/CAS/UI surface uneven vs docs/tests | Operators and agents over-trust “shipped” labels | §8 inventory above; fill tests + docs before treating as complete |

---

## 10. Related entry points

| Want | Open |
|------|------|
| Product idea | [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) |
| Rules | [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md) |
| Layer detail | [`ARCHITECTURE_NOTES.md`](ARCHITECTURE_NOTES.md) |
| What to build next | [`ROADMAP.md`](ROADMAP.md) |
| Operator install | [`README.md`](../README.md) |
| HTTP / multi-client | [`CONNECT.md`](CONNECT.md), [`PROD_RUN.md`](PROD_RUN.md) |
| UI modes | [`EGUI_USAGE.md`](EGUI_USAGE.md) |

---

*End of system map.*
