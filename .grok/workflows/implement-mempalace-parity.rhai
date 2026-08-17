let meta = #{
    name: "implement-mempalace-parity",
    description: "Implement MemPalace-parity features in rag-mcp (~20 agents)",
    phases: [
        #{ title: "Schema-KG", detail: "kg_facts placement fields tunnels" },
        #{ title: "Core-tools", detail: "taxonomy drawers search filters" },
        #{ title: "Memory-KG", detail: "kg diary wake_up tunnels checkpoint" },
        #{ title: "Integrate", detail: "tests cargo green docs" },
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
common += "MUST READ: " + root + "/docs/MEMPALACE_PARITY.md and " + root + "/docs/ORGANIZE.md and existing src/.\n";
common += "MemPalace parity: wings/rooms, drawers, scoped search, check_duplicate, delete_by_source, temporal KG, tunnels, diary, wake_up, checkpoint, status/taxonomy, graph_stats. Verbatim storage — no silent summarize on ingest.\n";
common += "Stack: Rust + DuckDB + rmcp. Extend working codebase; cargo test must stay green. stderr logging only.\n";
common += "Do NOT rename existing tools to mempalace_*. Prefer our names; can add aliases in instructions.\n";
common += "If another workflow left partial hybrid/wiki code, integrate carefully — do not delete graph tools.\n";

// ===== Phase 1: Schema (4 agents) =====
phase("Schema-KG");

let j = [];

let p = common;
p += "TASK: Extend db schema migrate for MemPalace parity:\n";
p += "- Ensure documents: wing, room, source_file, layer, kind, content_hash, pinned, boost, status (active|archived)\n";
p += "- kg_facts(id, subject, predicate, object, valid_from, valid_to, source, created_at, invalidated_at, metadata_json)\n";
p += "- Indexes on subject, predicate, valid_to\n";
p += "Idempotent migrate. File: src/db/schema.rs\n";
j.push(#{ prompt: p, label: "mp-schema", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: models.rs: KgFact, DiaryEntry, TaxonomyNode, StatusReport, DrawerListItem; extend Document with wing/room/source_file/layer/kind/content_hash/pinned/boost/status if missing.\n";
j.push(#{ prompt: p, label: "mp-models", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: store.rs methods: list_wings, list_rooms, get_taxonomy counts; find_by_content_hash; delete_by_source; update document placement fields; filter list by wing/room/status.\n";
j.push(#{ prompt: p, label: "mp-store-tax", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: src/db/kg.rs: kg_add, kg_query(subject?, predicate?, object?, at_time?), kg_invalidate, kg_supersede, kg_timeline(subject), kg_stats. Wire mod.\n";
j.push(#{ prompt: p, label: "mp-kg-db", capability_mode: "all", output_schema: done_schema });

let r1 = parallel(j);
log("Schema-KG done");

// ===== Phase 2: Core tools (5 agents) =====
phase("Core-tools");

j = [];

p = common;
p += "TASK: MCP tools: list_wings, list_rooms, get_taxonomy, status (docs/chunks/nodes/edges/wings summary, ready_for_search, backend=duckdb). tools.rs params + server.rs.\n";
j.push(#{ prompt: p, label: "mp-tools-tax", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: check_duplicate(content or hash) tool; content_hash via util; add_drawer tool = ingest with required/optional wing,room,source_file; delete_by_source tool.\n";
j.push(#{ prompt: p, label: "mp-tools-drawer", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Extend search + list_documents with wing, room, source_file, include_archived filters. search still works if hybrid incomplete (vec mode).\n";
j.push(#{ prompt: p, label: "mp-tools-search", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: update_document_meta / update_drawer: wing room title metadata pinned boost status; do not force re-embed unless content changes.\n";
j.push(#{ prompt: p, label: "mp-tools-update", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: graph_stats tool (node counts by kind, edge counts by rel_type); ensure tunnel rel_type works with link_nodes.\n";
j.push(#{ prompt: p, label: "mp-tools-graph-stats", capability_mode: "all", output_schema: done_schema });

let r2 = parallel(j);
log("Core-tools done");

// ===== Phase 3: Memory + KG + tunnels + diary (6 agents) =====
phase("Memory-KG");

j = [];

p = common;
p += "TASK: MCP kg_add, kg_query, kg_invalidate, kg_supersede, kg_timeline, kg_stats — map AppError to McpError; schemars params.\n";
j.push(#{ prompt: p, label: "mp-mcp-kg", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: create_tunnel, list_tunnels, delete_tunnel, follow_tunnels, find_tunnels (edges rel_type=tunnel; follow = multi-hop along tunnels).\n";
j.push(#{ prompt: p, label: "mp-mcp-tunnels", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: diary_write(agent_name, content, wing?) stores kind=diary document; diary_read(agent_name, limit) list recent. wake_up returns status + last N diary + pinned docs + optional schema doc if exists.\n";
j.push(#{ prompt: p, label: "mp-mcp-diary", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: checkpoint(summary, optional diary content, optional agent_name) — append ops_log + optional diary_write.\n";
j.push(#{ prompt: p, label: "mp-mcp-checkpoint", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: memories_filed_away / list_recent_ops from ops_log if table exists; reconnect tool returns ok true (DuckDB no cache).\n";
j.push(#{ prompt: p, label: "mp-mcp-ops", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Update get_info instructions with MemPalace-style protocol: on session start call wake_up/status; before claims search or kg_query; after session diary_write/checkpoint. List new tools.\n";
j.push(#{ prompt: p, label: "mp-mcp-instructions", capability_mode: "all", output_schema: done_schema });

let r3 = parallel(j);
log("Memory-KG done");

// ===== Phase 4: Tests + docs + fix (5 agents) =====
phase("Integrate");

j = [];

p = common;
p += "TASK: tests/mempalace_parity_test.rs: wing/room taxonomy, check_duplicate, delete_by_source, kg add/query/invalidate/supersede, diary write/read, wake_up, tunnel create/follow. mock embedder temp dir.\n";
j.push(#{ prompt: p, label: "mp-tests", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Update README MemPalace-inspired section: wings/rooms, kg, diary, wake_up, tool list honesty.\n";
j.push(#{ prompt: p, label: "mp-readme", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: INTEGRATE: cargo test in " + root + ". Fix all errors. success=true only if cargo test exits 0.\n";
j.push(#{ prompt: p, label: "mp-fix1", capability_mode: "all", output_schema: fix_schema });

p = common;
p += "TASK: SECONDARY FIX: missing mods, imports, schema. cargo test. success=true only on green.\n";
j.push(#{ prompt: p, label: "mp-fix2", capability_mode: "all", output_schema: fix_schema });

p = common;
p += "TASK: FINAL FIX cargo test until green. success=true only then.\n";
j.push(#{ prompt: p, label: "mp-fix3", capability_mode: "all", output_schema: fix_schema });

let r4 = parallel(j);

let ok = false;
let summary = "incomplete";
let rem = "";
for r in r4 {
    if r != () && r.success {
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
    pfix += "ULTIMATE FIX. Notes: " + rem + ". cargo test until green.\n";
    let last = agent(pfix, #{ label: "mp-fix-ultimate", capability_mode: "all", output_schema: fix_schema });
    if last.success && last.output.success == true {
        ok = true;
        summary = last.output.summary;
        rem = "";
    } else if last.success {
        summary = last.output.summary;
        rem = last.output.remaining_errors;
    }
}

// 4+5+6+5+1 = 21 agents

complete(#{
    ok: ok,
    summary: summary,
    remaining_errors: rem,
    root: root,
    agents: "schema4 core5 memory6 integrate5+1 ~21",
});
