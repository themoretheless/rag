let meta = #{
    name: "research-rag-features",
    description: "Karpathy + MemPalace + ~100 RAG/MCP repos feature gap analysis",
    phases: [
        #{ title: "Canon", detail: "Karpathy LLM Wiki + MemPalace deep dive" },
        #{ title: "Repos-batch-A", detail: "repos 1-25 feature extract" },
        #{ title: "Repos-batch-B", detail: "repos 26-50" },
        #{ title: "Repos-batch-C", detail: "repos 51-75" },
        #{ title: "Repos-batch-D", detail: "repos 76-100" },
        #{ title: "Synthesize", detail: "gap vs SPEC + FEATURES roadmap" },
    ],
};

let feat_schema = #{
    "type": "object",
    "required": ["source", "features", "mcp_tools", "notes"],
    "properties": #{
        "source": #{ "type": "string" },
        "features": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 40 },
        "mcp_tools": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 40 },
        "notes": #{ "type": "string" },
    },
};

let batch_schema = #{
    "type": "object",
    "required": ["repos_covered", "feature_frequency", "must_have", "nice_to_have", "notes"],
    "properties": #{
        "repos_covered": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 30 },
        "feature_frequency": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 50 },
        "must_have": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 25 },
        "nice_to_have": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 25 },
        "notes": #{ "type": "string" },
    },
};

let synth_schema = #{
    "type": "object",
    "required": ["ok", "path", "summary"],
    "properties": #{
        "ok": #{ "type": "boolean" },
        "path": #{ "type": "string" },
        "summary": #{ "type": "string" },
    },
};

let root = if args == () { () } else { args.root };
if root == () {
    root = "/Users/themoretheless/Documents/Sources/rag";
}

let common = "";
common += "Project root: " + root + "\n";
common += "Our product: rag-mcp (Rust + DuckDB) — read " + root + "/SPEC.md for current planned features.\n";
common += "Use web_search, open_page/web_fetch, grep as needed. Prefer primary sources (README, docs).\n";
common += "Return concrete feature names, not marketing fluff. English in structured fields.\n";

// ===================== Phase 1: Canon (4 agents) =====================
phase("Canon");

let p_karp = common;
p_karp += "DEEP DIVE: Andrej Karpathy LLM Wiki pattern.\n";
p_karp += "Primary: https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f (fetch raw content).\n";
p_karp += "Extract: architecture layers, operations (ingest/query/lint), index.md/log.md, schema file, compounding answers, optional search (qmd), graph/Obsidian role, anti-RAG stance and when hybrid is still needed.\n";
p_karp += "List MCP-relevant capabilities a server should expose to support this pattern.\n";
p_karp += "source=karpathy-llm-wiki\n";

let p_mem = common;
p_mem += "DEEP DIVE: Milla Jovovich MemPalace (memory palace AI memory).\n";
p_mem += "Repos: https://github.com/MemPalace/mempalace (or milla-jovovich/mempalace). Fetch README + docs on palace structure, wings/rooms/drawers, MCP tools, knowledge graph, hybrid search, hooks, wake-up, verbatim storage, backends.\n";
p_mem += "List every MCP tool name if documented and architectural features.\n";
p_mem += "source=mempalace-jovovich\n";

let p_cog = common;
p_cog += "DEEP DIVE popular graph+vector stacks used as MCP: cognee (topoteretes/cognee), getzep/graphiti MCP, modelcontextprotocol memory server.\n";
p_cog += "Feature lists and tool surface comparison.\n";
p_cog += "source=graph-vector-canon\n";

let p_qmd = common;
p_qmd += "DEEP DIVE local markdown hybrid search MCP: tobi/qmd (mentioned by Karpathy), enquire-mcp Obsidian, knowledge-rag, mindbase frankchu91/mindbase, gowtham0992/link.\n";
p_qmd += "What tools and retrieval modes do wiki-style systems expose?\n";
p_qmd += "source=wiki-search-mcp\n";

let canon = parallel([
    #{ prompt: p_karp, label: "canon-karpathy", capability_mode: "all", output_schema: feat_schema },
    #{ prompt: p_mem, label: "canon-mempalace", capability_mode: "all", output_schema: feat_schema },
    #{ prompt: p_cog, label: "canon-graph-vector", capability_mode: "all", output_schema: feat_schema },
    #{ prompt: p_qmd, label: "canon-wiki-search", capability_mode: "all", output_schema: feat_schema },
]);
log("Canon done");

