//! Temporal knowledge-graph tools.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::facade::RagServer;
use super::tools::{
    KgAddParams, KgInvalidateParams, KgQueryParams, KgSupersedeParams, KgTimelineParams,
};
use crate::util::parse_optional_timestamp;

#[tool_router(router = kg_router, vis = "pub(super)")]
impl RagServer {
    #[tool(
        name = "kg_add",
        description = "Add a temporal KG fact (subject, predicate, object). Idempotent for open active SPO. Optional valid_from/valid_to (half-open), source_document_id, confidence, metadata_json."
    )]
    pub(super) async fn kg_add(
        &self,
        Parameters(params): Parameters<KgAddParams>,
    ) -> Result<CallToolResult, McpError> {
        let valid_from = parse_optional_timestamp(params.valid_from.as_deref())?;
        let valid_to = parse_optional_timestamp(params.valid_to.as_deref())?;
        let fact = self.store.kg_add(
            &params.subject,
            &params.predicate,
            &params.object,
            valid_from,
            valid_to,
            params.source_document_id.as_deref(),
            params.confidence,
            params.metadata_json.as_deref(),
        )?;
        Self::json_result(&fact)
    }

    #[tool(
        name = "kg_query",
        description = "Query temporal KG facts by optional subject/predicate/object. Without at_time: active only. With at_time: half-open validity window filter."
    )]
    pub(super) async fn kg_query(
        &self,
        Parameters(params): Parameters<KgQueryParams>,
    ) -> Result<CallToolResult, McpError> {
        let at_time = parse_optional_timestamp(params.at_time.as_deref())?;
        let facts = self.store.kg_query(
            params.subject.as_deref(),
            params.predicate.as_deref(),
            params.object.as_deref(),
            at_time,
        )?;
        Self::json_result(&facts)
    }

    #[tool(
        name = "kg_invalidate",
        description = "Invalidate open active KG fact(s) matching subject+predicate+object. Sets valid_to=ended (default now), status=invalidated."
    )]
    pub(super) async fn kg_invalidate(
        &self,
        Parameters(params): Parameters<KgInvalidateParams>,
    ) -> Result<CallToolResult, McpError> {
        let ended = parse_optional_timestamp(params.ended.as_deref())?;
        let facts =
            self.store
                .kg_invalidate(&params.subject, &params.predicate, &params.object, ended)?;
        Self::json_result(&facts)
    }

    #[tool(
        name = "kg_supersede",
        description = "Supersede a KG fact object: close (subject,predicate,old_object) at boundary `at` and open (subject,predicate,new_object). Returns the successor fact."
    )]
    pub(super) async fn kg_supersede(
        &self,
        Parameters(params): Parameters<KgSupersedeParams>,
    ) -> Result<CallToolResult, McpError> {
        let at = parse_optional_timestamp(params.at.as_deref())?;
        let fact = self.store.kg_supersede(
            &params.subject,
            &params.predicate,
            &params.old_object,
            &params.new_object,
            at,
            params.source_document_id.as_deref(),
            params.confidence,
        )?;
        Self::json_result(&fact)
    }

    #[tool(
        name = "kg_timeline",
        description = "Chronological KG facts for a subject (any status), ordered by valid_from."
    )]
    pub(super) async fn kg_timeline(
        &self,
        Parameters(params): Parameters<KgTimelineParams>,
    ) -> Result<CallToolResult, McpError> {
        let facts = self.store.kg_timeline(&params.subject)?;
        Self::json_result(&facts)
    }

    #[tool(
        name = "kg_stats",
        description = "Temporal KG aggregates: total/active/invalidated/superseded facts, distinct subjects/predicates, relationship_types."
    )]
    pub(super) async fn kg_stats(&self) -> Result<CallToolResult, McpError> {
        let body = self.store.kg_stats()?;
        Self::json_result(&body)
    }
}
