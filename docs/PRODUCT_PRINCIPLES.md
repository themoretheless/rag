# Product Principles

**Product:** `rag-mcp`  
**Role:** ordered rules for design and review. If a feature fights these, change the feature or explicitly amend this file.

**North star:** [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md)  
**Mechanics:** [`ARCHITECTURE_NOTES.md`](ARCHITECTURE_NOTES.md) · [`SYSTEM_MAP.md`](SYSTEM_MAP.md)

---

## 1. Ordered principles

1. **Local-first, single binary.** Default deploy is one `rag-mcp` gateway on
   stdio and/or HTTP plus one store path. No Python runtime, no mandatory cloud.
   Logs stay on stderr; stdout is MCP when stdio is enabled.

2. **One logical store, pluggable physical backend.** A small `Storage`
   document contract exists, while search/graph/wiki still use focused DuckDB
   modules. **DuckDB is the only full application adapter.** Markdown is an
   opt-in document source of truth with honest limited capabilities;
   SQLite/Postgres remain possible ports. A remote vector DB **alone** is not
   the product’s primary source of truth.

3. **Verbatim first, synthesis second.** Ingest stores full text and chunks without mandatory paraphrase. Compiled wiki, consolidate, and file_answer are explicit workflows. Closets/summaries are L3 pages that cite L0, never silent rewrites of raw rows.

4. **Layers, not modes.** Raw, retrieval, graph, wiki, and agent memory coexist. Do not force “wiki-only” or “vector-only” product modes that delete the other layers.

5. **Honest retrieval.** Hybrid (lex|vec|hybrid + RRF) is substrate quality, not a leaderboard toy. Hits must be agent-usable: scores, snippets/offsets, diversity, token budget. Wrong embedding model/dims must fail closed (manifest), not rank nonsense.

6. **Deterministic graph structure.** `[[wikilink]]`, `#tag`, stubs, promote-on-title stay server-side and free of mandatory LLM NER. Entity pages and tunnels are additive; cognify is out of core.

7. **Scope before flat scan.** Wing, room, layer, kind, source_file, tags, archive filters are first-class. Palace hierarchy is metadata, not a rename of every tool.

8. **Compounding memory.** High-value answers and ops leave durable traces: wiki pages, index entries, ops_log, diaries, kg facts. A session that only retrieves and forgets is a failed product loop.

9. **Agent owns synthesis; server owns storage, search, graph hygiene, policy.**  
   - Default compile: **MCP client LLM** calls wiki tools.  
   - Optional: **server-side local chat** (`RAG_LLM_*`) for maintain/compile helpers when enabled.  
   - Never: mandatory server LLM to build the object graph.

10. **Substrate-honest MCP surface.** Keep stable tool names (`search`, `ingest_text`, …). Add tools; do not rebrand into a 36-name palace dialect or AAAK-by-default.

11. **Safety and integrity over convenience.**  
    - `RAG_INGEST_ROOTS` gates file reads.  
    - Atomic ingest (no half-chunks / ghost edges).  
    - Destructive maintain defaults to dry_run / confirm.  
    - `doctor` / `status` tell truth about FTS, schema, embed mismatch, readiness.

12. **Portable embeddings path until scale hurts.** Exact cosine over stored
    JSON vectors is the measured default. VSS/HNSW/Qdrant are evidence-gated
    scale escapes, not identity.

13. **Headless core, optional native product client.** MCP/product APIs remain
    complete without a GUI. `rag-mcp-ui` provides Home, Library, Search, Wiki,
    Connections, Operations and History over the gateway; it never becomes a
    second live DuckDB writer.

14. **Organize is a product layer, not a side script.** Place, rank, structure, compile, hygiene ([`ORGANIZE.md`](ORGANIZE.md)) keep the haystack shaped. Search finds needles; organization compounds.

15. **Code modularity scales with tool count.** Many MCP tools are OK if clusters are clear. A multi-thousand-line single `server.rs` is a **code** smell, not a license to rename tools. Split by domain modules; keep one ServerHandler façade if needed.

---

## 2. Invariants (definition of architectural success)

These are testable product laws (subset expanded from architecture notes):

