//! `RagServer` core: shared state, application-service adapters and the
//! `ServerHandler` dispatch (surface + role checks, audit, telemetry).
//!
//! Tool methods live in the sibling domain modules, one `#[tool_router]`
//! each; `server.rs` composes them into the public router.

use std::sync::Arc;

use rmcp::handler::server::tool::{ToolCallContext, ToolRouter};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde::Serialize;

use super::surface::{self, ToolSurface, INDEX_FIRST_PLAYBOOK, SPINE_TOOLS_BLURB};

use super::tools::{CollectionEntryParams, SyncSourcesParams, UpdateDocumentMetaParams};
use crate::config::Config;
use crate::db::store::CorpusMutationGuard;
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::ingest::{
    IngestCommand, IngestService, ReembedDocumentResult, UpdateDocumentCommand,
    UpdateDocumentResult,
};
use crate::llm::ChatClient;
use crate::models::{CollectionEntry, DocumentMetaUpdate, IngestResult, LlmStatusReport};
use crate::util::non_blank;

/// RAG MCP server holding store, embedder, optional chat client, and config.
#[derive(Clone)]
pub struct RagServer {
    pub(super) store: Arc<Store>,
    pub(super) embedder: Arc<dyn EmbeddingProvider>,
    /// OpenAI-compatible chat client for wiki compile / maintenance (Ollama default).
    pub(super) llm: Option<ChatClient>,
    pub(super) config: Config,
    tool_router: ToolRouter<Self>,
    /// Transport label recorded in the call log (`stdio` | `http-mcp`).
    transport: &'static str,
}

pub(super) struct FtsFinalizationOutcome {
    pub(super) guard: Option<CorpusMutationGuard>,
    pub(super) error: Option<String>,
}

#[cfg(test)]
const TEST_FINALIZE_FTS_FAILURE_STEMMER: &str = "__test_fail_facade_finalize_fts__";

/// Keep the exclusive corpus lane owned by detached blocking work.
///
/// Dropping the async waiter detaches `spawn_blocking`; moving the guard into
/// the closure prevents a new writer/search from entering while that work is
/// still using DuckDB. A normal waiter receives the guard back so it can keep
/// the lane through terminal response construction.
pub(super) async fn retain_mutation_guard_while_blocking<T, F>(
    mutation_guard: CorpusMutationGuard,
    work: F,
) -> std::result::Result<(CorpusMutationGuard, T), tokio::task::JoinError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(move || (mutation_guard, work())).await
}

fn ensure_fts_for_finalization(store: &Store, stemmer: &str) -> Result<(), AppError> {
    #[cfg(test)]
    if stemmer == TEST_FINALIZE_FTS_FAILURE_STEMMER {
        return Err(AppError::fts("injected facade FTS finalization failure"));
    }

    store.ensure_fts(stemmer)?;
    Ok(())
}

impl RagServer {
    fn request_role(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<crate::access::AccessRole, McpError> {
        if self.transport != "http-mcp" {
            return Ok(crate::access::AccessRole::Admin);
        }
        context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<crate::access::AccessRole>())
            .copied()
            .ok_or_else(|| {
                McpError::invalid_params(
                    "authenticated HTTP request required",
                    Some(serde_json::json!({"code":"AUTH_REQUIRED"})),
                )
            })
    }

    pub(crate) fn tool_count(&self) -> usize {
        self.tool_router.map.len()
    }

