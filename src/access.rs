//! Explicit permissions for the public MCP tool surface.
//!
//! Roles authorize operations, not database connection modes: retrieval can
//! refresh derived search indexes and records request telemetry. Content writes
//! need `Write`; bulk maintenance, recovery, and filesystem exports need `Admin`.
//! Tool names and MCP annotations are not an authorization boundary. Every new
//! tool must be reviewed and added here before it can be called over HTTP.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccessRole {
    Read,
    Write,
    Admin,
}

impl AccessRole {
    /// Unknown tools are forbidden even to an administrator.
    pub fn allows_tool(self, name: &str) -> bool {
        required_tool_role(name).is_some_and(|required| self >= required)
    }
}

/// Minimum role required for a tool, independent of the advertised tool surface.
///
/// Use the highest possible effect of the tool, including optional flags. In
/// particular, a maintenance preview is still an administrative operation, and
/// `get_schema` may seed a default document. Do not infer permission from a name
/// prefix, a client-supplied parameter, or `readOnlyHint` metadata.
pub fn required_tool_role(name: &str) -> Option<AccessRole> {
    match name {
        "collection_list"
        | "collection_get"
        | "check_duplicate"
        | "list_sources"
        | "get_source"
        | "search"
        | "multi_query_search"
        | "get_embedding_manifest"
        | "list_documents"
        | "get_document"
        | "multi_get"
        | "expand_chunks"
        | "find_similar"
        | "list_wings"
        | "list_rooms"
        | "get_taxonomy"
        | "stats"
        | "status"
        | "doctor"
        | "llm_status"
        | "get_graph"
        | "get_neighbors"
        | "get_backlinks"
        | "list_tunnels"
        | "follow_tunnels"
        | "find_tunnels"
        | "graph_stats"
        | "kg_query"
        | "kg_timeline"
        | "kg_stats"
        | "find_node"
        | "graph_expand_search"
        | "pack_context"
        | "diary_read"
        | "wake_up"
        | "read_log"
        | "list_recent_ops"
        | "memories_filed_away"
        | "reconnect"
        | "query_with_index"
        | "search_wiki"
        | "read_index"
        | "get_wiki_page"
        | "list_wiki_pages"
        | "list_memory_lifecycle_candidates"
        | "lint_wiki" => Some(AccessRole::Read),

        "collection_create"
        | "collection_update"
        | "ingest_text"
        | "ingest_file"
        | "ingest_raw"
        | "add_drawer"
        | "delete_document"
        | "update_document_meta"
        | "link_nodes"
        | "create_tunnel"
        | "delete_tunnel"
        | "kg_add"
        | "kg_invalidate"
        | "kg_supersede"
        | "get_schema"
        | "update_schema"
        | "diary_write"
        | "checkpoint"
        | "append_log"
        | "file_answer"
        | "update_index_entry"
        | "analyze_corpus"
        | "plan_maintenance"
        | "write_wiki_page"
        | "update_wiki_page"
        | "compile_source"
        | "consolidate"
        | "consolidate_memory_items"
        | "archive_memory_items" => Some(AccessRole::Write),

        "sync_sources"
        | "delete_by_source"
        | "cleanup_source_duplicates"
        | "reembed_document"
        | "doctor_repair"
        | "export_graph_snapshot"
        | "rebuild_index"
        | "apply_maintenance_plan"
        | "maintain_organize"
        | "maintain_refresh"
        | "maintain_compress"
        | "refresh_stale_wiki"
        | "vacuum_store"
        | "backup_db"
        | "export_bundle"
        | "export_vault"
        | "import_bundle" => Some(AccessRole::Admin),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_only_inherit_lower_permissions() {
        assert!(AccessRole::Read.allows_tool("search"));
        assert!(!AccessRole::Read.allows_tool("write_wiki_page"));
        assert!(!AccessRole::Read.allows_tool("backup_db"));
        assert!(AccessRole::Write.allows_tool("search"));
        assert!(AccessRole::Write.allows_tool("write_wiki_page"));
        assert!(!AccessRole::Write.allows_tool("backup_db"));
        assert!(AccessRole::Admin.allows_tool("search"));
        assert!(AccessRole::Admin.allows_tool("write_wiki_page"));
        assert!(AccessRole::Admin.allows_tool("backup_db"));
    }

    #[test]
    fn unknown_or_similarly_named_tools_are_never_authorized() {
        for name in ["", "get_secrets", "search_and_delete", "Search", "search "] {
            assert_eq!(required_tool_role(name), None, "{name}");
            for role in [AccessRole::Read, AccessRole::Write, AccessRole::Admin] {
                assert!(!role.allows_tool(name), "{role:?}: {name}");
            }
        }
    }

    #[test]
    fn read_named_operations_with_durable_side_effects_need_write() {
        for name in ["get_schema", "analyze_corpus", "plan_maintenance"] {
            assert_eq!(required_tool_role(name), Some(AccessRole::Write), "{name}");
            assert!(!AccessRole::Read.allows_tool(name), "{name}");
        }
        // wake_up only reads an existing schema; reconnect is a documented no-op.
        for name in ["wake_up", "reconnect"] {
            assert_eq!(required_tool_role(name), Some(AccessRole::Read), "{name}");
        }
    }

    #[test]
    fn filesystem_exports_and_maintenance_previews_need_admin() {
        for name in [
            "export_graph_snapshot",
            "export_bundle",
            "export_vault",
            "backup_db",
            "import_bundle",
            "apply_maintenance_plan",
            "doctor_repair",
            "sync_sources",
            "delete_by_source",
            "cleanup_source_duplicates",
        ] {
            assert_eq!(required_tool_role(name), Some(AccessRole::Admin), "{name}");
            assert!(!AccessRole::Write.allows_tool(name), "{name}");
        }
    }
}
