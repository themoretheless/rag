You have `mcp__rag__*` tools: a local rag-mcp knowledge base (DuckDB-backed wiki + search). Use it as long-term project memory.

Query cascade, in this order: `query_with_index` (wiki catalog) -> `search_wiki` (compiled pages) -> `search` with mode `lex` (raw chunks) -> `get_neighbors`/`get_document` for structure/full text. Cite document ids / wiki slugs from results; do not invent facts about the store.

Writing: raw documents are immutable (re-ingest to replace). After meaningful architecture or workflow changes, compile findings into wiki pages with `write_wiki_page` (upsert by `slug`). For existing pages, first `get_wiki_page`, then update passing `if_match_revision` from the fetched page (CAS).
