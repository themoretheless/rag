# Architecture Vision - rag-mcp

**Status:** product architecture core (IDEA-level). Binding for design and refactor priority.  
**Date:** 2026-07-29  
**Does not replace:** [`SPEC.md`](../SPEC.md) (v1 contract), [`ARCHITECTURE_NOTES.md`](ARCHITECTURE_NOTES.md) (earlier layer notes + diagrams).  
**Supersedes where they conflict:** co-equal multi-philosophy marketing; multi-backend as product identity; server-LLM compile as the happy path; “cluster ~60 advanced tools” as UX strategy.

**Related:** [`FEATURES.md`](../FEATURES.md) · [`ROADMAP.md`](ROADMAP.md) · [`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md) · [`MCP_TOOL_MATRIX.md`](MCP_TOOL_MATRIX.md) · [`LOCAL_LLM_WIKI.md`](LOCAL_LLM_WIKI.md) · [`LOCAL_LLM_MAINTENANCE.md`](LOCAL_LLM_MAINTENANCE.md)

---

## 0. What the product is

rag-mcp is a **local compounding agent memory store**, exposed as one **stdio MCP binary** over one **DuckDB file**.

It is **not**:

- a multi-backend RAG platform
- a second MemPalace with palace jargon
- a multi-tenant SaaS knowledge OS
- an in-process second brain that auto-synthesizes every ingest
- a peer listing of “RAG + graph + wiki + MemPalace + LLM”

**One sentence:**

> Immutable raw sources go in; the **client agent** compiles them into an interlinked wiki (index, schema, log); the server stores, searches, extracts graph structure, and enforces policy; hybrid retrieval is the honest fallback, not the product north star.

**Success metrics (north star, gate new work):**

| Metric | Why |
|--------|-----|
| Index coverage | fraction of wiki pages with catalog entries |
| Uncompiled raw debt | raw docs with no citing `source_summary` / index touch |
| Index-first wins | agent sessions that answer via index/wiki before hybrid |
| Filed answers | `file_answer` / wiki writes after useful answers |
| Zero forbidden leaks | L3 never rewrites L0 body; L1 never ranks on wrong embed model |

Parity checklists and “feature laundry” do **not** gate shipping. Coverage of surveyed repos is evidence, not architecture.

---

## 1. Product identity (ordered principles)

1. **Verbatim L0 is sacred.** No silent summarize-on-write into raw. No in-place mutation of `layer=raw` body after ingest.
2. **Compile is L4-owned.** The MCP client agent writes wiki pages. Server validates storage, graph extract, immutability, and logs. Server `ChatClient` is **not** product identity and must never be required for the compile loop.
3. **Index-first cascade.** Default query SOP: L3 catalog → wiki search → L1 hybrid → L2 expand → explicit raw. Documented in `schema://agents` and server instructions. Hybrid is substrate quality, not the recommended first step.
4. **Deterministic L2.** `[[wikilink]]`, `#tag`, stubs, promote-on-title. No mandatory LLM NER / cognify for core graph. No server-side layout positions in graph API.
5. **DuckDB single-file is the hero path.** README identity, default `RAG_DB_PATH`, co-located `embedding_json` + meta until measured full-scan pain. Adapters are portability, not co-equal products.
6. **Thin MCP port.** Business rules live in domain services + Storage façade. `mcp/server.rs` is parse → service → JSON. No new god growth.
7. **Small public surface.** Target **~15–20 spine tools** on by default. Everything else is deleted, folded, or behind explicit flags  -  not “advanced clustering” of 60 tools that clients still enumerate.
8. **Keep SPEC names.** No MemPalace 36-tool rename. No AAAK default-on. Wing/room are data columns, not mandatory dialect.
9. **Agent owns synthesis; server owns permanence and policy.**

---

## 2. Layers (L0–L4)  -  the only product/code map

Layers are **ownership and write policy**, not marketing modes. Tables, modules, and tools name layer ownership. Stop flat peer listing of RAG / graph / wiki / MemPalace / LLM.

```
┌──────────────────────────────────────────────────────────────┐
│  L4  Client agent (outside store identity)                   │
│      synthesis, multi-page compile, lint judgment, filing    │
├──────────────────────────────────────────────────────────────┤
│  L3  Compounding knowledge (agent-writable)                  │
│      wiki pages · index · schema · ops_log                   │
│      [parked subplanes: diary, kg_facts  -  see §7]            │
├──────────────────────────────────────────────────────────────┤
│  L2  Object graph (deterministic extract + explicit edges)   │
│      nodes · wikilink/tagged edges · tunnels via link_nodes  │
├──────────────────────────────────────────────────────────────┤
│  L1  Retrieval machinery                                     │
│      chunks · embedding_json · FTS · RRF · pack · manifest   │
├──────────────────────────────────────────────────────────────┤
│  L0  Verbatim corpus (immutable after ingest)                │
│      raw documents · content_hash · uri · wing/room          │
└──────────────────────────────────────────────────────────────┘
         Storage façade → DuckDB default (only hero identity)
```

| Layer | Mutability | Owner of content meaning | Server may |
|-------|------------|--------------------------|------------|
| **L0** | Body immutable; meta may refile under policy | Human / agent deposit | Chunk, embed, hash, scope columns, graph node, refuse body rewrite |
| **L1** | Rebuildable from L0/L3 text + manifest | Derived | Reembed, reindex FTS, refuse mismatched vec/hybrid |
| **L2** | Derived edges rebuild; explicit/tunnel preserved | Extract + agent `link_nodes` | Extract on write; BFS/export; no ChatClient edge invent |
| **L3** | Mutable by agent tools | Client agent | CRUD wiki/index/schema; append ops_log; never rewrite L0 |
| **L4** | Outside process | MCP client | Not stored as “the server’s brain” |

### 2.1 Write-gate (policy plane)

All writers pass a **single write-gate** (conceptual `policy::LayerKind` + assertions), used by corpus ingest, wiki CRUD, maintain apply, meta updates:

- `assert_content_mutable(layer)`  -  raw body writes after create are forbidden.
- Raw delete / hard wipe only via explicit allow flags + ops_log.
- Layer-legal transitions only (e.g. refile meta ≠ rewrite content).
- Maintain whitelist: structural ops + applying **client-supplied** text; no silent raw body rewrite; no unlogged deletes.

### 2.2 Forbidden leaks (zero tolerance)

| Leak | Why fatal |
|------|-----------|
| L3 / maintain rewrites L0 body | Destroys SoT trust |
| L1 ranks with wrong model/dims | Poisoned retrieval without error |
| Server LLM invents L2 edges | Non-reproducible graph |
| Summarize-on-write into L0 | MemPalace anti-pattern + Karpathy violation |
| Dual-live DuckDB + vault without exclusive reindex | Split brain |
| Unlogged significant mutation | Non-auditable compounding store |

### 2.3 Vocabulary map (product names only)

| External | rag-mcp |
|----------|---------|
| Drawer | chunk + full document content |
| Wing / room | `documents.wing` / `documents.room` |
| Tunnel | `graph_edges.rel_type = 'tunnel'` via `link_nodes` (no tool family) |
| Closet / summary | wiki page `kind=source_summary` (L3 only) |
| index.md | `wiki_index_entries` |
| log.md | `ops_log` |
| Schema / AGENTS.md | `wiki_schema` / `schema://agents` |

---

## 3. Consistency model

### 3.1 Identity

- **Document identity:** stable `uri` (unique); re-ingest by uri keeps graph node stable when policy allows.
- **Content identity:** `content_hash` for idempotency / `check_duplicate`.
- **Wiki identity:** slug (stable); graph node id derived from uri/slug.
- **Chunk identity:** subordinate to document; rebuildable.

### 3.2 Atomic ingest unit

One logical transaction:

`document (+ chunks + embeddings) + graph node + derived edges rebuild + ops_log (+ FTS maintain)`

Partial failure → full rollback. FTS must be **read-your-writes** before the MCP tool returns.

### 3.3 Graph rebuild policy

- On every L0 and L3 **content** write: delete derived edges from that node, re-extract `[[wikilink]]` / `#tag`, resolve stubs.
- **Preserve** explicit edges (`related`, `tunnel`, agent `link_nodes`).
- Topology JSON: nodes + edges only; **no** layout/canvas fields in store or graph API.

### 3.4 Embedding honesty

- Singleton `embedding_manifest` (provider, model, dims, base_url fingerprint).
- Vec / hybrid **refuse** on mismatch; point to `reembed`. Never silent wrong-model ranking.

### 3.5 Wiki vs raw

- Deleting wiki never deletes raw.
- Raw without wiki is valid (uncompiled debt); surfaces in `lint` / `status` / `doctor`.
- Wiki cites raw via `source_ids` / body links; citations are agent responsibility, server stores fields.

### 3.6 Ops log

- Append-only. Every significant mutation: ingest, wiki write/update, schema change, reembed, destructive delete, maintain apply.
- Not a second wiki. Not a substitute for filing synthesis into index/wiki.

### 3.7 Single active SoT per process

- `RAG_STORAGE_BACKEND` (future) selects one backend at start.
- Never dual-write DuckDB and markdown vault without exclusive mode + reindex contract.
- Cross-backend move only via explicit `export_vault` / bundle import.

---

## 4. Storage strategy

### 4.1 Thesis

Storage is the **permanence contract** for a local compounding store: one logical SoT, hard invariants, default physical form = **one DuckDB file**.

Domain code depends on a **domain-shaped `Storage` façade** (docs, chunks, search, graph, wiki, ops, caps)  -  not on `duckdb::Connection` scattered through services.

### 4.2 Priority (enforced order)

1. **Freeze spine behavior** on concrete DuckDB `Store` if needed: ingest → agent compile → cascade → `file_answer` → deterministic maintain.
2. **Land `Storage` trait** (sync `Send + Sync` OK) when forced by tests or to stop DuckDB types leaking  -  **not** as a multi-product roadmap.
3. Move `src/db/*` behind `storage/duckdb` without behavior change.
4. **Markdown vault** is the only second SoT with product justification (git / Obsidian / Karpathy files). Prefer `export_vault` first; live vault backend after trait + exclusive mode.
5. SQLite / Memory = engineering ports. Postgres+pgvector = shared-deploy only, not peer README identity.
6. External ANN (`VectorStore` half under Composite) only after **measured** full-scan cosine failure. Default remains co-located `embedding_json`.

### 4.3 Caps (honest refuse)

`StorageCaps`: `hybrid_fts`, `native_ann`, `transactions`, `concurrent_writers`, `vault_live_parse`.  
`status` / `doctor` surface caps; unsupported modes return structured errors, not fake success.

### 4.4 Non-goals (storage)

- Chroma / Pinecone / Qdrant as primary store or product identity  
- Multi-master replication / multi-tenant DB auth  
- Lowest-common-denominator SQL that cripples DuckDB FTS  
- Embeddings inside `.md` bodies (dirty git diffs)  
- Async-only rewrite as prerequisite for the trait  
- Multi-writer concurrent MCP+UI on one DuckDB file without exclusive StoreWorker  

### 4.5 Relation to STORAGE_ADAPTERS.md

[`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md) remains a **portability design**. This vision **demotes** adapter matrix from near-term architecture identity. Implement trait seam + DuckDB + export path before any second live backend story in README.

---

## 5. MCP surface strategy

### 5.1 Diagnosis

~73 tools in one `server.rs` is already a product failure mode: clients dump the full list; “advanced clusters” do not hide tools; co-equal hybrid / wiki / palace / maintain dilutes compile-first identity.

### 5.2 Hard-cap policy

**Default public surface: ~15–20 tools.** Prefer deletion, folding, and feature flags over documentation clustering of ~60 advanced tools.

### 5.3 Spine (promoted; instructions + schema lead with these)

| Tool | Layer | Role |
|------|-------|------|
| `ingest_raw` (unify text/file/drawer aliases under same policy) | L0 | Verbatim register; hash; immutable |
| `list_sources` / `get_source` | L0 | Read raw registry |
| `write_wiki_page` / `update_wiki_page` / `get_wiki_page` / `list_wiki_pages` | L3 | Agent compile surface |
| `get_schema` / `update_schema` | L3 | Conventions |
| `read_index` / `rebuild_index` (fold `update_index_entry` if needed) | L3 | Catalog |
| `append_log` / `read_log` | L3 | Timeline |
| `query_with_index` | L3→ | Cascade step 1 |
| `search_wiki` | L3/L1 | Cascade step 2 |
| `search` (`mode=lex\|vec\|hybrid`) | L1 | Cascade step 3 / escape hatch |
| `file_answer` | L3 | Compounding write-back |
| `get_neighbors` / `get_backlinks` (keep `get_graph` / `link_nodes` / `find_node` in spine or flag) | L2 | Structure nav |
| `status` / `doctor` | control | Layer health, not only row counts |
| `lint_wiki` (or minimal lint) | L3 hygiene | Uncompiled debt, orphans |

**Unify ingest aliases:** `ingest_text` / `ingest_file` / `add_drawer` apply the **same** hash / idempotency / immutable policy as `ingest_raw`, or become thin wrappers with one documented contract. Stop parallel raw APIs.

**Cascade (mandatory in instructions + schema):**

```
query_with_index → search_wiki → search (hybrid) → graph expand / neighbors → explicit search_raw
```

### 5.4 Fold or park (not co-equal spine)

| Area | Decision |
|------|----------|
| Tunnel family | Collapse into `link_nodes(rel_type=tunnel)` |
| Organize chrome (pin/boost/archive/outline/collections/topo) | Park / flag; not near-term identity |
| kg_* temporal suite | Park; do not ship a second fact graph beside wiki as core |
| diary / wake_up / checkpoint | Collapse to at most one bootstrap **or** express via wiki + ops_log |
| Server `compile_source` LLM path | Not spine; see §6 |
| Maintain analyze/plan/apply LLM theater | Deterministic analyze + whitelist apply of structural ops and **client-supplied** text; LLM plan optional/experimental |
| egui / server layout | Outside MCP product identity |
| MemPalace rename / AAAK | Refuse |

### 5.5 Layer health in status/doctor

Report at least:

- raw count, wiki count, index coverage  
- uncompiled raw debt  
- FTS ready, embedding_manifest match, `ready_for_search`  
- backend kind, redacted location, schema_version  
- not only undifferentiated row counts  

### 5.6 Dispatch shape

Target: `mcp/server.rs` shrinks toward **dispatch tables by layer cluster** (corpus / retrieval / graph / compile / control). No new business rules beyond parse → service → JSON.

---

## 6. Synthesis boundary (Karpathy-hard)

ARCHITECTURE_NOTES §6 is correct and is **restated as law**:

> The server does not call an LLM to cognify the graph or to own compilation. Compilation is tool-driven: the **MCP client LLM** writes pages using wiki tools.

### 6.1 P0 compile loop (only)

```
get_schema
  → get_source / get_document (raw)
  → write_wiki_page* / update_wiki_page*  (client-authored content)
  → rebuild_index / index touch
  → append_log
```

Optional: `file_answer` for durable Q&A pages.  
Optional later: `compile_source` as **apply-only** of client-supplied page payloads (validate structure, write, log)  -  **not** as `ChatClient` generation inside the store.

### 6.2 What server ChatClient must not be

- Happy-path compile in README / tool matrix leadership  
- Required when `RAG_LLM_ENABLED=false` (client-only compile must always work)  
- Invoked from L0 ingest content path  
- Invoked from L2 edge invent  
- Invoked from consolidate/refresh that invents page bodies without client text  

Optional `llm::ChatClient` may exist for experimental maintain assist only; domain services for L0–L3 **must not depend** on it for correctness. Prefer refuse with clear error over pretend success.

### 6.3 Maintain control plane

- Whitelist actions; **dry_run default**.  
- Allowed: refile meta, rebuild index/FTS, apply client-supplied wiki text, vacuum/checkpoint backend, logged deletes under flags.  
- Forbidden: silent raw body rewrite; unlogged deletes; auto-filing every search hit into wiki without L4 intent.

---

## 7. Parallel synthesis attics (amputation before rewrite)

Today the codebase and roadmap reify **too many places** synthesis can die: wiki + kg_facts + diary + ops_log + index + schema + maintain plans + drawers. Agents get confused; dead layers accumulate.

**Canonical compounding loop (only):**

```
raw (L0) → agent wiki write (L3) → index + ops_log → query cascade → file_answer
```

| Subplane | Role after this vision |
|----------|------------------------|
| wiki + index + schema + ops_log | **Core L3** |
| kg_facts | **Parked**  -  do not grow tool family; contradictions live in wiki/lint first |
| diary / wake_up / checkpoint | **Collapse or park**  -  bootstrap via schema + recent log + optional single tool |
| maintain plans | Ephemeral control-plane artifacts; not a second knowledge store |
| closets | = wiki `source_summary` only; no parallel summary system |

**Amputate features before module rewrite.** LayerKind and write-gates are worthless if the surface is still 73 co-equal tools.

---

## 8. Module map (target)

Build toward layer ownership. One binary; layered modules  -  not separate products.

```
src/
  main.rs, lib.rs, config.rs, error.rs, models.rs
  policy/              # LayerKind, write-gate, immutability asserts
  corpus/              # L0: ingest_raw path, list/get sources, hash, allowlist
  retrieval/           # L1: search, pack, manifest, reembed orchestration
  graph/               # L2: extract, resolve (pure); rebuild helpers
  compile/             # L3: wiki CRUD, index, schema, file_answer, lint surfaces
                       #     NO ChatClient required
  memory/              # parked or thin: diary/kg if kept behind flags
  maintain/            # control plane: analyze (det.), whitelist apply, vacuum
  storage/
    mod.rs             # trait Storage + StorageCaps + open_storage
    duckdb/            # current db/* moved here
  chunking/
  embeddings/          # vector providers only (not chat identity)
  llm/                 # optional chat  -  experimental, non-identity
  mcp/                 # thin L4 port: tools.rs params + server dispatch only
  util/
```

### 8.1 Immediate ownership fixes (before big split)

1. **Move L0 register paths out of `wiki/`**  -  `ingest_raw` / `list_sources` / `get_source` and immutability policy belong in corpus (or storage+corpus), not compile module.
2. **Unify L0 write aliases** under one immutable/hash contract.
3. **No new rules in `server.rs`** beyond dispatch.
4. **Graph rebuild** on all L0/L3 content write paths (no skipped extract → L2/L3 desync).
5. **Tests for invariants** (§10) before adapter matrix.

### 8.2 Current reality (honest)

| Area | Today (approx.) | Problem |
|------|-----------------|--------|
| `mcp/server.rs` | ~3.8k LOC, ~73 tools | God port; rules + surface mixed |
| `db/store.rs` | ~2.7k | Concrete DuckDB + domain mixed |
| `wiki/mod.rs` | ~2.4k | Owns L0 raw paths + L3 + LLM compile assist |
| `maintain/` | LLM plan/apply + compile_source | Dual-brain risk |
| Flat `db/` + peer features | RAG/graph/wiki/kg/diary/organize | No enforced layer map |

Refactor order: **cut surface → freeze synthesis boundary → write-gate → Storage trait → module split**. Not the reverse.

---

## 9. Workflows (canonical)

### 9.1 Ingest (verbatim)

```
ingest_raw | ingest_text | ingest_file   # one policy
  → allowlist (file)
  → content_hash / duplicate
  → atomic: doc(layer=raw, immutable) + chunks + embed + FTS + graph extract + ops_log
  → no entity page invention
```

### 9.2 Compile (client)

```
get_schema → get_source → write/update_wiki_page(s) → index → ops_log
```

### 9.3 Query

```
query_with_index → search_wiki → search(hybrid) → neighbors/expand → search_raw
  → optional file_answer
```

### 9.4 Hygiene

```
status / doctor / lint  →  deterministic uncompiled debt
maintain: dry_run structural apply only by default
```

---

## 10. Architectural invariants (encode as tests)

| # | Invariant | Test idea |
|---|-----------|-----------|
| 1 | Raw body immutable after ingest | `update` raw content → error |
| 2 | Wiki delete preserves raw | delete wiki → raw get still works |
| 3 | content_hash idempotent ingest | second ingest → no-op / explicit replace, no bloat |
| 4 | Manifest mismatch refuses vec/hybrid | wrong dims → structured error |
| 5 | FTS read-your-writes | ingest then lex search hits |
| 6 | Cascade helpers documented / available | spine tools exist; instructions list order |
| 7 | Maintain whitelist | non-whitelisted action rejected |
| 8 | Maintain dry_run default | no write without apply flag |
| 9 | Graph extract on wiki write | wikilink in page → edge present |
| 10 | Explicit edges survive rebuild | tunnel/related kept after re-extract |
| 11 | ops_log on significant mutation | wiki write increments log |
| 12 | Client compile without ChatClient | wiki CRUD works with LLM disabled |
| 13 | No layout fields in graph export | JSON schema free of x,y,fx,fy |
| 14 | Stdout MCP-only | logging on stderr |

---

## 11. Explicit non-goals

- Multi-tenant auth, team SaaS, HTTP MCP as architectural identity  
- External multi-backend vector DB as primary L1  
- Mandatory LLM NER for L2  
- Server-side force-directed layout / canvas positions in graph API  
- MemPalace 36-tool rename / AAAK default-on  
- Hybrid chunk search as product north star or first query step  
- Silent summarize-on-write into L0  
- In-process agent auto-filing every search hit without L4 intent  
- Separate binaries per layer  
- Feature parity with every surveyed repo  
- VSS/HNSW/HyDE/cross-encoder as P0 before index+wiki spine is crisp  
- Second parallel summary store beside L3 wiki  
- Multi-backend marketing before Storage trait + spine  
- Knowledge OS scope creep without hard tool cap  

---

## 12. Conflict resolutions (specialists vs critics)

| Tension | Resolution in this vision |
|---------|---------------------------|
| L0–L4 map vs “OS invites infinite scope” | Keep layers as **ownership/policy**; product name stays **compounding local agent memory store**; scope killed by tool cap + parked subplanes |
| Cluster ~60 advanced vs hard-cap 15–20 | **Hard-cap wins.** Clustering alone is insufficient. Flags/delete/fold. |
| Storage multi-backend story vs DuckDB hero | **DuckDB forever hero**; trait for domain boundary; markdown second SoT later; rest engineering-only |
| Layer-architect “optional LLM compile assist” vs Karpathy purist | **Purist wins for P0.** Client-only compile; server chat non-identity |
| Co-P0 hybrid + wiki vs compile-first | **Compile-first identity**; hybrid is substrate + cascade step 3 |
| kg/diary depth vs dead attics | **Park / collapse**; wiki+index+log is the compounding loop |
| Module rewrite vs amputate first | **Amputate surface first**, then gates, then modules |
| ARCHITECTURE_NOTES “pluggable backends first-class” | Softened: adapters are ports, not identity; see §4 |

---

## 13. Phased arrival (architecture, not feature laundry)

| Phase | Deliverable |
|-------|-------------|
| **V0 (now)** | Document this vision; stop adding co-equal tools; stop marketing server LLM compile as happy path |
| **V1 spine** | Unify L0 ingest policy; promote cascade in schema/instructions; status/doctor layer health; invariant tests |
| **V1 cut** | Reduce default MCP surface toward ≤20; park kg/organize/tunnel families/LLM compile |
| **V2 policy** | `policy` write-gate on all writers; corpus vs wiki module split (L0 out of wiki/) |
| **V3 storage** | `Storage` trait + `storage/duckdb`; MCP holds `Arc<dyn Storage>`; export_vault |
| **V4 optional** | Markdown vault backend; Memory for tests; ANN only if measured; experimental chat maintain behind flag |

v1 SPEC substrate (ingest/search/graph) remains shippable. Architecture is additive and subtractive: **add gates and tests; subtract surface; move modules last**.

---

## 14. Definition of done (architectural success)

1. A new contributor can name L0–L4 and say what may write each.  
2. Default tool list fits on one screen and matches the cascade.  
3. Client can compile a source into wiki with LLM server disabled.  
4. Deleting all wiki leaves raw intact.  
5. Wrong embed model cannot silently rank.  
6. `server.rs` does not own business rules.  
7. README says: local compounding store over DuckDB  -  not multi-backend RAG + palace + OS.  
8. North-star metrics are visible in `status`/`doctor` and used to reject new tools.

---

## 15. Doc relationship (summary)

| Doc | Role after this vision |
|-----|------------------------|
| **ARCHITECTURE_VISION.md** (this file) | Product/architecture core: identity, layers, surface, storage, consistency |
| ARCHITECTURE_NOTES.md | Historical detail + mermaid; defer to this file on conflicts |
| SPEC.md | v1 implementation contract (substrate) |
| FEATURES / ROADMAP / MCP_TOOL_MATRIX | Backlog; filter through §5 and §11 before implementing |
| STORAGE_ADAPTERS.md | Portability design; demoted from near-term identity |
| LOCAL_LLM_* | Experimental assist docs; must not redefine P0 compile |
| MEMPALACE_PARITY / ORGANIZE | Parity/chrome; park relative to spine |

Full supersede map: [§18 Relation to existing docs](#18-relation-to-existing-docs-which-supersedes-which).

---

## 16. Decision log

Product- and architecture-level. Graph/UI micro-decisions stay in [`GRAPH_EGUI_DECISIONS.md`](GRAPH_EGUI_DECISIONS.md).  
**Rule:** new identity choices get a row here; mechanics go to domain docs; schedule goes to ROADMAP.

### Decided

| ID | Decision | Where grounded |
|----|----------|----------------|
| V-D1 | Product is a **local compounding agent memory store** (stdio MCP + DuckDB), not multi-backend RAG / palace OS / SaaS | §0, §1 |
| V-D2 | **L0 body immutable** after ingest; L3 never rewrites L0 | §1.1, §2.2, §3.5 |
| V-D3 | **Compile is client-owned (L4)**; server ChatClient is non-identity; client-only compile must work with LLM disabled | §1.2, §6 |
| V-D4 | **Index-first cascade** is default SOP; hybrid is substrate + cascade step 3, not north star | §1.3, §5.3, §9.3 |
| V-D5 | **Deterministic L2** (`[[wikilink]]`, `#tag`, stubs); no mandatory LLM NER/cognify | §1.4, §11 |
| V-D6 | **DuckDB single-file is hero path**; adapters are ports, not co-equal products | §1.5, §4 |
| V-D7 | **Thin MCP port**: parse → service → JSON; no new business rules in `server.rs` | §1.6, §5.6, §8 |
| V-D8 | **Default public surface ~15–20 spine tools**; delete/fold/flag over clustering 60+ tools clients still see | §1.7, §5.2 |
| V-D9 | **Keep SPEC tool names**; no MemPalace rename; no AAAK default-on | §1.8, §11 |
| V-D10 | **Server emits topology only** (no layout/positions in graph API) | §3.3, §11 |
| V-D11 | **Atomic ingest unit** (doc+chunks+graph derived rebuild+ops_log+FTS) with full rollback | §3.2 |
| V-D12 | **Embedding manifest gate** refuses wrong-model vec/hybrid | §3.4, §10 |
| V-D13 | **FTS read-your-writes** before tool return | §3.2, §10 |
| V-D14 | **Single active SoT per process**; no dual-live DuckDB+vault without exclusive reindex | §3.7 |
| V-D15 | **ops_log append-only** on significant mutations | §3.6 |
| V-D16 | **Closet/summary = wiki `source_summary` only**; no parallel summary store | §2.3, §7 |
| V-D17 | **Tunnels via `link_nodes(rel_type=tunnel)`**; no co-equal tunnel tool family on spine | §5.4 |
| V-D18 | **kg_facts parked**; do not grow as second fact graph beside wiki core | §7, §5.4 |
| V-D19 | **diary / wake_up / checkpoint collapse or park**; bootstrap via schema + log (+ optional one tool) | §7, §5.4 |
| V-D20 | **Organize chrome (pin/boost/archive/collections/outlines) parked** relative to spine | §5.4 |
| V-D21 | **Maintain: dry_run default + whitelist**; structural ops and client-supplied text only; no silent raw rewrite | §6.3 |
| V-D22 | **Amputate surface before module rewrite** before Storage trait and big splits | §7, §8.2 |
| V-D23 | **Markdown vault** is only second SoT with product justification; prefer `export_vault` before live vault backend | §4.2 |
| V-D24 | **External ANN / VSS** only after measured full-scan pain | §4.2, §11 |
| V-D25 | **North-star metrics** (index coverage, uncompiled debt, index-first wins, filed answers, zero forbidden leaks) gate new work | §0, §14 |
| V-D26 | **This vision binds product intent** over FEATURES/ROADMAP/NOTES where they conflict | §15, §18 |

### Rejected

| ID | Rejection | Why |
|----|-----------|-----|
| V-R1 | Multi-backend vector DB (Chroma/Pinecone/Qdrant) as primary L1 or identity | Breaks single-file local story |
| V-R2 | Mandatory LLM NER / cognify for core graph | Non-reproducible; offline hostile |
| V-R3 | MemPalace 36-tool rename / AAAK default-on | Dialect tax; confuses clients |
| V-R4 | Hybrid chunk search as product north star or first query step | Dilutes compile-first identity |
| V-R5 | Server LLM compile as happy path / required for L3 | Dual-brain; fails air-gap and Karpathy split |
| V-R6 | Silent summarize-on-write into L0 | Destroys SoT trust |
| V-R7 | “Cluster ~60 advanced tools” as surface strategy without hard cap | Clients still enumerate everything |
| V-R8 | Co-equal marketing of RAG + graph + wiki + MemPalace + LLM | No product; feature pile |
| V-R9 | Multi-tenant auth / team SaaS / HTTP MCP as architectural identity | Different product |
| V-R10 | Server-side force-directed layout / canvas fields in store | Wrong layer |
| V-R11 | Dual-live DuckDB + vault or MCP + UI writers | Split brain / corruption |
| V-R12 | kg + diary + maintain plans as co-equal compounding stores | Too many synthesis attics |
| V-R13 | Feature parity with every surveyed repo | Evidence ≠ commitment |
| V-R14 | VSS/HNSW/HyDE/cross-encoder as P0 before index+wiki spine is crisp | Scale fashion before identity |
| V-R15 | Separate binaries per layer | Over-split; one binary is the product |
| V-R16 | Embeddings inside `.md` bodies | Dirty git diffs |
| V-R17 | In-process auto-file every search hit without L4 intent | Spurious compounding |
| V-R18 | Storage multi-backend matrix as near-term identity (NOTES softening) | Trait is domain boundary; DuckDB remains hero |

### Open

| ID | Question | Lean |
|----|----------|------|
| V-O1 | Exact default tool list (names) for the ≤20 spine | Use §5.3 as draft; freeze after one client playbook pass |
| V-O2 | Feature-flag mechanism (`RAG_TOOLS=spine\|full\|…`) vs hard delete of parked tools | Prefer flags first for one release, then delete dead code paths |
| V-O3 | Whether one `wake_up`-like bootstrap stays in spine | Lean: single optional bootstrap or pure schema+log; not a diary suite |
| V-O4 | `graph_expand_search` in spine vs neighbors-only + client expand | Lean: keep one expand helper if clients rely on it; not a second search identity |
| V-O5 | Timing of `Storage` trait vs surface cut | **Surface cut first** (V-D22); trait when DuckDB types leak or vault forced |
| V-O6 | Live markdown vault backend vs export-only long term | Export first; live vault only with exclusive mode + caps honesty |
| V-O7 | SPEC.md freeze-as-history vs rewrite to current spine | Lean: freeze v1 substrate; README + this vision for identity; no third competing SPEC unless implementers demand |
| V-O8 | blake3 stable ids vs UUID migrate gate | Follow GRAPH_DESIGN; do not block spine cut |
| V-O9 | Status/doctor metric schema for north-star counters | Design when implementing V1 spine health fields |
| V-O10 | Workspace crate split (`rag-mcp`, `rag-mcp-ui`, storage crates) timing | After policy + Storage seam; UI never blocks |
| V-O11 | Whether parked kg/diary code is deleted or `#[cfg]`/flag retained | Prefer flag retain one cycle, then delete if unused |
| V-O12 | Eval harness (MRR/recall@k) as gate for retrieval changes | Later ops unless regressions force earlier |

---

## 17. Phased evolution (now / next / later)

Maps §13 (V0–V4) onto operator language. **Not** a copy of ROADMAP P0/P1 feature laundry.

### Now (coherence + cut)

**Goal:** one teachable product; stop identity expansion; make the spine real.

| Work | Exit signal |
|------|-------------|
| Treat this vision as binding; stop adding co-equal tools | PR review cites V-D8 / V-R7 |
| Document cascade + spine in schema instructions + README | New user sees compile-first loop first |
| Unify L0 ingest policy across text/file/raw/drawer aliases | One hash/immutable contract |
| status/doctor: layer health + uncompiled debt + manifest | Not only row counts |
| Invariant tests from §10 (start with 1–5, 12) | CI fails on L0 rewrite / wrong embed rank |
| Plan (do not necessarily finish) surface reduction toward ≤20 | Flag or park list exists in ROADMAP truth-synced to code |
| No new kg/organize/LLM-compile identity work | Parked means parked |

**Explicitly not Now:** live multi-backend, UI paint, VSS, HTTP MCP, palace parity growth.

### Next (gates + structure)

**Goal:** enforce policy and domain boundaries without multi-product scope.

| Work | Exit signal |
|------|-------------|
| `policy` write-gate on all writers | Raw body update after create fails everywhere |
| L0 out of `wiki/` into corpus ownership | Module map §8.1 done |
| Fold tunnel family; park kg/diary/organize behind flags | Default tool list ≤20–25 |
| Maintain: deterministic analyze + whitelist apply; LLM plan experimental only | Client compile works with `RAG_LLM_ENABLED=false` |
| `Storage` trait + `storage/duckdb` move; MCP holds façade | No DuckDB types outside adapter |
| `export_vault` path | Human/git/Obsidian exit ramp exists |

### Later (ports + scale + optional chrome)

**Goal:** respond to measured pain and concrete users.

| Work | Exit signal |
|------|-------------|
| Markdown vault live backend (exclusive mode, caps) | Second SoT with honest doctor |
| Memory adapter for tests; SQLite/Postgres only if needed | Engineering ports, not README co-heroes |
| ANN / VSS / composite vectors | After measured full-scan failure |
| egui inspector (separate crate, snapshot/Mode C) | Optional; never MCP release gate |
| HTTP MCP, watch/sync, multi-format, eval harness | Deploy/ops demand only |
| Unpark kg/diary/organize only if spine metrics stay healthy | North-star metrics still gate |

### Phase mapping (internal labels)

| This section | §13 label | ROADMAP posture |
|--------------|-----------|-----------------|
| Now | V0 + V1 spine start | Truth-sync shipped vs backlog; cut noise |
| Next | V1 cut + V2 policy + V3 storage | Domain refactors; fewer default tools |
| Later | V4 optional + scale | Depth only when justified |

---

## 18. Relation to existing docs (which supersedes which)

### Precedence (high wins)

```
Maintainer decision / this chat
        │
        ▼
ARCHITECTURE_VISION.md     product identity, surface cap, synthesis boundary,
                           storage hero path, decided/rejected/open, now/next/later
        │
        ▼
ARCHITECTURE_NOTES.md      five-layer detail, mermaid, workflow sketches, invariants list
                           (defer to VISION on identity conflicts; NOTES still useful for diagrams)
        │
        ▼
Domain design docs         GRAPH_DESIGN, GRAPH_EGUI_DECISIONS, EGUI_GRAPH_VIEW,
                           ORGANIZE, MEMPALACE_PARITY, LOCAL_LLM_*, LLM_PROVIDERS,
                           STORAGE_ADAPTERS (mechanics only)
        │
        ▼
ROADMAP.md + MCP_TOOL_MATRIX.md   schedule + inventory (must track code; filtered by VISION)
        │
        ▼
FEATURES.md + REPOS_SURVEYED.md   research evidence (not commitments)
        │
        ▼
SPEC.md                    v1 substrate contract (lagging full product; freeze or rewrite open V-O7)
README.md                  operator-facing current behavior (must not contradict VISION identity)
```

### Supersede / defer matrix

| Doc | Still authoritative for | Superseded by VISION on |
|-----|-------------------------|-------------------------|
| **ARCHITECTURE_VISION.md** | Identity, spine, parks, storage priority, phases, decision log | (top for product intent) |
| **ARCHITECTURE_NOTES.md** | Layer table detail, ingest/compile/query mermaid, invariant wording inspiration | “Pluggable backends first-class” as identity; co-P0 hybrid-as-equal-north-star; any “server may own compile” soft language |
| **SPEC.md** | Early tool/schema baseline for substrate | Full product scope, “v1 is the whole story” |
| **FEATURES.md** | Gap analysis, adversarial A1–A30, proposed DDL ideas | Shipping commitments; parity-as-architecture |
| **ROADMAP.md** | Ordered engineering tasks (after filter) | Priority that expands surface without spine cut; treating shipped hybrid as still “P0 future” without truth-sync |
| **MCP_TOOL_MATRIX.md** | Tool × theme map | “All P0/P1 tools are spine”; existence claims without code check |
| **STORAGE_ADAPTERS.md** | Trait sketches, caps, vault layout, composite design | Near-term multi-backend product identity; README co-equal backends |
| **ORGANIZE.md** | Placement/rank/structure mechanics when unparked | Near-term identity / spine membership |
| **MEMPALACE_PARITY.md** | Capability mapping, reject list for palace clone | Growing default surface for parity score |
| **GRAPH_DESIGN.md** | Edge identity, extract/resolve, rebuild rules | (VISION does not override graph domain) |
| **GRAPH_EGUI_DECISIONS.md** | Graph+UI micro decided/rejected/open | Product-level surface (VISION); UI never gates MCP |
| **EGUI_GRAPH_VIEW.md** | Optional inspector UX | Any claim UI is required for product done |
| **LOCAL_LLM_MAINTENANCE.md** / **LOCAL_LLM_WIKI.md** | How to run Ollama assist paths | Compile happy path; ChatClient as L3 requirement |
| **LLM_PROVIDERS.md** | Provider env matrix | Expanding providers as architecture work |
| **REPOS_SURVEYED.md** | Survey evidence | Force-building surveyed features |
| **README.md** | How to run today | Must be updated to match VISION identity when it still markets multi-philosophy peer listing |

### Maintenance rule

1. Product-level choice → decision log (§16) here.  
2. Mechanics → domain doc.  
3. Schedule → ROADMAP (filtered by decided/rejected).  
4. Prefer deleting obsolete “will ship X as co-primary identity” language over duplicating it.  
5. Graph/UI micro-log stays in GRAPH_EGUI_DECISIONS; do not fork it here.

---

*End of architecture vision. Update §16 when product intent changes; update §17 when a phase exits; do not edit this file for every tool merge.*