// Helper to build a batch of 5 agents covering 5 repos each = 25 repos per phase
fn make_batch_prompt(common, batch_name, repo_list) {
    let p = common;
    p += "REPO FEATURE SURVEY batch=" + batch_name + "\n";
    p += "For EACH repo below: web_search or fetch GitHub README. Extract features + MCP tools if any.\n";
    p += "If a repo is dead/404, note and skip. Cover as many as possible.\n";
    p += "Repos list:\n" + repo_list + "\n";
    p += "Aggregate: feature_frequency as 'feature_name: N' sorted by frequency; must_have for a competitive local RAG MCP in Rust+DuckDB; nice_to_have.\n";
    p += "repos_covered = actual URLs or owner/name you inspected.\n";
    return p;
}

// ===================== Phase 2: Batch A 1-25 (5 agents) =====================
phase("Repos-batch-A");

let a1 = make_batch_prompt(common, "A1",
    "1 lyonzin/knowledge-rag\n2 ttommyth/rag-memory-mcp\n3 codingthefuturewithai/rag-memory\n4 199-biotechnologies/engram\n5 udjin-labs/mnemostack\n");
let a2 = make_batch_prompt(common, "A2",
    "6 mtrnix/metronix-memory\n7 getzep/graphiti\n8 topoteretes/cognee\n9 modelcontextprotocol/servers memory\n10 Aparnap2/RAG-C\n");
let a3 = make_batch_prompt(common, "A3",
    "11 frankchu91/mindbase\n12 gowtham0992/link\n13 ZeroDot1/LLMWikiNG\n14 sturlese/hippocampus\n15 kytmanov/synto\n");
let a4 = make_batch_prompt(common, "A4",
    "16 tobi/qmd\n17 oomkapwn/enquire-mcp\n18 OrangeproAI/orangepro-mcp\n19 betsallel-williamson/mdcp OR betsalel-williamson/mdcp\n20 theafh/ai-modules\n");
let a5 = make_batch_prompt(common, "A5",
    "21 upstash/context7\n22 shariqriazz/mcp-ragdocs\n23 Lvigentini/LTC-RAG-MCP\n24 crawl4ai RAG MCP\n25 ragie MCP server\n");

let batch_a = parallel([
    #{ prompt: a1, label: "repos-A1", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: a2, label: "repos-A2", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: a3, label: "repos-A3", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: a4, label: "repos-A4", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: a5, label: "repos-A5", capability_mode: "all", output_schema: batch_schema },
]);
log("Batch A done");

// ===================== Phase 3: Batch B 26-50 (5 agents) =====================
phase("Repos-batch-B");

let b1 = make_batch_prompt(common, "B1",
    "26 mem0ai/mem0 MCP\n27 zepai/zep MCP\n28 hindsight memory MCP\n29 supermemory MCP\n30 mastra AI memory duckdb\n");
let b2 = make_batch_prompt(common, "B2",
    "31 lancedb MCP RAG\n32 chroma MCP server\n33 weaviate MCP\n34 milvus MCP server\n35 qdrant MCP server\n");
let b3 = make_batch_prompt(common, "B3",
    "36 pinecone MCP\n37 neon MCP RAG\n38 supabase pgvector MCP\n39 motherduck duckdb MCP\n40 duckdb MCP server github\n");
let b4 = make_batch_prompt(common, "B4",
    "41 obsidian MCP server advanced\n42 logseq MCP\n43 notion MCP RAG\n44 outline wiki MCP\n45 dendron knowledge MCP\n");
let b5 = make_batch_prompt(common, "B5",
    "46 llama-index MCP\n47 langchain MCP RAG\n48 haystack RAG MCP\n49 dspy RAG MCP\n50 vercel ai sdk RAG MCP\n");

let batch_b = parallel([
    #{ prompt: b1, label: "repos-B1", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: b2, label: "repos-B2", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: b3, label: "repos-B3", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: b4, label: "repos-B4", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: b5, label: "repos-B5", capability_mode: "all", output_schema: batch_schema },
]);
log("Batch B done");

