let meta = #{
    name: "p0p1-40-agents",
    description: "Finish rag-mcp P0/P1 wiki/CAS/UI gaps with ~40 agents + critic pass",
    phases: [
        #{ title: "Audit", detail: "12 scouts inventory done vs missing" },
        #{ title: "Implement", detail: "18 implementers exclusive file ownership" },
        #{ title: "Critic", detail: "10 adversarial critics" },
    ],
};

let inventory_schema = #{
    "type": "object",
    "required": ["area", "status", "gaps", "files"],
    "properties": #{
        "area": #{ "type": "string" },
        "status": #{ "type": "string" },
        "gaps": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 12 },
        "files": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 12 },
    },
};

let implement_schema = #{
    "type": "object",
    "required": ["task", "done", "files_changed", "summary", "blocked"],
    "properties": #{
        "task": #{ "type": "string" },
        "done": #{ "type": "boolean" },
        "files_changed": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 20 },
        "summary": #{ "type": "string" },
        "blocked": #{ "type": "string" },
    },
};

let critic_schema = #{
    "type": "object",
    "required": ["dimension", "pass", "issues", "evidence"],
    "properties": #{
        "dimension": #{ "type": "string" },
        "pass": #{ "type": "boolean" },
        "issues": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 10 },
        "evidence": #{ "type": "string" },
    },
};

let root = if args == () { "/Users/themoretheless/Documents/Sources/rag" } else if args.root == () { "/Users/themoretheless/Documents/Sources/rag" } else { args.root };

// ---------------------------------------------------------------------------
phase("Audit");
// ---------------------------------------------------------------------------
let audits = [
    #{ a: "store-wiki-metas", p: "Read src/db/store.rs list_wiki_page_metas and wiki_backlinks_for_document. Report if implemented, tests present, any gaps (filter/pagination, empty graph)." },
    #{ a: "http-wiki-bind", p: "Read src/http_api.rs for /v1/wiki /v1/backlinks parse_bind RAG_HTTP_ALLOW_REMOTE. Gaps: q/limit/offset, PUT wiki, tests." },
    #{ a: "cas-revision", p: "Read src/db/store.rs upsert_document_cas, models revision/etag, wiki/mod WriteWikiOpts if_match. Gaps: require-if-match env." },
    #{ a: "mcp-ingest-wing", p: "Read IngestFileParams and ingest_file tool in mcp/tools.rs and mcp/server.rs for wing/room. Gaps: docs/tests." },
    #{ a: "ui-wiki-read", p: "Read crates/rag-mcp-ui ui/wiki.rs app.rs load.rs for sidebar metas, backlinks, link colors, bold/code. Gaps: editor, dual-pane, history polish." },
    #{ a: "hooks-claude", p: "Read .claude/settings.json hooks queue-rag-ingest.sh remind-rag-flush.sh CLAUDE.md. Gaps: executable bit, docs, .rag gitignore." },
    #{ a: "docs-connect", p: "Read docs/CONNECT.md PROD_RUN.md EGUI_USAGE.md for HTTP wiki/backlinks bind guard CAS. List outdated claims." },
    #{ a: "schema-v5", p: "Read src/db/schema.rs SCHEMA_VERSION revision column migrations. Gaps for CAS." },
    #{ a: "tests-existing", p: "Grep tests for document_cas list_wiki_page_metas wiki_backlinks parse_bind. List missing unit tests." },
    #{ a: "graph-wiki-bridge", p: "Read UI graph to wiki open by document_id and Show in graph. Gaps." },
    #{ a: "markdown-render", p: "Read render_wiki_markdown in wiki.rs. Gaps: italic, nested lists, tables." },
    #{ a: "cli-ingest", p: "Read src/bin/ingest_project.rs and Cargo.toml bins. Gap: simple ingest-file CLI with wing/room." },
];

