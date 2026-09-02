//! Corpus analysis and maintenance tools.

use super::facade::RagServer;
use super::take_routes;
use rmcp::handler::server::tool::ToolRouter;

const TOOLS: &[&str] = &[
    "analyze_corpus",
    "plan_maintenance",
    "apply_maintenance_plan",
    "maintain_organize",
    "maintain_refresh",
    "maintain_compress",
    "vacuum_store",
    "doctor_repair",
    "cleanup_source_duplicates",
];

pub(super) fn router(all: &mut ToolRouter<RagServer>) -> ToolRouter<RagServer> {
    take_routes(all, TOOLS)
}