// ===================== Phase 4: Batch C 51-75 (5 agents) =====================
phase("Repos-batch-C");

let c1 = make_batch_prompt(common, "C1",
    "51 anythingllm MCP\n52 privategpt MCP\n53 quivr RAG\n54 dripdrop RAG MCP\n55 gpt-researcher MCP\n");
let c2 = make_batch_prompt(common, "C2",
    "56 firecrawl MCP RAG\n57 jina AI reader MCP\n58 exa search MCP\n59 tavily MCP\n60 perplexity MCP\n");
let c3 = make_batch_prompt(common, "C3",
    "61 github code search MCP RAG\n62 sourcegraph MCP\n63 copilot knowledge base\n64 cursor docs indexing RAG\n65 continue.dev context provider\n");
let c4 = make_batch_prompt(common, "C4",
    "66 graph-rag microsoft\n67 neo4j graphrag MCP\n68 falkordb graph RAG\n69 kuzu graph RAG\n70 age apache graph\n");
let c5 = make_batch_prompt(common, "C5",
    "71 hybrid BM25 vector RRF RAG open source\n72 colbert late interaction RAG\n73 hyde RAG technique MCP\n74 multi-query RAG\n75 parent document retriever\n");

let batch_c = parallel([
    #{ prompt: c1, label: "repos-C1", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: c2, label: "repos-C2", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: c3, label: "repos-C3", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: c4, label: "repos-C4", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: c5, label: "repos-C5", capability_mode: "all", output_schema: batch_schema },
]);
log("Batch C done");

// ===================== Phase 5: Batch D 76-100 (5 agents) =====================
phase("Repos-batch-D");

let d1 = make_batch_prompt(common, "D1",
    "76 pdf ingest RAG unstructured\n77 markitdown microsoft\n78 docling IBM parse\n79 llama-parse\n80 marker pdf\n");
let d2 = make_batch_prompt(common, "D2",
    "81 ollama embeddings local RAG MCP\n82 onnx local embeddings RAG\n83 fastembed\n84 nomic embed\n85 voyage AI embeddings MCP\n");
let d3 = make_batch_prompt(common, "D3",
    "86 agent memory diary sessions MCP\n87 conversation transcript mine MCP\n88 auto-save hooks context compression\n89 wake-up session bootstrap memory\n90 temporal knowledge graph validity windows\n");
let d4 = make_batch_prompt(common, "D4",
    "91 contradiction detection knowledge base\n92 orphan pages lint wiki\n93 provenance citations RAG\n94 ACL multi-tenant RAG MCP\n95 cost tracking retrieval MCP\n");
let d5 = make_batch_prompt(common, "D5",
    "96 duckdb vss HNSW RAG\n97 full text search FTS duckdb\n98 export import vault markdown\n99 resources prompts MCP RAG\n100 evaluation harness ragas MCP\n");

let batch_d = parallel([
    #{ prompt: d1, label: "repos-D1", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: d2, label: "repos-D2", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: d3, label: "repos-D3", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: d4, label: "repos-D4", capability_mode: "all", output_schema: batch_schema },
    #{ prompt: d5, label: "repos-D5", capability_mode: "all", output_schema: batch_schema },
]);
log("Batch D done");

// ===================== Phase 6: Synthesize (6 agents parallel theme + 1 final writer) =====================
phase("Synthesize");

// Collect text blobs from prior results for synthesizers
let blob = "CANON RESULTS:\n";
let i = 0;
for r in canon {
    if r != () && r.success {
        blob += "SOURCE=" + r.output.source + " FEATURES=" + json_encode(r.output.features) + " TOOLS=" + json_encode(r.output.mcp_tools) + " NOTES=" + r.output.notes + "\n";
    }
    i += 1;
}
blob += "\nBATCH RESULTS:\n";
let all_batches = [];
for r in batch_a { all_batches.push(r); }
for r in batch_b { all_batches.push(r); }
for r in batch_c { all_batches.push(r); }
for r in batch_d { all_batches.push(r); }
for r in all_batches {
    if r != () && r.success {
        blob += "repos=" + json_encode(r.output.repos_covered) + " freq=" + json_encode(r.output.feature_frequency) + " must=" + json_encode(r.output.must_have) + " nice=" + json_encode(r.output.nice_to_have) + "\n";
    }
}

