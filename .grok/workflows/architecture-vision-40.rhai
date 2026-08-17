let meta = #{
    name: "architecture-vision-40",
    description: "Deep architectural + product vision for rag-mcp (~40 agents)",
    phases: [
        #{ title: "Product-ideas", detail: "12 specialists on product thesis and UX" },
        #{ title: "Architecture", detail: "10 specialists on system design" },
        #{ title: "Critics", detail: "12 adversarial critics" },
        #{ title: "Synthesize", detail: "6 writers + editors produce master doc" },
    ],
};

let expert_schema = #{
    "type": "object",
    "required": ["role", "thesis", "proposals", "non_goals", "risks"],
    "properties": #{
        "role": #{ "type": "string" },
        "thesis": #{ "type": "string" },
        "proposals": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 20 },
        "non_goals": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 12 },
        "risks": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 12 },
    },
};

let critic_schema = #{
    "type": "object",
    "required": ["role", "fatal", "must_change", "keep", "verdict"],
    "properties": #{
        "role": #{ "type": "string" },
        "fatal": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 12 },
        "must_change": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 15 },
        "keep": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 12 },
        "verdict": #{ "type": "string" },
    },
};

let synth_schema = #{
    "type": "object",
    "required": ["ok", "summary"],
    "properties": #{
        "ok": #{ "type": "boolean" },
        "summary": #{ "type": "string" },
    },
};

let root = if args == () { () } else { args.root };
if root == () {
    root = "/Users/themoretheless/Documents/Sources/rag";
}

let common = "";
common += "Project root: " + root + "\n";
common += "Product: rag-mcp — Rust + DuckDB local MCP server for RAG + Obsidian-like graph + Karpathy wiki + MemPalace-style memory + multi-LLM + maintenance.\n";
common += "Current reality: ~73 MCP tools in one large server.rs; docs: SPEC, ARCHITECTURE_NOTES, FEATURES, ORGANIZE, MEMPALACE_PARITY, GRAPH_DESIGN, EGUI, STORAGE_ADAPTERS, LOCAL_LLM_MAINTENANCE, LLM_PROVIDERS, ROADMAP.\n";
common += "READ key docs with read_file before answering. Prefer evidence from repo over invention.\n";
common += "Be rigorous, opinionated, no sycophancy. English structured fields; Russian ok in long thesis prose if natural.\n";
common += "Goal of this run: architectural + IDEA-level rethink — what should the product BE, not feature laundry list alone.\n";

// ========== 12 product / idea specialists ==========
phase("Product-ideas");

let roles_p = [
    "product-visionary: one-sentence product definition, who is the user, 3 jobs-to-be-done, anti-personas",
    "karpathy-theorist: compile-vs-retrieve as worldview; when RAG is wrong; wiki as binary",
    "mempalace-theorist: verbatim memory palace; what to steal and reject; agent session ritual",
    "obsidian-power-user: graph, backlinks, vault as home; what breaks trust with note-takers",
    "agent-ergonomics: how Claude Desktop / coding agents should use tools; tool explosion vs workflows",
    "personal-knowledge: second brain for 1 human; scale 100-10k notes; daily habits",
    "team-knowledge: what changes if multi-user (and should we refuse for 1 year)",
    "privacy-local-first: air-gap, keys, path allowlist, what never leaves machine",
    "competitive-position: vs Mem0, MemPalace, cognee, NotebookLM, plain Obsidian+Claude",
    "naming-identity: is 'rag-mcp' the right name; product narrative and metaphor",
    "north-star-metrics: how we know it works (recall, maintenance debt, time-to-answer)",
    "failure-modes-product: abandoned wiki, silent wrong answers, tool paralysis",
];

