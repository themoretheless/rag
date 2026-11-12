//! Collection, outline, and dependency-order tools.

use crate::mcp::error::mcp_error;
use chrono::Utc;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, tool, tool_router};
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
        self.blocking_tool("collection_create", move |store| {
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
            let detail = store
                .collections()
                .create_collection(&collection, &entries)
                .map_err(mcp_error)?;
            store.ops_log().record(
                &OpsLogEntry::new(
                    "collection_create",
                    "COLLECTION",
                    detail.collection.name.clone(),
                )
                .at(now)
                .entity(Some(detail.collection.id.clone()), "collection")
                .payload(serde_json::json!({"entry_count": detail.entries.len()}).to_string()),
            );
            Self::json_result(&detail)
        })
        .await
    }

    #[tool(
        name = "collection_list",
        description = "List durable collections ordered by most recently updated. Returns collection metadata without entries."
    )]
    pub(super) async fn collection_list(
        &self,
        Parameters(_params): Parameters<CollectionListParams>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking_tool("collection_list", move |store| {
            Self::json_result(&store.collections().list_collections().map_err(mcp_error)?)
        })
        .await
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
        self.blocking_tool("collection_get", move |store| {
            let detail = store
                .collections()
                .get_collection(params.collection_id.trim())
                .map_err(mcp_error)?
                .ok_or_else(|| {
                    mcp_error(AppError::not_found(format!(
                        "collection not found: {}",
                        params.collection_id
                    )))
                })?;
            Self::json_result(&detail)
        })
        .await
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
        self.blocking_tool("collection_update", move |store| {
            let entries = params.entries.map(Self::collection_entries);
            let description = params.description.as_deref().map(|value| {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            });
            let detail = store
                .collections()
                .update_collection(
                    params.collection_id.trim(),
                    params.name.as_deref(),
                    description,
                    params.metadata_json.as_deref(),
                    entries.as_deref(),
                )
                .map_err(mcp_error)?;
            store.ops_log().record(
                &OpsLogEntry::new(
                    "collection_update",
                    "COLLECTION",
                    detail.collection.name.clone(),
                )
                .entity(Some(detail.collection.id.clone()), "collection")
                .payload(serde_json::json!({"entry_count": detail.entries.len()}).to_string()),
            );
            Self::json_result(&detail)
        })
        .await
    }
}