let audit_jobs = [];
for item in audits {
    let pr = "Repo root: " + root + ". Area=" + item.a + ". ";
    pr += item.p;
    pr += " Use read_file and grep only. Do not edit. Return structured inventory.";
    audit_jobs.push(#{
        prompt: pr,
        label: "audit:" + item.a,
        capability_mode: "read-only",
        output_schema: inventory_schema,
    });
}
let audit_results = parallel(audit_jobs);
log("Audit panel finished");

let gap_text = "";
for r in audit_results {
    if r != () && r.success && r.output != () {
        gap_text += r.output.area + ":" + r.output.status + "; ";
        if r.output.gaps != () {
            for g in r.output.gaps {
                gap_text += g + " | ";
            }
        }
    }
}
if gap_text == "" { gap_text = "(no structured gaps; implementers still fill hard requirements)"; }
log("Gaps summary: " + gap_text);

// ---------------------------------------------------------------------------
phase("Implement");
// ---------------------------------------------------------------------------
// Exclusive ownership: each agent edits only listed paths to avoid collisions.
let tasks = [
    #{
        id: "t-store-tests",
        files: "src/db/store.rs (tests module only — append tests)",
        work: "Add unit tests for list_wiki_page_metas (wiki rows without content fields) and wiki_backlinks_for_document after writing wiki pages with [[links]]. Follow existing test patterns (temp db, mock embed if needed). Do not change production APIs unless a tiny bug blocks the test.",
    },
    #{
        id: "t-bind-tests",
        files: "src/http_api.rs (append tests module at end if missing)",
        work: "Add unit tests for parse_bind: empty->None, 127.0.0.1:7432 Ok, 0.0.0.0:7432 Err without env, Ok with RAG_HTTP_ALLOW_REMOTE=true. Use temp_env or set/remove env carefully in test. Do not break serve().",
    },
    #{
        id: "t-wiki-query",
        files: "src/http_api.rs wiki_list handler + Query struct; src/db/store.rs list_wiki_page_metas filter optional",
        work: "Add optional query params to GET /v1/wiki: q (substring title/slug/summary case-insensitive), limit (default 500 max 2000), offset (default 0). Filter in store or handler. Return count total if cheap else count of returned. Keep backward compatible JSON ok/pages.",
    },
    #{
        id: "t-wiki-put",
        files: "src/http_api.rs only",
        work: "Add PUT /v1/wiki with JSON body {slug, content, title?, if_match_revision?} calling crate::wiki write/update APIs. Return revision/etag. 409 on CAS mismatch. Route must share HttpState. Read wiki/mod.rs for WriteWikiOpts.",
    },
    #{
        id: "t-require-cas",
        files: "src/config.rs, src/wiki/mod.rs, CLAUDE.md",
        work: "Add config env RAG_WIKI_REQUIRE_IF_MATCH (default false). When true, update_wiki_page / write overwrite path must reject missing if_match_revision. Document in CLAUDE.md. Wire Config load from env.",
    },
    #{
        id: "t-ui-editor",
        files: "crates/rag-mcp-ui/src/ui/wiki.rs, crates/rag-mcp-ui/src/app.rs, crates/rag-mcp-ui/src/load.rs",
        work: "Add optional edit mode in wiki article: Edit button toggles multiline TextEdit; Save calls HTTP PUT /v1/wiki with slug+content+if_match_revision when source is HTTP, or local store write when --db. Show revision. Keep read mode default. Do not break graph mode.",
    },
    #{
        id: "t-md-italic",
        files: "crates/rag-mcp-ui/src/ui/wiki.rs emit_rich_plain only",
        work: "Extend emit_rich_plain (or equivalent) to support *italic* and __bold__ in addition to **bold** and `code`. Do not break wikilinks.",
    },
    #{
        id: "t-cli-ingest",
        files: "src/bin/ingest_file.rs (new), Cargo.toml [[bin]]",
        work: "Add CLI binary ingest_file: args --path --wing --room --db, uses same Store+config+mock/openai embed as server. Simple: open store, read file, upsert via existing pipeline or public store APIs. Prefer reusing library helpers. Keep small.",
    },
    #{
        id: "t-dual-pane",
        files: "crates/rag-mcp-ui/src/app.rs layout of Wiki mode only",
        work: "Wiki mode: left sidebar fixed min width ~220, center article, optional right backlinks column if easy. Ensure Back button and history still work. If already dual-pane, polish splitter with egui::SidePanel.",
    },
    #{
        id: "t-smoke-script",
        files: "scripts/smoke_http_wiki.sh (new)",
        work: "Write bash smoke script curling /health /v1/wiki /v1/backlinks?id=dummy expecting JSON. Exit non-zero on failure. Document BASE URL env default http://127.0.0.1:7432. chmod +x not required in script content.",
    },
    #{
        id: "t-docs-connect",
        files: "docs/CONNECT.md",
        work: "Document /v1/wiki (meta only), /v1/backlinks, RAG_HTTP_ALLOW_REMOTE, if_match_revision workflow for multi-LLM. Accurate, short sections.",
    },
    #{
        id: "t-docs-prod",
        files: "docs/PROD_RUN.md",
        work: "Document loopback bind default, RAG_HTTP_ALLOW_REMOTE danger, HTTP_ONLY gateway, restart after schema migration.",
    },
    #{
        id: "t-docs-egui",
        files: "docs/EGUI_USAGE.md",
        work: "Document Wiki|Graph modes, backlinks, resolved/unresolved link colors, how to open from graph, seed by document_id.",
    },
    #{
        id: "t-gitignore-rag",
        files: ".gitignore, .claude/hooks if needed",
        work: "Ensure .rag/ pending-ingest queue is gitignored. Make hooks scripts portable. Fix settings.json if broken.",
    },
    #{
        id: "t-claude-md",
        files: "CLAUDE.md",
        work: "Expand rules: index-first, re-ingest same path, CAS if_match, wing/room on ingest_file, hooks queue only (do not open DuckDB from hooks).",
    },
    #{
        id: "t-features",
        files: "FEATURES.md or docs/SYSTEM_MAP.md (pick one that tracks status)",
        work: "Mark shipped: wiki meta list, backlinks HTTP, bind guard, CAS revision, wing on ingest_file, UI wiki browser. Be honest about incomplete items.",
    },
    #{
        id: "t-mcp-get-backlinks",
        files: "src/mcp/server.rs get_backlinks tool path only if broken for wiki docs",
        work: "Verify get_backlinks MCP tool works for wiki document nodes. If missing occurrence of wikilink filter, improve only that tool handler. If already fine, report done=false blocked=already ok and make no edits.",
    },
    #{
        id: "t-sidebar-rev",
        files: "crates/rag-mcp-ui/src/ui/wiki.rs draw_wiki_sidebar only",
        work: "Show weak revision or updated_at under sidebar title if fields exist on WikiPageMeta. No other behavior change.",
    },
];

