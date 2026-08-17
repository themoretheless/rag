let meta = #{
    name: "design-graph-egui",
    description: "20 specialists + 20 critics: graph logic and egui dependency view",
    phases: [
        #{ title: "Graph-specialists", detail: "10 experts design graph construction logic" },
        #{ title: "Graph-critics", detail: "10 adversarial reviewers of graph design" },
        #{ title: "Egui-specialists", detail: "10 experts design egui dependency UI" },
        #{ title: "Egui-critics", detail: "10 adversarial reviewers of egui design" },
        #{ title: "Synthesize", detail: "merge into GRAPH_DESIGN + EGUI_VIEW docs" },
    ],
};

let expert_schema = #{
    "type": "object",
    "required": ["role", "proposals", "risks", "apis", "notes"],
    "properties": #{
        "role": #{ "type": "string" },
        "proposals": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 25 },
        "risks": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 15 },
        "apis": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 20 },
        "notes": #{ "type": "string" },
    },
};

let critic_schema = #{
    "type": "object",
    "required": ["role", "fatal_flaws", "must_fix", "keep", "verdict"],
    "properties": #{
        "role": #{ "type": "string" },
        "fatal_flaws": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 15 },
        "must_fix": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 20 },
        "keep": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 15 },
        "verdict": #{ "type": "string" },
    },
};

let synth_schema = #{
    "type": "object",
    "required": ["ok", "paths", "summary"],
    "properties": #{
        "ok": #{ "type": "boolean" },
        "paths": #{ "type": "array", "items": #{ "type": "string" } },
        "summary": #{ "type": "string" },
    },
};

let root = if args == () { () } else { args.root };
if root == () {
    root = "/Users/themoretheless/Documents/Sources/rag";
}

let common = "";
common += "Project: rag-mcp at " + root + "\n";
common += "Read: SPEC.md object graph section, docs/ARCHITECTURE_NOTES.md, docs/STORAGE_ADAPTERS.md markdown vault, FEATURES.md graph gaps if present.\n";
common += "Context: Rust MCP server, DuckDB default, planned Markdown vault + multi-DB adapters, Obsidian-like wikilinks/tags/stubs, Karpathy wiki compile, MemPalace wings.\n";
common += "Be concrete: types, algorithms, failure modes, API shapes. English for structured fields. No sycophancy.\n";
common += "Use web_search for egui graph crates (egui_graphs, petgraph layout) and dependency-graph UX patterns when useful.\n";

// ========== 10 Graph specialists ==========
phase("Graph-specialists");

let gs = [];

let roles_g = [
    "graph-data-model: typed nodes/edges, layers raw/wiki, stubs, tunnels, multi-edge, weights, provenance chunk offsets",
    "extract-resolve: [[wikilink]] #tag parsing, alias, casefold, unicode, code-fence ignore, stub promote, stable ids on re-ingest",
    "dependency-semantics: what is a dependency vs mention vs related; build/dep graphs vs PKB graphs; directed acyclic subsets",
    "incremental-rebuild: dirty sets, content_hash, partial re-extract, transactional graph update, delete cascades",
    "query-algos: BFS/DFS, k-hop, shortest path, backlinks, community/hub detection, orphan/stub lint, cycle detection",
    "hybrid-graph-rag: graph_expand_search, seed from vector, expand typed edges, diversity, token budget",
    "scale-perf: 10k-100k nodes, indexes, adjacency cache, materialized local graphs, lazy load",
    "markdown-live-graph: vault as SoT, live parse vs cache, mtime invalidation, multi-file consistency",
    "temporal-kg: validity windows, supersede, contradiction edges, timeline queries (MemPalace-like)",
    "mcp-graph-api: tool surface, filters, GraphView JSON schema, errors, caps per storage backend",
];

