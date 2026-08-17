let meta = #{
    name: "build-object-graph",
    description: "Add Obsidian-like object graph layer to rag-mcp",
    phases: [
        #{ title: "Graph-core", detail: "models extract resolve db graph" },
        #{ title: "Wire", detail: "ingest hooks mcp tools tests" },
        #{ title: "Integrate", detail: "cargo test green" },
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
common += "Read SPEC.md section Object graph (Obsidian-like) and full file. Implement graph without breaking existing RAG tools.\n";
common += "English code; tracing only on stderr; match public APIs in SPEC.\n";
common += "Inspect existing code with read_file/grep before editing. Minimal diffs outside owned files.\n";

phase("Graph-core");

let jobs = [];

let p = common;
p += "TASK: Extend src/models.rs with GraphNode, GraphEdge, GraphView, GraphFilter, and extend IngestResult/Stats per SPEC. Keep existing types compiling.\n";
jobs.push(#{ prompt: p, label: "graph-models", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Create src/graph/mod.rs and src/graph/extract.rs: parse [[wikilinks]] and #tags; ExtractedLink type; unit tests. Ignore ![[embeds]] for edges.\n";
jobs.push(#{ prompt: p, label: "graph-extract", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Create src/graph/resolve.rs: rebuild_document_graph(store, doc) -> Result<(node_id, edge_count)>; stub create/promote by title; delete old edges from source; tags upsert.\n";
jobs.push(#{ prompt: p, label: "graph-resolve", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Extend src/db/schema.rs with graph_nodes and graph_edges tables + indexes from SPEC. migrate remains idempotent.\n";
jobs.push(#{ prompt: p, label: "graph-schema", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Create src/db/graph.rs implementing graph CRUD, neighbors BFS, get_graph_view, backlinks, link_nodes using Store connection/Mutex pattern already in store.rs. Wire pub mod graph in db/mod.rs.\n";
jobs.push(#{ prompt: p, label: "graph-db", capability_mode: "all", output_schema: done_schema });

let r1 = parallel(jobs);
log("Graph-core done");

phase("Wire");

let wjobs = [];

p = common;
p += "TASK: Hook graph rebuild into ingest paths in mcp/server.rs (and any service layer): after chunks inserted, call rebuild_document_graph; on delete_document remove edges/node; extend stats. Add MCP tools get_graph, get_neighbors, get_backlinks, link_nodes, find_node with params in tools.rs.\n";
wjobs.push(#{ prompt: p, label: "graph-mcp", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Update src/lib.rs module tree for graph. Ensure Store methods used by resolve exist (add thin wrappers on Store if needed).\n";
wjobs.push(#{ prompt: p, label: "graph-lib", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Add graph integration tests (wikilink stub, promote, backlinks, neighbors) in tests/ or #[cfg(test)]. Update README graph section + tool list.\n";
wjobs.push(#{ prompt: p, label: "graph-tests-docs", capability_mode: "all", output_schema: done_schema });

let r2 = parallel(wjobs);
log("Wire done");

phase("Integrate");

p = common;
p += "TASK: Run cargo test in " + root + ". Fix all failures related to graph and any breakage. success=true only if cargo test exits 0.\n";
let fix = agent(p, #{ label: "graph-integrate", capability_mode: "all", output_schema: fix_schema });

let ok = false;
let summary = "failed";
let rem = "";
if fix.success && fix.output.success == true {
    ok = true;
    summary = fix.output.summary;
} else if fix.success {
    summary = fix.output.summary;
    rem = fix.output.remaining_errors;
    p = common;
    p += "FINAL fix. Notes: " + rem + ". cargo test until green.\n";
    let fix2 = agent(p, #{ label: "graph-integrate-2", capability_mode: "all", output_schema: fix_schema });
    if fix2.success && fix2.output.success == true {
        ok = true;
        summary = fix2.output.summary;
        rem = "";
    } else if fix2.success {
        summary = fix2.output.summary;
        rem = fix2.output.remaining_errors;
    }
}

complete(#{ ok: ok, summary: summary, remaining_errors: rem, root: root });
