//! Public MCP tool surface: **spine** (default) vs **full**.
//!
//! See `docs/ARCHITECTURE_VISION.md` §5. Env: `RAG_TOOLS=spine|full`.

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::LazyLock;

use crate::error::AppError;

/// Which tools are advertised and callable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolSurface {
    /// ~25 compile-first tools (default).
    #[default]
    Spine,
    /// All registered tools.
    Full,
}

impl ToolSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spine => "spine",
            Self::Full => "full",
        }
    }
}

impl FromStr for ToolSurface {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "spine" | "default" | "core" => Ok(Self::Spine),
            "full" | "all" | "advanced" => Ok(Self::Full),
            other => Err(AppError::config(format!(
                "invalid RAG_TOOLS '{other}': expected 'spine' or 'full'"
            ))),
        }
    }
}

/// Spine tools (index-first cascade + minimal control). Keep SPEC names.
///
/// Cascade: `query_with_index` → `search_wiki` → `search` → neighbors/backlinks.
static SPINE: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // L0 deposit / read
        "ingest_raw",
        "ingest_text",
        "ingest_file",
        "list_sources",
        "get_source",
        "list_documents",
        "get_document",
        // L3 compile
        "write_wiki_page",
        "update_wiki_page",
        "get_wiki_page",
        "list_wiki_pages",
        "get_schema",
        "update_schema",
        "read_index",
        "rebuild_index",
        "append_log",
        "read_log",
        "query_with_index",
        "search_wiki",
        "file_answer",
        "lint_wiki",
        // L1 escape hatch + pack
        "search",
        "pack_context",
        // L2 structure (+ snapshot for concurrent UI)
        "get_neighbors",
        "get_backlinks",
        "link_nodes",
        "find_node",
        "get_graph",
        "export_graph_snapshot",
        // control
        "status",
        "doctor",
        "doctor_repair",
    ])
});

/// Whether `tool_name` is allowed for the given surface.
pub fn tool_allowed(surface: ToolSurface, tool_name: &str) -> bool {
    match surface {
        ToolSurface::Full => true,
        ToolSurface::Spine => SPINE.contains(tool_name),
    }
}

/// Sorted spine tool names (tests / docs).
pub fn spine_tool_names() -> Vec<&'static str> {
    let mut v: Vec<_> = SPINE.iter().copied().collect();
    v.sort_unstable();
    v
}

/// Short index-first playbook for `get_info` and schema://agents.
pub const INDEX_FIRST_PLAYBOOK: &str = r#"## Index-first query cascade (mandatory)

Prefer this order before answering from the store:

1. **query_with_index** — match the wiki catalog (slug/title/summary).
2. **search_wiki** — semantic/lex search over compiled wiki only.
3. **search** (mode=hybrid when possible) — raw/chunk retrieval escape hatch.
4. **get_neighbors** / **get_backlinks** — structure after you have a node/doc.
5. **get_document** / **get_source** — open full text only for 1–2 ids.
6. **pack_context** — compress ranked hits under a token budget before long synthesis.

Do **not** start with a dump of list_documents or get_graph over the whole corpus.
Do **not** invent facts; cite document ids / wiki slugs from tool results.

## Compile loop (client LLM owns synthesis)

get_schema → get_source/get_document (raw) → write_wiki_page* → rebuild_index → append_log
Optional: file_answer for durable Q&A. Raw layer is immutable (re-ingest to replace).
"#;

/// Compact spine tool blurb for server instructions.
pub const SPINE_TOOLS_BLURB: &str = r#"Spine tools (RAG_TOOLS=spine, default):
L0: ingest_raw, ingest_text, ingest_file, list_sources, get_source, list_documents, get_document
L3: write/update/get/list_wiki_page(s), get/update_schema, read/rebuild_index, append/read_log,
    query_with_index, search_wiki, file_answer, lint_wiki
L1: search, pack_context
L2: get_neighbors, get_backlinks, link_nodes, find_node
Control: status, doctor
Set RAG_TOOLS=full for MemPalace kg/diary/tunnels, maintain_*, graph_expand, etc.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spine_is_bounded() {
        let n = spine_tool_names().len();
        assert!(n >= 15 && n <= 32, "spine size {n}");
        assert!(tool_allowed(ToolSurface::Spine, "search"));
        assert!(tool_allowed(ToolSurface::Spine, "query_with_index"));
        assert!(!tool_allowed(ToolSurface::Spine, "kg_add"));
        assert!(!tool_allowed(ToolSurface::Spine, "wake_up"));
        assert!(tool_allowed(ToolSurface::Full, "kg_add"));
    }

    #[test]
    fn parse_surface() {
        assert_eq!("spine".parse::<ToolSurface>().unwrap(), ToolSurface::Spine);
        assert_eq!("full".parse::<ToolSurface>().unwrap(), ToolSurface::Full);
    }
}