1. Deleting or corrupting wiki rows never loses raw source text.
2. Re-ingest of identical `content_hash` is no-op or explicit replace, never silent corpus bloat.
3. Graph edge endpoints remain valid across re-ingest when uri/slug is stable.
4. `search` can fall back to raw chunks even if wiki is empty.
5. Index/wiki query path can work with zero embeddings (mock/lex/offline catalog).
6. Durable domain mutations that promise an operation record appear in
   `ops_log`; bounded HTTP Activity is not an audit log.
7. Stdout is MCP-only; logging on stderr.
8. One process owns one active store path for writes (no silent multi-writer corruption).
9. `ingest_file` never reads outside `RAG_INGEST_ROOTS`.
10. Vec/hybrid never silently ranks under the wrong embed model/dims.
11. Ingest is atomic: failure leaves neither half-chunks nor ghost edges.
12. Writes advance the chunk generation; the next lexical/hybrid read performs
    one single-flight FTS refresh before ranking.
13. Hybrid results include citation-oriented fields, diversity controls, and token budget.
14. Soft organization (refile, pin, archive) does not rewrite immutable raw bodies.
15. Optional local LLM tools refuse cleanly when disabled or unreachable; core search/graph/ingest still work.

---

## 3. Conflict resolutions (canonical)

| Tension | Resolution |
|---------|------------|
| Karpathy “anti-RAG / index only” vs hybrid BM25 | **Both:** index-first for compiled layer; hybrid is substrate + raw fallback. |
| MemPalace “never summarize” vs wiki summaries | **Split layers:** raw never paraphrased; L3 cites L0. |
| Palace rename vs SPEC names | **Keep SPEC/rag names;** wing/room as data + additive tools. |
| LLM graph extract vs deterministic wikilinks | **Deterministic first;** no mandatory cognify. |
| “DuckDB only” vs Storage adapters | **Logical single store;** DuckDB default; adapters with caps; export/vault dual-use; remote ANN alone ≠ SoT. |
| “Server does not call LLM” vs local maintain/compile | **No LLM for graph NER.** Optional chat for maintenance/wiki helpers; client remains primary author of knowledge. |
| Feature list growth vs focus | Prefer loops in [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) §4 over paper features (HyDE, AAAK, multi-tenant). |
| Research/parity priority labels vs shipped code | **Capability status follows code + README/SYSTEM_MAP.** ROADMAP contains only the current release gate and evidence-gated candidates. |

---

## 4. Decision checklist (for PRs and new tools)

Before adding a tool or backend:

- [ ] Which layer (L0–L4) does it strengthen?
- [ ] Does it preserve raw immutability and ops_log?
- [ ] Is retrieval still honest if this fails (fail closed)?
- [ ] Does it require mandatory cloud or Python? (should be no)
- [ ] Does it rename existing tools for fashion? (should be no)
- [ ] Can `doctor`/`status` surface its health?
- [ ] Is it expressible on DuckDB default path first?

---

## 5. Explicit non-goals (early product)

- Multi-tenant auth / SaaS billing surface  
- LangChain-style in-server orchestration clone  
- Mandatory LightRAG/GraphRAG NER pipelines  
- AAAK default-on; `mempalace_*` rename surface  
- Server-side force-directed graph layout / canvas SoT  
- PDF/binary parser ecosystem as identity (may appear later as ops)  
- External multi-backend vector DB as **sole** primary store  

---

## 6. Related docs

| Doc | Role |
|-----|------|
| [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) | What the product is |
| [`SYSTEM_MAP.md`](SYSTEM_MAP.md) | Layers, modules, tool clusters, doc graph |
| [`ARCHITECTURE_NOTES.md`](ARCHITECTURE_NOTES.md) | Five-layer mechanics, workflows, invariants detail |
| [`ROADMAP.md`](ROADMAP.md) | Current release gate and evidence-gated candidates |
| [`MEMPALACE_PARITY.md`](MEMPALACE_PARITY.md) | Capability map vs MemPalace (not a rename guide) |
| [`ORGANIZE.md`](ORGANIZE.md) | Place / rank / structure / compile / hygiene |
| [`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md) | Backend strategy |
| [`SPEC.md`](../SPEC.md) | v1 wire contract |

---

*End of product principles.*
