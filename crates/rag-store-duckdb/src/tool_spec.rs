//! The one table of MCP tools: access class, telemetry group, spine flag and
//! code-index evidence flag per tool.
//!
//! `access::required_tool_role`, `telemetry::explicit_tool_group`,
//! `mcp::surface` (spine) and `analysis_tasks::index_tool` all read this
//! table; `mcp::facade` tests assert that every registered tool has exactly one
//! spec and every spec names a registered tool. A new tool is reviewed by
//! adding its row here (unknown tools are forbidden even to an administrator).

/// Minimum role a caller needs (ordered: `Read < Write < Admin`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccessRole {
    Read,
    Write,
    Admin,
}

impl AccessRole {
    /// Gateway environment variable that grants this role. Single owner of the
    /// role-to-variable mapping; the config reader and the reported auth
    /// posture both derive from it.
    pub const fn env_name(self) -> &'static str {
        match self {
            Self::Read => "RAG_HTTP_READ_TOKEN",
            Self::Write => "RAG_HTTP_WRITE_TOKEN",
            Self::Admin => "RAG_HTTP_ADMIN_TOKEN",
        }
    }

    /// Unknown tools are forbidden even to an administrator.
    pub fn allows_tool(self, name: &str) -> bool {
        tool_spec(name)
            .map(|spec| spec.role)
            .is_some_and(|required| self >= required)
    }
}

use AccessRole::{Admin, Read, Write};

/// One MCP tool's static policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: &'static str,
    /// Highest possible effect of the tool, including optional flags.
    pub role: AccessRole,
    /// Coarse family for the console call-share breakdown.
    pub group: &'static str,
    /// Advertised on the default `spine` surface.
    pub spine: bool,
    /// Counts as code-index evidence in analysis task stats.
    pub index_evidence: bool,
}

const fn spec(
    name: &'static str,
    role: AccessRole,
    group: &'static str,
    spine: bool,
    index_evidence: bool,
) -> ToolSpec {
    ToolSpec {
        name,
        role,
        group,
        spine,
        index_evidence,
    }
}