    /// Label the transport this server instance serves (call-log attribution).
    pub fn with_transport(mut self, transport: &'static str) -> Self {
        self.transport = transport;
        self
    }
    /// Start optional incremental source synchronization. Disabled unless
    /// `RAG_AUTO_SYNC_ROOTS` contains one or more `;`-separated directories.
    pub fn spawn_auto_sync(self) {
        let Ok(raw_roots) = std::env::var("RAG_AUTO_SYNC_ROOTS") else {
            return;
        };
        let roots: Vec<String> = raw_roots
            .split(';')
            .map(str::trim)
            .filter(|root| !root.is_empty())
            .map(str::to_string)
            .collect();
        if roots.is_empty() {
            return;
        }
        let interval = std::env::var("RAG_AUTO_SYNC_INTERVAL_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(3600);
        let interval_duration = std::time::Duration::from_secs(interval);
        crate::ops::configure_autosync(interval_duration);
        tokio::spawn(async move {
            loop {
                crate::ops::mark_autosync_running();
                let mut cycle_error = None;
                for path in &roots {
                    let params = SyncSourcesParams {
                        path: path.clone(),
                        remove_deleted: Some(false),
                        wing: None,
                        room: None,
                        max_file_bytes: None,
                    };
                    match self.sync_sources(Parameters(params)).await {
                        Ok(result) if tool_result_succeeded(&result) => {
                            tracing::info!(path, "automatic source sync completed")
                        }
                        Ok(_) => {
                            let error = "automatic source sync completed with errors".to_string();
                            cycle_error = Some(error.clone());
                            tracing::error!(path, error, "automatic source sync failed")
                        }
                        Err(error) => {
                            cycle_error = Some(error.to_string());
                            tracing::error!(path, error = %error, "automatic source sync failed")
                        }
                    }
                }
                if let Some(error) = cycle_error {
                    crate::ops::mark_autosync_error(&error, interval_duration);
                } else {
                    crate::ops::mark_autosync_success(interval_duration);
                }
                tokio::time::sleep(interval_duration).await;
            }
        });
    }

    pub(super) fn collection_entries(params: Vec<CollectionEntryParams>) -> Vec<CollectionEntry> {
        params
            .into_iter()
            .enumerate()
            .map(|(position, entry)| CollectionEntry {
                document_id: entry.document_id.trim().to_string(),
                position: position as i32,
                parent_document_id: non_blank(entry.parent_document_id),
                depends_on: entry
                    .depends_on
                    .into_iter()
                    .map(|id| id.trim().to_string())
                    .collect(),
            })
            .collect()
    }

    /// Build a server with an initialized tool router.
    ///
    /// Records `embedding_manifest` when the store has none (server start path).
    /// Builds [`ChatClient`] from `RAG_LLM_*` when possible (warns and continues on failure).
    pub fn new(store: Store, embedder: Arc<dyn EmbeddingProvider>, config: Config) -> Self {
        if let Err(e) = store.ensure_embedding_manifest(&config) {
            tracing::warn!(error = %e, "failed to ensure embedding_manifest on start");
        }
        let llm = match ChatClient::from_config(&config) {
            Ok(client) => Some(client),
            Err(e) => {
                tracing::warn!(error = %e, "failed to build ChatClient; LLM tools unavailable");
                None
            }
        };
        let server = Self {
            store: Arc::new(store),
            embedder,
            llm,
            config,
            tool_router: Self::all_tools_router(),
            transport: "stdio",
        };
        let advertised_tool_count = match server.config.tool_surface {
            ToolSurface::Spine => crate::mcp::surface::spine_tool_names().len(),
            ToolSurface::Full => server.tool_router.map.len(),
        };
        tracing::info!(
            tool_surface = server.config.tool_surface.as_str(),
            advertised_tool_count,
            registered_tool_count = server.tool_router.map.len(),
            "MCP tool surface initialized"
        );
        server
    }

    /// Borrow the wired chat client, or refuse when missing / disabled.
    pub(super) fn require_llm(&self) -> Result<&ChatClient, AppError> {
        if !self.config.llm_enabled {
            return Err(AppError::llm(
                "LLM is disabled (RAG_LLM_ENABLED=false); enable it for chat tools",
            ));
        }
        self.llm.as_ref().ok_or_else(|| {
            AppError::llm("ChatClient is not configured; check RAG_LLM_BASE_URL and RAG_LLM_MODEL")
        })
    }

    /// Ensure corpus manifest exists; refuse when its full identity differs.
    pub(super) fn require_vec_compatible(&self) -> Result<(), AppError> {
        self.store.ensure_embedding_manifest(&self.config)?;
        self.store.require_embedding_manifest_match(&self.config)
    }

    pub(super) fn json_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
        let content = Content::json(value)?;
        Ok(CallToolResult::success(vec![content]))
    }