let jobs = [];
let i = 0;
while i < roles_p.len() {
    let p = common;
    p += "SPECIALIST PRODUCT #" + (i + 1).to_string() + ": " + roles_p[i] + "\n";
    p += "Read " + root + "/docs/ARCHITECTURE_NOTES.md and " + root + "/SPEC.md (goal sections). Optionally FEATURES.md executive summary.\n";
    p += "thesis = your core claim in 2-5 sentences. proposals = concrete product decisions. non_goals = what we refuse. risks = product risks.\n";
    p += "role = short id.\n";
    jobs.push(#{ prompt: p, label: "prod-" + (i + 1).to_string(), capability_mode: "all", output_schema: expert_schema });
    i += 1;
}
let prod = parallel(jobs);
log("Product specialists done");

let pblob = "PRODUCT SPECIALISTS:\n";
for r in prod {
    if r != () && r.success {
        pblob += "ROLE=" + r.output.role + " THESIS=" + r.output.thesis + "\nPROP=" + json_encode(r.output.proposals) + "\nNON=" + json_encode(r.output.non_goals) + "\nRISK=" + json_encode(r.output.risks) + "\n---\n";
    }
}
write_scratch_file("vision_product.txt", pblob);

// ========== 10 architecture specialists ==========
phase("Architecture");

let roles_a = [
    "layer-architect: L0-L4 model; what belongs in which layer; forbidden leaks",
    "storage-architect: Storage trait, DuckDB default, markdown vault, when to split vector store",
    "mcp-surface-architect: 73 tools is too many?; domains, grouping, resources/prompts, workflows",
    "crate-modularization: how to split server.rs monorepo; lib boundaries; ui separate",
    "consistency-architect: transactions, ops_log, rebuild graph, dual write md+sql",
    "retrieval-architect: hybrid RRF, packs, when ANN, query planner",
    "graph-architect: PKB vs dependency; GRAPH_DESIGN alignment; egui optional",
    "llm-orchestration: multi-provider; who calls LLM (server vs client); maintain loop",
    "security-architect: allowlist, secrets, tool annotations, read-only mode",
    "evolution-architect: versioning schema, migrations, 1-year roadmap shape, kill switches",
];

jobs = [];
i = 0;
while i < roles_a.len() {
    let p = common;
    p += "SPECIALIST ARCH #" + (i + 1).to_string() + ": " + roles_a[i] + "\n";
    p += "Read ARCHITECTURE_NOTES, STORAGE_ADAPTERS, GRAPH_DESIGN summary if needed, ROADMAP.\n";
    p += "Ground in CURRENT code reality (~73 tools, DuckDB, graph, wiki, maintain modules).\n";
    p += "thesis + proposals + non_goals + risks. Be concrete (modules, traits, invariants).\n";
    // inject product context
    if pblob.len() > 8000 {
        p += "Product context excerpt:\n" + pblob.sub_string(0, 8000) + "\n";
    } else {
        p += "Product context:\n" + pblob + "\n";
    }
    jobs.push(#{ prompt: p, label: "arch-" + (i + 1).to_string(), capability_mode: "all", output_schema: expert_schema });
    i += 1;
}
let arch = parallel(jobs);
log("Architecture specialists done");

let ablob = "ARCH SPECIALISTS:\n";
for r in arch {
    if r != () && r.success {
        ablob += "ROLE=" + r.output.role + " THESIS=" + r.output.thesis + "\nPROP=" + json_encode(r.output.proposals) + "\nNON=" + json_encode(r.output.non_goals) + "\nRISK=" + json_encode(r.output.risks) + "\n---\n";
    }
}
write_scratch_file("vision_arch.txt", ablob);

// ========== 12 critics ==========
phase("Critics");

let roles_c = [
    "complexity-skeptic: we built a second MemPalace+Obsidian; cut 50%",
    "karpathy-purist: server should be dumb store; LLM client does compile",
    "mempalace-purist: verbatim only; wiki compile is a bug",
    "mcp-spec-lawyer: tool quality, descriptions, destructive annotations, stdout purity",
    "rust-scale-critic: one Connection Mutex, server.rs 3k LOC, compile times",
    "ux-agent-critic: agent cannot learn 73 tools; propose 8 workflows",
    "data-integrity-critic: silent data loss, rebuild bugs, archive semantics",
    "security-critic: path traversal, prompt injection via docs, key leakage",
    "market-realist: why would anyone pick this over existing tools",
    "scope-creep-historian: docs promise P0-P2 forever; force ranking",
    "simplicity-maximalist: minimal viable product that ships value this week",
    "adversarial-architect: attack the five-layer model; propose alternative topology",
];

jobs = [];
i = 0;
while i < roles_c.len() {
    let p = common;
    p += "CRITIC #" + (i + 1).to_string() + ": " + roles_c[i] + "\n";
    p += "Adversarially review product+architecture specialist outputs. Prefer killing bad ideas.\n";
    p += "fatal / must_change / keep / verdict (accept|revise|redesign).\n";
    let blob = pblob;
    if blob.len() > 7000 { blob = blob.sub_string(0, 7000); }
    p += "PRODUCT:\n" + blob + "\n";
    blob = ablob;
    if blob.len() > 7000 { blob = blob.sub_string(0, 7000); }
    p += "ARCH:\n" + blob + "\n";
    jobs.push(#{ prompt: p, label: "crit-" + (i + 1).to_string(), capability_mode: "all", output_schema: critic_schema });
    i += 1;
}
let crits = parallel(jobs);
log("Critics done");

let cblob = "CRITICS:\n";
for r in crits {
    if r != () && r.success {
        cblob += "ROLE=" + r.output.role + " FATAL=" + json_encode(r.output.fatal) + " FIX=" + json_encode(r.output.must_change) + " KEEP=" + json_encode(r.output.keep) + " V=" + r.output.verdict + "\n---\n";
    }
}
write_scratch_file("vision_critics.txt", cblob);

// ========== Synthesize 6 agents ==========
phase("Synthesize");

let s1 = common;
s1 += "WRITE " + root + "/docs/ARCHITECTURE_VISION.md part 1: Executive vision, product definition, users, jobs, metaphors, competitive position, non-goals.\n";
s1 += "Synthesize specialists + address critics. Opinionated final choices.\n";
s1 += "Sources product:\n";
if pblob.len() > 10000 { s1 += pblob.sub_string(0, 10000); } else { s1 += pblob; }
s1 += "\nCritics:\n";
if cblob.len() > 6000 { s1 += cblob.sub_string(0, 6000); } else { s1 += cblob; }

let s2 = common;
s2 += "APPEND or WRITE architecture core into " + root + "/docs/ARCHITECTURE_VISION.md: layers, invariants, storage, MCP surface strategy (tool domains / workflows), module map, consistency model.\n";
s2 += "Arch sources:\n";
if ablob.len() > 10000 { s2 += ablob.sub_string(0, 10000); } else { s2 += ablob; }
s2 += "\nCritics:\n";
if cblob.len() > 5000 { s2 += cblob.sub_string(0, 5000); } else { s2 += cblob; }

let s3 = common;
s3 += "APPEND to " + root + "/docs/ARCHITECTURE_VISION.md: decision log (decided/rejected/open), phased evolution (now / next / later), relation to existing docs (which supersede which).\n";
s3 += "Read existing ARCHITECTURE_VISION.md if present and finish it.\n";

let s4 = common;
s4 += "WRITE " + root + "/docs/PRODUCT_PRINCIPLES.md short (1-2 pages): 10 principles ranked, each with one example and one anti-example from our codebase/docs.\n";

let s5 = common;
s5 += "WRITE " + root + "/docs/SYSTEM_MAP.md: mermaid diagrams — (1) layer stack, (2) request paths ingest/search/maintain, (3) process boundaries MCP vs UI vs vault. Grounded in vision.\n";

let s6 = common;
s6 += "META-EDITOR: ensure ARCHITECTURE_VISION.md, PRODUCT_PRINCIPLES.md, SYSTEM_MAP.md exist and cross-link. Update ROADMAP.md top with pointer to ARCHITECTURE_VISION as north star. Fix contradictions. ok=true if coherent.\n";

let writers = parallel([
    #{ prompt: s1, label: "syn-vision-1", capability_mode: "all", output_schema: synth_schema },
    #{ prompt: s2, label: "syn-vision-2", capability_mode: "all", output_schema: synth_schema },
    #{ prompt: s3, label: "syn-vision-3", capability_mode: "all", output_schema: synth_schema },
    #{ prompt: s4, label: "syn-principles", capability_mode: "all", output_schema: synth_schema },
    #{ prompt: s5, label: "syn-sysmap", capability_mode: "all", output_schema: synth_schema },
    #{ prompt: s6, label: "syn-meta", capability_mode: "all", output_schema: synth_schema },
]);

let ok = false;
let summary = "incomplete";
if writers[5] != () && writers[5].success {
    ok = writers[5].output.ok;
    summary = writers[5].output.summary;
} else if writers[0] != () && writers[0].success {
    summary = writers[0].output.summary;
    ok = writers[0].output.ok;
}

// 12+10+12+6 = 40

complete(#{
    ok: ok,
    summary: summary,
    root: root,
    agents: "12 product + 10 arch + 12 critics + 6 synth = 40",
    artifacts: "docs/ARCHITECTURE_VISION.md, PRODUCT_PRINCIPLES.md, SYSTEM_MAP.md",
});