// Write intermediate research dump for agents to read
let dump_path = "research_raw.json.txt";
write_scratch_file(dump_path, blob);

let s_karp = common;
s_karp += "THEME SYNTHESIS: Karpathy-aligned features for rag-mcp. Intermediate research dump is in workflow scratch research_raw.json.txt — also use this blob excerpt:\n";
s_karp += blob.sub_string(0, 12000);
s_karp += "\nList P0/P1/P2 features that implement compile-don't-just-retrieve: wiki pages, index, log, lint, file answers back, schema, immutable raw vs compiled layer. Map each to MCP tools.\n";
s_karp += "source=synth-karpathy\n";

let s_mem = common;
s_mem += "THEME SYNTHESIS: MemPalace/Jovovich-aligned features for rag-mcp. Use research findings:\n";
s_mem += blob.sub_string(0, 12000);
s_mem += "\nMap wings/rooms/drawers, verbatim memory, wake-up, hooks, temporal KG, multi-agent diaries to our DuckDB design. P0/P1/P2 + tools.\n";
s_mem += "source=synth-mempalace\n";

let s_ret = common;
s_ret += "THEME SYNTHESIS: Retrieval quality features across ~100 repos (hybrid BM25+vector+RRF, rerank, HyDE, multi-query, FTS, HNSW, filters, citations).\n";
s_ret += blob.sub_string(0, 12000);
s_ret += "\nWhat should rag-mcp implement given DuckDB? P0/P1/P2.\n";
s_ret += "source=synth-retrieval\n";

let s_graph = common;
s_graph += "THEME SYNTHESIS: Graph/object features (Obsidian graph, entity graph, temporal edges, backlinks, stubs, GraphRAG). Compare to our SPEC graph section. Gaps?\n";
s_graph += blob.sub_string(0, 8000);
s_graph += "\nsource=synth-graph\n";

let s_ops = common;
s_ops += "THEME SYNTHESIS: Ops/product features: multi-format ingest, file watch, export markdown vault, MCP resources+prompts, eval harness, multi-tenant, cost tracking, local embeddings.\n";
s_ops += blob.sub_string(0, 8000);
s_ops += "\nsource=synth-ops\n";

let s_diff = common;
s_diff += "THEME: Competitive differentiation. What can Rust+DuckDB single-binary do better than Python stacks? What should we deliberately NOT build?\n";
s_diff += "Read SPEC at " + root + "/SPEC.md. source=synth-diff\n";

let themes = parallel([
    #{ prompt: s_karp, label: "synth-karpathy", capability_mode: "all", output_schema: feat_schema },
    #{ prompt: s_mem, label: "synth-mempalace", capability_mode: "all", output_schema: feat_schema },
    #{ prompt: s_ret, label: "synth-retrieval", capability_mode: "all", output_schema: feat_schema },
    #{ prompt: s_graph, label: "synth-graph", capability_mode: "all", output_schema: feat_schema },
    #{ prompt: s_ops, label: "synth-ops", capability_mode: "all", output_schema: feat_schema },
    #{ prompt: s_diff, label: "synth-diff", capability_mode: "all", output_schema: feat_schema },
]);

// Final writer agent merges into FEATURES.md + updates ROADMAP section in docs
let final_p = common;
final_p += "FINAL DELIVERABLE. Write TWO files under " + root + ":\n";
final_p += "1) FEATURES.md — executive research report in Russian+English mixed OK, structured:\n";
final_p += "   - Executive summary\n";
final_p += "   - Karpathy LLM Wiki method → implications for rag-mcp\n";
final_p += "   - MemPalace (Jovovich) method → implications\n";
final_p += "   - Survey of ~100 related repos (methodology, coverage count, top feature frequency table)\n";
final_p += "   - Gap analysis vs current SPEC.md (Have / Partial / Missing)\n";
final_p += "   - Prioritized roadmap P0 (next sprint), P1, P2, Out-of-scope\n";
final_p += "   - Proposed new MCP tools list\n";
final_p += "   - Proposed DuckDB schema additions\n";
final_p += "2) docs/ROADMAP.md — short bullet roadmap from P0-P2 only.\n";
final_p += "Also append a short '## Research-derived backlog' section to SPEC.md without deleting existing content.\n";
final_p += "Use all theme syntheses:\n";
for r in themes {
    if r != () && r.success {
        final_p += "SRC=" + r.output.source + " FEATS=" + json_encode(r.output.features) + " TOOLS=" + json_encode(r.output.mcp_tools) + " NOTES=" + r.output.notes + "\n";
    }
}
final_p += "\nAlso batch must_haves from earlier if needed. Be opinionated and specific to Rust+DuckDB+MCP+Obsidian graph.\n";
final_p += "Return ok=true, path to FEATURES.md, summary of top 10 must-build features.\n";