    /// Ingest text: upsert-by-uri, chunk, embed, store chunks.
    ///
    /// When `command.immutable` is true (raw layer policy): existing uri with the same
    /// content hash is a no-op; different content is refused (`Conflict`).
    pub(super) async fn ingest_pipeline(
        &self,
        command: IngestCommand,
    ) -> Result<IngestResult, AppError> {
        IngestService::new(&self.store, &self.embedder, &self.config)
            .ingest(command)
            .await
    }

    /// Refresh the corpus-wide lexical index without blocking a Tokio worker.
    ///
    /// The guard moves into the blocking task so cancellation of the request
    /// waiter cannot release the corpus lane before detached DuckDB work ends.
    pub(super) async fn refresh_fts_after_mutation(
        &self,
        operation: &'static str,
        mutation_guard: CorpusMutationGuard,
    ) -> FtsFinalizationOutcome {
        let store = self.store.clone();
        let stemmer = self.config.fts_stemmer.clone();
        match retain_mutation_guard_while_blocking(mutation_guard, move || {
            let refresh_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ensure_fts_for_finalization(&store, &stemmer)
            }))
            .unwrap_or_else(|_| Err(AppError::fts("corpus FTS finalization panicked")));
            refresh_result.map_err(|error| {
                store
                    .record_fts_finalization_failure(operation, true, &error.to_string())
                    .message
            })
        })
        .await
        {
            Ok((guard, Ok(_))) => FtsFinalizationOutcome {
                guard: Some(guard),
                error: None,
            },
            Ok((guard, Err(error))) => FtsFinalizationOutcome {
                guard: Some(guard),
                error: Some(error),
            },
            Err(error) => FtsFinalizationOutcome {
                guard: None,
                error: Some(
                    self.store
                        .record_fts_finalization_failure(
                            operation,
                            true,
                            &format!("FTS refresh task failed: {error}"),
                        )
                        .message,
                ),
            },
        }
    }

    /// Probe local chat LLM and report embedding config (no corpus mutation).
    pub(super) async fn llm_status_report(&self) -> Result<LlmStatusReport, AppError> {
        Ok(crate::diagnostics::llm_status(&self.config, self.llm.as_ref()).await)
    }

    /// Refresh one document only when the live embedding identity already
    /// matches the corpus manifest. Identity migration is corpus-wide.
    pub(super) async fn reembed_document_pipeline(
        &self,
        document_id: &str,
    ) -> Result<ReembedDocumentResult, AppError> {
        IngestService::new(&self.store, &self.embedder, &self.config)
            .reembed_document(document_id)
            .await
    }

    /// Apply document meta update; re-chunk + re-embed only when body text changes.
    pub(super) async fn update_document_via_service(
        &self,
        params: UpdateDocumentMetaParams,
    ) -> Result<UpdateDocumentResult, AppError> {
        IngestService::new(&self.store, &self.embedder, &self.config)
            .update_document(UpdateDocumentCommand {
                document_id: params.document_id,
                update: DocumentMetaUpdate {
                    wing: params.wing,
                    room: params.room,
                    status: params.status,
                    layer: params.layer,
                    kind: params.kind,
                    source_file: params.source_file,
                    title: params.title,
                    metadata_json: params.metadata_json,
                    pinned: params.pinned,
                    boost: params.boost,
                    content: params.content,
                },
            })
            .await
    }
}

