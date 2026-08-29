# rag-mcp Roadmap (P0–P2)

**North star (product idea, not this backlog):** [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md)  
**Laws:** [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md) · **Living map:** [`SYSTEM_MAP.md`](SYSTEM_MAP.md)

Derived from `FEATURES.md` research (including adversarial addenda A1–A30). Assumes SPEC v1 baseline (vector search + Obsidian graph tools) ships or is in progress.

This file is **sequencing only**. If a bullet fights the vision or principles, change the bullet or amend those docs deliberately; do not “win” by backlog inertia.

**Related:** [`FEATURES.md`](../FEATURES.md) · [`MCP_TOOL_MATRIX.md`](MCP_TOOL_MATRIX.md) · [`ARCHITECTURE_NOTES.md`](ARCHITECTURE_NOTES.md) · [`SPEC.md`](../SPEC.md) · [`GRAPH_DESIGN.md`](GRAPH_DESIGN.md) · [`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md) · [`GRAPH_EGUI_DECISIONS.md`](GRAPH_EGUI_DECISIONS.md)

## P0 — next sprint

### Retrieval honesty (hybrid + agent-usable hits)

- DuckDB FTS / BM25 on `chunks`; reindex on ingest/delete; **sync FTS refresh before tool return** (read-your-writes)
- Hybrid search: `mode=lex|vec|hybrid` + reciprocal rank fusion (RRF)
- Search filters DSL (`eq` / `in` / `exists` on wing, room, layer, kind, tag, uri, document_id, metadata, time range), `min_score`, snippets
- Citation-ready `SearchHit`: offsets, quote/snippet, `score_vec` / `score_lex` / `score_rrf`, layer, wing/room
- Token-budget pack: `max_context_tokens` (env `RAG_MAX_CONTEXT_TOKENS`) so `top_k` alone cannot overflow context
- Result diversity: `diversity=mmr|collapse_by_document` + `max_chunks_per_document` (env `RAG_MAX_CHUNKS_PER_DOC`)
- FTS tokenizer / language: env `RAG_FTS_STEMMER` (including `none` for CJK/code)
- Embedding cache by chunk `content_hash` / `embed_text_hash` (skip re-API on unchanged text)
- Contextual chunk prefix (title/heading into embed text) if cheap; else early P1

### Corpus integrity + safety

- `content_hash` + `check_duplicate` + stable upsert-by-uri
- Document columns: `wing`, `room`, `source_file`, `layer`, `kind`
- `list_wings` / `list_rooms` / `get_taxonomy`
- `delete_by_source`
- **`embedding_manifest`** (model/dims/provider fingerprint) + tools `get_embedding_manifest`, `reembed` / `reembed_document`; refuse vec/hybrid on mismatch
- **`schema_version`** + minimal **`doctor`** (migration lag, FTS present, embed dims, allowlist config)
- **Atomic ingest transaction** (doc + chunks + graph + ops_log commit/rollback as one unit)
- **`RAG_INGEST_ROOTS` path allowlist** for `ingest_file` (non-negotiable before directory/watch)
- Structured `STORE_BUSY` / write-lock timeout docs; MCP destructive tool annotations when rmcp supports
- Empty / tiny corpus: clear empty results + `status.ready_for_search`

### Compile layer (Karpathy) + graph expand

- Immutable raw layer (`ingest_raw`, `list_sources`, `get_source`)
- Wiki CRUD: `write_wiki_page`, `update_wiki_page`, `get_wiki_page`, `list_wiki_pages`
- Schema: `get_schema`, `update_schema`
- Index catalog: `read_index`, `update_index_entry`, `rebuild_index`
- Append-only log: `append_log`, `read_log`, `list_recent_ops`
- `ingest_source` (raw + multi-page wiki compile hooks)
- `query_with_index`, `search_wiki`, `file_answer`
- First-class `graph_expand_search` (vector/hybrid hits → neighbors)
- Extended `status` / index health (docs, chunks, nodes, edges, FTS, embed dims, db path)
- Keep SPEC v1 Obsidian graph tools (never drop): `get_graph`, `get_neighbors`, `get_backlinks`, `link_nodes`, `find_node`, wikilink/tag extract, stubs
- Wiki writes re-run wikilink/tag graph extract; stable graph node id on uri re-ingest
- Allow `rel_type=tunnel` via existing `link_nodes` (dedicated tunnel tools stay P1)

**Do not ship hybrid without diversity + citation fields + token budget.** Path allowlist and embedding manifest/reembed are not optional polish.

### Local LLM + maintenance (analyze / organize / compress / refresh)

See [`LOCAL_LLM_MAINTENANCE.md`](LOCAL_LLM_MAINTENANCE.md).

- Ollama / LM Studio OpenAI-compatible **chat** (`RAG_LLM_*`) — partial in `src/llm`
- Local **embeddings** via Ollama `/v1/embeddings` (provider ollama or openai_compat)
- `llm_status`, `analyze_corpus`, `plan_maintenance`, `apply_maintenance_plan` (dry_run)
- `maintain_organize`, `maintain_compress` (L0–L2), `maintain_refresh`, `vacuum_store`
- `refresh_stale_wiki`, batch `consolidate` — improve wiki without cloud
- Safety: action whitelist, dry_run default, ops_log, no silent raw delete
- Optional CLI: `rag-mcp maintain --dry-run|--apply`

### MemPalace parity (agent memory)

See [`MEMPALACE_PARITY.md`](MEMPALACE_PARITY.md). Workflow: `implement-mempalace-parity`.

- Wings / rooms / taxonomy / status
- Drawers: add_drawer, check_duplicate, delete_by_source, scoped search
- Temporal KG: kg_add/query/invalidate/supersede/timeline/stats
- Tunnels: create/list/delete/follow/find
- Diary + wake_up + checkpoint
- Verbatim storage (no silent summarize)
- Skip: AAAK default, mine/sync directory (P1), hooks (P2)

### Organize kit (P0.5 — ordering information)

See full design: [`ORGANIZE.md`](ORGANIZE.md).

- Placement: `refile`, `list_by_placement`, `list_taxonomy` (wing/room/layer/kind)
- Rank: `pin` / `unpin`, `set_boost`, list/search sort (`pinned` → boost → sort_key → updated)
- Lifecycle: `archive` / `unarchive`; default search skips archived
- Dedupe: `check_duplicate` (hash) wired to organize flow
- All mutations append `ops_log`

## P1

### Graph engine (domain — after P0c surface)

Full contract: [`GRAPH_DESIGN.md`](GRAPH_DESIGN.md). Decision log: [`GRAPH_EGUI_DECISIONS.md`](GRAPH_EGUI_DECISIONS.md).

- **P0a–P0c remain in P0** (label_key, delete_derived, no tag hijack, char offsets, multi-wikilink, frontier SQL, `graph_expand_search`, PKB defaults). Do not slip them into UI work.
- **P1 domain:** `unlink_nodes`; `resolve_stub`; tunnel CRUD; `node_aliases` + rename retention; `link_health` / orphans; demote-on-delete; blake3 ids + migrate gate; `depends_on` prereq/impact tools; occurrence-aware `get_backlinks`; edge provenance fields shipped
- **P1 vault graph:** live parse + extract-only rebuild + path resolve + explicit edge merge ([`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md) §4b, GRAPH_DESIGN §10)
- **P1 UI scaffold (optional, non-blocking):** workspace member `crates/rag-mcp-ui` non-default; pin egui/egui_graphs/petgraph; empty states + `--snapshot` load only — see [`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md) PR1. **Not** a release gate for headless MCP.

### Organize (structure + compile + hygiene)

- **Collections**: ordered sets (`collection_*` CRUD/reorder) + `pack_context` from collection
- **Outlines / tree**: `build_outline`, `get_outline`, `move_to_parent`, `sort_key`
- **Dependency order**: `topo_order` on `depends_on` (and related) edges; cycle report
- **Compile**: `consolidate` (propose/apply wiki page), richer `file_answer`
- **Hygiene**: `find_near_duplicates`, `merge_documents`, `merge_nodes`, `lint_organization`, aliases
- Search rank: document `boost` + pin bonus + optional recency half-life

### Storage adapters (multi-DB + markdown)

- Introduce `src/storage` + `trait Storage`; move current DuckDB code to `storage/duckdb` as default adapter
- `RAG_STORAGE_BACKEND` + `RAG_DATABASE_URL` + `RAG_VAULT_PATH`; `doctor` reports backend + caps
- **Markdown vault** adapter (P1 high): `.md` + YAML frontmatter SoT; `raw/` + `wiki/`; graph from `[[links]]`; vectors in `.rag/` sidecar (duckdb/sqlite/jsonl); `reindex`; works with Obsidian + git
- **`export_vault`** from DuckDB → same folder layout (migration / dual use)
- **SQLite** adapter (same logical schema, FTS5)
- **Postgres + pgvector** adapter (shared deploy)
- **Memory** adapter for fast tests
- Capability flags: hybrid FTS / native ANN / transactions / vault_live_parse (degrade or hard-error honestly)
- See [`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md)

