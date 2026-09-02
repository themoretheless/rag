//! Ingest, source inventory, document integrity, taxonomy, and health tools.

use super::facade::RagServer;
use super::take_routes;
use rmcp::handler::server::tool::ToolRouter;

const TOOLS: &[&str] = &[
    "ingest_text",
    "ingest_file",
    "sync_sources",
    "ingest_raw",
    "add_drawer",
    "check_duplicate",
    "delete_by_source",
    "list_sources",
    "get_source",
    "list_documents",
    "get_document",
    "delete_document",
    "update_document_meta",
    "list_wings",
    "list_rooms",
    "get_taxonomy",
    "stats",
    "status",
    "doctor",
    "llm_status",
];

pub(super) fn router(all: &mut ToolRouter<RagServer>) -> ToolRouter<RagServer> {
    take_routes(all, TOOLS)
}