let final = agent(final_p, #{ label: "synth-final-writer", capability_mode: "all", output_schema: synth_schema });

let ok = false;
let path = "";
let summary = "failed";
if final.success {
    ok = final.output.ok;
    path = final.output.path;
    summary = final.output.summary;
}

// Agent accounting: 4 + 5*4 + 6 + 1 = 4+20+6+1 = 31; add more parallel validators to approach 40
let v1 = common;
v1 += "Adversarial review of FEATURES.md at " + root + "/FEATURES.md if exists. List any missing must-have features from hybrid RAG / memory MCP world. Append '## Adversarial addenda' section if file exists.\n";
let v2 = common;
v2 += "Cross-check FEATURES.md P0 list against SPEC.md. Ensure graph/Obsidian items not dropped. Fix inconsistencies in FEATURES.md.\n";
let v3 = common;
v3 += "Produce a one-page MCP tool matrix markdown table in " + root + "/docs/MCP_TOOL_MATRIX.md: Tool | Purpose | Karpathy | MemPalace | Priority.\n";
let v4 = common;
v4 += "Count how many unique repos were effectively surveyed across research; write " + root + "/docs/REPOS_SURVEYED.md with the list and coverage notes.\n";

let validators = parallel([
    #{ prompt: v1, label: "validate-adversarial", capability_mode: "all", output_schema: synth_schema },
    #{ prompt: v2, label: "validate-spec-align", capability_mode: "all", output_schema: synth_schema },
    #{ prompt: v3, label: "validate-tool-matrix", capability_mode: "all", output_schema: synth_schema },
    #{ prompt: v4, label: "validate-repo-list", capability_mode: "all", output_schema: synth_schema },
    #{ prompt: common + "Write " + root + "/docs/ARCHITECTURE_NOTES.md: recommended target architecture blending Karpathy wiki compile + MemPalace verbatim/palace + our vector+graph DuckDB. Diagram in mermaid.\n", label: "validate-arch", capability_mode: "all", output_schema: synth_schema },
]);

// ~36 agents; boost with 4 more specialty digs if budget remains
let extra = parallel([
    #{ prompt: common + "Fetch and summarize DuckDB FTS + VSS capabilities for hybrid RAG in 2025-2026. Write bullets to append to FEATURES.md section 'DuckDB-native retrieval'.\n", label: "extra-duckdb", capability_mode: "all", output_schema: feat_schema },
    #{ prompt: common + "Survey Rust MCP servers (rmcp examples, grimoire, etc.) for tool design patterns useful to rag-mcp.\n", label: "extra-rust-mcp", capability_mode: "all", output_schema: feat_schema },
    #{ prompt: common + "Survey agent session memory: auto-save on compaction, wake-up prompts, diary per agent. Map to MCP tools for rag-mcp.\n", label: "extra-session-mem", capability_mode: "all", output_schema: feat_schema },
    #{ prompt: common + "FINAL polish: ensure FEATURES.md, docs/ROADMAP.md, docs/MCP_TOOL_MATRIX.md, docs/ARCHITECTURE_NOTES.md exist and are coherent. Fix gaps. ok=true if all present.\n", label: "extra-polish", capability_mode: "all", output_schema: synth_schema },
]);

if extra[3] != () && extra[3].success && extra[3].output.ok == true {
    ok = true;
    if extra[3].output.path != () && extra[3].output.path != "" {
        path = extra[3].output.path;
    }
    summary = extra[3].output.summary;
}

complete(#{
    ok: ok,
    path: path,
    summary: summary,
    root: root,
    agent_plan: "4canon+20repos+6theme+1final+5validate+4extra=~40",
});