let impl_jobs = [];
for t in tasks {
    let pr = "You are an implementer for rag-mcp at " + root + ". ";
    pr += "Task id: " + t.id + ". ";
    pr += "EDIT ONLY these paths (exclusive ownership): " + t.files + ". ";
    pr += "Do not reformat unrelated code. Do not use em dash character. ";
    pr += "Work: " + t.work + " ";
    pr += "Context gaps from audit (may be partial): " + gap_text + " ";
    pr += "After edits, if you can run a narrow cargo test for your module, do so with simple commands. ";
    pr += "Return done=true only if code/docs were written successfully.";
    impl_jobs.push(#{
        prompt: pr,
        label: "impl:" + t.id,
        capability_mode: "all",
        output_schema: implement_schema,
    });
}
let impl_results = parallel(impl_jobs);
log("Implement panel finished");

let done_n = 0;
let fail_n = 0;
let impl_summary = "";
for r in impl_results {
    if r != () && r.success && r.output != () {
        if r.output.done == true {
            done_n += 1;
        } else {
            fail_n += 1;
        }
        impl_summary += r.output.task + " done=" + r.output.done.to_string() + "; ";
    } else {
        fail_n += 1;
        impl_summary += "slot-failed; ";
    }
}
log("Implement done=" + done_n.to_string() + " fail_or_skip=" + fail_n.to_string());

