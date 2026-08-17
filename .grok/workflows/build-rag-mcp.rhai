let meta = #{
    name: "build-rag-mcp",
    description: "Build RAG MCP server (Rust + DuckDB) with ~20 agents",
    phases: [
        #{ title: "Scaffold", detail: "lib/main stubs and module skeleton" },
        #{ title: "Core", detail: "error config models math chunking embeddings" },
        #{ title: "Storage", detail: "DuckDB schema store search" },
        #{ title: "MCP", detail: "server tools wiring" },
        #{ title: "Polish", detail: "tests README client config" },
        #{ title: "Integrate", detail: "fix compile errors until green" },
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
common += "Read SPEC.md first (absolute path: " + root + "/SPEC.md). Implement exactly that contract.\n";
common += "Rules: own ONLY assigned files; English code; no stdout logging (stderr/tracing only); keep public APIs matching SPEC.\n";
common += "Use read_file/search_replace/write tools. After writing, try cargo check if useful.\n";
common += "If a dependency API differs from SPEC (esp. rmcp), adapt to what compiles on crates.io while keeping tool names and behavior.\n";

// ---------- Phase 1: Scaffold (2 agents) ----------
phase("Scaffold");

let p_lib = common;
p_lib += "TASK: Write src/lib.rs declaring modules: config, error, models, db, chunking, embeddings, mcp. Re-export public types needed by tests: Config, Store, AppError, Document, Chunk, SearchHit, IngestResult, Stats, and factory helpers if present. Keep it short.\n";
p_lib += "Also write empty-ish module roots if missing so the tree exists:\n";
p_lib += "- src/db/mod.rs pub mod schema; pub mod store; pub mod search; pub use store::Store;\n";
p_lib += "- src/chunking/mod.rs and embeddings/mod.rs and mcp/mod.rs as thin re-exports.\n";
p_lib += "Do NOT implement full logic in those roots if other agents own the leaf files; declare modules only.\n";

let p_main = common;
p_main += "TASK: Write src/main.rs: init tracing on stderr, Config::from_env, Store::open, build embedder from config, build RagServer, serve stdio via rmcp ServiceExt + transport::stdio, waiting(). Map errors with anyhow. Binary must be named rag-mcp (Cargo.toml already set).\n";
p_main += "If RagServer path is crate::mcp::server::RagServer, use that. Import from rag_mcp lib or local mods as appropriate.\n";
p_main += "Prefer using the library crate: either make main use rag_mcp::* or declare the same mods. Simplest: main uses the library via use rag_mcp::...\n";

let s1 = parallel([
    #{ prompt: p_lib, label: "scaffold-lib", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_main, label: "scaffold-main", capability_mode: "all", output_schema: done_schema },
]);
log("Scaffold agents done");

// ---------- Phase 2: Core modules (8 agents) ----------
phase("Core");

let p_err = common;
p_err += "TASK: Implement src/error.rs with thiserror AppError variants (Config, Db, Embeddings, Io, Chunking, NotFound, Other) and pub type Result<T> = std::result::Result<T, AppError>. Display-friendly messages. From impls for duckdb::Error, std::io::Error, reqwest::Error, serde_json::Error where useful.\n";

let p_cfg = common;
p_cfg += "TASK: Implement src/config.rs Config struct matching SPEC env vars and Config::from_env() -> Result<Config, AppError>. Defaults from SPEC. Parse usize/u64 carefully. Validate: if provider openai, api_key non-empty; chunk_overlap < chunk_size; dims > 0.\n";

let p_models = common;
p_models += "TASK: Implement src/models.rs with Document, Chunk, SearchHit, IngestResult, Stats as in SPEC. Derive Debug, Clone, Serialize, Deserialize where sensible. SearchHit score is f32 cosine.\n";

let p_chunk_mod = common;
p_chunk_mod += "TASK: Implement src/chunking/mod.rs: Chunker trait with fn chunk(&self, text: &str) -> Vec<(String, i32, i32)> returning (content, char_start, char_end). Factory from_config(chunk_size, overlap) -> FixedChunker. Re-export fixed::FixedChunker.\n";

let p_chunk_fixed = common;
p_chunk_fixed += "TASK: Implement src/chunking/fixed.rs FixedChunker { size, overlap } with constructor and chunking algorithm from SPEC (window + prefer break at whitespace/newline in last 20%). Unit tests in the same file with #[cfg(test)].\n";

let p_emb_mod = common;
p_emb_mod += "TASK: Implement src/embeddings/mod.rs: EmbeddingProvider async trait (async_trait) embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> and dimensions(). Factory build_provider(config: &Config) -> Result<Arc<dyn EmbeddingProvider>> for mock/openai. Re-export mock and openai modules. Include cosine_similarity(a, b) -> f32 helper and l2_normalize(v: &mut [f32]).\n";

let p_emb_mock = common;
p_emb_mock += "TASK: Implement src/embeddings/mock.rs MockEmbedder with deterministic hash-based vectors of given dims, L2-normalized. Same text => same vector. Unit tests.\n";

let p_emb_openai = common;
p_emb_openai += "TASK: Implement src/embeddings/openai.rs OpenAiEmbedder: base_url, api_key, model, dims. POST /embeddings OpenAI-compatible. Batch inputs <=64. Map errors to AppError. Parse data[].embedding arrays.\n";

let core = parallel([
    #{ prompt: p_err, label: "core-error", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_cfg, label: "core-config", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_models, label: "core-models", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_chunk_mod, label: "core-chunk-mod", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_chunk_fixed, label: "core-chunk-fixed", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_emb_mod, label: "core-emb-mod", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_emb_mock, label: "core-emb-mock", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_emb_openai, label: "core-emb-openai", capability_mode: "all", output_schema: done_schema },
]);
log("Core agents done");

// ---------- Phase 3: Storage (3 agents) ----------
phase("Storage");

let p_schema = common;
p_schema += "TASK: Implement src/db/schema.rs: SQL constants and fn migrate(conn: &Connection) -> Result<()> creating documents/chunks tables and index as in SPEC. Safe to call repeatedly.\n";

let p_store = common;
p_store += "TASK: Implement src/db/store.rs Store with Arc<Mutex<Connection>> and open/upsert/insert_chunks/delete/get/list/list_chunks/stats/find_by_uri as in SPEC. Serialize embeddings as JSON in embedding_json. Use chrono Utc timestamps. Call schema::migrate on open.\n";

let p_search = common;
p_search += "TASK: Implement src/db/search.rs: fn search_chunks(store, query_embedding: &[f32], top_k, document_id: Option<&str>) -> Result<Vec<SearchHit>>. Load chunks (optionally filtered), cosine vs query, sort desc, top_k. Join document title/uri.\n";

let stor = parallel([
    #{ prompt: p_schema, label: "db-schema", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_store, label: "db-store", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_search, label: "db-search", capability_mode: "all", output_schema: done_schema },
]);
log("Storage agents done");

// ---------- Phase 4: MCP (3 agents) ----------
phase("MCP");

let p_tools = common;
p_tools += "TASK: Implement src/mcp/tools.rs parameter structs with serde Deserialize + schemars JsonSchema for all tools in SPEC (IngestTextParams, IngestFileParams, SearchParams, GetDocumentParams, DeleteDocumentParams). Good field descriptions for LLMs.\n";

let p_server = common;
p_server += "TASK: Implement src/mcp/server.rs RagServer with tool_router using rmcp macros #[tool_router]/#[tool]/#[tool_handler] ServerHandler. Hold Store (Arc), Arc<dyn EmbeddingProvider>, Config. Implement all 7 tools. Ingest pipeline: chunk -> embed -> store; upsert by uri. Return CallToolResult JSON/text. get_info with instructions listing tools. Map AppError to McpError.\n";
p_server += "Read an example rmcp server pattern: tool_router on impl, Parameters<T> for args, Content::text/json, serve via ServiceExt in main.\n";

let p_mcp_mod = common;
p_mcp_mod += "TASK: Implement src/mcp/mod.rs re-exporting server::RagServer and tools. Ensure modules compile together. If server/tools missing pieces, add minimal glue only in mod.rs.\n";

let mcp = parallel([
    #{ prompt: p_tools, label: "mcp-tools", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_server, label: "mcp-server", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_mcp_mod, label: "mcp-mod", capability_mode: "all", output_schema: done_schema },
]);
log("MCP agents done");

// ---------- Phase 5: Polish (3 agents) ----------
phase("Polish");

let p_tests = common;
p_tests += "TASK: Write tests/integration_test.rs using tempfile + mock embedder path via env or direct API: open Store in temp dir, ingest two distinct texts, search for distinctive term, assert top hit from expected doc, delete, stats. Also unit-level tests if easier in src. Must be runnable with cargo test.\n";

let p_readme = common;
p_readme += "TASK: Write README.md: what it is, features, env vars table, build/run, MCP client JSON example, tool list, mock vs openai, limitations. Concise technical English or bilingual RU/EN short sections OK.\n";

let p_example = common;
p_example += "TASK: Write examples/mcp.client.json sample config for Claude/Cursor-style mcpServers pointing to target/release/rag-mcp with env RAG_EMBEDDING_PROVIDER=mock and RAG_DB_PATH. Also write .gitignore for /target, *.duckdb, .DS_Store.\n";

let pol = parallel([
    #{ prompt: p_tests, label: "tests-integration", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_readme, label: "docs-readme", capability_mode: "all", output_schema: done_schema },
    #{ prompt: p_example, label: "docs-example", capability_mode: "all", output_schema: done_schema },
]);
log("Polish agents done");

// ---------- Phase 6: Integrate / fix (2 agents sequential style via parallel of fixers) ----------
phase("Integrate");

let p_fix1 = common;
p_fix1 += "TASK: You are the integration lead. Run cargo check and cargo test in " + root + ". Fix ALL compile and test failures by editing any source files as needed. Align module imports, rmcp API, duckdb API, visibility. Do not change tool names or SPEC behavior. Prefer minimal diffs. Iterate until cargo test passes or you are blocked by missing network for deps (still fix code errors).\n";
p_fix1 += "Return success=true only if cargo test exits 0.\n";

let p_fix2 = common;
p_fix2 += "TASK: Secondary fixer: after scanning the codebase under " + root + ", fix obvious inconsistencies between modules (wrong paths, missing pub use, trait imports, Mutex poisoning maps). Run cargo check. Coordinate by reading files first; if already compiling, run tests and fix failures. success=true only if cargo test passes.\n";

let fixes = parallel([
    #{ prompt: p_fix1, label: "integrate-primary", capability_mode: "all", output_schema: fix_schema },
    #{ prompt: p_fix2, label: "integrate-secondary", capability_mode: "all", output_schema: fix_schema },
]);

let ok = false;
let summary = "integration incomplete";
let rem = "";
if fixes[0] != () && fixes[0].success && fixes[0].output.success == true {
    ok = true;
    summary = fixes[0].output.summary;
} else if fixes[1] != () && fixes[1].success && fixes[1].output.success == true {
    ok = true;
    summary = fixes[1].output.summary;
} else {
    if fixes[0] != () && fixes[0].success {
        summary = fixes[0].output.summary;
        rem = fixes[0].output.remaining_errors;
    }
    if fixes[1] != () && fixes[1].success {
        summary += " | " + fixes[1].output.summary;
        rem += " | " + fixes[1].output.remaining_errors;
    }
}

// Final single fixer if still red
if ok == false {
    let p_fix3 = common;
    p_fix3 += "TASK: FINAL fixer. cargo test still failing. Remaining notes: " + rem + "\n";
    p_fix3 += "Fix everything. Run cargo test until green. success=true only on green tests.\n";
    let last = agent(p_fix3, #{ label: "integrate-final", capability_mode: "all", output_schema: fix_schema });
    if last.success && last.output.success == true {
        ok = true;
        summary = last.output.summary;
        rem = "";
    } else if last.success {
        summary = last.output.summary;
        rem = last.output.remaining_errors;
    }
}

// Agent count accounting: 2+8+3+3+3+2+1 = 22 max; typical 21

complete(#{
    ok: ok,
    summary: summary,
    remaining_errors: rem,
    root: root,
    agent_phases: "scaffold2 core8 storage3 mcp3 polish3 integrate2-3",
});
