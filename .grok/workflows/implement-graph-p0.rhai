let meta = #{
    name: "implement-graph-p0",
    description: "Implement Obsidian graph + P0 hybrid/wiki for rag-mcp (~40 agents)",
    phases: [
        #{ title: "Schema-models", detail: "DDL columns, models, config" },
        #{ title: "Graph", detail: "extract resolve db graph" },
        #{ title: "Hybrid", detail: "FTS RRF search diversity" },
        #{ title: "Wiki", detail: "raw wiki index log ops" },
        #{ title: "MCP", detail: "tools server wiring" },
        #{ title: "Integrate", detail: "tests green + README" },
    ],
};

let done_schema = #{
    "type": "object",
    "required": ["files_written", "notes"],
    "properties": #{
        "files_written": #{ "type": "array", "items": #{ "type": "string" } },
        "notes": #{ "type": "string" },
    },
};

let fix_schema = #{
    "type": "object",
    "required": ["success", "summary", "remaining_errors"],
    "properties": #{
        "success": #{ "type": "boolean" },
        "summary": #{ "type": "string" },
        "remaining_errors": #{ "type": "string" },
    },
};

let root = if args == () { () } else { args.root };
if root == () {
    root = "/Users/themoretheless/Documents/Sources/rag";
}

let common = "";
common += "Workspace: " + root + "\n";
common += "Read first: " + root + "/SPEC.md, " + root + "/docs/ROADMAP.md (P0), " + root + "/FEATURES.md gap section.\n";
common += "Codebase is a WORKING rag-mcp (cargo test green). EXTEND it; do not rewrite from scratch.\n";
common += "Rules: English code; tracing only stderr; match existing style (thiserror AppError, Arc Store, rmcp tools).\n";
common += "Own only assigned files when possible. After edits run cargo check/test if useful.\n";
common += "DuckDB: bundled crate. FTS via INSTALL/LOAD fts + PRAGMA create_fts_index when available; if FTS extension fails in tests, implement lex score via simple term frequency in Rust as fallback behind same API.\n";

// ========== Phase 1: Schema + models + config (6 agents) ==========
phase("Schema-models");

let j = [];

