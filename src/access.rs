//! Explicit permissions for the public MCP tool surface.
//!
//! Roles authorize operations, not database connection modes: retrieval can
//! refresh derived search indexes and records request telemetry. Content writes
//! need `Write`; bulk maintenance, recovery, and filesystem exports need `Admin`.
//! Tool names and MCP annotations are not an authorization boundary. Every new
//! tool must be reviewed and added here before it can be called over HTTP.

pub use crate::tool_spec::AccessRole;

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
    crate::tool_spec::tool_spec(name).map(|spec| spec.role)
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