impl ServerHandler for RagServer {
    fn get_info(&self) -> ServerInfo {
        let surface = self.config.tool_surface;
        let mut instructions = String::from(
            "rag-mcp: local compounding agent memory (DuckDB). \
             Raw is immutable; client agent compiles wiki; hybrid search is the escape hatch.\n\n",
        );
        instructions.push_str(INDEX_FIRST_PLAYBOOK);
        instructions.push('\n');
        instructions.push_str(&format!(
            "Tool surface: RAG_TOOLS={} ({})\n",
            surface.as_str(),
            match surface {
                ToolSurface::Spine => "default — compile-first spine only",
                ToolSurface::Full => "all tools including MemPalace/maintain",
            }
        ));
        instructions.push_str(SPINE_TOOLS_BLURB);
        if matches!(surface, ToolSurface::Full) {
            instructions.push_str(
                "\nFull surface also includes: add_drawer, check_duplicate, delete_by_source, \
                 list_wings/rooms, get_taxonomy, kg_*, tunnels, diary/wake_up/checkpoint, \
                 maintain_*, graph_expand_search, get_graph, graph_stats, compile_source, \
                 consolidate, analyze_corpus, vacuum_store, reembed_document, llm_status, etc.\n",
            );
        }
        instructions.push_str(
            "\nSafety: ingest_file requires RAG_INGEST_ROOTS; vec/hybrid refuse a mismatched embedding identity. \
             Logs only on stderr. [[wikilinks]] and #tags extract to the object graph.\n",
        );

        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(instructions)
            .with_server_info(
                Implementation::new("rag-mcp", env!("CARGO_PKG_VERSION"))
                    .with_title("RAG MCP Server"),
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let role = self.request_role(&context)?;
        let surface = self.config.tool_surface;
        let tools = self
            .tool_router
            .list_all()
            .into_iter()
            .filter(|t| surface::tool_allowed(surface, t.name.as_ref()))
            .filter(|t| role.allows_tool(t.name.as_ref()))
            .collect();
        Ok(ListToolsResult {
            tools,
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let name = request.name.to_string();
        let role = self.request_role(&context)?;
        let agent = context
            .peer
            .peer_info()
            .map(|info| info.client_info.name.clone())
            .unwrap_or_else(|| "unknown".to_string());
        if !self.tool_router.map.contains_key(name.as_str()) {
            record_rejected_tool_call(
                &agent,
                self.transport,
                unknown_tool_audit_action(&name),
                404,
                "unknown tool",
            );
            return Err(McpError::invalid_params("unknown tool", None));
        }
        if !role.allows_tool(&name) {
            return Err(McpError::invalid_params(
                "credential does not permit this tool",
                Some(serde_json::json!({"code":"FORBIDDEN"})),
            ));
        }
        if !surface::tool_allowed(self.config.tool_surface, &name) {
            // `name` is known to the fixed tool router at this point, so keeping
            // it preserves safe operational lineage without copying arguments.
            let audit_action = mcp_audit_action(&name, request.arguments.as_ref());
            record_rejected_tool_call(
                &agent,
                self.transport,
                &audit_action,
                403,
                "tool forbidden by configured surface",
            );
            return Err(McpError::invalid_params(
                format!(
                    "tool '{name}' is not in RAG_TOOLS=spine; set RAG_TOOLS=full to enable advanced tools"
                ),
                None,
            ));
        }
        let audit_action = mcp_audit_action(&name, request.arguments.as_ref());
        // Tool arguments can contain queries, document bodies, paths and project names.
        // Both telemetry surfaces are remotely readable when the gateway is enabled,
        // so record only the tool lineage here.
        let call = crate::telemetry::begin(&agent, self.transport, &name, None);
        let started = std::time::Instant::now();
        let tcc = ToolCallContext::new(self, request, context);
        let result = self.tool_router.call(tcc).await;
        let operation_ok = match &result {
            Ok(value) => tool_result_succeeded(value),
            Err(_) => false,
        };
        crate::http_api::record_mcp_tool(
            &audit_action,
            if operation_ok { 200 } else { 500 },
            started.elapsed().as_secs_f64() * 1000.0,
        );
        match &result {
            Ok(value) => call.finish(
                operation_ok,
                (!operation_ok).then(|| "tool reported failure".to_string()),
                result_hint(value),
            ),
            // Error strings may echo paths or other user input. The transport-level
            // outcome is sufficient for the bounded operational call log.
            Err(_) => call.finish(false, Some("tool call failed".to_string()), None),
        }
        result
    }
}

fn record_rejected_tool_call(
    agent: &str,
    transport: &str,
    audit_action: &str,
    status: u16,
    error: &str,
) {
    let call = crate::telemetry::begin(agent, transport, audit_action, None);
    crate::http_api::record_mcp_tool(audit_action, status, 0.0);
    call.finish(false, Some(error.to_string()), None);
}

fn unknown_tool_audit_action(_requested_name: &str) -> &'static str {
    "unknown_tool"
}

fn mcp_audit_action(
    name: &str,
    _arguments: Option<&serde_json::Map<String, serde_json::Value>>,
) -> String {
    // `/v1/activity` can be reachable on an explicitly remote-bound gateway.
    // Never copy one MCP client's arguments into a cross-client activity feed.
    name.to_string()
}

#[cfg(test)]
mod audit_tests {
    use super::{
        mcp_audit_action, record_rejected_tool_call, result_hint, tool_result_succeeded,
        unknown_tool_audit_action,
    };
    use rmcp::model::{CallToolResult, Content};

    #[test]
    fn audit_action_keeps_lineage_and_omits_content() {
        let arguments = serde_json::json!({
            "path": "/docs/source.pdf",
            "title": "Source article",
            "content": "private body",
            "query": "private search",
            "document_id": "private-document",
            "wing": "secret-project"
        });
        let action = mcp_audit_action("ingest_file", arguments.as_object());
        assert_eq!(action, "ingest_file");
        assert!(!action.contains("/docs/source.pdf"));
        assert!(!action.contains("Source article"));
        assert!(!action.contains("private body"));
        assert!(!action.contains("private search"));
        assert!(!action.contains("private-document"));
        assert!(!action.contains("secret-project"));
    }

    #[test]
    fn known_disabled_tool_records_sanitized_failed_lineage_and_metrics() {
        let _telemetry_guard = crate::telemetry::TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _activity_guard = crate::http_api::ACTIVITY_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let agent = format!("disabled-tool-test-{}", uuid::Uuid::new_v4());
        let tool = "maintain_compress";
        assert!(!crate::mcp::surface::tool_allowed(
            crate::mcp::ToolSurface::Spine,
            tool
        ));

        record_rejected_tool_call(
            &agent,
            "stdio",
            tool,
            403,
            "tool forbidden by configured surface: private-project",
        );

        let calls = crate::telemetry::recent(1, Some(&agent), Some(tool));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, tool);
        assert_eq!(calls[0].group, "maintain");
        assert!(!calls[0].ok);
        assert_eq!(calls[0].error.as_deref(), Some("request forbidden"));
        assert!(!calls[0].args.contains("private-project"));

        let metric = crate::telemetry::summary()
            .by_tool
            .into_iter()
            .find(|metric| metric.tool == tool)
            .expect("disabled tool included in telemetry metrics");
        assert!(metric.count >= 1);
        assert!(metric.errors >= 1);

        let activity = crate::http_api::latest_mcp_activity_for_test(tool)
            .expect("disabled tool included in activity feed");
        assert_eq!(activity.status, Some(403));
        assert_eq!(activity.action, tool);
        assert!(!activity.action.contains("private-project"));
    }

    #[test]
    fn unknown_tool_audit_lineage_never_retains_the_requested_name() {
        let requested_name = "private-project-secret-tool";
        let action = unknown_tool_audit_action(requested_name);
        assert_eq!(action, "unknown_tool");
        assert!(!action.contains(requested_name));
    }

    #[test]
    fn result_hint_exposes_only_bounded_counts() {
        let result = CallToolResult::success(vec![Content::json(serde_json::json!({
            "count": 7,
            "query": "private search",
            "path": "/private/source.md"
        }))
        .unwrap()]);
        assert_eq!(result_hint(&result).as_deref(), Some("count 7"));
    }

    #[test]
    fn aggregate_report_failures_are_not_telemetry_successes() {
        let explicit_failure = CallToolResult::success(vec![Content::json(
            serde_json::json!({"success": false, "errors": []}),
        )
        .unwrap()]);
        assert!(!tool_result_succeeded(&explicit_failure));

        let additive_error = CallToolResult::success(vec![Content::json(
            serde_json::json!({"errors": ["FTS_FINALIZATION_FAILED"]}),
        )
        .unwrap()]);
        assert!(!tool_result_succeeded(&additive_error));

        let failed_documents = CallToolResult::success(vec![Content::json(
            serde_json::json!({"documents_failed": [{"path": "redacted"}]}),
        )
        .unwrap()]);
        assert!(!tool_result_succeeded(&failed_documents));

        let nested_partial_reembed =
            CallToolResult::success(vec![Content::json(serde_json::json!({
                "errors": [],
                "reembed_all": {"documents_failed": 1, "skipped_cap": 0}
            }))
            .unwrap()]);
        assert!(!tool_result_succeeded(&nested_partial_reembed));

        let organize_partial = CallToolResult::success(vec![Content::json(
            serde_json::json!({"applied_ok": 1, "applied_failed": 1}),
        )
        .unwrap()]);
        assert!(!tool_result_succeeded(&organize_partial));

        let graph_partial = CallToolResult::success(vec![Content::json(
            serde_json::json!({"rebuild_graph": {"failed": 1}}),
        )
        .unwrap()]);
        assert!(!tool_result_succeeded(&graph_partial));

        let successful = CallToolResult::success(vec![Content::json(
            serde_json::json!({"success": true, "errors": []}),
        )
        .unwrap()]);
        assert!(tool_result_succeeded(&successful));
    }
}

/// Aggregate tool reports remain JSON-RPC successes so callers can inspect
/// committed counters, but their explicit failure fields must still drive
/// operational telemetry and activity status.
fn tool_result_succeeded(result: &CallToolResult) -> bool {
    if result.is_error.unwrap_or(false) {
        return false;
    }
    let Some(text) = result.content.first().and_then(|content| content.as_text()) else {
        return true;
    };
    let Ok(serde_json::Value::Object(report)) = serde_json::from_str(&text.text) else {
        return true;
    };
    if report.get("success").and_then(serde_json::Value::as_bool) == Some(false) {
        return false;
    }
    if ["errors", "documents_failed"]
        .iter()
        .filter_map(|key| report.get(*key))
        .any(|value| value.as_array().is_some_and(|items| !items.is_empty()))
    {
        return false;
    }
    if [
        "error_count",
        "failed",
        "documents_failed",
        "applied_failed",
    ]
    .iter()
    .filter_map(|key| report.get(*key))
    .any(|value| value.as_u64().is_some_and(|count| count > 0))
    {
        return false;
    }
    if report
        .get("reembed_all")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|reembed| {
            ["documents_failed", "skipped_cap"]
                .iter()
                .filter_map(|key| reembed.get(*key))
                .any(|value| value.as_u64().is_some_and(|count| count > 0))
        })
    {
        return false;
    }
    if report
        .get("rebuild_graph")
        .and_then(serde_json::Value::as_object)
        .and_then(|graph| graph.get("failed"))
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|failed| failed > 0)
    {
        return false;
    }
    true
}

/// Short human hint for the call log, derived from the first JSON content item.
fn result_hint(result: &CallToolResult) -> Option<String> {
    let text = result.content.first()?.as_text()?.text.as_str();
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    match value {
        serde_json::Value::Array(items) => Some(format!("{} items", items.len())),
        serde_json::Value::Object(map) => ["count", "chunk_count", "hits", "total"]
            .iter()
            .find_map(|key| {
                map.get(*key)
                    .and_then(|v| v.as_u64())
                    .map(|n| format!("{key} {n}"))
            }),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
