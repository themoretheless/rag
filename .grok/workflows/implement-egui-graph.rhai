let meta = #{
    name: "implement-egui-graph",
    description: "Implement rag-mcp-ui egui graph MVP (~20 agents)",
    phases: [
        #{ title: "Workspace", detail: "workspace + crate scaffold" },
        #{ title: "Core", detail: "load adapter layout models" },
        #{ title: "UI", detail: "app canvas detail status" },
        #{ title: "Integrate", detail: "build run tests docs" },
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
common += "Workspace root: " + root + "\n";
common += "MUST READ: " + root + "/docs/EGUI_GRAPH_VIEW.md and " + root + "/docs/GRAPH_EGUI_DECISIONS.md\n";
common += "MVP rules: separate crate crates/rag-mcp-ui; ZERO egui deps on rag-mcp package; default-members = [rag-mcp only]; RadialLocal layout; read-only; caps 300 nodes / 2000 edges; dual-write DuckDB+MCP forbidden.\n";
common += "Use GraphView/GraphNode/GraphEdge from rag-mcp lib. egui 0.31+ / eframe matching versions that compile together. Prefer plain egui Painter if egui_graphs API fights versions.\n";
common += "cargo build -p rag-mcp-ui must succeed. English code.\n";

// Phase 1: workspace (3)
phase("Workspace");
let j = [];

let p = common;
p += "TASK: Convert root to Cargo workspace: members = [\".\", \"crates/rag-mcp-ui\"], default-members = [\".\"]. Keep existing rag-mcp package at root working (cargo build -p rag-mcp).\n";
j.push(#{ prompt: p, label: "egui-ws", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Create crates/rag-mcp-ui/Cargo.toml: package rag-mcp-ui, bin rag-mcp-ui, deps eframe+egui (compatible versions), serde/serde_json, clap for CLI, path dependency on rag-mcp. No rmcp.\n";
j.push(#{ prompt: p, label: "egui-cargo", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Scaffold crates/rag-mcp-ui/src/{main,app,load,adapter,layout}.rs and ui/{mod,canvas,status,detail,empty}.rs with modules compiling (stubs ok if next agents fill).\n";
j.push(#{ prompt: p, label: "egui-scaffold", capability_mode: "all", output_schema: done_schema });

let _r1 = parallel(j);
log("Workspace done");

// Phase 2: core (6)
phase("Core");
j = [];

p = common;
p += "TASK: load.rs — load GraphView from --snapshot JSON path; optional --db exclusive Store::open + get_graph_view with filter rel_types wikilink+related, max_nodes 300. Fail clearly if cannot open. Export GraphView serde JSON format matching models.\n";
j.push(#{ prompt: p, label: "egui-load", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: adapter.rs — GraphView → UiGraph: nodes with label/kind/color, multi-edge collapse for display, hard cap 300 nodes / 2000 edges, drop excess with note.\n";
j.push(#{ prompt: p, label: "egui-adapter", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: layout.rs — RadialLocal: seed at center, neighbors on rings by BFS depth, freeze positions after place. Deterministic order by node id.\n";
j.push(#{ prompt: p, label: "egui-layout", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Implement Store graph export helper if needed: pub fn on Store or use existing graph APIs to build GraphView for --db mode. Do not add egui to rag-mcp.\n";
j.push(#{ prompt: p, label: "egui-store-export", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: CLI main.rs: clap --snapshot PATH | --db PATH, --seed LABEL_OR_ID, --depth 1, --max-nodes 100. Open window with eframe. stderr logging only.\n";
j.push(#{ prompt: p, label: "egui-cli", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Add tools or script: export_graph_snapshot — if missing, add MCP tool under full surface OR a small `cargo run -p rag-mcp-ui -- export` that writes graph.json from --db for Mode C. Prefer CLI subcommand export on ui crate using Store.\n";
j.push(#{ prompt: p, label: "egui-export", capability_mode: "all", output_schema: done_schema });

let _r2 = parallel(j);
log("Core done");

// Phase 3: UI (6)
phase("UI");
j = [];

p = common;
p += "TASK: app.rs GraphApp implements eframe::App: hold UiGraph, positions, selected id, pan/zoom, seed input. update() draws panels.\n";
j.push(#{ prompt: p, label: "egui-app", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: ui/canvas.rs — draw nodes as circles + labels, edges as lines; pan drag background, zoom scroll; click select; colors by kind (document/tag/stub).\n";
j.push(#{ prompt: p, label: "egui-canvas", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: ui/detail.rs — side panel for selected node: id, kind, label, document_id, uri, incident edges list.\n";
j.push(#{ prompt: p, label: "egui-detail", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: ui/status.rs — top/status bar: mode snapshot|db, node/edge counts, caps, seed, instructions.\n";
j.push(#{ prompt: p, label: "egui-status", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: ui/empty.rs — empty state when no seed: prompt to enter seed label/id; show graph stats if loaded.\n";
j.push(#{ prompt: p, label: "egui-empty", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Expand neighbors: button expands selected node depth+1 up to max_nodes using loaded graph BFS (snapshot) or Store if --db.\n";
j.push(#{ prompt: p, label: "egui-expand", capability_mode: "all", output_schema: done_schema });

let _r3 = parallel(j);
log("UI done");

// Phase 4: integrate (5)
phase("Integrate");
j = [];

p = common;
p += "TASK: Unit tests for layout RadialLocal and adapter caps (no eframe window). cargo test -p rag-mcp-ui.\n";
j.push(#{ prompt: p, label: "egui-tests", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: README section: build/run rag-mcp-ui; export snapshot from db; coexistence with MCP. docs/EGUI_USAGE.md short.\n";
j.push(#{ prompt: p, label: "egui-docs", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: cargo build -p rag-mcp-ui && cargo test -p rag-mcp-ui && cargo test -p rag-mcp. Fix all. success=true only green.\n";
j.push(#{ prompt: p, label: "egui-fix1", capability_mode: "all", output_schema: fix_schema });

p = common;
p += "TASK: SECONDARY fix compile. success=true only green builds.\n";
j.push(#{ prompt: p, label: "egui-fix2", capability_mode: "all", output_schema: fix_schema });

p = common;
p += "TASK: FINAL fix until cargo build -p rag-mcp-ui and cargo test -p rag-mcp pass.\n";
j.push(#{ prompt: p, label: "egui-fix3", capability_mode: "all", output_schema: fix_schema });

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
    pfix += "ULTIMATE FIX. " + rem + " cargo build -p rag-mcp-ui green + cargo test -p rag-mcp green.\n";
    let last = agent(pfix, #{ label: "egui-fix-ultimate", capability_mode: "all", output_schema: fix_schema });
    if last.success && last.output.success == true {
        ok = true;
        summary = last.output.summary;
        rem = "";
    } else if last.success {
        summary = last.output.summary;
        rem = last.output.remaining_errors;
    }
}

// 3+6+6+5+1 = 21

complete(#{
    ok: ok,
    summary: summary,
    remaining_errors: rem,
    root: root,
    agents: "ws3 core6 ui6 int5+1 ~21",
    run: "cargo run -p rag-mcp-ui -- --db PATH --seed LABEL",
});
