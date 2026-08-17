# MCP Tool Matrix (rag-mcp)

One-page map of planned MCP tools vs **Karpathy LLM Wiki** (compile-once wiki pattern) and **MemPalace** (36 MCP tools). Priority: **v1** = SPEC baseline; **P0/P1/P2** = post-v1 roadmap (`FEATURES.md` / `docs/ROADMAP.md`).

**Related:** [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) · [`SYSTEM_MAP.md`](SYSTEM_MAP.md) · [`ROADMAP.md`](ROADMAP.md) · [`ARCHITECTURE_NOTES.md`](ARCHITECTURE_NOTES.md) · [`FEATURES.md`](../FEATURES.md) · [`SPEC.md`](../SPEC.md)

Priority labels in this matrix may lag shipped code; for “exists today” prefer [`README.md`](../README.md) and `src/mcp/server.rs`. Sequencing: ROADMAP. Identity: ARCHITECTURE_VISION.

## v1 substrate (SPEC)

| Tool | Purpose | Karpathy | MemPalace | Priority |
|------|---------|----------|-----------|----------|
| `ingest_text` | Ingest raw text; chunk, embed, extract wikilinks/tags | Drop source into raw (pre-compile) | `mempalace_add_drawer` (verbatim unit) | v1 |
| `ingest_file` | Read UTF-8 path and ingest (P0: gated by `RAG_INGEST_ROOTS`) | Clip / file into raw sources | `mempalace_mine` (file path) | v1 / P0 harden |
| `search` | Semantic (v1); later `lex\|vec\|hybrid` + RRF, filter DSL, snippets, diversity, token pack | Optional hybrid when index.md scales; `search_raw` fallback | `mempalace_search` (scoped wing/room) | v1 / P0 extend |
| `list_documents` | List docs without full body | Browse wiki/raw inventory | `mempalace_list_drawers` | v1 |
| `get_document` | Fetch doc (+ optional chunks) | Open a wiki/raw page | `mempalace_get_drawer` | v1 |
| `delete_document` | Delete doc, chunks, graph cleanup | Remove page / source | `mempalace_delete_drawer` | v1 |
| `stats` | Counts + db path | Wiki size sanity | `mempalace_status` / `kg_stats` / `graph_stats` | v1 |
| `get_graph` | Export `{nodes,edges}` topology | Obsidian graph view of wiki | `mempalace_graph_stats` (palace graph) | v1 |
| `get_neighbors` | Local BFS subgraph (depth) | Follow [[links]] neighborhood | `mempalace_traverse` | v1 |
| `get_backlinks` | Incoming edges / who links here | Backlinks panel | Incoming KG / room links | v1 |
| `link_nodes` | Explicit related edge (P0 also `tunnel`) | Manual cross-ref | `mempalace_create_tunnel` (cross-wing) | v1 |
| `find_node` | Resolve node by label/id/doc | Find page by title | Entity/room lookup | v1 |

## P0 — search quality, integrity, compile layer

| Tool | Purpose | Karpathy | MemPalace | Priority |
|------|---------|----------|-----------|----------|
| `search` (extend) | `mode=lex\|vec\|hybrid`, filters DSL, min_score, snippets, diversity, `max_context_tokens` | Hybrid when catalog large | Scoped hybrid search | P0 |
| `check_duplicate` | content_hash / similarity probe before write | Avoid re-ingest of same source | `mempalace_check_duplicate` | P0 |
| `list_wings` / `list_rooms` / `get_taxonomy` | Hierarchical scope nav | Category sections in index.md | `list_wings` / `list_rooms` / `get_taxonomy` | P0 |
| `delete_by_source` | Bulk delete by `source_file` | Remove all pages from one raw file | `mempalace_delete_by_source` | P0 |
| `status` | Health: FTS, dims, counts, path, `ready_for_search` | Session orientation | `mempalace_status` | P0 |
| `doctor` | Integrity: schema_version, FTS, embed dims, orphans, allowlist | Drift control | reconnect / status depth | P0 minimal |
| `get_embedding_manifest` | Active model/dims/provider fingerprint | Corpus honesty for search | Embed config surface | P0 |
| `reembed` / `reembed_document` | Migrate vectors after model/dim change | Keep wiki searchable after config change | Re-index drawers | P0 |
| `graph_expand_search` | Search hits + neighbor subgraph | Query then follow wiki links | Search + `traverse` / tunnels | P0 |
| `ingest_raw` / `list_sources` / `get_source` | Immutable raw layer register | **Raw sources** (never mutate) | Drawers as verbatim store | P0 |
| `write_wiki_page` / `update_wiki_page` / `get_wiki_page` / `list_wiki_pages` | Compiled wiki CRUD | **Wiki** layer LLM owns | — (MemPalace is store-not-compile) | P0 |
| `get_schema` / `update_schema` | Agent conventions document | **Schema** (AGENTS.md / CLAUDE.md) | Protocol text in `status` | P0 |
| `read_index` / `update_index_entry` / `rebuild_index` | Content catalog | **index.md** first-pass nav | Taxonomy tree (wing/room counts) | P0 |
| `append_log` / `read_log` / `list_recent_ops` | Append-only ops timeline | **log.md** chronological | Diary + checkpoint history | P0 |
| `ingest_source` | Raw + multi-page compile hooks | **Ingest** operation (one source → many pages) | `mempalace_mine` + agent compile outside | P0 |
| `query_with_index` | Index-first answer path | **Query**: read index → open pages | Wing/room scoped search first | P0 |
| `search_wiki` | Search compiled layer only | Query against wiki not raw | — | P0 |
| `file_answer` | Persist cited answer into wiki | **File answers back** into wiki | `checkpoint` / diary write | P0 |
| `pack_context` | Optional: hits → token-budgeted citation block | Cited answer packing | Multi-drawer pack | P0 optional |

