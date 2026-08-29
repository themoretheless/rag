//! Backup and portable recovery bundle MCP tools.

use super::facade::RagServer;
use super::take_routes;
use rmcp::handler::server::tool::ToolRouter;

const TOOLS: &[&str] = &["backup_db", "export_bundle", "import_bundle"];

pub(super) fn router(all: &mut ToolRouter<RagServer>) -> ToolRouter<RagServer> {
    take_routes(all, TOOLS)
}
