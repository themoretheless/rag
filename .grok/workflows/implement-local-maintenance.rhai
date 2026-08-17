let meta = #{
    name: "implement-local-maintenance",
    description: "Local LLM + analyze/organize/compress/refresh maintenance (~20 agents)",
    phases: [
        #{ title: "Providers", detail: "ollama embed + llm_status + timeouts" },
        #{ title: "Analyze-refresh", detail: "analyze_corpus vacuum reindex refresh" },
        #{ title: "Plan-apply", detail: "plan JSON apply dry_run organize compress" },
        #{ title: "Integrate", detail: "tests README cargo green" },
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
common += "READ: " + root + "/docs/LOCAL_LLM_MAINTENANCE.md and ORGANIZE.md. Extend existing src/llm, wiki, db — do not rewrite from scratch.\n";
common += "Goal: local Ollama-compatible LLM + embeddings; analyze, plan, dry_run apply, compress, refresh wiki/DB. Safety whitelist + ops_log.\n";
common += "Rust + DuckDB + rmcp. cargo test green. stderr only.\n";

// Phase 1: 4 agents
phase("Providers");
let j = [];

let p = common;
p += "TASK: Extend embeddings: support provider ollama OR document that openai + local BASE_URL works; add ollama.rs if cleaner (POST /api/embeddings or /v1/embeddings). Config EmbeddingProviderKind. Unit test with mock server optional skip.\n";
j.push(#{ prompt: p, label: "lm-embed-ollama", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Harden src/llm/chat.rs: timeouts from RAG_LLM_TIMEOUT_SECS, max_tokens; llm_status method ping models or minimal chat. Export from llm/mod.rs.\n";
j.push(#{ prompt: p, label: "lm-chat-harden", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: MCP tool llm_status: returns llm_enabled, base_url, model, reachable bool, embed provider/model/dims. Wire ChatClient on RagServer if missing.\n";
j.push(#{ prompt: p, label: "lm-tool-status", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: Config env RAG_LLM_TIMEOUT_SECS, RAG_LLM_MAX_TOKENS, RAG_MAINT_MAX_DOCS, RAG_MAINT_NEAR_DUP_THRESHOLD. Document defaults in config.rs.\n";
j.push(#{ prompt: p, label: "lm-config", capability_mode: "all", output_schema: done_schema });

let _r1 = parallel(j);
log("Providers done");

// Phase 2: 5 agents
phase("Analyze-refresh");
j = [];

p = common;
p += "TASK: Create src/maintain/mod.rs + analyze.rs: AnalysisReport with counts, orphans, stubs, exact_dups, stale_wiki candidates, embed_mismatch, fts_ready, db_size_bytes. Pure Store/SQL/graph — no LLM.\n";
j.push(#{ prompt: p, label: "lm-analyze", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: maintain/refresh.rs: reindex_fts, rebuild_graph_for_all_or_dirty (best effort), rebuild_wiki_index, reembed_all optional. MCP maintain_refresh with flags.\n";
j.push(#{ prompt: p, label: "lm-refresh", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: vacuum_store: DuckDB CHECKPOINT; return file size before/after if possible. MCP tool.\n";
j.push(#{ prompt: p, label: "lm-vacuum", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: MCP analyze_corpus returns AnalysisReport JSON. Wire maintain module in lib.rs.\n";
j.push(#{ prompt: p, label: "lm-tool-analyze", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: refresh_stale_wiki: find wiki docs older than linked raw; re-run compile_source or mark list for agent; at least list + optional LLM recompile if ChatClient present.\n";
j.push(#{ prompt: p, label: "lm-stale-wiki", capability_mode: "all", output_schema: done_schema });

let _r2 = parallel(j);
log("Analyze-refresh done");

// Phase 3: 6 agents
phase("Plan-apply");
j = [];

p = common;
p += "TASK: maintain/plan.rs: MaintenanceAction enum whitelist (Refile, Archive, Pin, MergeExactDup, RebuildIndex, Vacuum, ReindexFts, ConsolidatePropose, ResolveStub…). serde JSON schema for LLM. validate_plan().\n";
j.push(#{ prompt: p, label: "lm-plan-types", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: plan_maintenance tool: call local LLM with analysis JSON + system prompt from maintain/prompts.rs; parse JSON actions; validate; return plan. If llm disabled, return heuristic plan without LLM.\n";
j.push(#{ prompt: p, label: "lm-plan-tool", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: apply_maintenance_plan: dry_run default true; execute whitelist via Store/wiki/graph; cap RAG_MAINT_MAX_DOCS; ops_log each action; return applied/skipped/errors.\n";
j.push(#{ prompt: p, label: "lm-apply", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: maintain_compress: levels 0=checkpoint+fts, 1=exact hash merge dups, 2=near-dup list (apply only with confirm). Never delete raw without allow_raw_delete.\n";
j.push(#{ prompt: p, label: "lm-compress", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: maintain_organize: heuristic or LLM suggest refiles for docs missing wing; dry_run; optional apply.\n";
j.push(#{ prompt: p, label: "lm-organize", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: consolidate tool if missing: LLM merges N doc texts into one wiki page proposal; apply flag writes wiki + graph rebuild.\n";
j.push(#{ prompt: p, label: "lm-consolidate", capability_mode: "all", output_schema: done_schema });

let _r3 = parallel(j);
log("Plan-apply done");

// Phase 4: 5 agents
phase("Integrate");
j = [];

p = common;
p += "TASK: tests/maintain_test.rs: analyze on fixture corpus; plan heuristic without network; apply dry_run; compress level 0; vacuum. mock embedder.\n";
j.push(#{ prompt: p, label: "lm-tests", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: README section Local LLM + maintenance: ollama setup, env table, tool list, dry_run safety.\n";
j.push(#{ prompt: p, label: "lm-readme", capability_mode: "all", output_schema: done_schema });

p = common;
p += "TASK: cargo test PRIMARY fix all. success=true only green.\n";
j.push(#{ prompt: p, label: "lm-fix1", capability_mode: "all", output_schema: fix_schema });

p = common;
p += "TASK: cargo test SECONDARY. success=true only green.\n";
j.push(#{ prompt: p, label: "lm-fix2", capability_mode: "all", output_schema: fix_schema });

p = common;
p += "TASK: FINAL cargo test until green.\n";
j.push(#{ prompt: p, label: "lm-fix3", capability_mode: "all", output_schema: fix_schema });

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
    pfix += "ULTIMATE FIX. " + rem + " cargo test green.\n";
    let last = agent(pfix, #{ label: "lm-fix-ultimate", capability_mode: "all", output_schema: fix_schema });
    if last.success && last.output.success == true {
        ok = true;
        summary = last.output.summary;
        rem = "";
    } else if last.success {
        summary = last.output.summary;
        rem = last.output.remaining_errors;
    }
}

// 4+5+6+5+1 = 21

complete(#{
    ok: ok,
    summary: summary,
    remaining_errors: rem,
    root: root,
    agents: "prov4 analyze5 plan6 int5+1 ~21",
});
