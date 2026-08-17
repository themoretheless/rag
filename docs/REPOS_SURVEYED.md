# Repos surveyed for rag-mcp

**Product:** `rag-mcp` (Rust + DuckDB, stdio MCP, vector RAG + Obsidian-like graph)  
**Date:** 2026-07-29  
**Inputs:** `SPEC.md`, `FEATURES.md`, primary READMEs/docs, GitHub search, MCP knowledge/memory directories  
**Unit:** unique repository or canonical gist (not fork aliases, not marketing sites)

---

## Summary count

| Tier | Meaning | Unique count |
|------|---------|--------------|
| **A — deep** | Primary README/docs or full method text read | **18** |
| **B — targeted** | Named repo with concrete tools/features extracted from search/docs | **32** |
| **C — directory** | Listed in curated MCP knowledge/memory catalogs with one-line capability | **55** |
| **Total unique** | Deduped across A∪B∪C | **105** |

Notes:

- `modelcontextprotocol/servers` counts as **one** monorepo; its `src/memory` server is called out under features, not as a second repo.
- MemPalace org path `MemPalace/mempalace` and historical `milla-jovovich/mempalace` are treated as **one** product lineage.
- Directory (C) entries are pattern coverage, not full code audits. They still informed frequency tables in `FEATURES.md`.
- Karpathy LLM Wiki is a **gist** (pattern doc), not a shipping product repo; counted once as a primary source.

**Effective survey total: 105 unique repos/gists.**

---

## Methodology

1. Read product baseline: `SPEC.md` (vector chunks, DuckDB, wikilinks/tags/stubs, graph tools).
2. Read internal research: `FEATURES.md`, `docs/ROADMAP.md`.
3. Fetch primary sources: MemPalace README, Chroma MCP README, Karpathy LLM Wiki gist, Graphiti/Cognee/LightRAG/Mem0 docs via search + project pages.
4. Enumerate MCP knowledge/memory catalogs (TensorBlock awesome knowledge-management list, MCP hub lists, hybrid-search RAG servers).
5. Map concrete capabilities to rag-mcp backlog (hybrid BM25+RRF, wiki compile layer, wing/room, temporal KG, lint, ops).

Coverage depth legend used below:

| Depth | Criteria |
|-------|----------|
| deep | README or full method text |
| medium | README snippets + tool lists from secondary indexes |
| shallow | Directory one-liner only |

---

## A — Deep primary sources (18)