### Wiki / memory / retrieval (continued)

- Page kinds: entity, concept, source_summary; `list_by_category` / `list_by_tag`
- Lint suite: `lint_wiki`, orphans, missing backlinks/concepts, contradictions, stale claims, data gaps, `link_health`, `resolve_stub`
- Temporal KG: `kg_add`, `kg_query`, `kg_invalidate`, `kg_supersede`, `kg_timeline`, `kg_stats`
- Agent memory: `wake_up`, `diary_write` / `diary_read`, `checkpoint`
- Tunnels CRUD: `create_tunnel`, `list_tunnels`, `delete_tunnel`, `follow_tunnels` (edge type already valid in P0)
- Soft-delete / memory lifecycle: `forget` / `undelete`, optional TTL; session/agent scopes on diaries/ops/kg
- `consolidate_memory` (diary/episode → wiki/kg compression)
- Retrieval UX: section-aware Markdown + `heading_path` and opt-in neighbor/parent-section expansion shipped as additive `search` / `pack_context` params; dedicated `expand_chunks`, `multi_get`, `find_similar`, multi-query RRF, recency boost, `group_by=document` remain
- Optional top-N rerank
- Ops: `sync_sources` directory ingest + explicit deleted-source pruning shipped; `backup_db`, full `doctor`, prune orphans, progress events on bulk ops remain
- Wiki write conflict: etag / `updated_at` If-Match on `update_wiki_page`
- Graph edge provenance: `chunk_id` / char offsets on edges
- Export: `export_vault` / markdown wiki+index+log, graph JSON
- MCP resources (doc/chunk/graph) + prompts (cited answer, ingest)
- Optional: `dry_run` / idempotency keys, pin/boost documents, `record_feedback` schema

