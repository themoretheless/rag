# Graph engine + egui viewer — decision log

**Status:** meta-critic / editor freeze  
**Sources:** [`GRAPH_DESIGN.md`](GRAPH_DESIGN.md) (domain), [`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md) (UI), SPEC / FEATURES / STORAGE_ADAPTERS  
**Rule:** domain conflicts → GRAPH_DESIGN; canvas/process/MVP chrome → EGUI_GRAPH_VIEW.

---

## Decided

1. **One physical edge table, two UX projections (PKB vs Dep).** Never auto-upgrade `wikilink` → `depends_on`; never default local graph to Dep edge set.
2. **Server emits topology only.** No positions, force layout, or canvas fields in DuckDB / MCP tools.
3. **Unicode scalar offsets end-to-end** on extract/edges/chunks; no public `byte_start`/`byte_end`.
4. **Partitioned edge identity:** multi-row wikilink by occurrence; explicit rels mostly idempotent on `(source, target, rel)`; rebuild deletes **derived only** (`origin=extract` OR managed extract rels).
5. **Single `label_key`:** trim → NFKC → lowercase → collapse whitespace. No dual NFC/NFKC columns.
6. **Resolve never binds wikilink to `kind=tag`.** Multi-doc same title → no auto-bind; path/uri first; lint ambiguity.
7. **Document delete demotes to stub** and keeps inbound edges; no default incident wipe of depends_on/tunnel/wikilink.
8. **Project/UI neighbors use scoped frontier SQL** before the 300-node/depth-3
   product cap. General MCP `get_neighbors` retains its 100-node default and
   graph export its 500-node default; extract remains capped at 2000 links/doc.
9. **`graph_expand_search`:** search hits + PKB neighbor merge; do not invent edges from scores.
10. **Optional UI is a separate crate/binary** (`crates/rag-mcp-ui`); never `ui` feature or GUI deps on headless `rag-mcp`.
11. **UI normal live path:** HTTP gateway. Snapshot / `.rag/graph.json` and
    direct read-only DB are limited inspection modes; a Connections focus loads
    local neighbors at depth 1 and freezes **RadialLocal**. No cold-start global
    FR.
12. **One DuckDB writer.** Dual-live UI+MCP write is unsupported forever;
    coexistence is a gateway-owned HTTP client or Mode C snapshot. Direct
    `--db` remains exclusive and exposes read-only Wiki/Connections.
13. **UI always multi-edge collapses for canvas**; store keeps multi-wikilink; detail panel lists members/provenance.
14. **UI PKB chrome mirrors server:** default rels `wikilink`+`related`; tags/tunnel/Dep off until toggled; UI hard layout cap 300 nodes / 2000 draw edges.
15. **Read-only Connections canvas.** No link/unlink menus; Expand + Open only.
    Wiki editing is a separate HTTP-only workspace. FR / Hierarchical remain
    deferred.
16. **Markdown vault:** live `[[links]]` preferred for correctness; explicit sidecar edges merge-preserved on reindex (GRAPH_DESIGN §10).

## Rejected

17. Server-side force-directed or stored node coordinates.
18. `ui` feature / monobinary eframe on MCP (stdio corruption + GPU bloat for agents).
19. Dual-writer live DuckDB (UI + MCP).
20. Global UNIQUE(source,target,rel) that kills multi-wikilink; weight=count as sole storage.
21. UNIQUE(alias_key) exclusive lock; resolve-by-newest on title collision; title-based document node steal.
22. `delete_edges_from` all outgoing on rebuild (wipes tunnel/related).
23. Mandatory LLM NER / LightRAG for core graph.
24. Four layout modes + write chrome + GraphQuery trait sandwich before first paint.
25. Continuous fit_to_screen / continuous FR after settle (layout thrash).
26. Hierarchical layout on full wikilink+tag soup (Dep filter only).
27. Treating GUI as a runtime prerequisite for headless hybrid/wiki/MCP. The
    integrated product may still require native visual QA before release.

## Open

28. Exact FR params and soft n when post-MVP filtered global ships (EGUI ~150–200 freeze band).
30. OS “open document” behavior (path open vs clipboard id/uri).
31. Vault live parse vs `.rag/graph.json` freshness UX when both present (domain prefers live; UI badges mtime).
33. Mixed-origin multi-edge: dominant origin heuristic when collapsing extract+explicit members.
34. Greenfield blake3 stable ids timing vs migrate/merge on existing UUID DBs (GRAPH_DESIGN §5.1 gate).

---

*Keep this log short. Long rationale lives in GRAPH_DESIGN / EGUI_GRAPH_VIEW. Schedule: [`ROADMAP.md`](ROADMAP.md) graph section.*