let i = 0;
while i < roles_g.len() {
    let p = common;
    p += "You are SPECIALIST #" + (i + 1).to_string() + ": " + roles_g[i] + "\n";
    p += "Design your slice thoroughly for rag-mcp. Proposals must be implementable in Rust.\n";
    p += "apis = concrete fn/tool/type names. risks = what breaks at scale or with bad input.\n";
    p += "role = short id of your specialty.\n";
    gs.push(#{ prompt: p, label: "gs-" + (i + 1).to_string(), capability_mode: "all", output_schema: expert_schema });
    i += 1;
}

let graph_specs = parallel(gs);
log("Graph specialists done");

// Pack specialist output for critics
let gblob = "GRAPH SPECIALIST OUTPUTS:\n";
for r in graph_specs {
    if r != () && r.success {
        gblob += "ROLE=" + r.output.role + "\nPROPOSALS=" + json_encode(r.output.proposals) + "\nRISKS=" + json_encode(r.output.risks) + "\nAPIS=" + json_encode(r.output.apis) + "\nNOTES=" + r.output.notes + "\n---\n";
    }
}
write_scratch_file("graph_specialists.txt", gblob);

// ========== 10 Graph critics ==========
phase("Graph-critics");

let gc = [];
let critics_g = [
    "correctness-skeptic: find logic bugs, id instability, race conditions, wrong cascade deletes",
    "obsidian-user: will this feel wrong vs Obsidian graph/backlinks? missing features?",
    "karpathy-purist: over-engineered vs simple wiki compile? what to cut?",
    "security-critic: path traversal, graph bombs, ReDoS in extract, huge depth BFS DoS",
    "perf-critic: O(n^2) cosine of graph ops, full scans, memory blowups",
    "consistency-critic: dual write SQL+md, cache staleness, partial failure mid-rebuild",
    "api-critic: MCP tool explosion, bad ergonomics for agents, missing filters",
    "dependency-theorist: misuse of dependency term; cycles; versioned deps; false edges",
    "i18n-critic: cyrillic labels, CJK, casefold, path encoding",
    "testability-critic: untestable designs; demand property tests and fixtures",
];

i = 0;
while i < critics_g.len() {
    let p = common;
    p += "You are CRITIC #" + (i + 1).to_string() + ": " + critics_g[i] + "\n";
    p += "Adversarially review the specialist proposals below. Prefer killing bad ideas over praise.\n";
    p += "fatal_flaws = show-stoppers; must_fix = required changes; keep = good ideas; verdict = ship / revise / redesign.\n";
    p += "SPECIALIST BLOB (excerpt):\n";
    // sub_string to avoid huge prompts
    if gblob.len() > 14000 {
        p += gblob.sub_string(0, 14000);
    } else {
        p += gblob;
    }
    p += "\nrole = your critic id.\n";
    gc.push(#{ prompt: p, label: "gc-" + (i + 1).to_string(), capability_mode: "all", output_schema: critic_schema });
    i += 1;
}

let graph_crits = parallel(gc);
log("Graph critics done");

let cblob = "GRAPH CRITIC OUTPUTS:\n";
for r in graph_crits {
    if r != () && r.success {
        cblob += "ROLE=" + r.output.role + " FATAL=" + json_encode(r.output.fatal_flaws) + " FIX=" + json_encode(r.output.must_fix) + " KEEP=" + json_encode(r.output.keep) + " VERDICT=" + r.output.verdict + "\n---\n";
    }
}
write_scratch_file("graph_critics.txt", cblob);

// ========== 10 Egui specialists ==========
phase("Egui-specialists");

let es = [];
let roles_e = [
    "egui-arch: where UI lives (binary feature flag, separate crate rag-mcp-ui), IPC to store, headless MCP coexistence",
    "layout-engine: force-directed, hierarchical dagre-like, radial local graph, petgraph + egui_graphs research",
    "interaction: pan zoom select multi-select, drag nodes, open doc on click, context menu link/unlink",
    "filters-ui: kind/rel_type/wing/room/layer toggles, depth slider, search-in-graph, hide stubs/tags",
    "visual-encoding: color by kind/layer, size by degree, edge style by rel_type, stub dashed, tunnel thick",
    "local-vs-global: Obsidian-style local graph vs full vault; focus node; pin neighbors",
    "dependency-view-mode: specialized layout for depends_on/build edges; topo levels; cycle highlight red",
    "perf-ui: virtualize, level-of-detail, max nodes hard cap, progressive reveal",
    "accessibility-export: keyboard nav, tooltips, export PNG/SVG optional, copy GraphView JSON",
    "integration-data: subscribe to store events, live reload markdown vault, status bar doctor/backend",
];

i = 0;
while i < roles_e.len() {
    let p = common;
    p += "You are EGUI SPECIALIST #" + (i + 1).to_string() + ": " + roles_e[i] + "\n";
    p += "Design egui dependency/object graph visualization for rag-mcp. Prefer egui 0.29+ ecosystem crates that exist on crates.io.\n";
    p += "apis = Rust types/widgets/commands. risks = UI freezes, layout thrash, memory.\n";
    p += "role = short id.\n";
    // give graph consensus context
    p += "Graph design context (specialists+critics excerpt):\n";
    let combo = gblob;
    if combo.len() > 6000 {
        combo = combo.sub_string(0, 6000);
    }
    p += combo + "\n";
    if cblob.len() > 4000 {
        p += cblob.sub_string(0, 4000);
    } else {
        p += cblob;
    }
    es.push(#{ prompt: p, label: "es-" + (i + 1).to_string(), capability_mode: "all", output_schema: expert_schema });
    i += 1;
}

let egui_specs = parallel(es);
log("Egui specialists done");

let eblob = "EGUI SPECIALIST OUTPUTS:\n";
for r in egui_specs {
    if r != () && r.success {
        eblob += "ROLE=" + r.output.role + "\nPROPOSALS=" + json_encode(r.output.proposals) + "\nRISKS=" + json_encode(r.output.risks) + "\nAPIS=" + json_encode(r.output.apis) + "\nNOTES=" + r.output.notes + "\n---\n";
    }
}
write_scratch_file("egui_specialists.txt", eblob);

// ========== 10 Egui critics ==========
phase("Egui-critics");

let ec = [];
let critics_e = [
    "ux-critic: clutter, cognitive load, missing empty states, bad defaults",
    "egui-realist: crates maturity, platform issues (macOS), feature flag bloat of MCP binary",
    "perf-ui-critic: 5k nodes freeze, layout every frame, allocations",
    "product-critic: does MCP server need GUI at all? separate app? webview alternative?",
    "a11y-critic: color-only encoding, tiny targets, no keyboard",
    "data-binding-critic: stale graph after ingest, wrong thread, blocking duckdb on UI thread",
    "layout-math-critic: unstable force layout, jumpiness, bad hierarchical for cyclic PKB graphs",
    "security-ui-critic: open file paths, SSRF not apply but path exfil via UI",
    "scope-critic: gold-plating vs MVP; what ships in first PR",
    "integration-critic: conflicts with stdio MCP (GUI+stdio), how user runs both",
];

i = 0;
while i < critics_e.len() {
    let p = common;
    p += "You are EGUI CRITIC #" + (i + 1).to_string() + ": " + critics_e[i] + "\n";
    p += "Adversarially review egui proposals. Kill overdesign. Demand MVP path.\n";
    p += "EGUI SPECIALISTS:\n";
    if eblob.len() > 14000 {
        p += eblob.sub_string(0, 14000);
    } else {
        p += eblob;
    }
    p += "\nrole = critic id. verdict = ship_mvp / revise / drop_gui.\n";
    ec.push(#{ prompt: p, label: "ec-" + (i + 1).to_string(), capability_mode: "all", output_schema: critic_schema });
    i += 1;
}

let egui_crits = parallel(ec);
log("Egui critics done");

let ecblob = "EGUI CRITIC OUTPUTS:\n";
for r in egui_crits {
    if r != () && r.success {
        ecblob += "ROLE=" + r.output.role + " FATAL=" + json_encode(r.output.fatal_flaws) + " FIX=" + json_encode(r.output.must_fix) + " KEEP=" + json_encode(r.output.keep) + " VERDICT=" + r.output.verdict + "\n---\n";
    }
}
write_scratch_file("egui_critics.txt", ecblob);

// ========== Synthesize (2 writers + 1 final critic of docs) ==========
phase("Synthesize");

let w1 = common;
w1 += "WRITE " + root + "/docs/GRAPH_DESIGN.md — definitive design for object/dependency graph logic in rag-mcp.\n";
w1 += "Must include: data model, edge type ontology, extract/resolve algorithm, incremental rebuild, query ops, hybrid expand, scale limits, markdown live graph, tests plan, open questions resolved by critics.\n";
w1 += "Incorporate specialists AND address critic must_fix. Be opinionated; mark rejected ideas.\n";
w1 += "Sources to merge:\n" + gblob.sub_string(0, 8000) + "\n" + cblob.sub_string(0, 8000) + "\n";
w1 += "Return ok=true paths=[GRAPH_DESIGN.md path] summary=top decisions.\n";

let w2 = common;
w2 += "WRITE " + root + "/docs/EGUI_GRAPH_VIEW.md — definitive design for egui dependency/object graph UI.\n";
w2 += "Must include: process architecture (separate binary vs feature), crate choices, layout modes (force/local/dependency-topo), interaction, filters, visual encoding, perf caps, MVP PR plan, non-goals.\n";
w2 += "Address critics especially: stdio MCP coexistence, UI thread, drop_gui risk.\n";
w2 += "Sources:\n" + eblob.sub_string(0, 8000) + "\n" + ecblob.sub_string(0, 8000) + "\n";
w2 += "Return ok=true paths=[EGUI path] summary=MVP recommendation.\n";

let writers = parallel([
    #{ prompt: w1, label: "synth-graph-doc", capability_mode: "all", output_schema: synth_schema },
    #{ prompt: w2, label: "synth-egui-doc", capability_mode: "all", output_schema: synth_schema },
]);

let w3 = common;
w3 += "FINAL EDITOR + META-CRITIC. Read if exist:\n";
w3 += root + "/docs/GRAPH_DESIGN.md and " + root + "/docs/EGUI_GRAPH_VIEW.md\n";
w3 += "Fix contradictions between them. Add cross-links. Write " + root + "/docs/GRAPH_EGUI_DECISIONS.md short decision log (10-20 bullets): decided / rejected / open.\n";
w3 += "Update " + root + "/docs/ROADMAP.md with a short P1/P2 section for graph engine + egui viewer if missing.\n";
w3 += "ok=true if all three docs exist and coherent.\n";

let final = agent(w3, #{ label: "synth-final-editor", capability_mode: "all", output_schema: synth_schema });

let ok = false;
let summary = "incomplete";
if writers[0] != () && writers[0].success {
    if writers[0].output.ok == true {
        ok = true;
    }
    if writers[0].output.summary != () {
        summary = writers[0].output.summary;
    }
}
if writers[1] != () && writers[1].success {
    if writers[1].output.summary != () {
        summary += " | " + writers[1].output.summary;
    }
    if writers[1].output.ok != true {
        ok = false;
    }
}
if final.success {
    if final.output.summary != () {
        summary += " | " + final.output.summary;
    }
    if final.output.ok == true {
        ok = true;
    } else {
        ok = false;
    }
}

// 10+10+10+10+2+1 = 43 agents; within budget 50

complete(#{
    ok: ok,
    summary: summary,
    root: root,
    agents: "10 graph specialists + 10 graph critics + 10 egui specialists + 10 egui critics + 3 synth",
});
