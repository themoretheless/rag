//! Collection, outline, and dependency-order tools.

use chrono::Utc;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router, ErrorData as McpError};
use uuid::Uuid;

use super::facade::RagServer;
use super::tools::{
    CollectionCreateParams, CollectionGetParams, CollectionListParams, CollectionUpdateParams,
};
use crate::error::AppError;
use crate::models::{Collection, OpsLogEntry};
use crate::util::non_blank;

#[tool_router(router = collections_router, vis = "pub(super)")]
impl RagServer {
    #[tool(
        name = "collection_create",
        description = "Create a durable named collection. entries array order is the reading \
        order; each entry may name a parent_document_id for outline nesting and depends_on \
        document ids for prerequisites. All ids must reference existing documents in the \
        collection."
    )]
    pub(super) async fn collection_create(
        &self,
        Parameters(params): Parameters<CollectionCreateParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::util::blocking_section(|| {
            let now = Utc::now();
            let collection = Collection {
                id: Uuid::new_v4().to_string(),
                name: params.name.trim().to_string(),
                description: non_blank(params.description),
                metadata_json: params.metadata_json.unwrap_or_else(|| "{}".into()),
                created_at: now.to_rfc3339(),
                updated_at: now.to_rfc3339(),
            };
            let entries = Self::collection_entries(params.entries);
            let detail = self.store.create_collection(&collection, &entries)?;
            let _ = self.store.append_ops_log(&OpsLogEntry {
                id: String::new(),
                seq: 0,
                ts: now,
                op: "collection_create".into(),
                prefix: Some("COLLECTION".into()),
                message: detail.collection.name.clone(),
                entity_id: Some(detail.collection.id.clone()),
                entity_kind: Some("collection".into()),
                payload_json: serde_json::json!({"entry_count": detail.entries.len()}).to_string(),
                agent_name: None,
            });
            Self::json_result(&detail)
        })
    }

    #[tool(
        name = "collection_list",
        description = "List durable collections ordered by most recently updated. Returns collection metadata without entries."
    )]
    pub(super) async fn collection_list(
        &self,
        Parameters(_params): Parameters<CollectionListParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::util::blocking_section(|| Self::json_result(&self.store.list_collections()?))
    }

    #[tool(
        name = "collection_get",
        description = "Get a collection with entries in reading order, outline \
        parent_document_id values, optional depends_on prerequisite links, and a derived \
        dependency_order. Dependency cycles are reported in dependency_cycle_members."
    )]
    pub(super) async fn collection_get(
        &self,
        Parameters(params): Parameters<CollectionGetParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::util::blocking_section(|| {
            let detail = self
                .store
                .get_collection(params.collection_id.trim())?
                .ok_or_else(|| {
                    McpError::from(AppError::not_found(format!(
                        "collection not found: {}",
                        params.collection_id
                    )))
                })?;
            Self::json_result(&detail)
        })
    }

    #[tool(
        name = "collection_update",
        description = "Update collection metadata and optionally replace its entries. When \
        entries is present, array order replaces reading order and parent_document_id/depends_on \
        replace the outline and prerequisite links; omit entries to preserve membership."
    )]
    pub(super) async fn collection_update(
        &self,
        Parameters(params): Parameters<CollectionUpdateParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::util::blocking_section(|| {
            let entries = params.entries.map(Self::collection_entries);
            let description = params.description.as_deref().map(|value| {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            });
            let detail = self.store.update_collection(
                params.collection_id.trim(),
                params.name.as_deref(),
                description,
                params.metadata_json.as_deref(),
                entries.as_deref(),
            )?;
            let _ = self.store.append_ops_log(&OpsLogEntry {
                id: String::new(),
                seq: 0,
                ts: Utc::now(),
                op: "collection_update".into(),
                prefix: Some("COLLECTION".into()),
                message: detail.collection.name.clone(),
                entity_id: Some(detail.collection.id.clone()),
                entity_kind: Some("collection".into()),
                payload_json: serde_json::json!({"entry_count": detail.entries.len()}).to_string(),
                agent_name: None,
            });
            Self::json_result(&detail)
        })
    }
}