let p = common;
p += "TASK: Extend src/db/schema.rs migrate() for P0 columns without breaking existing DBs:\n";
p += "- documents: content_hash, wing, room, source_file, layer (raw|wiki), kind (default document)\n";
p += "- chunks: content_hash optional\n";
p += "- ops_log table (id, ts, op, payload_json)\n";
p += "- wiki_index entries table OR store index as special docs; prefer wiki_index(id, label, summary, category, document_id, updated_at)\n";
p += "- embedding_manifest (id, provider, model, dims, created_at)\n";
p += "- meta schema_version\n";
p += "Use ALTER TABLE best-effort or versioned migrate; CREATE IF NOT EXISTS for new tables. Keep graph_* tables.\n";
j.push(#{ prompt: p, label: "p0-schema", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Extend src/models.rs: Document fields wing/room/source_file/layer/kind/content_hash; SearchHit with score_vec/score_lex/score_rrf/snippet/char offsets optional fields with serde defaults; OpsLogEntry; WikiIndexEntry; EmbeddingManifest; SearchMode enum. Keep backward-compatible Serialize defaults.\n";
j.push(#{ prompt: p, label: "p0-models", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Extend src/config.rs: RAG_INGEST_ROOTS (comma paths), RAG_MAX_CONTEXT_TOKENS, RAG_MAX_CHUNKS_PER_DOC, RAG_FTS_STEMMER, RAG_DEFAULT_SEARCH_MODE (vec|lex|hybrid). Validation.\n";
j.push(#{ prompt: p, label: "p0-config", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Extend src/error.rs with variants needed: Fts, Conflict, Forbidden (path allowlist), Busy if useful.\n";
j.push(#{ prompt: p, label: "p0-error", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Extend src/db/store.rs Document CRUD for new columns; content_hash on upsert; find_by_content_hash; list by layer/wing; delete_by_source; ensure embedding_manifest get/set. Keep existing tests working (update as needed).\n";
j.push(#{ prompt: p, label: "p0-store-cols", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Add src/util/hash.rs or src/hashutil.rs with content_hash (blake3 or sha256 of text) and path allowlist check against RAG_INGEST_ROOTS. Wire from lib.rs.\n";
j.push(#{ prompt: p, label: "p0-hash-util", capability_mode: "all", output_schema: done_schema });

let r1 = parallel(j);
log("Schema-models done");

// ========== Phase 2: Graph (6 agents) ==========
phase("Graph");

j = [];

p = common;
p += "TASK: Create src/graph/mod.rs and src/graph/extract.rs: parse [[wikilinks]] and #tags from text; skip ![[embeds]]; ExtractedLink { target_label, rel_type, context, alias }. Unit tests. Do not match tags inside fenced code best-effort.\n";
j.push(#{ prompt: p, label: "graph-extract", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Create src/graph/resolve.rs rebuild_document_graph(store, &Document) -> Result<(node_id, edge_count)>: upsert doc node, delete edges from source, create stubs/tags, promote stubs by title match. Use uuid for new nodes.\n";
j.push(#{ prompt: p, label: "graph-resolve", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Create src/db/graph.rs: graph CRUD on Store connection (upsert node, insert edges, delete edges from, find by label/doc, get_graph_view filter, neighbors BFS undirected, backlinks, link_nodes). pub use from db/mod.rs.\n";
j.push(#{ prompt: p, label: "graph-db", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Wire graph into lib.rs (pub mod graph). Hook rebuild_document_graph into mcp ingest_pipeline after chunks; on delete clean graph node edges. Return node_id/edge_count in IngestResult.\n";
j.push(#{ prompt: p, label: "graph-wire-ingest", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Add graph MCP param structs in tools.rs: GetGraphParams, GetNeighborsParams, GetBacklinksParams, LinkNodesParams, FindNodeParams, GraphExpandSearchParams.\n";
j.push(#{ prompt: p, label: "graph-tool-params", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Add graph tools on RagServer: get_graph, get_neighbors, get_backlinks, link_nodes, find_node, graph_expand_search. Update get_info instructions.\n";
j.push(#{ prompt: p, label: "graph-mcp-tools", capability_mode: "all", output_schema: done_schema });

let r2 = parallel(j);
log("Graph done");

// ========== Phase 3: Hybrid search (6 agents) ==========
phase("Hybrid");

j = [];

p = common;
p += "TASK: Implement FTS index management in src/db/fts.rs: ensure_fts(conn), reindex on demand, search_bm25(query, top_k, filters) -> Vec with scores. DuckDB fts extension; document fallback if INSTALL fails.\n";
j.push(#{ prompt: p, label: "hybrid-fts", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Implement RRF fusion + diversity collapse in src/db/search.rs: search modes vec|lex|hybrid; merge lists; max_chunks_per_document; min_score; fill SearchHit score_* fields and snippets.\n";
j.push(#{ prompt: p, label: "hybrid-rrf", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Implement token pack helper pack_hits(hits, max_tokens) estimating ~4 chars/token; optional tool pack_context. Put in src/search_pack.rs or embeddings-adjacent.\n";
j.push(#{ prompt: p, label: "hybrid-pack", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Extend SearchParams in tools.rs with mode, min_score, wing, room, layer, diversity, max_context_tokens, max_chunks_per_document. Wire search tool.\n";
j.push(#{ prompt: p, label: "hybrid-search-params", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Embedding manifest: on server start or first ingest record manifest; get_embedding_manifest tool; refuse vec search if stored dims != config (clear error). reembed_document optional simple path re-embed all chunks for one doc.\n";
j.push(#{ prompt: p, label: "hybrid-manifest", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: doctor + status tools: schema_version, fts ready, counts, embed dims, ready_for_search, ingest_roots configured. Minimal JSON.\n";
j.push(#{ prompt: p, label: "hybrid-doctor", capability_mode: "all", output_schema: done_schema });

let r3 = parallel(j);
log("Hybrid done");

// ========== Phase 4: Wiki / Karpathy layer (7 agents) ==========
phase("Wiki");

j = [];

p = common;
p += "TASK: src/wiki/mod.rs service helpers: write/update/get/list wiki pages as documents layer=wiki; enforce raw layer immutable (reject content update if layer=raw except via re-ingest replace).\n";
j.push(#{ prompt: p, label: "wiki-service", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: ops_log append/read/list_recent on Store; tools append_log, read_log, list_recent_ops.\n";
j.push(#{ prompt: p, label: "wiki-ops-log", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: wiki_index rebuild/read/update_entry: catalog of wiki pages with one-line summaries; tools read_index, update_index_entry, rebuild_index.\n";
j.push(#{ prompt: p, label: "wiki-index", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: schema document tools get_schema/update_schema storing at uri schema://agents as wiki/schema doc.\n";
j.push(#{ prompt: p, label: "wiki-schema-doc", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: file_answer tool: create wiki page from title+body+citations metadata; rebuild graph; append log; touch index.\n";
j.push(#{ prompt: p, label: "wiki-file-answer", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: ingest_raw + list_sources + get_source tools (layer=raw). ingest_file respects RAG_INGEST_ROOTS allowlist (reject outside).\n";
j.push(#{ prompt: p, label: "wiki-raw-tools", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: query_with_index + search_wiki tools (search filtered layer=wiki; index-first helper returning index entries matching query string simply).\n";
j.push(#{ prompt: p, label: "wiki-query-tools", capability_mode: "all", output_schema: done_schema });

let r4 = parallel(j);
log("Wiki done");

// ========== Phase 5: MCP polish + taxonomy (5 agents) ==========
phase("MCP");

j = [];

p = common;
p += "TASK: Add check_duplicate, list_wings, list_rooms, get_taxonomy, delete_by_source tools and store methods.\n";
j.push(#{ prompt: p, label: "mcp-taxonomy", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Unify all new params in tools.rs with schemars; ensure server.rs compiles with full tool list; update instructions string listing every tool.\n";
j.push(#{ prompt: p, label: "mcp-unify", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Update src/lib.rs and db/mod.rs and mcp/mod.rs module tree for graph, wiki, fts, util.\n";
j.push(#{ prompt: p, label: "mcp-modtree", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: main.rs: after open store, ensure FTS, record embedding manifest from config, log ready status to stderr.\n";
j.push(#{ prompt: p, label: "mcp-main", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Cargo.toml: add blake3 or sha2 if used for hashes; any other deps needed. Keep build working.\n";
j.push(#{ prompt: p, label: "mcp-cargo", capability_mode: "all", output_schema: done_schema });

let r5 = parallel(j);
log("MCP done");

// ========== Phase 6: Tests + docs + integrate (8 agents → fixers) ==========
phase("Integrate");

j = [];

p = common;
p += "TASK: tests/graph_test.rs: ingest A with [[B]] and #idea; stub B; ingest B title B; promote; backlinks; neighbors. Use mock embedder temp duckdb.\n";
j.push(#{ prompt: p, label: "test-graph", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: tests/hybrid_test.rs: ingest docs with distinctive keywords; hybrid/lex finds them; diversity collapse. Mock embeddings.\n";
j.push(#{ prompt: p, label: "test-hybrid", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: tests/wiki_test.rs: write wiki page, file_answer, index rebuild, raw immutability or re-ingest policy, ops log entry.\n";
j.push(#{ prompt: p, label: "test-wiki", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Update README.md honestly for implemented tools (graph+hybrid+wiki if present); env table; examples. Do not claim unimplemented features.\n";
j.push(#{ prompt: p, label: "docs-readme", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: examples/mcp.client.json update with env vars. .gitignore if missing.\n";
j.push(#{ prompt: p, label: "docs-examples", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: INTEGRATE PRIMARY: cd " + root + " && cargo test. Fix ALL compile/test failures. success=true only if cargo test exit 0.\n";
j.push(#{ prompt: p, label: "fix-primary", capability_mode: "all", output_schema: fix_schema });

p = common;
p += "TASK: INTEGRATE SECONDARY: scan for missing pub use, wrong imports, dead graph hooks. cargo test. success=true only on green.\n";
j.push(#{ prompt: p, label: "fix-secondary", capability_mode: "all", output_schema: fix_schema });

p = common;
p += "TASK: FINAL polish after others: cargo test until green; fix remaining. success=true only on green.\n";
j.push(#{ prompt: p, label: "fix-final", capability_mode: "all", output_schema: fix_schema });

let r6 = parallel(j);
log("Integrate panel done");

let ok = false;
let summary = "incomplete";
let rem = "";
for r in r6 {
    if r != () && r.success {
        // fix schema results have success field
        if r.output.success == true {
            ok = true;
            summary = r.output.summary;
            rem = "";
        } else if r.output.remaining_errors != () {
            if rem == "" {
                rem = r.output.remaining_errors;
                summary = r.output.summary;
            }
        }
    }
}

if ok == false {
    let pfix = common;
    pfix += "ULTIMATE FIXER. cargo test still failing. Notes: " + rem + "\n";
    pfix += "Fix everything needed. Iterate until cargo test passes. success=true only then.\n";
    let last = agent(pfix, #{ label: "fix-ultimate", capability_mode: "all", output_schema: fix_schema });
    if last.success && last.output.success == true {
        ok = true;
        summary = last.output.summary;
        rem = "";
    } else if last.success {
        summary = last.output.summary;
        rem = last.output.remaining_errors;
    }
}

// Agent count: 6+6+6+7+5+8+1 = 39

complete(#{
    ok: ok,
    summary: summary,
    remaining_errors: rem,
    root: root,
    agents: "schema6 graph6 hybrid6 wiki7 mcp5 integrate8+1 ~39",
});