// ---------------------------------------------------------------------------
phase("Critic");
// ---------------------------------------------------------------------------
let critics = [
    #{ d: "compile-risk", p: "Inspect recent edits in store.rs http_api.rs wiki/mod.rs config.rs UI crate for compile errors: wrong imports, missing fields, API mismatches. Use cargo check -p rag-mcp and cargo check -p rag-mcp-ui if possible." },
    #{ d: "cas-correctness", p: "Verify upsert_document_cas and wiki if_match semantics: stale revision must 409/Conflict; missing doc with if_match; require-if-match env if present." },
    #{ d: "http-contract", p: "Verify /v1/wiki and /v1/backlinks JSON shapes match UI load.rs parsers. Breaking field renames are bugs." },
    #{ d: "bind-security", p: "Verify non-loopback bind is refused without RAG_HTTP_ALLOW_REMOTE. Tests exist or gap." },
    #{ d: "ui-regressions", p: "Wiki sidebar/article/backlinks/graph switch must not panic on empty corpus. Read app.rs wiki path." },
    #{ d: "hooks-safety", p: "Hooks must only queue paths, never open DuckDB or call MCP write. Read scripts." },
    #{ d: "ingest-wing", p: "ingest_file must pass wing/room into pipeline; params optional vs required consistent with docs." },
    #{ d: "test-coverage", p: "Check unit tests cover metas, backlinks, parse_bind, document_cas. List still-missing tests." },
    #{ d: "docs-truth", p: "CONNECT/PROD/EGUI/CLAUDE must not claim features that do not exist in code." },
    #{ d: "perf-wiki-list", p: "list_wiki_page_metas must not SELECT content. Confirm SQL." },
];

let crit_jobs = [];
for c in critics {
    let pr = "Adversarial critic for rag-mcp at " + root + ". Dimension: " + c.d + ". ";
    pr += c.p + " ";
    pr += "Use read_file/grep/cargo. Set pass=true only with concrete evidence. Issues must be real, not speculative. ";
    pr += "Implement summary: " + impl_summary;
    crit_jobs.push(#{
        prompt: pr,
        label: "critic:" + c.d,
        capability_mode: "execute",
        output_schema: critic_schema,
    });
}
let crit_results = parallel(crit_jobs);

let issues = [];
let pass_n = 0;
let fail_c = 0;
for r in crit_results {
    if r != () && r.success && r.output != () {
        if r.output.pass == true {
            pass_n += 1;
        } else {
            fail_c += 1;
            if r.output.issues != () {
                for i in r.output.issues {
                    issues.push(i);
                }
            }
        }
    } else {
        fail_c += 1;
        issues.push("critic slot failed or invalid output");
    }
}

let report = "#{";
report += " audits: 12, implementers: 18, critics: 10 (total 40),";
report += " implement_done: " + done_n.to_string() + ",";
report += " implement_fail: " + fail_n.to_string() + ",";
report += " critics_pass: " + pass_n.to_string() + ",";
report += " critics_fail: " + fail_c.to_string() + ",";
report += " issues: " + json_encode(issues) + ",";
report += " impl_summary: " + json_encode(impl_summary);
report += " }";

let path = write_scratch_file("p0p1-40-report.json", report);
complete(#{
    path: path,
    implement_done: done_n,
    implement_fail: fail_n,
    critics_pass: pass_n,
    critics_fail: fail_c,
    issues: issues,
    summary: "40-agent P0/P1 batch complete; see scratch report",
});
