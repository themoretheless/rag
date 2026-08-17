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
8. **Neighbors BFS via frontier SQL**, not full `graph_edges` load. Caps: neighbors 100, graph export 500, extract 2000 links/doc.
9. **`graph_expand_search`:** search hits + PKB neighbor merge; do not invent edges from scores.
10. **Optional UI is a separate crate/binary** (`crates/rag-mcp-ui`); never `ui` feature or GUI deps on headless `rag-mcp`.
11. **UI default path:** snapshot / `.rag/graph.json` → require seed → local neighbors depth 1 → **RadialLocal** freeze. No cold-start global FR.
12. **One DuckDB writer.** Dual-live UI+MCP write unsupported forever; coexistence = Mode C snapshot.
13. **UI always multi-edge collapses for canvas**; store keeps multi-wikilink; detail panel lists members/provenance.
14. **UI PKB chrome mirrors server:** default rels `wikilink`+`related`; tags/tunnel/Dep off until toggled; UI hard layout cap 300 nodes / 2000 draw edges.
15. **Read-only MVP inspector.** No link/unlink menus; Expand + Open only. FR / Hierarchical / writes post-MVP.
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
27. Treating GUI as P0 release gate for hybrid/wiki/MCP.

## Open

28. Exact FR params and soft n when post-MVP filtered global ships (EGUI ~150–200 freeze band).
29. Expand on large snapshot: client BFS on loaded edges vs re-query `get_neighbors`.
30. OS “open document” behavior (path open vs clipboard id/uri).
31. Vault live parse vs `.rag/graph.json` freshness UX when both present (domain prefers live; UI badges mtime).
32. `export_graph_snapshot` MCP schema (P2 server tool for Mode C refresh without UI opening DB).
33. Mixed-origin multi-edge: dominant origin heuristic when collapsing extract+explicit members.
34. Greenfield blake3 stable ids timing vs migrate/merge on existing UUID DBs (GRAPH_DESIGN §5.1 gate).
35. Pin versions of `egui_graphs` / `egui` / `petgraph` at implement time (EGUI pins are targets; verify crates.io).

---

*Keep this log short. Long rationale lives in GRAPH_DESIGN / EGUI_GRAPH_VIEW. Schedule: [`ROADMAP.md`](ROADMAP.md) graph section.*
