//! Wiki compilation, index, diary, and durable operations-log tools.

use super::facade::RagServer;
use super::take_routes;
use rmcp::handler::server::tool::ToolRouter;

const TOOLS: &[&str] = &[
    "get_schema", "update_schema", "diary_write", "diary_read", "wake_up",
    "checkpoint", "append_log", "read_log", "list_recent_ops",
    "memories_filed_away", "reconnect", "query_with_index", "search_wiki",
    "file_answer", "read_index", "update_index_entry", "rebuild_index",
    "write_wiki_page", "update_wiki_page", "get_wiki_page", "list_wiki_pages",
    "compile_source", "consolidate", "lint_wiki", "refresh_stale_wiki",
];

pub(super) fn router(all: &mut ToolRouter<RagServer>) -> ToolRouter<RagServer> {
    take_routes(all, TOOLS)
}