## P0.5 — organize (placement + rank)

| Tool | Purpose | Karpathy | MemPalace | Priority |
|------|---------|----------|-----------|----------|
| `refile` | Set wing/room/kind/sort_key | Filing wiki pages into sections | Wing/room assignment | P0.5 |
| `list_by_placement` / `list_taxonomy` | Browse ordered shelves | index.md by category | `list_wings` / `list_rooms` | P0.5 |
| `pin` / `unpin` / `set_boost` | Manual rank | Evergreen pages on top | Priority drawers | P0.5 |
| `archive` / `unarchive` | Soft remove from default views | Drop without delete | Forget-lite | P0.5 |
| `check_duplicate` | content_hash probe | Avoid double sources | `check_duplicate` | P0.5 |

See [`ORGANIZE.md`](ORGANIZE.md).

## P1 — organize structure + lint + memory

| Tool | Purpose | Karpathy | MemPalace | Priority |
|------|---------|----------|-----------|----------|
| `collection_*` / `pack_context` | Ordered curated packs | Reading lists / query packs | Multi-drawer pack | P1 |
| `build_outline` / `get_outline` / `move_to_parent` | Hierarchical order | Wiki outline pages | Taxonomy tree | P1 |
| `topo_order` | Dependency reading/build order | Concept prerequisites | Traverse order | P1 |
| `consolidate` / `merge_*` / `lint_organization` | Compile + hygiene | Lint + synthesize | repair / dedupe | P1 |
| `lint_wiki` (+ orphans, missing backlinks, contradictions, stale, gaps) | Health-check compiled graph | **Lint** operation | — (no wiki lint suite) | P1 |
| `resolve_stub` / `link_health` | Promote stubs; edge hygiene | Fix missing concept pages | Reconnect after mine/sync | P1 |
| `create_tunnel` / `list_tunnels` / `delete_tunnel` / `follow_tunnels` | Cross-scope links | Cross-topic wiki links | Full tunnel tool family | P1 |
| `wake_up` | Bootstrap L0/L1 session memory | Schema + recent log on session start | `wake-up` CLI / protocol | P1 |
| `diary_write` / `diary_read` | Per-agent chronological notes | log.md agent notes | `mempalace_diary_write` / `diary_read` | P1 |
| `kg_add` / `kg_query` / `kg_invalidate` / `kg_supersede` / `kg_timeline` / `kg_stats` | Temporal fact graph | Stale-claim / contradiction lint substrate | Full `mempalace_kg_*` suite | P1 |
| `checkpoint` | Batch file + optional diary | Multi-page ingest savepoint | `mempalace_checkpoint` | P1 |
| `forget` / `undelete` | Soft-delete tombstone path | Remove without breaking node ids | Memory update/delete | P1 |
| `consolidate_memory` | Diary/episode → wiki/kg compression | Compile pass over agent notes | Sleep / consolidate pattern | P1–P2 |
| `backup_db` | Consistent DuckDB file snapshot | Before reembed / migration | Export outside core | P1 |
| `multi_get` / `find_similar` / `expand_chunks` | Retrieval UX | Open several index hits | Multi-drawer fetch | P1 |
| `export_vault` / `export_markdown` / `export_graph` | Obsidian/git-friendly dump | Wiki is markdown repo | Export outside core MCP | P1 |
| `ingest_directory` / `sync_sources` | Bulk mine + prune orphans | Batch ingest + cleanup | `mine` + `mempalace_sync` | P1 |
| `get_subgraph` | Seeded graph export | Local graph seed | Traverse with seeds | P1 |
| `reindex` | Rebuild FTS / embeddings subset | Repair catalog search | Re-mine subset | P1 |
| `record_feedback` | Label hits for eval (schema early) | — | Relevance labels | P1 schema / P2 harness |

## P2 — scale and optional protocols

| Tool | Purpose | Karpathy | MemPalace | Priority |
|------|---------|----------|-----------|----------|
| `search_raw` | Explicit raw-layer fallback | When wiki insufficient | Verbatim drawer search | P2 |
| `hyde_search` / `evaluate_retrieval` | Advanced retrieval / metrics | Optional at scale | Hybrid + rerank path | P2 |
| `repair` / `prune_orphans` / `watch_path` | Integrity + file watch | Drift control | `sync` / reconnect | P2 |
| `hook_settings` | Client auto-save protocol | — | `mempalace_hook_settings` | P2 |

## Config / non-tool surface (P0)

| Mechanism | Purpose |
|-----------|---------|
| `RAG_INGEST_ROOTS` | Path allowlist for `ingest_file` / later directory+watch |
| `RAG_FTS_STEMMER` | FTS language / `none` for CJK/code |
| `RAG_MAX_CONTEXT_TOKENS` | Default token pack for search hits |
| `RAG_MAX_CHUNKS_PER_DOC` | Diversity collapse per document |
| `embedding_manifest` table | Model/dims/provider; mismatch refuses vec search |
| Atomic ingest txn | Doc + chunks + graph + log commit as one unit |
| MCP safety annotations | Destructive vs read-only tools (when rmcp supports) |

**Legend — Karpathy layers:** raw (immutable) · wiki (LLM-maintained) · schema · index.md · log.md · ops ingest / query / lint.  
**Legend — MemPalace surface:** drawers, wings/rooms, search, tunnels, temporal KG, diaries, mine/sync, checkpoint.  
**Non-builds (research):** palace rename of tools, AAAK default-on, external multi-backend VDB as primary, mandatory LLM cognify.