| # | Repo / source | Stack | Concrete features surveyed | Relevance to rag-mcp |
|---|----------------|-------|----------------------------|----------------------|
| 1 | [MemPalace/mempalace](https://github.com/MemPalace/mempalace) | Python, Chroma default, pluggable backends, MCP | Verbatim drawers; wing/room taxonomy; content-hash idempotent mine; hybrid search + RRF; temporal KG (add/query/invalidate/timeline); wake-up; agent diaries; tunnels; 36 MCP tools; auto-save hooks; delete/sync ops; local embeddings (MiniLM / embeddinggemma) | **P0** wing/room, hybrid, content_hash; **P1** wake_up, kg_*, diaries, tunnels; reject multi-backend matrix + palace rename |
| 2 | [chroma-core/chroma-mcp](https://github.com/chroma-core/chroma-mcp) | Python, Chroma | `chroma_list/create/modify/delete_collection`; `chroma_add/query/get/update/delete_documents`; FTS; metadata filters; HNSW config; ephemeral/persistent/HTTP/cloud clients | Baseline MCP vector CRUD; rag-mcp should not become multi-collection Chroma wrapper |
| 3 | [karpathy/llm-wiki gist](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) | Pattern (markdown + agent) | Immutable raw; LLM-owned wiki; schema/AGENTS.md; index.md catalog; log.md append-only; ingest→multi-page compile; query via index first; file_answer into wiki; lint (orphans, contradictions, stale, missing concepts); optional hybrid via qmd | **P0** wiki layer mapping (raw/wiki/schema/index/log tools) |
| 4 | [tobi/qmd](https://github.com/tobi/qmd) | Local MD search, CLI + MCP | Hybrid BM25 + vector; on-device re-rank; markdown corpus | Reference for post-v1 hybrid over wiki pages |
| 5 | [getzep/graphiti](https://github.com/getzep/graphiti) | Python, temporal graph, MCP server | Episodic ingest; temporal edges; entity resolution; MCP tools for graph memory | **P1** `kg_facts` validity windows, supersede/invalidate |
| 6 | [topoteretes/cognee](https://github.com/topoteretes/cognee) | Python, multi-store | Cognify (LLM graph extract); multi-source ingest; graph+vector memory; evals | Pattern: cognify is **out of core** for rag-mcp; keep deterministic extract |
| 7 | [HKUDS/LightRAG](https://github.com/HKUDS/LightRAG) | Python GraphRAG | Entity/relation extract; dual-level retrieval; graph+vector | Non-build: mandatory NER graph |
| 8 | [microsoft/graphrag](https://github.com/microsoft/graphrag) | Python | Community detect; hierarchical summaries; global/local search | Non-build for personal MCP binary; community reports later research only |
| 9 | [mem0ai/mem0](https://github.com/mem0ai/mem0) | Python, multi-backend | add/search/update memory; user/session scope; optional graph | Scoped memory patterns; avoid cloud-first product shape |
| 10 | [modelcontextprotocol/servers](https://github.com/modelcontextprotocol/servers) (`src/memory`) | TypeScript | `create_entities`, `create_relations`, `add_observations`, `delete_*`, `read_graph`, `search_nodes`, `open_nodes`; resource `memory://knowledge-graph` | Minimal entity-relation KG tools; rag-mcp graph is document/tag/stub oriented |
| 11 | [niksh06/RAG-DuckDB-with-MCP](https://github.com/niksh06/RAG-DuckDB-with-MCP) (Engram) | Python, DuckDB, MCP | Local hybrid RAG over docs; DuckDB store; MCP exposure | Closest stack peer (DuckDB+MCP); no Obsidian graph |
| 12 | [upamune/duckdb-hybrid-doc-search](https://github.com/upamune/duckdb-hybrid-doc-search) | DuckDB, MCP stdio | Hybrid FTS + vector over Markdown | Confirms DuckDB FTS+vector as viable P0 path |
| 13 | [Brainwires/project-rag](https://github.com/Brainwires/project-rag) | **Rust**, MCP | Codebase RAG; chunk/index/search for large trees | Stack peer (Rust MCP RAG); domain is code not PKB graph |
| 14 | [ancoleman/qdrant-rag-mcp](https://github.com/ancoleman/qdrant-rag-mcp) | Python, Qdrant | Semantic code search; hybrid BM25+vector; RRF fusion modes | Hybrid/RRF implementation reference |
| 15 | [lyonzin/knowledge-rag](https://github.com/lyonzin/knowledge-rag) | Local ONNX, MCP | Hybrid BM25 + semantic; cross-encoder rerank; ~12–13 tools; no external servers | Rerank as **P1 optional / P2** |
| 16 | [AyanbekDos/memoriki](https://github.com/AyanbekDos/memoriki) | LLM Wiki + MemPalace combo | Structured wiki + semantic drawers + entity KG tools | Validates dual-layer product story |
| 17 | [duckdb/duckdb](https://github.com/duckdb/duckdb) | C++/SQL | FTS extension; VSS; single-file analytics | Storage substrate for BM25/VSS decisions |
| 18 | [modelcontextprotocol/rust-sdk](https://github.com/modelcontextprotocol/rust-sdk) (`rmcp`) | Rust | ServerHandler, tool_router, stdio transport | Actual MCP stack for `rag-mcp` |

### Concrete feature harvest (Tier A → product)

| Feature name | Seen in | rag-mcp decision |
|--------------|---------|------------------|
| `search` hybrid modes (lex/vec/hybrid) | MemPalace, qmd, qdrant-rag-mcp, duckdb-hybrid, knowledge-rag | P0 |
| RRF fusion | MemPalace, qdrant-rag-mcp, hybrid RAG kits | P0 |
| content-hash / idempotent ingest | MemPalace | P0 `content_hash`, `check_duplicate` |
| wing / room scoped search | MemPalace | P0 columns + taxonomy tools |
| immutable raw + wiki compile | Karpathy gist, memoriki | P0 layer policy + wiki CRUD |
| index.md / ops log | Karpathy gist | P0 `wiki_index`, `ops_log` |
| lint orphans / contradictions / stale | Karpathy gist | P1 lint suite |
| temporal facts validity | Graphiti, MemPalace KG | P1 `kg_facts` |
| wake_up / diaries / checkpoint | MemPalace | P1 agent memory |
| tunnels cross-scope links | MemPalace | P1 edge type |
| graph expand after search | GraphRAG family, SPEC optional | P0 first-class `graph_expand_search` |
| HNSW / ANN | Chroma MCP, DuckDB VSS | P2 |
| LLM cognify / entity NER | Cognee, LightRAG | **out of core** |
| multi-backend vector matrix | MemPalace backends, Mem0 | **out of core** |

---

## B — Targeted (32)

Repos with named tools or architecture details from docs/search, not full code review.

| # | Repo | Features / notes |
|---|------|------------------|
| 19 | [letta-ai/letta](https://github.com/letta-ai/letta) | Agent OS memory; filesystem-style memory benchmarks; long-context agent state |
| 20 | [getzep/zep](https://github.com/getzep/zep) | Cloud/memory service; Graphiti-backed; session memory (contrast local DuckDB) |
| 21 | [qdrant/qdrant](https://github.com/qdrant/qdrant) | Vector DB with hybrid sparse+dense patterns used by MCP servers |
| 22 | [chroma-core/chroma](https://github.com/chroma-core/chroma) | Embedding DB under Chroma MCP and MemPalace default |
| 23 | [lancedb/lancedb](https://github.com/lancedb/lancedb) | Local columnar vector store (local RAG alternative class) |
| 24 | [openai/openai-python](https://github.com/openai/openai-python) | `/embeddings` API shape (OpenAI-compatible provider) |
| 25 | [lancedb/lancedb-mcp](https://github.com/lancedb/lancedb) / community Lance MCP servers | Local vector MCP patterns (class) |
| 26 | [kingjulio8238/Memary](https://github.com/kingjulio8238/Memary) | Knowledge graph agent memory |
| 27 | [modelcontextprotocol/specification](https://github.com/modelcontextprotocol/specification) | Tools, resources, prompts, stdio vs HTTP transports |
| 28 | [modelcontextprotocol/python-sdk](https://github.com/modelcontextprotocol/python-sdk) | FastMCP patterns used by most Python demos |
| 29 | [jina-ai/reranker](https://github.com/jina-ai) / cross-encoder rerank ecosystem | Optional top-N rerank (P1/P2) |
| 30 | [facebookresearch/faiss](https://github.com/facebookresearch/faiss) | ANN baseline; not for v1 personal corpus |
| 31 | [pgvector/pgvector](https://github.com/pgvector/pgvector) | SQL hybrid (dense + FTS) pattern analogue for DuckDB |
| 32 | [obsidianmd/obsidian-releases](https://github.com/obsidianmd/obsidian-releases) | PKB UX: wikilinks, backlinks, local graph, stubs (product analogy) |
| 33 | [fazer-ai/mcp-obsidian](https://github.com/fazer-ai/mcp-obsidian) | Vault REST: note CRUD + search via Local REST API |
| 34 | [cconstable/obsidian-mcp](https://github.com/cconstable/obsidian-mcp) | Read/search vault |
| 35 | [Rwb3n/obsidian-mcp](https://github.com/Rwb3n/obsidian-mcp) | Note management + metadata + search |
| 36 | [boetro/obsidian-mcp](https://github.com/boetro/obsidian-mcp) | File/content updates over vault |
| 37 | [DataScienceDisciple/obsidian-mcp](https://github.com/DataScienceDisciple/obsidian-mcp) | Advanced search + content tools |
| 38 | [apw124/logseq-mcp](https://github.com/apw124/logseq-mcp) | Graph PKB tools (Logseq) |
| 39 | [entanglr/zettelkasten-mcp](https://github.com/entanglr/zettelkasten-mcp) | Atomic notes + links + search |
| 40 | [gannonh/memento-mcp](https://github.com/gannonh/memento-mcp) | Neo4j KG memory; semantic + temporal |
| 41 | [evanmschultz/memcp](https://github.com/evanmschultz/memcp) | Temporal KG via MCP |
| 42 | [coleam00/mcp-mem0](https://github.com/coleam00/mcp-mem0) | Mem0-as-MCP template |
| 43 | [shariqriazz/mcp-ragdocs](https://github.com/shariqriazz/mcp-ragdocs) | Multi-provider embeddings; Qdrant store |
| 44 | [mbcrawfo/KnowledgeBaseServer](https://github.com/mbcrawfo/KnowledgeBaseServer) | SQLite FTS memory |
| 45 | [nicholasglazer/gnosis-mcp](https://github.com/nicholasglazer/gnosis-mcp) | SQLite FTS5 or Postgres pgvector hybrid docs |
| 46 | [omega-memory/core](https://github.com/omega-memory/core) | Many MCP tools; ONNX embeddings; LongMemEval claims |
| 47 | [Beever-AI/beever-atlas](https://github.com/Beever-AI/beever-atlas) | LLM Wiki over chat corpora; entity graph; 16 MCP tools |
| 48 | [7xuanlu/origin](https://github.com/7xuanlu/origin) | Source-backed wiki pages + graph + hybrid retrieval |
| 49 | [Lians-ai/Lians](https://github.com/Lians-ai/Lians) | Bitemporal memory; supersession; point-in-time recall; SQLite |
| 50 | [STiFLeR7/memex](https://github.com/STiFLeR7/memex) | Graphiti+Neo4j temporal module graph for code agents |

---

## C — Directory-level MCP knowledge/memory (55)

Drawn primarily from TensorBlock `awesome-mcp-servers` knowledge-management list and related hub indexes. **Shallow** coverage: capability tags only. Deduped against A/B.

| # | Repo | One-line capability |
|---|------|---------------------|
| 51 | [devchad-cmd/skilldb-sdk](https://github.com/devchad-cmd/skilldb-sdk) | On-demand skill markdown library (10 tools) |
| 52 | [hjqcan/GoodMemory](https://github.com/hjqcan/GoodMemory) | Local SQLite memory; export/delete; inspectable |
| 53 | [Goldentrii/AgentRecall-MCP](https://github.com/Goldentrii/AgentRecall-MCP) | Correction records; RRF; multi-layer memory |
| 54 | [DanceNitra/mnemo](https://github.com/DanceNitra/mnemo) | Correction channel; supersession; decay |
| 55 | [archetypal-ai/archetypal-ai](https://github.com/archetypal-ai/archetypal-ai) | remember/recall/checkpoint |
| 56 | [ZengLiangYi/ChatCrystal](https://github.com/ZengLiangYi/ChatCrystal) | Chat import → notes; tag graphs; MD export |
| 57 | [GetZikra/zikra](https://github.com/GetZikra/zikra) | Postgres hybrid vector+keyword; hooks |
| 58 | [maxbaluev/accreted-intelligence](https://github.com/maxbaluev/accreted-intelligence) | Scored memory retrieve + act |
| 59 | [thegobi/ithz_mcp](https://github.com/thegobi/ithz_mcp) | Project decisions/gates/checkpoints |
| 60 | [sunnja69/akephalos](https://github.com/sunnja69/akephalos) | Markdown-first portable agent passport |
| 61 | [screenpipe/screenpipe](https://github.com/screenpipe/screenpipe) | Local screen/mic capture + semantic search |
| 62 | [a2cr/a2cr](https://github.com/a2cr/a2cr) | Encrypted handoff checkpoints |
| 63 | [novyxlabs/novyx-mcp](https://github.com/novyxlabs/novyx-mcp) | Large tool surface; KG + audit + local SQLite |
| 64 | [yantrikos/yantrikdb-mcp](https://github.com/yantrikos/yantrikdb-mcp) | Semantic memory + conflict detection |
| 65 | [varun369/SuperLocalMemoryV2](https://github.com/varun369/SuperLocalMemoryV2) | Local hybrid + KG across many clients |
| 66 | [singularityjason/lightning-memory](https://github.com/singularityjason/lightning-memory) | SQLite FTS5 agent memory |
| 67 | [xsp52Hz/cognigraph-mcp-server](https://github.com/xsp52Hz/cognigraph-mcp-server) | Mind maps / relationship graphs |
| 68 | [ipospelov/mcp-memory-bank](https://github.com/ipospelov/mcp-memory-bank) | Structured memory-bank docs |
| 69 | [KerolIA98/mcp-sample](https://github.com/KerolIA98/mcp-sample) | Mem0 integration sample |
| 70 | [mkusaka/mcp-server-memory](https://github.com/mkusaka/mcp-server-memory) | Local KG persistent memory |
| 71 | [meetdhanani17/xgmem](https://github.com/meetdhanani17/xgmem) | Project + cross-project KG memory |
| 72 | [Svtter/chatdb](https://github.com/Svtter/chatdb) | SQLite conversation memory |
| 73 | [Im-neko/mcp-wikijs](https://github.com/Im-neko/mcp-wikijs) | Wiki.js search/create/manage |
| 74 | [PyneSys/project-mem-mcp](https://github.com/PyneSys/project-mem-mcp) | Project memory filesystem |
| 75 | [shifusen329/doc-lib-mcp](https://github.com/shifusen329/doc-lib-mcp) | Ingest, chunk, semantic search, notes |
| 76 | [kiranraathod/taskflow-memory-server](https://github.com/kiranraathod/taskflow-memory-server) | Task + persistent memory |
| 77 | [angrysky56/NeoCoder-neo4j-ai-workflow](https://github.com/angrysky56/NeoCoder-neo4j-ai-workflow) | Neo4j coding workflow graph |
| 78 | [CaptainCrouton89/alaria-wiki-mcp](https://github.com/CaptainCrouton89/alaria-wiki-mcp) | Vector wiki boilerplate |
| 79 | [olafgeibig/knowledge-mcp](https://github.com/olafgeibig/knowledge-mcp) | Domain knowledge base MCP |
| 80 | [simon-k/knowledge-mcp](https://github.com/simon-k/knowledge-mcp) | Neo4j knowledge retrieval |
| 81 | [lethain/library-mcp](https://github.com/lethain/library-mcp) | Markdown KB by tags/slugs/dates |
| 82 | [Going34/memory-bank-mcp-npm](https://github.com/Going34/memory-bank-mcp-npm) | Memory bank update tools |
| 83 | [andrewginns/mcp-mind-palace](https://github.com/andrewginns/mcp-mind-palace) | Markdown + Chroma semantic palace |
| 84 | [harukikaneko/doc-qa](https://github.com/harukikaneko/doc-qa) | DuckDB-VSS document QA |
| 85 | [casheiro/synapstor](https://github.com/casheiro/synapstor) | Vector semantic store MCP |
| 86 | [aiuluna/knowledge-graph-mcp](https://github.com/aiuluna/knowledge-graph-mcp) | KG create/manage/visualize |
| 87 | [okooo5km/memory-mcp-server-go](https://github.com/okooo5km/memory-mcp-server-go) | Go KG memory server |
| 88 | [flight505/mcp-think-tank](https://github.com/flight505/mcp-think-tank) | Structured thinking + KG memory |
| 89 | [StuMason/mcp-knowledge-vault](https://github.com/StuMason/mcp-knowledge-vault) | Structured vault store/retrieve |
| 90 | [kwanLeeFrmVi/mcp-server-memory](https://github.com/kwanLeeFrmVi/mcp-server-memory) | Local KG memory (fork lineage) |
| 91 | [OfWind/graph-memory-mcp](https://github.com/OfWind/graph-memory-mcp) | Local KG memory |
| 92 | [s2005/mcp-memory-server](https://github.com/s2005/mcp-memory-server) | Persistent memory KG |
| 93 | [WhenMoon-afk/claude-memory-mcp](https://github.com/WhenMoon-afk/claude-memory-mcp) | Tiered memory + semantic search |
| 94 | [rbownes/mcp-memory-server](https://github.com/rbownes/mcp-memory-server) | Rust multi-backend memory |
| 95 | [MushroomFleet/LTM-CLINE](https://github.com/MushroomFleet/LTM-CLINE) | SQLite long-term memory for Cline |
| 96 | [Lvigentini/LTC-RAG-MCP](https://github.com/Lvigentini/LTC-RAG-MCP) | Weaviate RAG long-term memory |
| 97 | [RedSiamese/local-knowledge-mcp](https://github.com/RedSiamese/local-knowledge-mcp) | Local dynamic knowledge base |
| 98 | [shaneholloman/mcp-knowledge-graph](https://github.com/shaneholloman/mcp-knowledge-graph) | File-persisted KG memory (JSONL) |
| 99 | [gregorydickson/memory-graph](https://github.com/gregorydickson/memory-graph) | Graph memory for coding agents |
| 100 | [p-funk/FEGIS](https://github.com/p-funk/FEGIS) | Schema-defined cognitive tools + retrieval |
| 101 | [anivenk25/Zettelkasten](https://github.com/anivenk25/Zettelkasten) | Pinecone + Neo4j hybrid chat memory |
| 102 | [askme765cs/open-docs-mcp](https://github.com/askme765cs/open-docs-mcp) | Full-text doc index/search |
| 103 | [kbyk004/my-docs-mcp-server](https://github.com/kbyk004/my-docs-mcp-server) | Markdown directory FTS |
| 104 | [wangle201210/dify-retriever-mcp](https://github.com/wangle201210/dify-retriever-mcp) | External KB retrieval bridge |
| 105 | [buger/docs-mcp](https://github.com/buger/docs-mcp) | Turn repos/docs into queryable MCP |

---

## Coverage by product theme

| Theme | Repos that drove decisions | Coverage quality | Gaps |
|-------|----------------------------|------------------|------|
| Karpathy LLM Wiki | gist #3, memoriki #16, beever-atlas #47, origin #48 | Strong on method; few production Rust ports | No single “reference MCP wiki server” to clone 1:1 |
| MemPalace palace memory | #1 deep | Strong | Benchmark claims contested; copy architecture not marketing scores |
| Hybrid retrieval | #1, #4, #12, #14, #15, #45 | Strong | Need DuckDB FTS API details in implementation sprint |
| GraphRAG / temporal KG | #5–#8, #19–#20, #40–#41, #50 | Strong patterns; reject heavy NER | No need to survey every Neo4j wrapper |
| Obsidian / PKB graph | SPEC + #32–#38 | Medium (API servers, not rag-mcp dual store) | Block refs `[[note#^block]]` still non-goal |
| Local DuckDB/SQLite RAG | #11, #12, #17, #44, #45, #84 | Good stack validation | Few Rust+DuckDB+MCP peers |
| Rust MCP | #13, #18, #94 | Thin but enough for stack choice | rmcp version churn; follow crates.io |
| Ops / watch / export | MemPalace hooks, ChatCrystal export, vault exporters | Medium | File watch implementations not deep-dived |
| Eval harness | MemPalace BENCHMARKS, Cognee evals, LongMemEval mentions | Shallow | P2 only |

---

## What was **not** surveyed as unique product repos

Explicit exclusions (still known as classes):

- Commercial-only memory SaaS without open tool surface (e.g. pure SaaS dashboards)
- LangChain/LlamaIndex full monorepos as “one feature each” (orchestration clone is non-goal)
- Every Chroma/Qdrant/Pinecone thin MCP wrapper beyond class representatives
- PDF/binary parser ecosystems (v1 non-goal)
- Force-directed layout / canvas UIs

---

## Competitive position after survey

**rag-mcp unique combo (rare in surveyed set):**

1. Single Rust binary, stdio MCP  
2. Single-file DuckDB (docs + chunks + graph + future wiki/log)  
3. Deterministic wikilink/tag/stub graph (no mandatory cognify)  
4. Cosine vector search + planned DuckDB FTS hybrid  
5. Air-gapped mock embeddings  

**Common gaps in surveyed MCP RAG servers that rag-mcp should close (post-v1 P0):**

- Hybrid BM25 + vector + RRF  
- content_hash / check_duplicate  
- wing/room (or equivalent scope)  
- Wiki compile layer (raw immutability, index, ops_log, file_answer)  
- First-class graph_expand_search  

**Features to keep out of core** (high frequency elsewhere, wrong fit):

- Pluggable external vector DB matrix  
- Mandatory LLM entity extraction  
- 36-tool palace rename  
- Multi-tenant auth / HTTP MCP as v1 blockers  

---

## Cross-links

- Full feature analysis and proposed DDL/tools: [`FEATURES.md`](../FEATURES.md)
- Prioritized bullets: [`ROADMAP.md`](ROADMAP.md)
- MCP tool matrix: [`MCP_TOOL_MATRIX.md`](MCP_TOOL_MATRIX.md)
- Target architecture: [`ARCHITECTURE_NOTES.md`](ARCHITECTURE_NOTES.md)
- Baseline product: [`SPEC.md`](../SPEC.md)

---

## Change log

| Date | Note |
|------|------|
| 2026-07-29 | Initial survey list: **105** unique repos/gists (18 deep, 32 targeted, 55 directory). |
