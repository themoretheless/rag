# MemPalace feature parity (rag-mcp)

**Source studied:** [MemPalace/mempalace](https://github.com/MemPalace/mempalace) `mcp_server.py` (36 tools), README palace model, temporal KG, hooks.  
**Principle:** same *capabilities*, our names + DuckDB (no AAAK dialect required; no Chroma). See [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md) §1.10 and [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md).

**Status note:** many rows marked “impl now” are shipped (see README / `server.rs`). This file is a capability map, not a rename guide and not the sequencing authority (that is [`ROADMAP.md`](ROADMAP.md)).

## Palace model mapping

| MemPalace | rag-mcp |
|-----------|---------|
| Drawer (verbatim unit) | document body + chunks; `ingest_text` / `add_drawer` alias |
| Wing | `documents.wing` |
| Room | `documents.room` |
| Hall | optional tag category / `hall` metadata (P1) |
| Closet (summary) | wiki `kind=source_summary` (compile layer) |
| Tunnel | `graph_edges.rel_type='tunnel'` |
| Hallway | named multi-hop path (optional table / edge bag) P1 |
| Content-hash drawer id | `content_hash` + `check_duplicate` |
| wake-up | `wake_up` tool: status + pinned + recent diary + schema snippet |
| Diary | documents `kind=diary`, wing `agents/<name>` |
| Temporal KG | `kg_facts` table + tools |
| mine / sync | `ingest_directory` / `sync_sources` (P1–P2) |
| reconnect | `reconnect` / cache clear (no-op for DuckDB single process) |
| hooks | protocol doc only (P2); not in-server for MCP stdio alone |

## Tool parity matrix

| MemPalace tool | rag-mcp target | Priority |
|----------------|----------------|----------|
| `status` | `status` / extend `stats` | **impl now** |
| `list_wings` | `list_wings` | **impl now** |
| `list_rooms` | `list_rooms` | **impl now** |
| `get_taxonomy` | `get_taxonomy` | **impl now** |
| `search` (wing/room/source filters) | extend `search` | **impl now** |
| `check_duplicate` | `check_duplicate` | **impl now** |
| `add_drawer` | `add_drawer` (= ingest with wing/room) | **impl now** |
| `delete_drawer` | `delete_document` (alias ok) | have |
| `delete_by_source` | `delete_by_source` | **impl now** |
| `get_drawer` / `list_drawers` | `get_document` / `list_documents` + filters | **impl now** |
| `update_drawer` | `update_document_meta` (placement/body policy) | **impl now** |
| `checkpoint` | `checkpoint` (batch note + optional diary) | **impl now** |
| `kg_add/query/invalidate/supersede/timeline/stats` | `kg_*` | **impl now** |
| `create/list/delete_tunnel`, `follow_tunnels`, `find_tunnels` | tunnel tools | **impl now** |
| `traverse` / `graph_stats` | `get_neighbors` / `graph_stats` | partial → **impl** |
| `diary_write` / `diary_read` | `diary_write` / `diary_read` | **impl now** |
| `wake-up` (CLI) | `wake_up` | **impl now** |
| `mine` / `sync` | directory mine + prune | P1 |
| `list/delete_hallway` | hallways | P1 optional |
| `get_aaak_spec` | skip (or minimal protocol text in status) | skip |
| `hook_settings` | config surface / docs | P2 |
| `memories_filed_away` | recent ops_log filter | **impl now** |
| `reconnect` | `reconnect` no-op success | **impl now** |

## Reject (do not clone blindly)

- AAAK dialect on by default  
- 36 tools with `mempalace_` prefix rename  
- Chroma multi-backend as primary  
- Mandatory LLM cognify  
- Process-global peer writer flock complexity (single DuckDB mutex enough for v1)

## Verbatim thesis (keep)

Store content as-is; do not summarize on ingest. Summaries only via explicit wiki/consolidate tools.