/// Every registered MCP tool, grouped by telemetry family.
pub const TOOL_SPECS: &[ToolSpec] = &[
    spec("collection_create", Write, "collections", false, false),
    spec("collection_get", Read, "collections", false, false),
    spec("collection_list", Read, "collections", false, false),
    spec("collection_update", Write, "collections", false, false),
    spec("affected_tests", Read, "graph", false, true),
    spec("apply_rename", Admin, "graph", false, false),
    spec("callees", Read, "graph", false, true),
    spec("callers", Read, "graph", false, true),
    spec("catalog", Read, "graph", false, true),
    spec("class_usage", Read, "graph", false, true),
    spec("code", Read, "graph", false, true),
    spec("create_tunnel", Write, "graph", false, false),
    spec("db_cross", Read, "graph", false, false),
    spec("db_drift", Read, "graph", false, false),
    spec("db_refresh", Admin, "graph", false, false),
    spec("db_relations", Read, "graph", false, false),
    spec("db_sample", Read, "graph", false, false),
    spec("db_schema", Read, "graph", false, false),
    spec("db_table", Read, "graph", false, false),
    spec("dead_code", Read, "graph", false, true),
    spec("delete_tunnel", Write, "graph", false, false),
    spec("detect_changes", Read, "graph", false, false),
    spec("diff", Read, "graph", false, true),
    spec("drift_ack", Write, "graph", false, false),
    spec("drift_report", Read, "graph", false, false),
    spec("duplicates", Read, "graph", false, true),
    spec("entrypoints", Read, "graph", false, true),
    spec("export_graph_snapshot", Admin, "graph", true, false),
    spec("find_node", Read, "graph", true, false),
    spec("find_tunnels", Read, "graph", false, false),
    spec("follow_tunnels", Read, "graph", false, false),
    spec("freshness", Read, "graph", false, true),
    spec("get_backlinks", Read, "graph", true, false),
    spec("get_graph", Read, "graph", true, false),
    spec("get_neighbors", Read, "graph", true, false),
    spec("graph_expand_search", Read, "graph", false, false),
    spec("graph_stats", Read, "graph", false, false),
    spec("impact", Read, "graph", false, true),
    spec("index_code", Admin, "graph", false, false),
    spec("index_revisions", Read, "graph", false, false),
    spec("index_status", Read, "graph", false, true),
    spec("infrastructure", Read, "graph", false, false),
    spec("inheritance", Read, "graph", false, true),
    spec("link_nodes", Write, "graph", true, false),
    spec("list_tunnels", Read, "graph", false, false),
    spec("live_read", Read, "graph", false, true),
    spec("policy_trace", Read, "graph", false, true),
    spec("references", Read, "graph", false, true),
    spec("rename", Read, "graph", false, false),
    spec("repo_map", Read, "graph", false, true),
    spec("routes", Read, "graph", false, true),
    spec("schema", Read, "graph", false, true),
    spec("search_multi", Read, "graph", false, true),
    spec("slice", Read, "graph", false, true),
    spec("symbol", Read, "graph", false, true),
    spec("table_usage", Read, "graph", false, true),
    spec("trace_path", Read, "graph", false, true),
    spec("write_check", Read, "graph", false, true),
    spec("add_drawer", Write, "ingest", false, false),
    spec("delete_by_source", Admin, "ingest", false, false),
    spec("delete_document", Write, "ingest", false, false),
    spec("ingest_file", Write, "ingest", true, false),
    spec("ingest_raw", Write, "ingest", true, false),
    spec("ingest_text", Write, "ingest", true, false),
    spec("ingest_url", Write, "ingest", true, false),
    spec("reembed_document", Admin, "ingest", false, false),
    spec("sync_sources", Admin, "ingest", false, false),
    spec("update_document_meta", Write, "ingest", false, false),
    spec("kg_add", Write, "kg", false, false),
    spec("kg_invalidate", Write, "kg", false, false),
    spec("kg_query", Read, "kg", false, false),
    spec("kg_stats", Read, "kg", false, false),
    spec("kg_supersede", Write, "kg", false, false),
    spec("kg_timeline", Read, "kg", false, false),
    spec("analyze_corpus", Write, "maintain", false, false),
    spec("apply_maintenance_plan", Admin, "maintain", false, false),
    spec("calibrate", Write, "maintain", false, false),
    spec("calibration_reports", Read, "maintain", false, false),
    spec("gate", Write, "maintain", false, false),
    spec("maintain_compress", Admin, "maintain", false, false),
    spec("maintain_organize", Admin, "maintain", false, false),
    spec("maintain_refresh", Admin, "maintain", false, false),
    spec("plan_maintenance", Write, "maintain", false, false),
    spec("append_log", Write, "memory", true, false),
    spec("archive_memory_items", Write, "memory", false, false),
    spec("checkpoint", Write, "memory", false, false),
    spec("consolidate_memory_items", Write, "memory", false, false),
    spec("diary_read", Read, "memory", false, false),
    spec("diary_write", Write, "memory", false, false),
    spec(
        "list_memory_lifecycle_candidates",
        Read,
        "memory",
        false,
        false,
    ),
    spec("list_recent_ops", Read, "memory", false, false),
    spec("memories_filed_away", Read, "memory", false, false),
    spec("note", Write, "memory", false, false),
    spec("notes", Read, "memory", false, false),
    spec("read_log", Read, "memory", true, false),
    spec("task_set", Write, "memory", false, false),
    spec("task_stats", Read, "memory", false, false),
    spec("wake_up", Read, "memory", false, false),
    spec("cleanup_source_duplicates", Admin, "ops", false, false),
    spec("doctor", Read, "ops", true, false),
    spec("doctor_repair", Admin, "ops", true, false),
    spec("get_embedding_manifest", Read, "ops", false, false),
    spec("get_taxonomy", Read, "ops", false, false),
    spec("list_rooms", Read, "ops", false, false),
    spec("list_wings", Read, "ops", false, false),
    spec("llm_status", Read, "ops", false, false),
    spec("reconnect", Read, "ops", false, false),
    spec("reembed_status", Read, "ops", false, false),
    spec("stats", Read, "ops", false, false),
    spec("status", Read, "ops", true, false),
    spec("vacuum_store", Admin, "ops", false, false),
    spec("backup_db", Admin, "recovery", false, false),
    spec("export_bundle", Admin, "recovery", false, false),
    spec("export_vault", Admin, "recovery", false, false),
    spec("import_bundle", Admin, "recovery", false, false),
    spec("check_duplicate", Read, "search", false, false),
    spec("expand_chunks", Read, "search", false, false),
    spec("find_similar", Read, "search", false, false),
    spec("get_document", Read, "search", true, false),
    spec("get_source", Read, "search", true, false),
    spec("list_documents", Read, "search", true, false),
    spec("list_sources", Read, "search", true, false),
    spec("multi_get", Read, "search", false, false),
    spec("multi_query_search", Read, "search", true, false),
    spec("pack_context", Read, "search", true, false),
    spec("query_with_index", Read, "search", true, false),
    spec("search", Read, "search", true, true),
    spec("search_wiki", Read, "search", true, false),
    spec("compile_source", Write, "wiki", false, false),
    spec("consolidate", Write, "wiki", false, false),
    spec("file_answer", Write, "wiki", true, false),
    spec("get_schema", Write, "wiki", true, false),
    spec("get_wiki_page", Read, "wiki", true, false),
    spec("lint_wiki", Read, "wiki", true, false),
    spec("list_wiki_pages", Read, "wiki", true, false),
    spec("read_index", Read, "wiki", true, false),
    spec("rebuild_index", Admin, "wiki", true, false),
    spec("refresh_stale_wiki", Admin, "wiki", false, false),
    spec("update_index_entry", Write, "wiki", false, false),
    spec("update_schema", Write, "wiki", true, false),
    spec("update_wiki_page", Write, "wiki", true, false),
    spec("write_wiki_page", Write, "wiki", true, false),
];

/// The spec for `name`, if it is a known tool.
pub fn tool_spec(name: &str) -> Option<&'static ToolSpec> {
    TOOL_SPECS.iter().find(|spec| spec.name == name)
}

/// Sorted names of the spine surface.
pub fn spine_tool_names() -> Vec<&'static str> {
    let mut names: Vec<_> = TOOL_SPECS
        .iter()
        .filter(|spec| spec.spine)
        .map(|spec| spec.name)
        .collect();
    names.sort_unstable();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_unique() {
        let mut names: Vec<_> = TOOL_SPECS.iter().map(|spec| spec.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len());
    }
}
