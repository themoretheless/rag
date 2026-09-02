# Spine tools + index-first playbook

**Env:** `RAG_TOOLS=spine` (default) | `full`

## Why

The broad compatibility surface overloads MCP clients. The default advertises
**33 compile-first tools**, with a test-enforced 15–33 bound; advanced
MemPalace/maintenance tools stay behind `RAG_TOOLS=full`.

## Index-first cascade

```
query_with_index → search_wiki → search (hybrid) → get_neighbors / get_backlinks
  → get_document / get_source (1–2 ids) → pack_context
```

Also seeded into `schema://agents` (`DEFAULT_SCHEMA` in `src/wiki/mod.rs`) and `get_info` instructions.

## Spine list

| Cluster | Tools |
|---------|--------|
| L0 | `ingest_raw`, `ingest_text`, `ingest_file`, `list_sources`, `get_source`, `list_documents`, `get_document` |
| L3 | `write_wiki_page`, `update_wiki_page`, `get_wiki_page`, `list_wiki_pages`, `get_schema`, `update_schema`, `read_index`, `rebuild_index`, `append_log`, `read_log`, `query_with_index`, `search_wiki`, `file_answer`, `lint_wiki` |
| L1 | `search`, `multi_query_search`, `pack_context` |
| L2 | `get_neighbors`, `get_backlinks`, `link_nodes`, `find_node`, `get_graph`, `export_graph_snapshot` |
| Control | `status`, `doctor`, `doctor_repair` |

Code: `src/mcp/surface.rs`.

## Full mode

```bash
export RAG_TOOLS=full
```

Unlocks: `add_drawer`, `kg_*`, tunnels, diary/wake_up, maintain_*,
`graph_expand_search`, and the other compatibility tools not listed above.

Calls to non-spine tools under `spine` return `invalid_params` with a clear message.