## P2

### Graph engine + egui viewer

- Domain: `mentions` extract; optional `embeds` rel; confusable detection only if needed
- **egui inspector MVP paint:** RadialLocal, seed → neighbors, multi-edge collapse, caps 300/2000, status line, read-only Expand/Open ([`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md) PR2)
- Exclusive `--db` StoreWorker mode (single writer); never dual-live with MCP
- Post-MVP UI: filtered FR ≤~200, Hierarchical **Dep** only, wing/room chrome, optional writes in exclusive mode
- Server: `export_graph_snapshot` topology dump for Mode C coexistence
- Fallback if `egui_graphs` rots: plain egui painter + RadialLocal; drop UI without breaking agents

**Out of early scope (unchanged):** server-side layout; GUI as agent dependency; monobinary `ui` feature on `rag-mcp`.

### Storage adapters (scale)

- Markdown vault: file watcher reindex, pure tantivy/`none` index, multi-vault, concurrent Obsidian+MCP conflict policy
- Composite: SQL meta + **Qdrant** / **LanceDB** vector backend
- MotherDuck / LibSQL DSN variants
- `export_bundle` / `import_bundle` cross-backend migrate (including md ↔ sql)
- Connection pool tuning, read replicas (postgres)

### Other

- DuckDB VSS / HNSW when full-scan is too slow
- `hyde_search`, retrieval eval harness (MRR, recall@k, nDCG) + feedback export
- File watcher, multi-format ingest (html/pdf/code)
- HTTP+TLS MCP transport, read-only mode env gate
- Local embedding provider (e.g. ONNX / MiniLM)
- Explicit `search_raw` fallback; git-friendly export polish
- Client auto-save hooks protocol; integrity `repair`
- Usage/cost metrics; optional multi-dataset
- Optional AAAK off-by-default

## Out of scope (reminder)

Aligned with [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md) §5:

- External multi-backend vector DBs as **sole** primary store (composite ANN half later is optional scale, not identity)
- Mandatory LLM cognify / LightRAG NER for graph
- LangChain orchestration clone, **server-side** graph layout / canvas, multi-tenant auth in early releases
- Optional **client** inspector (`rag-mcp-ui`) is P1 scaffold / P2 paint, not an early release gate (see graph section above)
- AAAK default-on; palace rename of existing tools

## Doc map

| Doc | Role |
|-----|------|
| [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) | **North star**: what the product is |
| [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md) | ordered laws + conflict resolutions |
| [`SYSTEM_MAP.md`](SYSTEM_MAP.md) | layers, modules, tool clusters, doc graph |
| [`SPEC.md`](../SPEC.md) | v1 implementation contract |
| [`FEATURES.md`](../FEATURES.md) | research, gap analysis, DDL, adversarial A1–A30 |
| [`MCP_TOOL_MATRIX.md`](MCP_TOOL_MATRIX.md) | tool × Karpathy × MemPalace × priority |
| [`ARCHITECTURE_NOTES.md`](ARCHITECTURE_NOTES.md) | five-layer target architecture (mechanics) |
| [`GRAPH_DESIGN.md`](GRAPH_DESIGN.md) | L2 object graph: domain model, extract/resolve, rebuild, queries |
| [`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md) | optional `rag-mcp-ui` inspector (layout, process, MVP) |
| [`GRAPH_EGUI_DECISIONS.md`](GRAPH_EGUI_DECISIONS.md) | decided / rejected / open (graph + UI) |
| [`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md) | multi-DB + markdown vault |
| [`REPOS_SURVEYED.md`](REPOS_SURVEYED.md) | survey evidence (~105 sources) |
