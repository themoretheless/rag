# Architecture Vision - rag-mcp

**Status:** product architecture core and evidence gates; binding for design and refactor priority.

**Updated:** 2026-09-02

**Does not replace:** [`SPEC.md`](../SPEC.md) (v1 wire contract), [`ARCHITECTURE_NOTES.md`](ARCHITECTURE_NOTES.md) (layer mechanics and diagrams).

**Supersedes where they conflict:** co-equal multi-philosophy marketing; multi-backend as product identity; server-LLM compile as the happy path; “cluster ~60 advanced tools” as UX strategy.

**Related:** [`FEATURES.md`](../FEATURES.md) · [`ROADMAP.md`](ROADMAP.md) · [`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md) · [`MCP_TOOL_MATRIX.md`](MCP_TOOL_MATRIX.md) · [`LOCAL_LLM_WIKI.md`](LOCAL_LLM_WIKI.md) · [`LOCAL_LLM_MAINTENANCE.md`](LOCAL_LLM_MAINTENANCE.md)

---

## 0. What the product is

rag-mcp is a **local compounding agent memory store**, exposed through stdio or
streamable HTTP MCP plus focused `/v1/*` product APIs over one gateway-owned
DuckDB file.

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
6. **Thin MCP port.** Business rules live in domain services and persistence
   seams. `mcp/server.rs` only composes bounded routers; the rmcp macro root in
   `mcp/facade.rs` is parse → service → JSON. No new business-rule growth there.
7. **Bounded public surface.** The default spine is currently 33 tools and is
   protected by a 15–33 count test. Compatibility depth is available only
   through explicit `RAG_TOOLS=full`; new defaults must strengthen the cascade
   or replace an existing entry.
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
│      nodes · wikilink/tagged edges · explicit tunnels       │
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
| Tunnel | `graph_edges.rel_type = 'tunnel'`; `link_nodes` in the spine and dedicated helpers in the full surface |
| Closet / summary | wiki page `kind=source_summary` (L3 only) |
| index.md | `wiki_index` |
| log.md | `ops_log` |
| Schema / AGENTS.md | `wiki_schema` / `schema://agents` |

---

## 3. Consistency model

### 3.1 Identity

- **Document identity:** stable `uri` is the logical re-ingest key. The Store
  transaction refuses a different document id that already owns the URI, so
  racing initial writers resolve to one success and one conflict.
- **Content identity:** `content_hash` for idempotency / `check_duplicate`.
- **Wiki identity:** slug (stable); graph node id derived from uri/slug.
- **Chunk identity:** subordinate to document; rebuildable.

### 3.2 Atomic ingest unit

One logical content transaction:

`document (+ chunks + embeddings) + graph node + derived edges rebuild + chunk generation`

Partial failure → full rollback. Writes advance one chunk generation; the next
lexical/hybrid read must refresh stale FTS once before ranking.

Source-manifest and operational metadata remain repairable adjacent state; they
are not part of this transaction. Deleted source state is a separate atomic
transaction covering documents, chunks, derived graph, wiki index and manifest
rows.

### 3.3 Graph rebuild policy

- On every L0 and L3 **content** write: delete derived edges from that node, re-extract `[[wikilink]]` / `#tag`, resolve stubs.
- **Preserve** explicit edges (`related`, `tunnel`, agent `link_nodes`).
- Topology JSON: nodes + edges only; **no** layout/canvas fields in store or graph API.
- Project graph reads scope project documents and direct companion nodes in SQL,
  then run deterministic traversal bounded to 300 nodes and depth 3.

### 3.4 Embedding honesty

- Singleton `embedding_manifest` (provider, model, dims, base_url fingerprint).
- Vec / hybrid **refuse** on mismatch; point to `reembed`. Never silent wrong-model ranking.

### 3.5 Wiki vs raw

- Deleting wiki never deletes raw.
- Raw without wiki is valid (uncompiled debt); surfaces in `lint` / `status` / `doctor`.
- Wiki cites raw via `source_ids` / body links; citations are agent responsibility, server stores fields.
- Revision history is lean and cursor-paginated; full snapshots load on demand.
  Restore is CAS-protected and refuses immutable raw documents.

### 3.6 Ops log

- Append-only. Domain operations that promise durable history (wiki/schema,
  maintenance and explicit checkpoints) record it there.
- Not a second wiki. Not a substitute for filing synthesis into index/wiki.
- HTTP Activity and background job state are bounded process-local
  observability, not replacements for `ops_log`.

### 3.7 Single active SoT per process

- The production `rag-mcp` process currently opens DuckDB only. The
  backend-neutral library document factory can opt into Markdown for its
  bounded document slice; it is not a full application runtime.
- Never dual-write DuckDB and markdown vault without exclusive mode + reindex contract.
- Cross-backend move only via explicit `export_vault` / bundle import.

---

## 4. Storage strategy

### 4.1 Thesis

Storage is the **permanence contract** for a local compounding store: one logical SoT, hard invariants, default physical form = **one DuckDB file**.

Domain services depend on focused application/repository seams. The current
`Storage` trait is deliberately limited to document lifecycle; search, graph,
wiki and transactional orchestration remain focused DuckDB services rather
than a fictional all-backend façade.

### 4.2 Priority (enforced order)

1. **Keep spine behavior frozen by tests** on the concrete DuckDB `Store`:
   ingest → agent compile → cascade → `file_answer` → deterministic maintain.
2. The sync `Storage` trait has landed as a **document-lifecycle vertical
   slice**. Expand it only behind shared capability conformance; do not claim a
   lowest-common-denominator product façade.
3. Keep DuckDB-specific search, graph, wiki and transaction behavior in focused
   `db/*` modules until a second full backend forces another seam.
4. **Markdown vault** is the only justified second SoT. Export plus opt-in
   document CRUD, sidecar rebuild and watcher are shipped; full application
   status still requires search/graph/recovery conformance.
5. SQLite / Memory remain possible engineering ports. Postgres+pgvector is a
   shared-deploy response, not peer README identity.
6. External ANN (`VectorStore` half under Composite) starts only after the
   measured exact-search gate fails. Default remains co-located
   `embedding_json`.

### 4.3 Capabilities (honest refuse)

`BackendMetadata` publishes the closed `StorageCapability` set: documents,
full-text search, vector search, transactions, graph and temporal KG. DuckDB
advertises the full runtime set; Markdown advertises documents only.
Unsupported backend configurations return structured errors, not fake success.

### 4.4 Non-goals (storage)

- Chroma / Pinecone / Qdrant as primary store or product identity  
- Multi-master replication / multi-tenant DB auth  
- Lowest-common-denominator SQL that cripples DuckDB FTS  
- Embeddings inside `.md` bodies (dirty git diffs)  
- Async-only rewrite as prerequisite for the trait  
- Multi-writer concurrent MCP+UI on one DuckDB file without exclusive StoreWorker  

### 4.5 Relation to STORAGE_ADAPTERS.md

[`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md) records the shipped document
contract and the conformance boundary for expansion. The adapter matrix remains
engineering portability, not product identity; no second full backend enters
README until it passes the advertised capability suite.

---

## 5. MCP surface strategy

### 5.1 Diagnosis

The registered compatibility surface is intentionally broad, but it is split
across bounded routers and hidden from default clients. Advertising every tool
would dilute the compile-first flow even if the code were modular.

### 5.2 Hard-cap policy

**Default public surface: 33 tools, bounded by test.** Prefer replacement,
folding and explicit feature flags over growing the default for parity.

### 5.3 Spine (promoted; instructions + schema lead with these)

| Tool | Layer | Role |
|------|-------|------|
| `ingest_raw` / `ingest_text` / `ingest_file` | L0 | Verbatim register through shared indexing; command-specific path and immutability policy |
| `list_sources` / `get_source` / `list_documents` / `get_document` | L0 | Read raw and document registry |
| `write_wiki_page` / `update_wiki_page` / `get_wiki_page` / `list_wiki_pages` | L3 | Agent compile surface |
| `get_schema` / `update_schema` | L3 | Conventions |
| `read_index` / `rebuild_index` | L3 | Catalog |
| `append_log` / `read_log` | L3 | Timeline |
| `query_with_index` | L3→ | Cascade step 1 |
| `search_wiki` | L3/L1 | Cascade step 2 |
| `search` / `multi_query_search` / `pack_context` | L1 | Raw/hybrid escape hatch and bounded context |
| `file_answer` / `lint_wiki` | L3 | Compounding write-back and hygiene |
| `get_neighbors` / `get_backlinks` / `link_nodes` / `find_node` / `get_graph` / `export_graph_snapshot` | L2 | Bounded structure navigation and topology export |
| `status` / `doctor` / `doctor_repair` | control | Layer health and explicit repair |

`ingest_text`, `ingest_file`, `ingest_raw` and drawer/wiki write paths share the
pure `DocumentIndexer` and atomic content transaction. Their deliberate wire
and immutability-policy differences remain at the command boundary.

**Cascade (mandatory in instructions + schema):**

```
query_with_index → search_wiki → search (hybrid) → neighbors/backlinks → get_document → pack_context
```

### 5.4 Compatibility depth (full surface)

| Area | Decision |
|------|----------|
| Tunnel family | Dedicated helpers remain in `full`; `link_nodes(rel_type=tunnel)` covers the spine |
| Organize, collections and outlines | Supported compatibility workflows; not default identity |
| `kg_*` temporal suite | Supported in `full`; not a second core source of truth beside wiki |
| diary / wake_up / checkpoint | Supported in `full`; wiki + ops log remain the compounding spine |
| Server `compile_source` LLM path | Full/experimental assist; not required by the compile loop |
| Maintain analyze/plan/apply | Deterministic analysis and dry-run/whitelist controls remain mandatory; LLM planning is optional |
| Native egui client | Product HTTP port; never a server-side layout or second writer |
| MemPalace rename / AAAK | Refuse |

### 5.5 Layer health in status/doctor

Report at least:

- raw count, wiki count, index coverage  
- uncompiled raw debt  
- FTS ready, embedding_manifest match, `ready_for_search`  
- backend kind, database path, storage capabilities, schema_version
- not only undifferentiated row counts  

### 5.6 Dispatch shape

Shipped shape: `mcp/server.rs` composes **routers by layer cluster** (corpus /
retrieval / graph / compile / control). `mcp/facade.rs` remains the macro-owned
compatibility root; no new business rules belong beyond parse → service → JSON.

---

## 6. Synthesis boundary (Karpathy-hard)

ARCHITECTURE_NOTES §6 is correct and is **restated as law**:

> The server does not call an LLM to cognify the graph or to own compilation. Compilation is tool-driven: the **MCP client LLM** writes pages using wiki tools.

### 6.1 Default compile loop

```
get_schema
  → get_source / get_document (raw)
  → write_wiki_page* / update_wiki_page*  (client-authored content)
  → rebuild_index / index touch
  → append_log
```

Optional: `file_answer` for durable Q&A pages.

The shipped full-surface `compile_source` is an optional LLM compatibility
assist, not the default compile loop and never a dependency of ingest or
retrieval correctness. New core compile services should be **apply-only** for
client-supplied page payloads (validate structure, write, log), rather than
moving `ChatClient` generation into the storage path.

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

## 8. Module map

Current high-level ownership. One binary; layered modules, not separate
products.

```
src/
  document_indexer.rs        pure chunk/embed preparation
  ingest.rs, source_sync.rs  application orchestration
  retrieval.rs, revisions.rs shared use cases
  graph/, wiki/              structure and compiled-knowledge policy
  db/                        DuckDB repositories + transaction owner
  storage/                   bounded document adapter contract
  mcp/, http_api/            wire adapters
crates/rag-mcp-ui/           optional native gateway client
```

### 8.1 Ownership status

| Boundary | Status |
|----------|--------|
| L0/L3 document preparation | `DocumentIndexer` + `IngestService` are transport-independent; raw registration compatibility functions still live in `wiki/` |
| Content persistence | document, chunks and derived graph commit through one atomic Store transaction |
| Retrieval/diagnostics/revisions | application services are shared by MCP and HTTP |
| Ports | MCP tool families and HTTP route clusters are split; composition roots remain |
| Invariant tests | immutable raw, CAS, embedding failure and graph rollback paths are covered |

The remaining L0 move is an ownership cleanup, not permission to rewrite the
public raw-source contract.

### 8.2 Current reality (honest)

| Area | Today (approx.) | Boundary |
|------|-----------------|----------|
| `mcp/facade.rs` | ~4.6k LOC | rmcp macro composition root; bounded tool routers own surface groups |
| `db/store.rs` | ~4.1k LOC | compatibility façade and transaction owner; focused repositories own search/FTS/graph/catalog/manifest algorithms |
| `wiki/mod.rs` | ~3.1k LOC | L3 behavior plus legacy raw compatibility entry points |
| `crates/rag-mcp-ui/src/app.rs` | ~3.7k LOC | workspace/navigation coordinator; gateway DTOs and rendering are split |
| `Storage` | document lifecycle slice | DuckDB remains the only full application backend |

Refactor order is now **freeze behavior → route through an application seam →
extract a tested slice → leave compatibility delegation**. Mechanical file
splitting is not progress.

---

## 9. Workflows (canonical)

### 9.1 Ingest (verbatim)

```
ingest_raw | ingest_text | ingest_file   # one policy
  → allowlist (file)
  → content_hash / duplicate
  → atomic: doc(layer=raw, immutable) + chunks + embed + graph extract + generation
  → repairable manifest / operation metadata
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
| 5 | FTS generation consistency | ingest then the first lex search refreshes once and returns the hit |
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
- VSS/HNSW/HyDE/cross-encoder before the measured retrieval gate fails
- Second parallel summary store beside L3 wiki  
- Multi-backend marketing before capability conformance
- Knowledge OS scope creep without hard tool cap  

---

## 12. Conflict resolutions (specialists vs critics)

| Tension | Resolution in this vision |
|---------|---------------------------|
| L0–L4 map vs “OS invites infinite scope” | Keep layers as **ownership/policy**; product name stays **compounding local agent memory store**; scope killed by tool cap + parked subplanes |
| Broad compatibility surface vs bounded default | **The tested 33-tool hard cap wins.** Clustering alone is insufficient; replacement/flags prevent unbounded growth. |
| Storage multi-backend story vs DuckDB hero | **DuckDB forever hero**; trait for domain boundary; Markdown document SoT is limited and full parity is gated; rest engineering-only |
| Layer-architect “optional LLM compile assist” vs Karpathy purist | **Client compile wins.** Server chat remains non-identity |
| Hybrid + wiki vs compile-first | **Compile-first identity**; hybrid is substrate + cascade step 3 |
| kg/diary depth vs dead attics | **Park / collapse**; wiki+index+log is the compounding loop |
| Module rewrite vs amputate first | **Amputate surface first**, then gates, then modules |
| ARCHITECTURE_NOTES “pluggable backends first-class” | Softened: adapters are ports, not identity; see §4 |

---

## 13. Evolution rule

The former V0–V4 sequence is implemented where it proved useful: a small
default spine, atomic ingest, focused services/repositories, HTTP/native ports,
portable recovery and a bounded Markdown document adapter. It is not an active
backlog.

Architecture evolves through tested seams: freeze behavior, extract one
responsibility, preserve the compatibility façade, then remove delegation that
no longer has callers. New storage or retrieval machinery enters only through
the measurable gates in §17 and [`ROADMAP.md`](ROADMAP.md).

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
| ARCHITECTURE_NOTES.md | Current layer mechanics + mermaid; defer to this file on product-identity conflicts |
| SPEC.md | v1 implementation contract (substrate) |
| README / SYSTEM_MAP | Current operator behavior and code ownership |
| ROADMAP | Current release gate and evidence-gated candidates only |
| FEATURES / MCP_TOOL_MATRIX | Historical research; not execution queues |
| STORAGE_ADAPTERS.md | Current bounded adapter truth plus capability gates |
| LOCAL_LLM_* | Experimental assist contracts; must not redefine client-owned compile |
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
| V-D8 | **Default public surface is currently 33 spine tools**, protected by a 15–33 count test; replace/fold/flag before adding more | §1.7, §5.2 |
| V-D9 | **Keep SPEC tool names**; no MemPalace rename; no AAAK default-on | §1.8, §11 |
| V-D10 | **Server emits topology only** (no layout/positions in graph API) | §3.3, §11 |
| V-D11 | **Atomic content unit** is document + replacement chunks + derived graph + chunk generation; source manifest, ops log and lazy FTS are adjacent state | §3.2 |
| V-D12 | **Embedding manifest gate** refuses wrong-model vec/hybrid | §3.4, §10 |
| V-D13 | **FTS read-your-writes** at the next lex/hybrid read, which refreshes stale generation before ranking | §3.2, §10 |
| V-D14 | **Single active SoT per process**; no dual-live DuckDB+vault without exclusive reindex | §3.7 |
| V-D15 | **ops_log append-only** for domain mutations that explicitly promise durable operation history | §3.6 |
| V-D16 | **Closet/summary = wiki `source_summary` only**; no parallel summary store | §2.3, §7 |
| V-D17 | **Tunnels via `link_nodes(rel_type=tunnel)`**; no co-equal tunnel tool family on spine | §5.4 |
| V-D18 | **kg_facts parked**; do not grow as second fact graph beside wiki core | §7, §5.4 |
| V-D19 | **diary / wake_up / checkpoint collapse or park**; bootstrap via schema + log (+ optional one tool) | §7, §5.4 |
| V-D20 | **Organize chrome (pin/boost/archive/collections/outlines) parked** relative to spine | §5.4 |
| V-D21 | **Maintain: dry_run default + whitelist**; structural ops and client-supplied text only; no silent raw rewrite | §6.3 |
| V-D22 | **Freeze behavior and application seams before adapter expansion or big splits** | §7, §8.2 |
| V-D23 | **Markdown vault** is the only second SoT with product justification; export and a limited document backend precede full parity | §4.2 |
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
| V-R14 | VSS/HNSW/HyDE/cross-encoder before the measured retrieval gate fails | Scale fashion before evidence |
| V-R15 | Separate binaries per layer | Over-split; one binary is the product |
| V-R16 | Embeddings inside `.md` bodies | Dirty git diffs |
| V-R17 | In-process auto-file every search hit without L4 intent | Spurious compounding |
| V-R18 | Storage multi-backend matrix as near-term identity (NOTES softening) | Trait is domain boundary; DuckDB remains hero |

### Open

| ID | Question | Lean |
|----|----------|------|
| V-O3 | Whether one `wake_up`-like bootstrap stays in spine | Lean: single optional bootstrap or pure schema+log; not a diary suite |
| V-O4 | `graph_expand_search` in spine vs neighbors-only + client expand | Lean: keep one expand helper if clients rely on it; not a second search identity |
| V-O5 | How far `Storage` should expand beyond the shipped document slice | Only when a second backend has a shared capability-conformance test |
| V-O6 | Full live Markdown application backend vs document/export use | Require lexical search, wikilink graph, recovery and honest refusal conformance |
| V-O7 | SPEC.md freeze-as-history vs rewrite to current spine | Lean: freeze v1 substrate; README + this vision for identity; no third competing SPEC unless implementers demand |
| V-O8 | blake3 stable ids vs UUID migrate gate | Follow GRAPH_DESIGN; do not block spine cut |
| V-O9 | Remaining north-star metrics beyond shipped layer health/index coverage | Add only when a client can act on the metric |
| V-O10 | Native visual-regression coverage | Add deterministic workspace screenshots after a real escaped layout regression or second desktop target |
| V-O11 | Whether parked kg/diary code is deleted or `#[cfg]`/flag retained | Prefer flag retain one cycle, then delete if unused |

---

## 17. Evolution status and gates

The earlier “HTTP/UI/eval later” phase has shipped. This does not promote those
ports to product identity; they remain clients and evidence around the same
compile-first store.

### Shipped foundation

| Contract | Evidence |
|----------|----------|
| One teachable default surface | `RAG_TOOLS=spine` is the default; full compatibility tools are opt-in |
| Verbatim/compiled boundary | immutable raw checks, wiki commands and rollback tests share the atomic document seam |
| Honest retrieval | lex/vec/hybrid, citations, packing, generation-aware FTS/vector caches and a labeled eval CLI |
| One-writer runtime | stdio + HTTP MCP and `/v1/*` share one Store; native live mode uses the gateway |
| Product navigation | Project Home, Unified Library, Search, Wiki, project Connections, Operations and document History are implemented |
| Port safety | product HTTP and mounted MCP reject bodies over 1 MiB even without `Content-Length`; Activity omits raw IP/UA, bodies, source paths and titles |
| Portability exit ramps | verified backup, bundle import/export, vault export and a limited Markdown document backend |

### Current release gate

Native Operations and revision inspection are integrated and their package
tests are green. The remaining release gate is full-workspace test/strict
Clippy evidence, a live one-writer rollout including sync/revision/backup
recovery, and native visual QA at default and compact window sizes. The exact
acceptance list lives in `ROADMAP.md`.

### Evidence-gated expansion

| Expansion | Entry gate |
|-----------|------------|
| ANN/VSS | repeated representative exact vector/hybrid p95 exceeds 300 ms or the declared resource threshold; a recorded local 100,111-chunk run observed 133.98 ms hybrid p95 and must be rerun for rollout decisions |
| Full Markdown backend | a real workflow requires Markdown as active SoT and shared search/graph/recovery conformance exists |
| Screenshot regression suite | an escaped workspace layout regression or a second supported desktop target justifies deterministic baselines |
| SQLite/Postgres or external vector half | a concrete deployment cannot be met by DuckDB or the justified Markdown path |

Parity checklists, old P0/P1 labels and the archived 500-item audit do not enter
this phase model.

---

## 18. Relation to existing docs (which supersedes which)

### Precedence (high wins)

```
Maintainer decision / this chat
        │
        ▼
ARCHITECTURE_VISION.md     product identity, surface cap, synthesis boundary,
                           storage hero path, decided/rejected/open, status/gates
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
| **ARCHITECTURE_VISION.md** | Identity, spine, storage rules, evidence gates and decision log | (top for product intent) |
| **ARCHITECTURE_NOTES.md** | Layer table detail, ingest/compile/query mermaid, invariant wording inspiration | “Pluggable backends first-class” as identity; co-P0 hybrid-as-equal-north-star; any “server may own compile” soft language |
| **SPEC.md** | Early tool/schema baseline for substrate | Full product scope, “v1 is the whole story” |
| **FEATURES.md** | Gap analysis, adversarial A1–A30, proposed DDL ideas | Shipping commitments; parity-as-architecture |
| **ROADMAP.md** | Current release gate and evidence-gated candidates | Priority that expands surface without spine cut; treating shipped work as future backlog |
| **MCP_TOOL_MATRIX.md** | Tool × theme map | “All P0/P1 tools are spine”; existence claims without code check |
| **STORAGE_ADAPTERS.md** | Implemented document contract, capability truth and full-backend conformance gate | Multi-backend product identity; README co-equal backends |
| **ORGANIZE.md** | Placement/rank/structure mechanics when unparked | Near-term identity / spine membership |
| **MEMPALACE_PARITY.md** | Capability mapping, reject list for palace clone | Growing default surface for parity score |
| **GRAPH_DESIGN.md** | Edge identity, extract/resolve, rebuild rules | (VISION does not override graph domain) |
| **GRAPH_EGUI_DECISIONS.md** | Graph+UI micro decided/rejected/open | Product-level surface (VISION); UI never gates MCP |
| **EGUI_GRAPH_VIEW.md** | Connections workspace design and graph caps | Treating the native port as the storage/product identity |
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
