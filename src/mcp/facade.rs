//! MCP tool implementations and `ServerHandler` wiring.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, NaiveDateTime, Utc};
use rmcp::handler::server::tool::{ToolCallContext, ToolRouter};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, ErrorData as McpError, RoleServer, ServerHandler};
use serde::Serialize;
use uuid::Uuid;

use super::surface::{self, ToolSurface, INDEX_FIRST_PLAYBOOK, SPINE_TOOLS_BLURB};

use super::tools::{
    AddDrawerParams, AnalyzeCorpusParams, AppendLogParams, ApplyMaintenancePlanParams,
    ArchiveMemoryItemsParams, BackupDbParams, CheckDuplicateParams, CheckpointParams,
    CleanupSourceDuplicatesParams, CollectionCreateParams, CollectionEntryParams,
    CollectionGetParams, CollectionListParams, CollectionUpdateParams, CompileSourceParams,
    ConsolidateMemoryItemsParams, ConsolidateParams, CreateTunnelParams, DeleteBySourceParams,
    DeleteDocumentParams, DeleteTunnelParams, DiaryReadParams, DiaryWriteParams,
    DoctorRepairParams, ExpandChunksParams, ExportBundleParams, ExportGraphSnapshotParams,
    ExportVaultParams, FileAnswerCitationParams, FileAnswerParams, FindNodeParams,
    FindSimilarParams, FindTunnelsParams, FollowTunnelsParams, GetBacklinksParams,
    GetDocumentParams, GetGraphParams, GetNeighborsParams, GetSchemaParams, GetSourceParams,
    GetTaxonomyParams, GetWikiPageParams, GraphExpandSearchParams, ImportBundleParams,
    IngestFileParams, IngestRawParams, IngestTextParams, KgAddParams, KgInvalidateParams,
    KgQueryParams, KgSupersedeParams, KgTimelineParams, LinkNodesParams, ListDocumentsParams,
    ListMemoryLifecycleCandidatesParams, ListRecentOpsParams, ListRoomsParams, ListSourcesParams,
    ListTunnelsParams, ListWikiPagesParams, ListWingsParams, MaintainCompressParams,
    MaintainOrganizeParams, MaintainRefreshParams, MemoriesFiledAwayParams, MultiGetParams,
    MultiQuerySearchParams, PackContextParams, PackHitParams, PlanMaintenanceParams,
    QueryWithIndexParams, ReadIndexParams, ReadLogParams, RebuildIndexParams, ReconnectParams,
    ReembedDocumentParams, RefreshStaleWikiParams, SearchParams, SearchWikiParams,
    SyncSourcesParams, UpdateDocumentMetaParams, UpdateIndexEntryParams, UpdateSchemaParams,
    WakeUpParams, WriteWikiPageParams,
};
use crate::config::Config;
#[cfg(test)]
use crate::db::recovery::BundleDocument;
use crate::db::recovery::{
    decode_recovery_bundle, encode_recovery_bundle, preflight_recovery_bundle,
    preflight_recovery_bundle_reembed, publish_recovery_artifact, read_recovery_bundle_file,
    BundleExportReport, ConflictPolicy, RecoveryBundle, BUNDLE_VERSION,
};
#[cfg(test)]
use crate::db::schema::SCHEMA_VERSION;
use crate::db::search::{
    acquire_corpus_search_guard, attach_context, fuse_rrf_many, search_with_corpus_guard,
    ContextExpansion, DiversityMode, SearchQuery,
};
use crate::db::store::{embedding_manifest_from_config, CorpusMutationGuard, WikiPageMetaFilter};
use crate::db::{DocumentCatalogFilter, Store, DEFAULT_CATALOG_PAGE_SIZE, MAX_CATALOG_PAGE_SIZE};
use crate::diagnostics::DiagnosticsService;
use crate::diary;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::ingest::{
    IngestCommand, IngestFileCommand, IngestService, ReembedDocumentResult, UpdateDocumentCommand,
    UpdateDocumentResult,
};
use crate::llm::ChatClient;
use crate::maintain::{
    self, ApplyPlanOptions, CompressOptions, MaintainRefreshFlags, MaintenancePlanItem,
};
use crate::memory_lifecycle;
#[cfg(test)]
use crate::models::{Chunk, DoctorReport, StatusReport};
use crate::models::{
    Collection, CollectionEntry, Document, DocumentFilter, DocumentMetaUpdate, GraphEdge,
    GraphFilter, GraphNode, GraphView, IndexQueryPage, IndexQueryResult, IngestResult,
    LlmStatusReport, OpsLogEntry, SearchHit, SearchMode, Stats, VacuumStoreReport, WikiIndexEntry,
};
use crate::retrieval::SearchCommand;
use crate::retrieval::{self, DocumentWithChunks, SimilarDocumentsQuery};
use crate::search_pack::pack_hits;
use crate::source_sync::{
    sync_sources_nonblocking, SourceSyncCommand, SourceSyncControl, SourceSyncOutcome,
};
#[cfg(test)]
use crate::util::{check_path_allowlist, content_hash};
use crate::util::{
    refuse_live_database_target, resolve_allowlisted_output_file, validate_backup_output_paths,
};
use crate::wiki;
use crate::wiki::FileAnswerCitation;

/// Empty / whitespace-only optional strings become `None` (ingest wing/room/source_file).
fn nonempty_opt(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

fn clamp_maintenance_max_docs(configured: usize, requested: Option<usize>) -> usize {
    let configured = configured.max(1);
    requested.unwrap_or(configured).max(1).min(configured)
}

/// RAG MCP server holding store, embedder, optional chat client, and config.
#[derive(Clone)]
pub struct RagServer {
    store: Arc<Store>,
    embedder: Arc<dyn EmbeddingProvider>,
    /// OpenAI-compatible chat client for wiki compile / maintenance (Ollama default).
    llm: Option<ChatClient>,
    config: Config,
    tool_router: ToolRouter<Self>,
    /// Transport label recorded in the call log (`stdio` | `http-mcp`).
    transport: &'static str,
}

struct FtsFinalizationOutcome {
    guard: Option<CorpusMutationGuard>,
    error: Option<String>,
}

#[cfg(test)]
const TEST_FINALIZE_FTS_FAILURE_STEMMER: &str = "__test_fail_facade_finalize_fts__";

const LEGACY_RECOVERY_BUNDLE_VERSION: u32 = 1;
const LEGACY_RECOVERY_REEMBED_BATCH_SIZE: usize = 64;

struct PreparedRecoveryBundle {
    bundle: RecoveryBundle,
    legacy_bundle_version: Option<u32>,
    embeddings_reembed_planned: u64,
    embeddings_reembedded: u64,
}

/// Keep the exclusive corpus lane owned by detached blocking work.
///
/// Dropping the async waiter detaches `spawn_blocking`; moving the guard into
/// the closure prevents a new writer/search from entering while that work is
/// still using DuckDB. A normal waiter receives the guard back so it can keep
/// the lane through terminal response construction.
async fn retain_mutation_guard_while_blocking<T, F>(
    mutation_guard: CorpusMutationGuard,
    work: F,
) -> std::result::Result<(CorpusMutationGuard, T), tokio::task::JoinError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(move || (mutation_guard, work())).await
}

async fn run_blocking<T, F>(operation: &'static str, work: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| AppError::db(format!("{operation} task failed: {error}")))?
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

    fn collection_entries(params: Vec<CollectionEntryParams>) -> Vec<CollectionEntry> {
        params
            .into_iter()
            .enumerate()
            .map(|(position, entry)| CollectionEntry {
                document_id: entry.document_id.trim().to_string(),
                position: position as i32,
                parent_document_id: nonempty_opt(entry.parent_document_id),
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
            tool_router: super::server::compose_tool_router(Self::all_tools_router()),
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

    fn map_err(err: AppError) -> McpError {
        match err {
            AppError::NotFound(msg) => McpError::resource_not_found(msg, None),
            AppError::Config(msg) => McpError::invalid_params(msg, None),
            AppError::Chunking(msg) => McpError::invalid_params(msg, None),
            AppError::Embeddings(msg) => McpError::invalid_params(msg, None),
            AppError::Llm(msg) => McpError::internal_error(msg, None),
            AppError::Conflict(msg) => McpError::invalid_params(msg, None),
            AppError::Forbidden(msg) => McpError::invalid_params(msg, None),
            AppError::Busy(msg) => McpError::internal_error(
                msg,
                Some(serde_json::json!({
                    "code": "STORE_BUSY",
                    "retryable": true,
                    "retry_after_ms": 1_000,
                })),
            ),
            other => McpError::internal_error(other.to_string(), None),
        }
    }

    /// Borrow the wired chat client, or refuse when missing / disabled.
    #[allow(dead_code)]
    fn require_llm(&self) -> Result<&ChatClient, AppError> {
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
    fn require_vec_compatible(&self) -> Result<(), AppError> {
        self.store.ensure_embedding_manifest(&self.config)?;
        self.store.require_embedding_manifest_match(&self.config)
    }

    fn json_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
        let content = Content::json(value)?;
        Ok(CallToolResult::success(vec![content]))
    }

    /// Ingest text: upsert-by-uri, chunk, embed, store chunks.
    ///
    /// When `command.immutable` is true (raw layer policy): existing uri with the same
    /// content hash is a no-op; different content is refused (`Conflict`).
    async fn ingest_pipeline(&self, command: IngestCommand) -> Result<IngestResult, AppError> {
        IngestService::new(&self.store, &self.embedder, &self.config)
            .ingest(command)
            .await
    }

    /// Refresh the corpus-wide lexical index without blocking a Tokio worker.
    ///
    /// The guard moves into the blocking task so cancellation of the request
    /// waiter cannot release the corpus lane before detached DuckDB work ends.
    async fn refresh_fts_after_mutation(
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

    /// Build the MemPalace-style `status` health payload (vision §5.5 layer health).
    #[cfg(test)]
    pub(crate) fn status_report(&self) -> Result<StatusReport, AppError> {
        DiagnosticsService::new(&self.store, &self.config).status()
    }

    /// Build the minimal `doctor` integrity payload.
    #[cfg(test)]
    pub(crate) fn doctor_report(&self) -> Result<DoctorReport, AppError> {
        DiagnosticsService::new(&self.store, &self.config).doctor()
    }

    /// Probe local chat LLM and report embedding config (no corpus mutation).
    async fn llm_status_report(&self) -> Result<LlmStatusReport, AppError> {
        Ok(crate::diagnostics::llm_status(&self.config, self.llm.as_ref()).await)
    }

    /// Refresh one document only when the live embedding identity already
    /// matches the corpus manifest. Identity migration is corpus-wide.
    async fn reembed_document_pipeline(
        &self,
        document_id: &str,
    ) -> Result<ReembedDocumentResult, AppError> {
        IngestService::new(&self.store, &self.embedder, &self.config)
            .reembed_document(document_id)
            .await
    }

    /// Apply document meta update; re-chunk + re-embed only when body text changes.
    async fn update_document_via_service(
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

#[tool_router(router = all_tools_router, vis = "pub(super)")]
impl RagServer {
    #[tool(
        name = "collection_create",
        description = "Create a durable named collection. entries array order is the reading order; each entry may name a parent_document_id for outline nesting and depends_on document ids for prerequisites. All ids must reference existing documents in the collection."
    )]
    async fn collection_create(
        &self,
        Parameters(params): Parameters<CollectionCreateParams>,
    ) -> Result<CallToolResult, McpError> {
        let now = Utc::now();
        let collection = Collection {
            id: Uuid::new_v4().to_string(),
            name: params.name.trim().to_string(),
            description: nonempty_opt(params.description),
            metadata_json: params.metadata_json.unwrap_or_else(|| "{}".into()),
            created_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
        };
        let entries = Self::collection_entries(params.entries);
        let detail = self
            .store
            .create_collection(&collection, &entries)
            .map_err(Self::map_err)?;
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
    }

    #[tool(
        name = "collection_list",
        description = "List durable collections ordered by most recently updated. Returns collection metadata without entries."
    )]
    async fn collection_list(
        &self,
        Parameters(_params): Parameters<CollectionListParams>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(&self.store.list_collections().map_err(Self::map_err)?)
    }

    #[tool(
        name = "collection_get",
        description = "Get a collection with entries in reading order, outline parent_document_id values, optional depends_on prerequisite links, and a derived dependency_order. Dependency cycles are reported in dependency_cycle_members."
    )]
    async fn collection_get(
        &self,
        Parameters(params): Parameters<CollectionGetParams>,
    ) -> Result<CallToolResult, McpError> {
        let detail = self
            .store
            .get_collection(params.collection_id.trim())
            .map_err(Self::map_err)?
            .ok_or_else(|| {
                Self::map_err(AppError::not_found(format!(
                    "collection not found: {}",
                    params.collection_id
                )))
            })?;
        Self::json_result(&detail)
    }

    #[tool(
        name = "collection_update",
        description = "Update collection metadata and optionally replace its entries. When entries is present, array order replaces reading order and parent_document_id/depends_on replace the outline and prerequisite links; omit entries to preserve membership."
    )]
    async fn collection_update(
        &self,
        Parameters(params): Parameters<CollectionUpdateParams>,
    ) -> Result<CallToolResult, McpError> {
        let entries = params.entries.map(Self::collection_entries);
        let description = params.description.as_deref().map(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });
        let detail = self
            .store
            .update_collection(
                params.collection_id.trim(),
                params.name.as_deref(),
                description,
                params.metadata_json.as_deref(),
                entries.as_deref(),
            )
            .map_err(Self::map_err)?;
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
    }

    #[tool(
        name = "ingest_text",
        description = "Ingest raw text: chunk, embed, and store. Upserts by uri when provided."
    )]
    async fn ingest_text(
        &self,
        Parameters(params): Parameters<IngestTextParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .ingest_pipeline(IngestCommand {
                text: params.text,
                title: params.title,
                uri: params.uri,
                metadata_json: params.metadata_json,
                wing: None,
                room: None,
                source_file: None,
                layer: "raw".into(),
                kind: "document".into(),
                immutable: false,
            })
            .await
            .map_err(Self::map_err)?;
        Self::json_result(&result)
    }

    #[tool(
        name = "ingest_file",
        description = "Read text, Markdown, HTML, PDF, or source code from disk and ingest it. Path must be under RAG_INGEST_ROOTS. Upserts by uri."
    )]
    async fn ingest_file(
        &self,
        Parameters(params): Parameters<IngestFileParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = IngestService::new(&self.store, &self.embedder, &self.config)
            .ingest_file(IngestFileCommand {
                path: params.path,
                title: params.title,
                uri: params.uri,
                metadata_json: params.metadata_json,
                wing: params.wing,
                room: params.room,
            })
            .await
            .map_err(Self::map_err)?;
        Self::json_result(&result)
    }

    #[tool(
        name = "sync_sources",
        description = "Recursively sync supported text, Markdown, HTML, PDF, and source-code files from an allowlisted directory. Adds or updates changed files and removes missing sources only when requested."
    )]
    async fn sync_sources(
        &self,
        Parameters(params): Parameters<SyncSourcesParams>,
    ) -> Result<CallToolResult, McpError> {
        let outcome = sync_sources_nonblocking(
            self.store.clone(),
            self.embedder.clone(),
            self.config.clone(),
            SourceSyncCommand {
                path: PathBuf::from(params.path),
                remove_deleted: params.remove_deleted.unwrap_or(false),
                wing: params.wing,
                room: params.room,
                max_file_bytes: params.max_file_bytes,
            },
            SourceSyncControl::default(),
        )
        .await
        .map_err(Self::map_err)?;
        let report = match outcome {
            SourceSyncOutcome::Completed(report) | SourceSyncOutcome::Cancelled(report) => report,
        };
        Self::json_result(&report)
    }

    #[tool(
        name = "ingest_raw",
        description = "Register an immutable raw source (layer=raw): chunk, embed, store. Same uri+content is a no-op; content change is refused."
    )]
    async fn ingest_raw(
        &self,
        Parameters(params): Parameters<IngestRawParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .ingest_pipeline(IngestCommand {
                text: params.text,
                title: params.title,
                uri: params.uri,
                metadata_json: params.metadata_json,
                wing: params.wing,
                room: params.room,
                source_file: params.source_file,
                layer: "raw".into(),
                kind: "document".into(),
                immutable: true,
            })
            .await
            .map_err(Self::map_err)?;
        Self::json_result(&result)
    }

    #[tool(
        name = "add_drawer",
        description = "File verbatim content as a drawer (document): chunk, embed, store. Requires wing and room; optional source_file/title/uri. Does not summarize on ingest."
    )]
    async fn add_drawer(
        &self,
        Parameters(params): Parameters<AddDrawerParams>,
    ) -> Result<CallToolResult, McpError> {
        let wing = params.wing.trim();
        let room = params.room.trim();
        if wing.is_empty() {
            return Err(Self::map_err(AppError::config("wing must be non-empty")));
        }
        if room.is_empty() {
            return Err(Self::map_err(AppError::config("room must be non-empty")));
        }
        if params.content.is_empty() {
            return Err(Self::map_err(AppError::config("content must be non-empty")));
        }

        let result = self
            .ingest_pipeline(IngestCommand {
                text: params.content,
                title: params.title,
                uri: params.uri,
                metadata_json: params.metadata_json,
                wing: Some(wing.to_string()),
                room: Some(room.to_string()),
                source_file: params.source_file,
                layer: "raw".into(),
                kind: "document".into(),
                immutable: false,
            })
            .await
            .map_err(Self::map_err)?;
        Self::json_result(&result)
    }

    #[tool(
        name = "check_duplicate",
        description = "Exact content-hash / uri dedupe probe before filing. Pass content and/or hash (or content_hash), and/or uri. Returns is_duplicate, content_hash, and matches."
    )]
    async fn check_duplicate(
        &self,
        Parameters(params): Parameters<CheckDuplicateParams>,
    ) -> Result<CallToolResult, McpError> {
        let body = self
            .store
            .check_duplicate(
                params.content.as_deref(),
                params.content_hash.as_deref(),
                params.uri.as_deref(),
            )
            .map_err(Self::map_err)?;
        Self::json_result(&body)
    }

    #[tool(
        name = "delete_by_source",
        description = "Bulk-delete all documents (and chunks/graph) whose source_file matches exactly. dry_run=true reports ids without deleting."
    )]
    async fn delete_by_source(
        &self,
        Parameters(params): Parameters<DeleteBySourceParams>,
    ) -> Result<CallToolResult, McpError> {
        let source_file = params.source_file.trim();
        if source_file.is_empty() {
            return Err(Self::map_err(AppError::config(
                "source_file (or source) must be non-empty",
            )));
        }
        let dry_run = params.dry_run.unwrap_or(false);
        if dry_run {
            let matched = self
                .store
                .list_documents_filtered(&DocumentFilter {
                    source_file: Some(source_file.to_string()),
                    include_archived: Some(true),
                    ..Default::default()
                })
                .map_err(Self::map_err)?;
            let document_ids: Vec<String> = matched.iter().map(|d| d.id.clone()).collect();
            return Self::json_result(&serde_json::json!({
                "success": true,
                "dry_run": true,
                "source_file": source_file,
                "match_count": document_ids.len() as u64,
                "deleted": 0,
                "document_ids": document_ids,
            }));
        }

        let mut mutation_guard = Some(
            self.store
                .try_corpus_mutation_guard("source deletion")
                .map_err(Self::map_err)?,
        );
        let document_ids = self
            .store
            .delete_by_source_ids(source_file)
            .map_err(Self::map_err)?;
        let deleted = document_ids.len() as u64;
        let match_count = deleted;
        let mut finalization_errors = Vec::new();
        if deleted > 0 {
            let guard = mutation_guard
                .take()
                .expect("non-dry source deletion owns the mutation lane");
            let outcome = self
                .refresh_fts_after_mutation("delete_by_source", guard)
                .await;
            mutation_guard = outcome.guard;
            if let Some(error) = outcome.error {
                finalization_errors.push(error);
            }
        }
        let _mutation_guard = mutation_guard;
        Self::json_result(&serde_json::json!({
            "success": finalization_errors.is_empty(),
            "dry_run": false,
            "durable_mutation_committed": deleted > 0,
            "source_file": source_file,
            "match_count": match_count,
            "deleted": deleted,
            "document_ids": document_ids,
            "errors": finalization_errors,
        }))
    }

    #[tool(
        name = "cleanup_source_duplicates",
        description = "Safely preview or remove legacy active raw source duplicates. A candidate must share the exact non-empty source_file and content_hash with one canonical file://<source_file> survivor. dry_run defaults true; apply requires dry_run=false and confirm=true. The operation is atomic, capped, rewires graph/KG/manifest references, and skips groups with wiki or collection references."
    )]
    async fn cleanup_source_duplicates(
        &self,
        Parameters(params): Parameters<CleanupSourceDuplicatesParams>,
    ) -> Result<CallToolResult, McpError> {
        let dry_run = params.dry_run.unwrap_or(true);
        let mut mutation_guard = if dry_run {
            None
        } else {
            Some(
                self.store
                    .try_corpus_mutation_guard("duplicate cleanup")
                    .map_err(Self::map_err)?,
            )
        };
        let configured_cap = self.config.maint_max_docs.max(1);
        let max_candidates = params
            .max_candidates
            .map(|value| value as usize)
            .unwrap_or(configured_cap)
            .max(1)
            .min(configured_cap);
        let mut report = self
            .store
            .cleanup_source_duplicates(dry_run, params.confirm.unwrap_or(false), max_candidates)
            .map_err(Self::map_err)?;
        if report.applied {
            // Keep the exclusive source-mutation lane until the derived lexical
            // index covers the committed deletions. Otherwise the first user
            // search after a successful cleanup pays the full BM25 rebuild.
            let guard = mutation_guard
                .take()
                .expect("applied duplicate cleanup owns the mutation lane");
            let outcome = self
                .refresh_fts_after_mutation("cleanup_source_duplicates", guard)
                .await;
            if let Some(error) = outcome.error {
                report.success = false;
                report.errors.push(error);
            }
            mutation_guard = outcome.guard;
        }
        let _mutation_guard = mutation_guard;
        Self::json_result(&report)
    }

    #[tool(
        name = "list_sources",
        description = "List immutable raw-layer sources without loading bodies. Filters: wing?, room?; limit defaults 50 (hard cap 200), offset defaults 0."
    )]
    async fn list_sources(
        &self,
        Parameters(params): Parameters<ListSourcesParams>,
    ) -> Result<CallToolResult, McpError> {
        let filter = DocumentCatalogFilter {
            wing: nonempty_opt(params.wing),
            room: nonempty_opt(params.room),
            layer: Some("raw".into()),
            include_archived: true,
            limit: params
                .limit
                .map(|value| value as usize)
                .unwrap_or(DEFAULT_CATALOG_PAGE_SIZE)
                .clamp(1, MAX_CATALOG_PAGE_SIZE),
            offset: params.offset.unwrap_or(0) as usize,
            ..DocumentCatalogFilter::default()
        };
        let store = self.store.clone();
        let docs = run_blocking("list_sources", move || store.list_document_catalog(&filter))
            .await
            .map_err(Self::map_err)?;
        let summaries: Vec<SourceSummary> = docs
            .items
            .into_iter()
            .map(|d| SourceSummary {
                id: d.id,
                title: d.title,
                uri: d.uri,
                content_hash: d.content_hash,
                wing: d.wing,
                room: d.room,
                source_file: d.source_file,
                layer: d.layer,
                kind: d.kind,
                created_at: d.created_at.to_rfc3339(),
                updated_at: d.updated_at.to_rfc3339(),
            })
            .collect();
        Self::json_result(&summaries)
    }

    #[tool(
        name = "get_source",
        description = "Fetch one raw-layer source by document_id or uri. include_chunks returns text-only chunks, default 100 and hard cap 500 via chunk_limit; one response has an 8 MiB aggregate chunk-text budget."
    )]
    async fn get_source(
        &self,
        Parameters(params): Parameters<GetSourceParams>,
    ) -> Result<CallToolResult, McpError> {
        let doc_id = params
            .document_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let uri = params
            .uri
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        if doc_id.is_none() && uri.is_none() {
            return Err(Self::map_err(AppError::config(
                "get_source requires document_id or uri",
            )));
        }
        let include_chunks = params.include_chunks.unwrap_or(false);
        let chunk_limit = params.chunk_limit.map(|value| value as usize);
        let document_id = doc_id.map(str::to_owned);
        let uri = uri.map(str::to_owned);
        let store = self.store.clone();
        let body = run_blocking("get_source", move || {
            let doc = if let Some(id) = document_id.as_deref() {
                store
                    .get_document(id)?
                    .ok_or_else(|| AppError::not_found(format!("source not found: {id}")))?
            } else if let Some(uri) = uri.as_deref() {
                store
                    .find_by_uri(uri)?
                    .ok_or_else(|| AppError::not_found(format!("source not found: uri={uri}")))?
            } else {
                unreachable!("validated source key")
            };
            if doc.layer != "raw" {
                return Err(AppError::not_found(format!(
                    "document {} is not a raw source (layer={})",
                    doc.id, doc.layer
                )));
            }
            retrieval::document_with_chunks(&store, doc, include_chunks, chunk_limit)
                .map(SourceDetail::from)
        })
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&body)
    }

    #[tool(
        name = "search",
        description = "Search stored chunks (mode=lex|vec|hybrid). Supports filters, diversity, token packing, and optional context_expansion=neighbors|parent_section. Markdown hits include heading_path/section."
    )]
    async fn search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let command = SearchCommand {
            query: params.query,
            mode: params.mode,
            default_mode: self.config.default_search_mode,
            top_k: params.top_k.map(|value| value as usize),
            default_top_k: self.config.default_top_k,
            document_id: params.document_id,
            wing: params.wing,
            room: params.room,
            layer: params.layer,
            source_file: params.source_file,
            include_archived: params.include_archived.unwrap_or(false),
            min_score: params.min_score,
            diversity: params.diversity,
            group_by: params.group_by,
            recency_half_life_days: params.recency_half_life_days,
            max_context_tokens: params
                .max_context_tokens
                .map(|value| value as usize)
                .or(Some(self.config.max_context_tokens)),
            max_chunks_per_document: params
                .max_chunks_per_document
                .map(|value| value as usize)
                .or(Some(self.config.max_chunks_per_doc)),
            context_expansion: params.context_expansion,
            neighbor_chunks: params.neighbor_chunks.map(|value| value as usize),
            timeout_ms: params.timeout_ms.or(Some(5_000)),
            fts_stemmer: self.config.fts_stemmer.clone(),
            rrf_k: params.rrf_k,
        };
        let hits = crate::retrieval::execute_search(
            &self.store,
            self.embedder.as_ref(),
            &self.config,
            command,
        )
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&hits)
    }

    #[tool(
        name = "multi_query_search",
        description = "Fuse an original query and caller-supplied rewrites with RRF. No LLM rewrite is performed implicitly."
    )]
    async fn multi_query_search(
        &self,
        Parameters(params): Parameters<MultiQuerySearchParams>,
    ) -> Result<CallToolResult, McpError> {
        if params.queries.is_empty() || params.queries.len() > 16 {
            return Err(Self::map_err(AppError::config(
                "queries must contain 1..=16 entries",
            )));
        }
        let top_k = params
            .top_k
            .map(|v| v as usize)
            .unwrap_or(self.config.default_top_k);
        let mode =
            retrieval::resolve_search_mode(params.mode.as_deref(), self.config.default_search_mode)
                .map_err(Self::map_err)?;
        let corpus_guard = acquire_corpus_search_guard(&self.store, mode).map_err(Self::map_err)?;
        if matches!(mode, SearchMode::Vec | SearchMode::Hybrid) {
            self.require_vec_compatible().map_err(Self::map_err)?;
        }
        let mut lists = Vec::with_capacity(params.queries.len());
        for text in params.queries {
            let embedding = if matches!(mode, SearchMode::Vec | SearchMode::Hybrid) {
                Some(
                    self.embedder
                        .embed(std::slice::from_ref(&text))
                        .await
                        .map_err(Self::map_err)?
                        .into_iter()
                        .next()
                        .ok_or_else(|| {
                            McpError::internal_error("embedder returned no vector for query", None)
                        })?,
                )
            } else {
                None
            };
            lists.push(
                search_with_corpus_guard(
                    &self.store,
                    &SearchQuery {
                        mode,
                        top_k,
                        query_text: Some(text),
                        query_embedding: embedding,
                        wing: nonempty_opt(params.wing.clone()),
                        room: nonempty_opt(params.room.clone()),
                        layer: nonempty_opt(params.layer.clone()),
                        source_file: nonempty_opt(params.source_file.clone()),
                        timeout_ms: params.timeout_ms.or(Some(5_000)),
                        fts_stemmer: self.config.fts_stemmer.clone(),
                        ..SearchQuery::default()
                    },
                    &corpus_guard,
                )
                .map_err(Self::map_err)?,
            );
        }
        Self::json_result(&fuse_rrf_many(
            &lists,
            crate::db::search::DEFAULT_RRF_K,
            top_k,
        ))
    }

    #[tool(
        name = "get_embedding_manifest",
        description = "Return the corpus embedding identity (provider, model, dims, endpoint fingerprint). Empty object fields when none recorded yet."
    )]
    async fn get_embedding_manifest(&self) -> Result<CallToolResult, McpError> {
        let manifest = self.store.get_embedding_manifest().map_err(Self::map_err)?;
        match manifest {
            Some(m) => Self::json_result(&m),
            None => Self::json_result(&serde_json::json!({
                "id": null,
                "provider": null,
                "model": null,
                "dims": null,
                "recorded": false,
            })),
        }
    }

    #[tool(
        name = "reembed_document",
        description = "Refresh one document's vectors only when the live embedding identity already matches the corpus manifest. Use complete uncapped reembed_all for provider/model/dims/endpoint migration."
    )]
    async fn reembed_document(
        &self,
        Parameters(params): Parameters<ReembedDocumentParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .reembed_document_pipeline(params.document_id.trim())
            .await
            .map_err(Self::map_err)?;
        Self::json_result(&result)
    }

    #[tool(
        name = "list_documents",
        description = "List stored documents without loading full content. Filters: wing?, room?, source_file?, include_archived?, layer?, kind?; limit defaults 50 (hard cap 200), offset defaults 0. Default excludes archived/tombstone."
    )]
    async fn list_documents(
        &self,
        Parameters(params): Parameters<ListDocumentsParams>,
    ) -> Result<CallToolResult, McpError> {
        let nonempty = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
        let filter = DocumentCatalogFilter {
            wing: nonempty(params.wing),
            room: nonempty(params.room),
            source_file: nonempty(params.source_file),
            include_archived: params.include_archived.unwrap_or(false),
            layer: nonempty(params.layer),
            kind: nonempty(params.kind),
            limit: params
                .limit
                .map(|value| value as usize)
                .unwrap_or(DEFAULT_CATALOG_PAGE_SIZE)
                .clamp(1, MAX_CATALOG_PAGE_SIZE),
            offset: params.offset.unwrap_or(0) as usize,
            ..DocumentCatalogFilter::default()
        };
        let store = self.store.clone();
        let page = run_blocking("list_documents", move || {
            store.list_document_catalog(&filter)
        })
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&page.items)
    }

    #[tool(
        name = "get_document",
        description = "Get a document by id with metadata. include_chunks returns text-only chunks, default 100 and hard cap 500 via chunk_limit; one response has an 8 MiB aggregate chunk-text budget."
    )]
    async fn get_document(
        &self,
        Parameters(params): Parameters<GetDocumentParams>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let document_id = params.document_id;
        let include_chunks = params.include_chunks.unwrap_or(false);
        let chunk_limit = params.chunk_limit.map(|value| value as usize);
        let body = run_blocking("get_document", move || {
            retrieval::get_document(&store, &document_id, include_chunks, chunk_limit)
                .map(DocumentDetail::from)
        })
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&body)
    }

    #[tool(
        name = "multi_get",
        description = "Get up to 100 documents by id in request order. Returns found documents plus missing ids. include_chunks returns no embeddings, defaults to 100 chunks per document (hard cap 500 via chunk_limit), and has an 8 MiB aggregate chunk-text budget."
    )]
    async fn multi_get(
        &self,
        Parameters(params): Parameters<MultiGetParams>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let include_chunks = params.include_chunks.unwrap_or(false);
        let chunk_limit = params.chunk_limit.map(|value| value as usize);
        let result = run_blocking("multi_get", move || {
            retrieval::multi_get(&store, params.document_ids, include_chunks, chunk_limit)
        })
        .await
        .map_err(Self::map_err)?;
        let documents = result
            .documents
            .into_iter()
            .map(DocumentDetail::from)
            .collect::<Vec<_>>();
        Self::json_result(&serde_json::json!({
            "documents": documents,
            "missing": result.missing,
        }))
    }

    #[tool(
        name = "expand_chunks",
        description = "Return the selected chunk and neighboring chunks from the same document, ordered by chunk_index."
    )]
    async fn expand_chunks(
        &self,
        Parameters(params): Parameters<ExpandChunksParams>,
    ) -> Result<CallToolResult, McpError> {
        let chunks = retrieval::expand_chunks(
            &self.store,
            &params.document_id,
            params.chunk_index,
            params.radius.unwrap_or(1),
        )
        .map_err(Self::map_err)?
        .into_iter()
        .map(ChunkView::from)
        .collect::<Vec<_>>();
        Self::json_result(&chunks)
    }

    #[tool(
        name = "find_similar",
        description = "Find documents with chunk embeddings similar to a seed document. Uses the normalized mean of its existing chunk vectors and excludes the seed."
    )]
    async fn find_similar(
        &self,
        Parameters(params): Parameters<FindSimilarParams>,
    ) -> Result<CallToolResult, McpError> {
        let hits = retrieval::find_similar(
            &self.store,
            &self.config,
            SimilarDocumentsQuery {
                document_id: params.document_id,
                top_k: params.top_k.unwrap_or(self.config.default_top_k as u32) as usize,
                wing: params.wing.filter(|value| !value.trim().is_empty()),
                room: params.room.filter(|value| !value.trim().is_empty()),
                fts_stemmer: self.config.fts_stemmer.clone(),
            },
        )
        .map_err(Self::map_err)?;
        Self::json_result(&hits)
    }

    #[tool(
        name = "delete_document",
        description = "Delete a document, its chunks, and its graph node edges by document_id."
    )]
    async fn delete_document(
        &self,
        Parameters(params): Parameters<DeleteDocumentParams>,
    ) -> Result<CallToolResult, McpError> {
        let deleted = self
            .store
            .delete_document(&params.document_id)
            .map_err(Self::map_err)?;
        if !deleted {
            return Err(Self::map_err(AppError::not_found(format!(
                "document not found: {}",
                params.document_id
            ))));
        }
        Self::json_result(&serde_json::json!({
            "deleted": true,
            "document_id": params.document_id,
        }))
    }

    #[tool(
        name = "update_document_meta",
        description = "Update document meta without re-embedding: wing, room, title, metadata_json, pinned, boost, status (and layer/kind/source_file). Optional content triggers re-chunk+re-embed only when body changes (refused for layer=raw). MemPalace update_drawer analogue."
    )]
    async fn update_document_meta(
        &self,
        Parameters(params): Parameters<UpdateDocumentMetaParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .update_document_via_service(params)
            .await
            .map_err(Self::map_err)?;
        Self::json_result(&result)
    }

    #[tool(
        name = "list_wings",
        description = "List distinct document wings with document counts (MemPalace taxonomy)."
    )]
    async fn list_wings(
        &self,
        Parameters(_params): Parameters<ListWingsParams>,
    ) -> Result<CallToolResult, McpError> {
        let wings = self.store.list_wings().map_err(Self::map_err)?;
        Self::json_result(&wings)
    }

    #[tool(
        name = "list_rooms",
        description = "List distinct document rooms with counts; optional wing filter (MemPalace taxonomy)."
    )]
    async fn list_rooms(
        &self,
        Parameters(params): Parameters<ListRoomsParams>,
    ) -> Result<CallToolResult, McpError> {
        let wing = params
            .wing
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let rooms = self.store.list_rooms(wing).map_err(Self::map_err)?;
        Self::json_result(&rooms)
    }

    #[tool(
        name = "get_taxonomy",
        description = "Full wing → room tree with document counts plus unscoped/total totals."
    )]
    async fn get_taxonomy(
        &self,
        Parameters(_params): Parameters<GetTaxonomyParams>,
    ) -> Result<CallToolResult, McpError> {
        let tax = self.store.get_taxonomy().map_err(Self::map_err)?;
        Self::json_result(&tax)
    }

    #[tool(
        name = "stats",
        description = "Return store counts (documents, chunks, graph nodes/edges) and db path."
    )]
    async fn stats(&self) -> Result<CallToolResult, McpError> {
        let (document_count, chunk_count, node_count, edge_count) =
            self.store.stats().map_err(Self::map_err)?;
        let body = Stats {
            document_count,
            chunk_count,
            node_count,
            edge_count,
            db_path: self.store.path().display().to_string(),
        };
        Self::json_result(&body)
    }

    #[tool(
        name = "status",
        description = "Index health: backend=duckdb, docs/chunks/nodes/edges, wings summary, ready_for_search, fts_ready, embed_dims, db path."
    )]
    async fn status(&self) -> Result<CallToolResult, McpError> {
        let body = DiagnosticsService::new(&self.store, &self.config)
            .status()
            .map_err(Self::map_err)?;
        Self::json_result(&body)
    }

    #[tool(
        name = "doctor",
        description = "Minimal integrity: schema_version vs expected, FTS ready, embed dims vs manifest, ingest_roots, ready_for_search."
    )]
    async fn doctor(&self) -> Result<CallToolResult, McpError> {
        let body = DiagnosticsService::new(&self.store, &self.config)
            .doctor()
            .map_err(Self::map_err)?;
        Self::json_result(&body)
    }

    #[tool(
        name = "doctor_repair",
        description = "Preview or repair doctor findings. Defaults to dry_run=true. Reingests non-schema documents missing chunks, prunes orphan chunks/nodes/edges transactionally, checkpoints, and returns before/after doctor reports."
    )]
    async fn doctor_repair(
        &self,
        Parameters(params): Parameters<DoctorRepairParams>,
    ) -> Result<CallToolResult, McpError> {
        let report = DiagnosticsService::new(&self.store, &self.config)
            .repair(
                &self.embedder,
                params.dry_run.unwrap_or(true),
                Some(clamp_maintenance_max_docs(
                    self.config.maint_max_docs,
                    params.max_docs,
                )),
            )
            .await
            .map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "llm_status",
        description = "Local chat LLM + embeddings: llm_enabled, base_url, model, reachable (short probe), embed provider/model/dims. Does not hang long when Ollama is down."
    )]
    async fn llm_status(&self) -> Result<CallToolResult, McpError> {
        let body = self.llm_status_report().await.map_err(Self::map_err)?;
        Self::json_result(&body)
    }

    #[tool(
        name = "get_graph",
        description = "Export object graph topology {nodes, edges} with optional kind/rel/seed filters."
    )]
    async fn get_graph(
        &self,
        Parameters(params): Parameters<GetGraphParams>,
    ) -> Result<CallToolResult, McpError> {
        let filter = GraphFilter {
            kinds: params.kinds,
            rel_types: params.rel_types,
            seed_ids: params.seed_ids,
            max_nodes: params.max_nodes,
        };
        let view = self.store.get_graph_view(filter).map_err(Self::map_err)?;
        Self::json_result(&view)
    }

    /// Write GraphView JSON for rag-mcp-ui while this process holds the DuckDB lock.
    /// Concurrent UI must use `--snapshot` (not a second `--db` open).
    #[tool(
        name = "export_graph_snapshot",
        description = "Write GraphView topology JSON to disk for rag-mcp-ui (Mode C). Use while MCP holds the DB; then run: rag-mcp-ui --snapshot PATH --seed LABEL. Path defaults to <db_dir>/graph.json; must be under DB directory or RAG_INGEST_ROOTS."
    )]
    async fn export_graph_snapshot(
        &self,
        Parameters(params): Parameters<ExportGraphSnapshotParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_nodes = params.max_nodes.unwrap_or(500);
        let include_tags = params.include_tags.unwrap_or(false);
        #[derive(Serialize)]
        struct ExportSnapResult {
            path: String,
            node_count: usize,
            edge_count: usize,
            max_nodes: u32,
            include_tags: bool,
            ui_hint: String,
        }
        let requested_path = params.path;
        let store = self.store.clone();
        let config = self.config.clone();
        let report = run_blocking("export_graph_snapshot", move || {
            let view = store.export_graph_for_ui(Some(max_nodes), include_tags)?;
            let default_path = store
                .path()
                .parent()
                .map(|parent| parent.join("graph.json"))
                .unwrap_or_else(|| Path::new("graph.json").to_path_buf());
            let out = requested_path
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(Path::new)
                .map(Path::to_path_buf)
                .unwrap_or(default_path);
            let out = std::fs::canonicalize(out.parent().unwrap_or(Path::new(".")))
                .map(|parent| {
                    parent.join(
                        out.file_name()
                            .unwrap_or_else(|| std::ffi::OsStr::new("graph.json")),
                    )
                })
                .unwrap_or(out.clone());

            let mut allowed = config.ingest_roots;
            if let Some(parent) = store.path().parent() {
                allowed.push(parent.to_path_buf());
            }
            let out_canon = if out.exists() {
                std::fs::canonicalize(&out).unwrap_or(out.clone())
            } else if let Some(parent) = out.parent() {
                let parent_c =
                    std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
                parent_c.join(out.file_name().unwrap_or_default())
            } else {
                out.clone()
            };
            if !allowed.iter().any(|root| {
                let root_c = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
                out_canon.starts_with(&root_c)
            }) {
                return Err(AppError::forbidden(format!(
                    "export path {} is outside DB directory and RAG_INGEST_ROOTS",
                    out.display()
                )));
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    AppError::db(format!("create parent for {}: {error}", out.display()))
                })?;
            }
            let json = serde_json::to_vec_pretty(&view)
                .map_err(|error| AppError::db(format!("serialize GraphView: {error}")))?;
            std::fs::write(&out, json)
                .map_err(|error| AppError::db(format!("write {}: {error}", out.display())))?;

            Ok(ExportSnapResult {
                path: out.display().to_string(),
                node_count: view.nodes.len(),
                edge_count: view.edges.len(),
                max_nodes,
                include_tags,
                ui_hint: format!(
                    "Run: rag-mcp-ui --snapshot {} --seed <title_or_node_id>",
                    out.display()
                ),
            })
        })
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "get_neighbors",
        description = "Local undirected BFS subgraph around a node (Obsidian local graph)."
    )]
    async fn get_neighbors(
        &self,
        Parameters(params): Parameters<GetNeighborsParams>,
    ) -> Result<CallToolResult, McpError> {
        let depth = params.depth.unwrap_or(1);
        let max_nodes = params.max_nodes.unwrap_or(100);
        let view = self
            .store
            .neighbors(&params.node_id, depth, max_nodes)
            .map_err(Self::map_err)?;
        Self::json_result(&view)
    }

    #[tool(
        name = "get_backlinks",
        description = "Incoming edges and source nodes for a node (resolve by node_id, document_id, or label)."
    )]
    async fn get_backlinks(
        &self,
        Parameters(params): Parameters<GetBacklinksParams>,
    ) -> Result<CallToolResult, McpError> {
        let target = if let Some(id) = params
            .node_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.store.find_node_by_id(id).map_err(Self::map_err)?
        } else if let Some(doc_id) = params
            .document_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.store
                .find_node_by_document_id(doc_id)
                .map_err(Self::map_err)?
        } else if let Some(label) = params
            .label
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.store
                .find_nodes_by_label(label)
                .map_err(Self::map_err)?
                .into_iter()
                .next()
        } else {
            return Err(Self::map_err(AppError::config(
                "get_backlinks requires node_id, document_id, or label",
            )));
        }
        .ok_or_else(|| {
            Self::map_err(AppError::not_found(
                "graph node not found (provide node_id, document_id, or label)",
            ))
        })?;
        let view = self.store.backlinks(&target.id).map_err(Self::map_err)?;
        Self::json_result(&view)
    }

    #[tool(
        name = "link_nodes",
        description = "Create an explicit graph edge (default rel_type=related; tunnel allowed)."
    )]
    async fn link_nodes(
        &self,
        Parameters(params): Parameters<LinkNodesParams>,
    ) -> Result<CallToolResult, McpError> {
        let rel = params.rel_type.as_deref().unwrap_or("related");
        let weight = params.weight.unwrap_or(1.0);
        let edge = self
            .store
            .link_nodes(&params.source_id, &params.target_id, rel, weight)
            .map_err(Self::map_err)?;
        Self::json_result(&edge)
    }

    #[tool(
        name = "create_tunnel",
        description = "Create an explicit tunnel edge (rel_type=tunnel) between two graph nodes. Same pair either order updates weight/context."
    )]
    async fn create_tunnel(
        &self,
        Parameters(params): Parameters<CreateTunnelParams>,
    ) -> Result<CallToolResult, McpError> {
        let weight = params.weight.unwrap_or(1.0);
        let context = params
            .context
            .as_deref()
            .or(params.label.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let edge = self
            .store
            .create_tunnel(&params.source_id, &params.target_id, weight, context)
            .map_err(Self::map_err)?;
        Self::json_result(&edge)
    }

    #[tool(
        name = "list_tunnels",
        description = "List tunnel edges (rel_type=tunnel). Optional node_id filters to incident tunnels."
    )]
    async fn list_tunnels(
        &self,
        Parameters(params): Parameters<ListTunnelsParams>,
    ) -> Result<CallToolResult, McpError> {
        let edges = self
            .store
            .list_tunnels(params.node_id.as_deref())
            .map_err(Self::map_err)?;
        Self::json_result(&ListTunnelsResult {
            count: edges.len() as u64,
            tunnels: edges,
        })
    }

    #[tool(
        name = "delete_tunnel",
        description = "Delete a tunnel edge by id (only rows with rel_type=tunnel)."
    )]
    async fn delete_tunnel(
        &self,
        Parameters(params): Parameters<DeleteTunnelParams>,
    ) -> Result<CallToolResult, McpError> {
        let deleted = self
            .store
            .delete_tunnel(&params.tunnel_id)
            .map_err(Self::map_err)?;
        if !deleted {
            return Err(Self::map_err(AppError::not_found(format!(
                "tunnel not found: {}",
                params.tunnel_id
            ))));
        }
        Self::json_result(&DeleteTunnelResult {
            deleted: true,
            tunnel_id: params.tunnel_id,
        })
    }

    #[tool(
        name = "follow_tunnels",
        description = "Multi-hop undirected BFS along tunnel edges only (node_id, depth?=2, max_nodes?=100)."
    )]
    async fn follow_tunnels(
        &self,
        Parameters(params): Parameters<FollowTunnelsParams>,
    ) -> Result<CallToolResult, McpError> {
        let depth = params.depth.unwrap_or(2);
        let max_nodes = params.max_nodes.unwrap_or(100);
        let view = self
            .store
            .follow_tunnels(&params.node_id, depth, max_nodes)
            .map_err(Self::map_err)?;
        Self::json_result(&view)
    }

    #[tool(
        name = "find_tunnels",
        description = "Find tunnel edges: node_id?, other_node_id? (pair), wing? (endpoint document wing), limit?."
    )]
    async fn find_tunnels(
        &self,
        Parameters(params): Parameters<FindTunnelsParams>,
    ) -> Result<CallToolResult, McpError> {
        let edges = self
            .store
            .find_tunnels(
                params.node_id.as_deref(),
                params.other_node_id.as_deref(),
                params.wing.as_deref(),
                params.limit,
            )
            .map_err(Self::map_err)?;
        Self::json_result(&ListTunnelsResult {
            count: edges.len() as u64,
            tunnels: edges,
        })
    }

    #[tool(
        name = "graph_stats",
        description = "Object graph aggregates: total nodes/edges, node counts by kind, edge counts by rel_type (includes tunnel)."
    )]
    async fn graph_stats(&self) -> Result<CallToolResult, McpError> {
        let body = self.store.graph_stats().map_err(Self::map_err)?;
        Self::json_result(&body)
    }

    #[tool(
        name = "kg_add",
        description = "Add a temporal KG fact (subject, predicate, object). Idempotent for open active SPO. Optional valid_from/valid_to (half-open), source_document_id, confidence, metadata_json."
    )]
    async fn kg_add(
        &self,
        Parameters(params): Parameters<KgAddParams>,
    ) -> Result<CallToolResult, McpError> {
        let valid_from = parse_optional_ts(params.valid_from.as_deref()).map_err(Self::map_err)?;
        let valid_to = parse_optional_ts(params.valid_to.as_deref()).map_err(Self::map_err)?;
        let fact = self
            .store
            .kg_add(
                &params.subject,
                &params.predicate,
                &params.object,
                valid_from,
                valid_to,
                params.source_document_id.as_deref(),
                params.confidence,
                params.metadata_json.as_deref(),
            )
            .map_err(Self::map_err)?;
        Self::json_result(&fact)
    }

    #[tool(
        name = "kg_query",
        description = "Query temporal KG facts by optional subject/predicate/object. Without at_time: active only. With at_time: half-open validity window filter."
    )]
    async fn kg_query(
        &self,
        Parameters(params): Parameters<KgQueryParams>,
    ) -> Result<CallToolResult, McpError> {
        let at_time = parse_optional_ts(params.at_time.as_deref()).map_err(Self::map_err)?;
        let facts = self
            .store
            .kg_query(
                params.subject.as_deref(),
                params.predicate.as_deref(),
                params.object.as_deref(),
                at_time,
            )
            .map_err(Self::map_err)?;
        Self::json_result(&facts)
    }

    #[tool(
        name = "kg_invalidate",
        description = "Invalidate open active KG fact(s) matching subject+predicate+object. Sets valid_to=ended (default now), status=invalidated."
    )]
    async fn kg_invalidate(
        &self,
        Parameters(params): Parameters<KgInvalidateParams>,
    ) -> Result<CallToolResult, McpError> {
        let ended = parse_optional_ts(params.ended.as_deref()).map_err(Self::map_err)?;
        let facts = self
            .store
            .kg_invalidate(&params.subject, &params.predicate, &params.object, ended)
            .map_err(Self::map_err)?;
        Self::json_result(&facts)
    }

    #[tool(
        name = "kg_supersede",
        description = "Supersede a KG fact object: close (subject,predicate,old_object) at boundary `at` and open (subject,predicate,new_object). Returns the successor fact."
    )]
    async fn kg_supersede(
        &self,
        Parameters(params): Parameters<KgSupersedeParams>,
    ) -> Result<CallToolResult, McpError> {
        let at = parse_optional_ts(params.at.as_deref()).map_err(Self::map_err)?;
        let fact = self
            .store
            .kg_supersede(
                &params.subject,
                &params.predicate,
                &params.old_object,
                &params.new_object,
                at,
                params.source_document_id.as_deref(),
                params.confidence,
            )
            .map_err(Self::map_err)?;
        Self::json_result(&fact)
    }

    #[tool(
        name = "kg_timeline",
        description = "Chronological KG facts for a subject (any status), ordered by valid_from."
    )]
    async fn kg_timeline(
        &self,
        Parameters(params): Parameters<KgTimelineParams>,
    ) -> Result<CallToolResult, McpError> {
        let facts = self
            .store
            .kg_timeline(&params.subject)
            .map_err(Self::map_err)?;
        Self::json_result(&facts)
    }

    #[tool(
        name = "kg_stats",
        description = "Temporal KG aggregates: total/active/invalidated/superseded facts, distinct subjects/predicates, relationship_types."
    )]
    async fn kg_stats(&self) -> Result<CallToolResult, McpError> {
        let body = self.store.kg_stats().map_err(Self::map_err)?;
        Self::json_result(&body)
    }

    #[tool(
        name = "find_node",
        description = "Resolve graph node metadata by node_id, document_id, or exact label."
    )]
    async fn find_node(
        &self,
        Parameters(params): Parameters<FindNodeParams>,
    ) -> Result<CallToolResult, McpError> {
        if params
            .node_id
            .as_ref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
            && params
                .document_id
                .as_ref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
            && params
                .label
                .as_ref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
        {
            return Err(Self::map_err(AppError::config(
                "find_node requires node_id, document_id, or label",
            )));
        }

        // When label is the only key, return all label matches; otherwise single resolve.
        let only_label = params
            .node_id
            .as_ref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
            && params
                .document_id
                .as_ref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
            && params
                .label
                .as_ref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);

        if only_label {
            let label = params.label.as_deref().unwrap_or("").trim();
            let nodes = self
                .store
                .find_nodes_by_label(label)
                .map_err(Self::map_err)?;
            return Self::json_result(&FindNodeResult {
                node: nodes.first().cloned(),
                matches: nodes,
            });
        }

        let node = self
            .store
            .resolve_node(
                params.node_id.as_deref(),
                params.document_id.as_deref(),
                params.label.as_deref(),
            )
            .map_err(Self::map_err)?;
        let matches = match &node {
            Some(n) => vec![n.clone()],
            None => Vec::new(),
        };
        Self::json_result(&FindNodeResult { node, matches })
    }

    #[tool(
        name = "graph_expand_search",
        description = "Semantic search then expand neighbor subgraph from hit document nodes."
    )]
    async fn graph_expand_search(
        &self,
        Parameters(params): Parameters<GraphExpandSearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let corpus_guard =
            acquire_corpus_search_guard(&self.store, SearchMode::Vec).map_err(Self::map_err)?;
        self.require_vec_compatible().map_err(Self::map_err)?;

        let top_k = params
            .top_k
            .map(|k| k as usize)
            .unwrap_or(self.config.default_top_k);
        let depth = params.depth.unwrap_or(1);
        let max_nodes = params.max_nodes.unwrap_or(100);

        let query_vecs = self
            .embedder
            .embed(&[params.query])
            .await
            .map_err(Self::map_err)?;
        let query_emb = query_vecs.into_iter().next().ok_or_else(|| {
            McpError::internal_error("embedder returned no vector for query", None)
        })?;

        let hits = search_with_corpus_guard(
            &self.store,
            &SearchQuery {
                mode: SearchMode::Vec,
                top_k,
                query_embedding: Some(query_emb),
                document_id: params.document_id,
                ..SearchQuery::default()
            },
            &corpus_guard,
        )
        .map_err(Self::map_err)?;

        let mut merged_nodes: std::collections::HashMap<String, GraphNode> =
            std::collections::HashMap::new();
        let mut merged_edges: std::collections::HashMap<String, crate::models::GraphEdge> =
            std::collections::HashMap::new();

        let mut seen_doc_nodes = std::collections::HashSet::new();
        for hit in &hits {
            let Some(node) = self
                .store
                .find_node_by_document_id(&hit.document_id)
                .map_err(Self::map_err)?
            else {
                continue;
            };
            if !seen_doc_nodes.insert(node.id.clone()) {
                continue;
            }
            let local = self
                .store
                .neighbors(&node.id, depth, max_nodes)
                .map_err(Self::map_err)?;
            for n in local.nodes {
                merged_nodes.entry(n.id.clone()).or_insert(n);
            }
            for e in local.edges {
                merged_edges.entry(e.id.clone()).or_insert(e);
            }
            if merged_nodes.len() >= max_nodes as usize {
                break;
            }
        }

        // Cap nodes if over max_nodes after merge.
        let mut nodes: Vec<GraphNode> = merged_nodes.into_values().collect();
        if nodes.len() > max_nodes as usize {
            nodes.truncate(max_nodes as usize);
        }
        let keep: std::collections::HashSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
        let edges: Vec<_> = merged_edges
            .into_values()
            .filter(|e| keep.contains(&e.source_id) && keep.contains(&e.target_id))
            .collect();

        let body = GraphExpandSearchResult {
            hits,
            graph: GraphView { nodes, edges },
        };
        Self::json_result(&body)
    }

    #[tool(
        name = "pack_context",
        description = "Pack ranked search hits under a token budget (~4 chars/token), optionally expanding neighbors or the parent Markdown section."
    )]
    async fn pack_context(
        &self,
        Parameters(params): Parameters<PackContextParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_tokens = params
            .max_tokens
            .map(|t| t as usize)
            .unwrap_or(self.config.max_context_tokens);

        let expansion = match params.context_expansion.as_deref() {
            Some(raw) => Some(ContextExpansion::parse(raw).map_err(Self::map_err)?),
            None => None,
        };
        let mut hits: Vec<SearchHit> = params
            .hits
            .into_iter()
            .map(pack_hit_to_search_hit)
            .collect();
        attach_context(
            &self.store,
            &mut hits,
            expansion,
            params.neighbor_chunks.unwrap_or(1) as usize,
        )
        .map_err(Self::map_err)?;
        let packed = pack_hits(&hits, max_tokens);

        let body = PackContextResult {
            hits: packed.hits,
            total_tokens: packed.total_tokens,
            max_tokens: packed.max_tokens,
            omitted_count: packed.omitted_count,
            context_text: packed.context_text,
        };
        Self::json_result(&body)
    }

    #[tool(
        name = "get_schema",
        description = "Read the agent conventions schema document at uri schema://agents (layer=schema). Seeds a default when missing unless no_default=true."
    )]
    async fn get_schema(
        &self,
        Parameters(params): Parameters<GetSchemaParams>,
    ) -> Result<CallToolResult, McpError> {
        let no_default = params.no_default.unwrap_or(false);
        if no_default {
            let doc = self
                .store
                .get_schema_document()
                .map_err(Self::map_err)?
                .ok_or_else(|| {
                    Self::map_err(AppError::not_found(
                        "schema document not found at schema://agents",
                    ))
                })?;
            let body = crate::wiki::SchemaDocumentView {
                document_id: doc.id,
                uri: doc.uri,
                title: doc.title,
                content: doc.content,
                layer: doc.layer,
                kind: doc.kind,
                content_hash: doc.content_hash,
                created_at: doc.created_at.to_rfc3339(),
                updated_at: doc.updated_at.to_rfc3339(),
                created: false,
            };
            return Self::json_result(&body);
        }
        let body = crate::wiki::get_schema(&self.store).map_err(Self::map_err)?;
        Self::json_result(&body)
    }

    #[tool(
        name = "update_schema",
        description = "Create or replace the agent conventions schema document at uri schema://agents (layer=schema, kind=schema)."
    )]
    async fn update_schema(
        &self,
        Parameters(params): Parameters<UpdateSchemaParams>,
    ) -> Result<CallToolResult, McpError> {
        let body = crate::wiki::update_schema(
            &self.store,
            &params.content,
            params.title.as_deref(),
            params.agent.as_deref(),
        )
        .map_err(Self::map_err)?;
        Self::json_result(&body)
    }

    #[tool(
        name = "diary_write",
        description = "Append a verbatim per-agent diary note (layer=diary, kind=diary, wing=agents/<name> or wing?). Chunks+embeds for search; logs ops_log diary_write."
    )]
    async fn diary_write(
        &self,
        Parameters(params): Parameters<DiaryWriteParams>,
    ) -> Result<CallToolResult, McpError> {
        let res = diary::diary_write(
            &self.store,
            &self.embedder,
            &self.config,
            diary::DiaryWriteCommand {
                agent_name: params.agent_name,
                content: params.content,
                wing: params.wing,
                topic: params.topic,
                title: params.title,
                log_ops: true,
            },
        )
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&res)
    }

    #[tool(
        name = "diary_read",
        description = "Read recent diary entries for an agent (newest first). Filters kind=diary under wing agents/<name> (or room=agent). last_n/limit default 10."
    )]
    async fn diary_read(
        &self,
        Parameters(params): Parameters<DiaryReadParams>,
    ) -> Result<CallToolResult, McpError> {
        let last_n = params.last_n.unwrap_or(10) as usize;
        let entries =
            diary::diary_read(&self.store, &params.agent_name, last_n).map_err(Self::map_err)?;
        Self::json_result(&entries)
    }

    #[tool(
        name = "wake_up",
        description = "Session bootstrap: status + last N diary entries (agent_name?) + pinned docs + schema snippet only if schema document already exists (does not seed default)."
    )]
    async fn wake_up(
        &self,
        Parameters(params): Parameters<WakeUpParams>,
    ) -> Result<CallToolResult, McpError> {
        let status = DiagnosticsService::new(&self.store, &self.config)
            .status()
            .map_err(Self::map_err)?;
        let diary_limit = params.diary_limit.unwrap_or(5) as usize;
        let pinned_limit = params.pinned_limit.unwrap_or(20) as usize;
        let body = diary::wake_up(
            &self.store,
            status,
            params.agent_name.as_deref(),
            diary_limit,
            pinned_limit,
        )
        .map_err(Self::map_err)?;
        Self::json_result(&body)
    }

    #[tool(
        name = "checkpoint",
        description = "Session savepoint: always append ops_log (op=checkpoint, message=summary). Optional diary content writes a diary entry for agent_name (default agent). Prefer this over separate append_log + diary_write at session boundaries."
    )]
    async fn checkpoint(
        &self,
        Parameters(params): Parameters<CheckpointParams>,
    ) -> Result<CallToolResult, McpError> {
        let res = diary::checkpoint(
            &self.store,
            &self.embedder,
            &self.config,
            &params.summary,
            params.diary.as_deref(),
            params.agent_name.as_deref(),
        )
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&res)
    }

    #[tool(
        name = "append_log",
        description = "Append an ops_log timeline entry (Karpathy log.md). Auto-assigns id/seq/ts."
    )]
    async fn append_log(
        &self,
        Parameters(params): Parameters<AppendLogParams>,
    ) -> Result<CallToolResult, McpError> {
        let payload_json = match params.payload_json {
            Some(s) if !s.trim().is_empty() => {
                if let Err(e) = serde_json::from_str::<serde_json::Value>(&s) {
                    return Err(Self::map_err(AppError::config(format!(
                        "payload_json is not valid JSON: {e}"
                    ))));
                }
                s
            }
            _ => "{}".to_string(),
        };
        let entry = OpsLogEntry {
            id: String::new(),
            seq: 0,
            ts: Utc::now(),
            op: params.op,
            prefix: params.prefix,
            message: params.message.unwrap_or_default(),
            entity_id: params.entity_id,
            entity_kind: params.entity_kind,
            payload_json,
            agent_name: params.agent_name,
        };
        let written = self.store.append_ops_log(&entry).map_err(Self::map_err)?;
        Self::json_result(&written)
    }

    #[tool(
        name = "read_log",
        description = "Read ops_log by id or seq; without either, list recent entries (limit default 50)."
    )]
    async fn read_log(
        &self,
        Parameters(params): Parameters<ReadLogParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = params
            .id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let has_key = id.is_some() || params.seq.is_some();
        let rows = self
            .store
            .read_ops_log(id, params.seq, params.limit.map(|n| n as usize))
            .map_err(Self::map_err)?;
        if has_key {
            let entry = rows.into_iter().next().ok_or_else(|| {
                Self::map_err(AppError::not_found(format!(
                    "ops_log entry not found (id={:?}, seq={:?})",
                    params.id, params.seq
                )))
            })?;
            return Self::json_result(&entry);
        }
        Self::json_result(&rows)
    }

    #[tool(
        name = "list_recent_ops",
        description = "List recent ops_log entries newest-first (default limit 20). Empty list if ops_log table is missing."
    )]
    async fn list_recent_ops(
        &self,
        Parameters(params): Parameters<ListRecentOpsParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(20) as usize;
        let ops = self.store.list_recent_ops(limit).map_err(Self::map_err)?;
        Self::json_result(&ops)
    }

    #[tool(
        name = "memories_filed_away",
        description = "Recent memory-filing ops from ops_log (ingest/drawer/wiki/diary/checkpoint), newest first. Empty/quiet when ops_log missing or no matches. Optional limit (default 20)."
    )]
    async fn memories_filed_away(
        &self,
        Parameters(params): Parameters<MemoriesFiledAwayParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(20) as usize;
        let ops_log_available = self.store.ops_log_exists().map_err(Self::map_err)?;
        let ops = if ops_log_available {
            self.store
                .memories_filed_away(limit)
                .map_err(Self::map_err)?
        } else {
            Vec::new()
        };
        let count = ops.len();
        let timestamp = ops.first().map(|e| e.ts.to_rfc3339());
        let (status, message) = if !ops_log_available {
            (
                "quiet",
                "No ops_log table; no filed memories tracked".to_string(),
            )
        } else if count == 0 {
            (
                "quiet",
                "No recent memory-filing ops in ops_log".to_string(),
            )
        } else {
            (
                "ok",
                format!("{count} memories filed away (recent ops_log)"),
            )
        };
        Self::json_result(&serde_json::json!({
            "status": status,
            "message": message,
            "count": count,
            "timestamp": timestamp,
            "ops_log_available": ops_log_available,
            "ops": ops,
        }))
    }

    #[tool(
        name = "reconnect",
        description = "Force reconnect / cache clear after external DB writes. DuckDB single-process has no client cache: always returns ok=true (no-op success)."
    )]
    async fn reconnect(
        &self,
        Parameters(_params): Parameters<ReconnectParams>,
    ) -> Result<CallToolResult, McpError> {
        // DuckDB is opened once per process under a mutex; there is no Chroma-style
        // HNSW client cache to drop. Success is always reported for MemPalace parity.
        Self::json_result(&serde_json::json!({
            "ok": true,
            "success": true,
            "backend": "duckdb",
            "message": "DuckDB has no client cache; reconnect is a no-op",
            "db_path": self.store.path().display().to_string(),
        }))
    }

    #[tool(
        name = "query_with_index",
        description = "Index-first wiki navigation: match query against wiki_index catalog (slug/title/summary/kind), optionally load page bodies."
    )]
    async fn query_with_index(
        &self,
        Parameters(params): Parameters<QueryWithIndexParams>,
    ) -> Result<CallToolResult, McpError> {
        let top_k = params
            .top_k
            .map(|k| k as usize)
            .unwrap_or(self.config.default_top_k);
        let include_content = params.include_content.unwrap_or(false);

        let matches = self
            .store
            .query_wiki_index(params.query.trim(), top_k)
            .map_err(Self::map_err)?;

        let mut pages = Vec::new();
        if include_content {
            let mut seen = std::collections::HashSet::new();
            for m in &matches {
                let Some(pid) = m.entry.page_id.as_deref() else {
                    continue;
                };
                if !seen.insert(pid.to_string()) {
                    continue;
                }
                if let Some(doc) = self.store.get_document(pid).map_err(Self::map_err)? {
                    pages.push(IndexQueryPage {
                        document_id: doc.id,
                        uri: doc.uri,
                        title: doc.title,
                        layer: doc.layer,
                        kind: doc.kind,
                        content: doc.content,
                    });
                }
            }
        }

        let body = IndexQueryResult {
            query: params.query,
            match_count: matches.len(),
            matches,
            pages,
        };
        Self::json_result(&body)
    }

    #[tool(
        name = "search_wiki",
        description = "Search compiled wiki layer only (layer=wiki). Same modes as search: lex|vec|hybrid with filters, min_score, diversity."
    )]
    async fn search_wiki(
        &self,
        Parameters(params): Parameters<SearchWikiParams>,
    ) -> Result<CallToolResult, McpError> {
        let top_k = params
            .top_k
            .map(|k| k as usize)
            .unwrap_or(self.config.default_top_k);

        let mode =
            retrieval::resolve_search_mode(params.mode.as_deref(), self.config.default_search_mode)
                .map_err(Self::map_err)?;
        let corpus_guard = acquire_corpus_search_guard(&self.store, mode).map_err(Self::map_err)?;

        if matches!(mode, SearchMode::Vec | SearchMode::Hybrid) {
            self.require_vec_compatible().map_err(Self::map_err)?;
        }

        let diversity = match params
            .diversity
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => Some(DiversityMode::parse(raw).map_err(Self::map_err)?),
            None => None,
        };

        let max_context_tokens = params
            .max_context_tokens
            .map(|n| n as usize)
            .or(Some(self.config.max_context_tokens));

        let max_chunks_per_document = params
            .max_chunks_per_document
            .map(|n| n as usize)
            .or(Some(self.config.max_chunks_per_doc));

        let query_embedding = if matches!(mode, SearchMode::Vec | SearchMode::Hybrid) {
            let query_vecs = self
                .embedder
                .embed(std::slice::from_ref(&params.query))
                .await
                .map_err(Self::map_err)?;
            let emb = query_vecs.into_iter().next().ok_or_else(|| {
                McpError::internal_error("embedder returned no vector for query", None)
            })?;
            Some(emb)
        } else {
            None
        };

        let opts = SearchQuery {
            mode,
            top_k,
            query_text: Some(params.query),
            query_embedding,
            document_id: None,
            wing: params.wing,
            room: params.room,
            layer: Some("wiki".into()),
            min_score: params.min_score,
            diversity,
            max_chunks_per_document,
            max_context_tokens,
            fts_stemmer: self.config.fts_stemmer.clone(),
            ..SearchQuery::default()
        };

        let hits =
            search_with_corpus_guard(&self.store, &opts, &corpus_guard).map_err(Self::map_err)?;
        Self::json_result(&hits)
    }

    #[tool(
        name = "file_answer",
        description = "Persist a cited answer as a wiki page (layer=wiki, category=answers): write body + citations metadata, rebuild graph, append ops_log, touch wiki_index."
    )]
    async fn file_answer(
        &self,
        Parameters(params): Parameters<FileAnswerParams>,
    ) -> Result<CallToolResult, McpError> {
        self.require_vec_compatible().map_err(Self::map_err)?;

        let citations = params.citations.map(|cs| {
            cs.into_iter()
                .map(|c: FileAnswerCitationParams| FileAnswerCitation {
                    document_id: c.document_id,
                    uri: c.uri,
                    title: c.title,
                    chunk_id: c.chunk_id,
                    quote: c.quote,
                    note: c.note,
                })
                .collect::<Vec<_>>()
        });

        let result = wiki::file_answer(
            &self.store,
            &self.embedder,
            &self.config,
            &params.title,
            &params.body,
            params.slug.as_deref(),
            citations,
            params.agent.as_deref(),
        )
        .await
        .map_err(Self::map_err)?;

        Self::json_result(&result)
    }

    #[tool(
        name = "read_index",
        description = "Read the wiki content catalog (index.md analogue): one row per page with slug, title, kind, category, one-line summary. format=json|markdown."
    )]
    async fn read_index(
        &self,
        Parameters(params): Parameters<ReadIndexParams>,
    ) -> Result<CallToolResult, McpError> {
        let entries = self.store.list_wiki_index().map_err(Self::map_err)?;
        let format = params
            .format
            .as_deref()
            .unwrap_or("json")
            .trim()
            .to_ascii_lowercase();
        let markdown = if format == "markdown" || format == "md" {
            Some(
                self.store
                    .render_wiki_index_markdown()
                    .map_err(Self::map_err)?,
            )
        } else {
            None
        };
        let count = entries.len();
        Self::json_result(&ReadIndexResult {
            entries,
            count,
            markdown,
        })
    }

    #[tool(
        name = "update_index_entry",
        description = "Create or merge-update one wiki catalog entry (slug, title?, kind?, category?, one-line summary?, page_id?)."
    )]
    async fn update_index_entry(
        &self,
        Parameters(params): Parameters<UpdateIndexEntryParams>,
    ) -> Result<CallToolResult, McpError> {
        let entry = self
            .store
            .update_wiki_index_entry_fields(
                &params.slug,
                params.title,
                params.summary,
                params.kind,
                params.category,
                params.page_id,
            )
            .map_err(Self::map_err)?;

        let _ = self
            .store
            .append_ops_log(&OpsLogEntry {
                id: Uuid::new_v4().to_string(),
                seq: 0,
                ts: Utc::now(),
                op: "index_update".into(),
                prefix: Some("INDEX".into()),
                message: format!("updated index entry {}", entry.slug),
                entity_id: Some(entry.id.clone()),
                entity_kind: Some("wiki_index".into()),
                payload_json: serde_json::json!({
                    "slug": entry.slug,
                    "title": entry.title,
                    "kind": entry.kind,
                })
                .to_string(),
                agent_name: None,
            })
            .map_err(Self::map_err)?;

        Self::json_result(&entry)
    }

    #[tool(
        name = "rebuild_index",
        description = "Clear and rebuild the wiki content catalog from all layer=wiki documents (one-line summaries from content or metadata)."
    )]
    async fn rebuild_index(
        &self,
        Parameters(_params): Parameters<RebuildIndexParams>,
    ) -> Result<CallToolResult, McpError> {
        let count = self
            .store
            .rebuild_wiki_index_from_docs()
            .map_err(Self::map_err)?;

        let _ = self
            .store
            .append_ops_log(&OpsLogEntry {
                id: Uuid::new_v4().to_string(),
                seq: 0,
                ts: Utc::now(),
                op: "index_rebuild".into(),
                prefix: Some("INDEX".into()),
                message: format!("rebuilt wiki_index ({count} entries)"),
                entity_id: None,
                entity_kind: Some("wiki_index".into()),
                payload_json: serde_json::json!({ "count": count }).to_string(),
                agent_name: None,
            })
            .map_err(Self::map_err)?;

        Self::json_result(&RebuildIndexResult { count })
    }

    #[tool(
        name = "analyze_corpus",
        description = "Deterministic corpus health report (no LLM): counts by layer/kind/status/wing, exact dups, optional near-dups, orphan nodes/wiki, unresolved/aging stubs, stale wiki, embed mismatch, FTS, size, archive candidates. Returns AnalysisReport JSON; logs to ops_log."
    )]
    async fn analyze_corpus(
        &self,
        Parameters(params): Parameters<AnalyzeCorpusParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut opts = maintain::AnalyzeOptions::from_config(&self.config);
        if let Some(d) = params.stub_age_days {
            opts.stub_age_days = d;
        }
        if let Some(v) = params.include_near_dups {
            opts.include_near_dups = v;
        }
        if let Some(t) = params.near_dup_threshold {
            opts.near_dup_threshold = t;
        }
        if let Some(d) = params.archive_min_age_days {
            opts.archive_min_age_days = d;
        }
        if let Some(v) = params.log_ops {
            opts.log_ops = v;
        }
        let report =
            maintain::analyze_corpus(&self.store, &self.config, &opts).map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "plan_maintenance",
        description = "Build an ordered maintenance plan from AnalysisReport. When RAG_LLM_ENABLED, calls local chat with analysis JSON + system prompt, parses JSON actions, validates whitelist. When LLM disabled (or force_heuristic), returns deterministic heuristic plan. Optional analysis JSON; else runs analyze_corpus. Logs to ops_log."
    )]
    async fn plan_maintenance(
        &self,
        Parameters(params): Parameters<PlanMaintenanceParams>,
    ) -> Result<CallToolResult, McpError> {
        let report = if let Some(value) = params.analysis {
            serde_json::from_value::<maintain::AnalysisReport>(value).map_err(|e| {
                Self::map_err(AppError::config(format!(
                    "plan_maintenance analysis must be an AnalysisReport JSON object: {e}"
                )))
            })?
        } else {
            let mut opts = maintain::AnalyzeOptions::from_config(&self.config);
            opts.log_ops = false;
            if let Some(v) = params.include_near_dups {
                opts.include_near_dups = v;
            }
            maintain::analyze_corpus(&self.store, &self.config, &opts).map_err(Self::map_err)?
        };

        let mut plan_opts = maintain::PlanOptions::from_config(&self.config);
        if let Some(n) = params.max_actions {
            plan_opts.max_actions = (n as usize).max(1);
        }
        if let Some(v) = params.force_heuristic {
            plan_opts.force_heuristic = v;
        }
        if let Some(v) = params.log_ops {
            plan_opts.log_ops = v;
        }

        let llm = self.llm.as_ref();
        let plan = maintain::plan_maintenance(&report, &self.config, llm, &plan_opts)
            .await
            .map_err(Self::map_err)?;

        if plan_opts.log_ops {
            maintain::log_plan(&self.store, &plan).map_err(Self::map_err)?;
        }

        Self::json_result(&plan)
    }

    #[tool(
        name = "apply_maintenance_plan",
        description = "Execute a whitelist-only maintenance plan (refile/pin/archive/set_tags/rebuild_*/reembed/compile_source/refresh_stale_wiki/merge_exact_dup/vacuum/…). dry_run defaults to true (preview + ops_log, no mutation). Set dry_run=false to apply. Document-scoped ops capped by max_docs / RAG_MAINT_MAX_DOCS. Returns applied/skipped/errors; ops_log each action. Never hard-deletes layer=raw without params.allow_raw_delete."
    )]
    async fn apply_maintenance_plan(
        &self,
        Parameters(params): Parameters<ApplyMaintenancePlanParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut actions: Vec<MaintenancePlanItem> = Vec::with_capacity(params.actions.len());
        for a in params.actions {
            let kind = maintain::MaintenanceAction::parse(&a.action).ok_or_else(|| {
                Self::map_err(AppError::forbidden(format!(
                    "maintenance action '{}' is not on the whitelist; allowed: {}",
                    a.action.trim(),
                    maintain::ALLOWED_ACTIONS.join(", ")
                )))
            })?;
            actions.push(MaintenancePlanItem {
                action: kind,
                reason: a.reason,
                target_id: a.target_id,
                params: a.params.unwrap_or_else(|| serde_json::json!({})),
            });
        }

        let opts = ApplyPlanOptions {
            dry_run: params.dry_run.unwrap_or(true),
            max_docs: Some(clamp_maintenance_max_docs(
                self.config.maint_max_docs,
                params.max_docs.map(|n| n as usize),
            )),
            agent: params.agent.filter(|s| !s.trim().is_empty()),
        };

        let report = if opts.dry_run {
            let llm = self.llm.as_ref();
            maintain::apply_maintenance_plan(
                &self.store,
                &self.embedder,
                &self.config,
                llm,
                actions,
                &opts,
            )
            .await
        } else {
            let mutation_guard = self
                .store
                .try_corpus_mutation_guard("maintenance plan")
                .map_err(Self::map_err)?;
            let server = self.clone();
            tokio::spawn(async move {
                // This owned task intentionally survives a dropped MCP request:
                // mutations and their terminal FTS refresh keep one lane lease.
                let _mutation_guard = mutation_guard;
                let llm = server.llm.as_ref();
                maintain::apply_maintenance_plan(
                    &server.store,
                    &server.embedder,
                    &server.config,
                    llm,
                    actions,
                    &opts,
                )
                .await
            })
            .await
            .map_err(|error| {
                Self::map_err(AppError::db(format!(
                    "apply_maintenance_plan task failed: {error}"
                )))
            })?
        };
        let report = report.map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "maintain_organize",
        description = "Suggest refiles for documents missing wing (heuristic path/title/embedding; mode=llm|auto uses local chat when enabled). dry_run default true; dry_run=false applies whitelist refile via placement meta only. Optional rebuild_index. Cap: max_docs / RAG_MAINT_MAX_DOCS. Logs ops_log."
    )]
    async fn maintain_organize(
        &self,
        Parameters(params): Parameters<MaintainOrganizeParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut opts = maintain::OrganizeOptions::from_config(&self.config);
        opts.dry_run = params.dry_run.unwrap_or(true);
        if let Some(ref mode) = params.mode {
            opts.mode = maintain::OrganizeMode::parse(mode).map_err(Self::map_err)?;
        }
        if let Some(n) = params.max_docs {
            opts.max_docs =
                clamp_maintenance_max_docs(self.config.maint_max_docs, Some(n as usize));
        }
        if let Some(c) = params.min_confidence {
            opts.min_confidence = c;
        }
        if let Some(v) = params.rebuild_index {
            opts.rebuild_index = v;
        }
        opts.agent = params.agent.filter(|s| !s.trim().is_empty());

        let report = if opts.dry_run {
            let llm = self.llm.as_ref();
            maintain::maintain_organize(&self.store, &self.config, llm, &opts).await
        } else {
            let server = self.clone();
            tokio::spawn(async move {
                // The maintenance service acquires the exclusive lease inside
                // this owned task. A disconnected MCP client therefore cannot
                // release it while an LLM-derived plan is still being applied.
                let llm = server.llm.as_ref();
                maintain::maintain_organize(&server.store, &server.config, llm, &opts).await
            })
            .await
            .map_err(|error| {
                Self::map_err(AppError::db(format!(
                    "maintain_organize task failed: {error}"
                )))
            })?
        };
        let report = report.map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "maintain_refresh",
        description = "Actualize store after bulk edits: reindex_fts, rebuild_graph (dirty or all), rebuild_wiki_index, optional reembed_all. Whitelist actions only; logs to ops_log. dry_run previews without mutation. Defaults (no flags): fts+dirty graph+wiki index. Cap: RAG_MAINT_MAX_DOCS / max_docs."
    )]
    async fn maintain_refresh(
        &self,
        Parameters(params): Parameters<MaintainRefreshParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_docs = clamp_maintenance_max_docs(
            self.config.maint_max_docs,
            params.max_docs.map(|n| n as usize),
        );
        let flags = MaintainRefreshFlags::from_options(
            params.reindex_fts,
            params.rebuild_graph,
            params.graph_dirty_only,
            params.rebuild_wiki_index,
            params.reembed_all,
            params.dry_run,
            max_docs,
        );
        let report = if flags.dry_run {
            maintain::maintain_refresh(&self.store, &self.embedder, &self.config, flags).await
        } else {
            let mutation_guard = self
                .store
                .try_corpus_mutation_guard("maintenance refresh")
                .map_err(Self::map_err)?;
            let server = self.clone();
            tokio::spawn(async move {
                // See apply_maintenance_plan: cancellation detaches this owned
                // workflow instead of releasing its corpus lease mid-refresh.
                let _mutation_guard = mutation_guard;
                maintain::maintain_refresh(&server.store, &server.embedder, &server.config, flags)
                    .await
            })
            .await
            .map_err(|error| {
                Self::map_err(AppError::db(format!(
                    "maintain_refresh task failed: {error}"
                )))
            })?
        };
        let report = report.map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "maintain_compress",
        description = "Compress store: L0=CHECKPOINT+FTS reindex; L1=exact content_hash merge (keep canonical); L2=near-dup list (merge only with confirm=true). dry_run default true. Never hard-deletes layer=raw without allow_raw_delete (tombstones). Cap: max_docs / RAG_MAINT_MAX_DOCS. Logs ops_log."
    )]
    async fn maintain_compress(
        &self,
        Parameters(params): Parameters<MaintainCompressParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut opts = CompressOptions::from_config(&self.config);
        if let Some(level) = params.level {
            opts.level = level.min(u32::from(u8::MAX)) as u8;
        }
        opts.dry_run = params.dry_run.unwrap_or(true);
        opts.confirm = params.confirm.unwrap_or(false);
        opts.allow_raw_delete = params.allow_raw_delete.unwrap_or(false);
        if let Some(t) = params.near_dup_threshold {
            opts.near_dup_threshold = t;
        }
        if let Some(n) = params.max_docs {
            opts.max_docs =
                clamp_maintenance_max_docs(self.config.maint_max_docs, Some(n as usize));
        }
        opts.validate().map_err(Self::map_err)?;
        let mutation_guard = if opts.dry_run {
            None
        } else {
            Some(
                self.store
                    .try_corpus_mutation_guard("maintenance compression")
                    .map_err(Self::map_err)?,
            )
        };
        let store = self.store.clone();
        let config = self.config.clone();
        let report = tokio::task::spawn_blocking(move || {
            // Keep the exclusive lane in the blocking task itself so request
            // cancellation cannot release it while compression continues.
            let _mutation_guard = mutation_guard;
            maintain::maintain_compress(&store, &config, &opts)
        })
        .await
        .map_err(|error| {
            Self::map_err(AppError::db(format!(
                "maintain_compress task failed: {error}"
            )))
        })?
        .map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "write_wiki_page",
        description = "Create/overwrite a compiled wiki page (layer=wiki, uri wiki://slug); re-embeds and rebuilds graph + index entry."
    )]
    async fn write_wiki_page(
        &self,
        Parameters(params): Parameters<WriteWikiPageParams>,
    ) -> Result<CallToolResult, McpError> {
        self.require_vec_compatible().map_err(Self::map_err)?;
        let if_match = crate::models::resolve_if_match(
            params.if_match_revision,
            params.if_match_etag.as_deref(),
        )
        .map_err(Self::map_err)?;
        let res = wiki::write_wiki_page_command(
            &self.store,
            &self.embedder,
            &self.config,
            wiki::WikiWriteCommand {
                slug: params.slug,
                title: params.title,
                content: params.content,
                kind: params.kind.unwrap_or_else(|| "wiki".into()),
                category: params.category,
                summary: params.summary,
                agent: params.agent,
                options: wiki::WriteWikiOpts {
                    if_match_revision: if_match,
                    ..Default::default()
                },
            },
        )
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&res)
    }

    #[tool(
        name = "update_wiki_page",
        description = "Update an existing wiki page by slug. Omitted kind/category/summary preserve their current values, as do project placement, status, pin/boost state, source ownership and unrelated metadata. Pass if_match_revision/etag from get_wiki_page to avoid clobbering concurrent writes; use write_wiki_page to create a page."
    )]
    async fn update_wiki_page(
        &self,
        Parameters(params): Parameters<WriteWikiPageParams>,
    ) -> Result<CallToolResult, McpError> {
        self.require_vec_compatible().map_err(Self::map_err)?;
        let if_match = crate::models::resolve_if_match(
            params.if_match_revision,
            params.if_match_etag.as_deref(),
        )
        .map_err(Self::map_err)?;
        let res = wiki::update_wiki_page_cas(
            &self.store,
            &self.embedder,
            &self.config,
            &params.slug,
            Some(&params.title),
            &params.content,
            params.kind.as_deref(),
            params.category.as_deref(),
            params.summary.as_deref(),
            params.agent.as_deref(),
            if_match,
        )
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&res)
    }

    #[tool(
        name = "get_wiki_page",
        description = "Fetch a wiki page by slug, wiki://uri, or document id. Returns revision+etag for If-Match on write."
    )]
    async fn get_wiki_page(
        &self,
        Parameters(params): Parameters<GetWikiPageParams>,
    ) -> Result<CallToolResult, McpError> {
        let key = params.id_or_slug.trim().to_owned();
        let store = self.store.clone();
        let doc = run_blocking("get_wiki_page", move || {
            if let Some(document) = store.get_document(&key)? {
                return Ok(document);
            }
            if let Some(document) = store.find_by_uri(&key)? {
                return Ok(document);
            }
            store
                .find_by_uri(&format!("wiki://{key}"))?
                .ok_or_else(|| AppError::not_found(format!("wiki page not found: {key}")))
        })
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&document_with_etag(&doc))
    }

    #[tool(
        name = "list_wiki_pages",
        description = "List lean layer=wiki catalog rows without page bodies. Filters: q?, kind?, category?, wing?, room?; limit defaults 50 (hard cap 200), offset defaults 0."
    )]
    async fn list_wiki_pages(
        &self,
        Parameters(params): Parameters<ListWikiPagesParams>,
    ) -> Result<CallToolResult, McpError> {
        let filter = WikiPageMetaFilter {
            q: nonempty_opt(params.q),
            limit: Some(
                params
                    .limit
                    .map(|value| value as usize)
                    .unwrap_or(DEFAULT_CATALOG_PAGE_SIZE)
                    .clamp(1, MAX_CATALOG_PAGE_SIZE),
            ),
            offset: Some(params.offset.unwrap_or(0) as usize),
            kind: nonempty_opt(params.kind),
            category: nonempty_opt(params.category),
            wing: nonempty_opt(params.wing),
            room: nonempty_opt(params.room),
        };
        let store = self.store.clone();
        let (rows, _) = run_blocking("list_wiki_pages", move || {
            store.list_wiki_page_metas_filtered(&filter)
        })
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&rows)
    }

    #[tool(
        name = "compile_source",
        description = "Local LLM (Ollama chat) compiles a raw document into wiki pages. dry_run proposes without write."
    )]
    async fn compile_source(
        &self,
        Parameters(params): Parameters<CompileSourceParams>,
    ) -> Result<CallToolResult, McpError> {
        self.require_vec_compatible().map_err(Self::map_err)?;
        if !self.config.llm_enabled {
            return Err(Self::map_err(AppError::config(
                "LLM disabled (RAG_LLM_ENABLED=false)",
            )));
        }
        let llm = ChatClient::new(
            self.config.llm_base_url.clone(),
            self.config.llm_api_key.clone(),
            self.config.llm_model.clone(),
        )
        .map_err(Self::map_err)?;
        let res = wiki::compile_source(
            &self.store,
            &self.embedder,
            &self.config,
            &llm,
            &params.source_id_or_uri,
            params.dry_run.unwrap_or(false),
            params.agent.as_deref(),
        )
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&res)
    }

    #[tool(
        name = "consolidate",
        description = "Local LLM merges N document texts into one wiki page proposal (concept/entity). apply=false (default) returns proposal only; apply=true writes wiki page, rebuilds graph+index, links sources via related edges, ops_log. Capped by RAG_MAINT_MAX_DOCS / max_docs. Optional slug/title/kind/category overrides."
    )]
    async fn consolidate(
        &self,
        Parameters(params): Parameters<ConsolidateParams>,
    ) -> Result<CallToolResult, McpError> {
        let apply = params.apply.unwrap_or(false);
        if apply {
            self.require_vec_compatible().map_err(Self::map_err)?;
        }
        let llm = self.require_llm().map_err(Self::map_err)?;
        let opts = wiki::ConsolidateOpts {
            slug: params.slug,
            title: params.title,
            kind: params.kind,
            category: params.category,
            max_docs: Some(clamp_maintenance_max_docs(
                self.config.maint_max_docs,
                params.max_docs.map(|n| n as usize),
            )),
        };
        let res = wiki::consolidate(
            &self.store,
            &self.embedder,
            &self.config,
            llm,
            &params.document_ids,
            apply,
            opts,
            params.agent.as_deref(),
        )
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&res)
    }

    #[tool(
        name = "list_memory_lifecycle_candidates",
        description = "List durable memory lifecycle candidates without content. Defaults to active items; optional status/layer/kind filters and limit. Deterministic and does not require an LLM."
    )]
    async fn list_memory_lifecycle_candidates(
        &self,
        Parameters(params): Parameters<ListMemoryLifecycleCandidatesParams>,
    ) -> Result<CallToolResult, McpError> {
        let rows = memory_lifecycle::list_candidates(
            &self.store,
            params.status.as_deref(),
            params.layer.as_deref(),
            params.kind.as_deref(),
            params.limit.unwrap_or(100) as usize,
        )
        .map_err(Self::map_err)?;
        Self::json_result(&rows)
    }

    #[tool(
        name = "consolidate_memory_items",
        description = "Idempotently mark selected memory documents consolidated into an existing output document. Adds structured provenance to the output and source metadata; no LLM or content rewrite."
    )]
    async fn consolidate_memory_items(
        &self,
        Parameters(params): Parameters<ConsolidateMemoryItemsParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = memory_lifecycle::consolidate_selected(
            &self.store,
            &params.document_ids,
            &params.output_document_id,
            params.agent.as_deref(),
        )
        .map_err(Self::map_err)?;
        Self::json_result(&result)
    }

    #[tool(
        name = "archive_memory_items",
        description = "Idempotently archive selected memory documents. Already archived ids are reported as skipped; missing ids are structured in the result. No LLM required."
    )]
    async fn archive_memory_items(
        &self,
        Parameters(params): Parameters<ArchiveMemoryItemsParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = memory_lifecycle::archive_selected(
            &self.store,
            &params.document_ids,
            params.agent.as_deref(),
        )
        .map_err(Self::map_err)?;
        Self::json_result(&result)
    }

    #[tool(
        name = "lint_wiki",
        description = "Lint compiled wiki and graph link health: missing/stale index, broken or duplicate wikilinks, unresolved stubs, orphan pages/documents, self-links, uncompiled raw, and aggregate health counts."
    )]
    async fn lint_wiki(&self) -> Result<CallToolResult, McpError> {
        let report = wiki::lint_wiki(&self.store).map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "refresh_stale_wiki",
        description = "Find wiki pages older than linked raw parents (graph related / metadata source_* / citations / source: lines). dry_run=true (default) lists for the agent; dry_run=false re-runs compile_source per unique raw when LLM is enabled and ChatClient is present (capped by max_docs / RAG_MAINT_MAX_DOCS). Always logs ops_log."
    )]
    async fn refresh_stale_wiki(
        &self,
        Parameters(params): Parameters<RefreshStaleWikiParams>,
    ) -> Result<CallToolResult, McpError> {
        let dry_run = params.dry_run.unwrap_or(true);
        let max_docs = Some(clamp_maintenance_max_docs(
            self.config.maint_max_docs,
            params.max_docs.map(|n| n as usize),
        ));

        if !dry_run {
            self.require_vec_compatible().map_err(Self::map_err)?;
        }

        let llm_ref = if !dry_run && self.config.llm_enabled {
            self.llm.as_ref()
        } else {
            None
        };

        let res = wiki::refresh_stale_wiki(
            &self.store,
            &self.embedder,
            &self.config,
            llm_ref,
            dry_run,
            max_docs,
            params.agent.as_deref(),
        )
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&res)
    }

    #[tool(
        name = "vacuum_store",
        description = "Safe L0 maintenance: DuckDB CHECKPOINT (flush WAL into main file). Returns db_path and file size bytes_before/bytes_after/bytes_delta when readable. Logs to ops_log. Does not delete rows."
    )]
    async fn vacuum_store(&self) -> Result<CallToolResult, McpError> {
        let report: VacuumStoreReport = self.store.vacuum_store().map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "backup_db",
        description = "Create and verify an MVCC-consistent DuckDB snapshot. CHECKPOINT and snapshot-connection cloning briefly hold the Store mutex; the potentially long database copy does not block normal Store queries. path must be under RAG_INGEST_ROOTS. dry_run=false writes; overwrite defaults false."
    )]
    async fn backup_db(
        &self,
        Parameters(params): Parameters<BackupDbParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = resolve_allowlisted_output_file(&params.path, &self.config.ingest_roots)
            .map_err(Self::map_err)?;
        validate_backup_output_paths(&path, &self.config.ingest_roots, self.store.path())
            .map_err(Self::map_err)?;
        let store = self.store.clone();
        let dry_run = params.dry_run.unwrap_or(false);
        let overwrite = params.overwrite.unwrap_or(false);
        let report =
            tokio::task::spawn_blocking(move || store.backup_database(&path, dry_run, overwrite))
                .await
                .map_err(|error| {
                    Self::map_err(AppError::db(format!("backup task failed: {error}")))
                })?
                .map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "export_bundle",
        description = "Export a bounded in-memory portable JSON/JSONL recovery bundle (maximum 64 MiB, 10,000 documents, 50,000 chunks). path must be under RAG_INGEST_ROOTS; never overwrites unless overwrite=true. For a larger corpus, use backup_db and verify the DuckDB backup."
    )]
    async fn export_bundle(
        &self,
        Parameters(params): Parameters<ExportBundleParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = resolve_allowlisted_output_file(&params.path, &self.config.ingest_roots)
            .map_err(Self::map_err)?;
        refuse_live_database_target(&path, self.store.path()).map_err(Self::map_err)?;
        let format = recovery_format(params.format.as_deref(), &path).map_err(Self::map_err)?;
        let overwrite = params.overwrite.unwrap_or(false);
        let dry_run = params.dry_run.unwrap_or(false);
        let store = self.store.clone();
        let report = run_blocking("export_bundle", move || {
            let existed = path.exists();
            if existed && !overwrite {
                return Err(AppError::conflict(format!(
                    "bundle destination '{}' already exists; set overwrite=true explicitly",
                    path.display()
                )));
            }
            store.portable_recovery_preflight()?;
            let bundle = store.recovery_bundle()?;
            let documents = bundle.documents.len() as u64;
            let chunks = bundle.documents.iter().map(|d| d.chunks.len() as u64).sum();
            let encoded = encode_recovery_bundle(&bundle, format)?;
            let overwritten = if dry_run {
                existed && overwrite
            } else {
                publish_recovery_artifact(&path, &encoded, overwrite)?
            };
            Ok(BundleExportReport {
                success: true,
                dry_run,
                path: path.display().to_string(),
                format: format.into(),
                overwritten,
                documents,
                chunks,
                bytes: Some(encoded.len() as u64),
                errors: Vec::new(),
            })
        })
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "export_vault",
        description = "Export a git-friendly Markdown vault partitioned by projects/<wing>/<room>/<layer>, with .rag graph, ops log, and embedding manifest JSON. path must be under RAG_INGEST_ROOTS. dry_run defaults true; overwrite retains the prior vault as a dated sibling."
    )]
    async fn export_vault(
        &self,
        Parameters(params): Parameters<ExportVaultParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = resolve_allowlisted_output_file(&params.path, &self.config.ingest_roots)
            .map_err(Self::map_err)?;
        refuse_live_database_target(&path, self.store.path()).map_err(Self::map_err)?;
        let store = self.store.clone();
        let dry_run = params.dry_run.unwrap_or(true);
        let overwrite = params.overwrite.unwrap_or(false);
        let report = run_blocking("export_vault", move || {
            store.export_vault(&path, dry_run, overwrite)
        })
        .await
        .map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "import_bundle",
        description = "Import a bounded in-memory portable JSON/JSONL recovery bundle (maximum 64 MiB, 10,000 documents, 50,000 chunks) from RAG_INGEST_ROOTS. dry_run defaults true. conflict_policy is error (default), skip, or overwrite; overwrite must be explicit. Legacy v1 chunks require reembed_legacy=true. For a larger corpus, use a verified DuckDB backup."
    )]
    async fn import_bundle(
        &self,
        Parameters(params): Parameters<ImportBundleParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = resolve_allowlisted_output_file(&params.path, &self.config.ingest_roots)
            .map_err(Self::map_err)?;
        if !path.is_file() {
            return Err(Self::map_err(AppError::not_found(format!(
                "bundle file '{}'",
                path.display()
            ))));
        }
        let format = recovery_format(params.format.as_deref(), &path).map_err(Self::map_err)?;
        let policy =
            ConflictPolicy::parse(params.conflict_policy.as_deref()).map_err(Self::map_err)?;
        let read_path = path.clone();
        let bundle = run_blocking("read recovery bundle", move || {
            let input = read_recovery_bundle_file(&read_path)?;
            decode_recovery_bundle(&input, format)
        })
        .await
        .map_err(Self::map_err)?;
        let dry_run = params.dry_run.unwrap_or(true);
        let mut mutation_guard = if dry_run {
            None
        } else {
            Some(
                self.store
                    .try_corpus_mutation_guard("bundle import")
                    .map_err(Self::map_err)?,
            )
        };
        let prepared = prepare_recovery_bundle_for_import(
            bundle,
            params.reembed_legacy.unwrap_or(false),
            dry_run,
            &self.embedder,
            &self.config,
        )
        .await
        .map_err(Self::map_err)?;
        let PreparedRecoveryBundle {
            bundle,
            legacy_bundle_version,
            embeddings_reembed_planned,
            embeddings_reembedded,
        } = prepared;
        let store = self.store.clone();
        let import_path = path.clone();
        let mut report = if let Some(guard) = mutation_guard.take() {
            let (guard, result) = retain_mutation_guard_while_blocking(guard, move || {
                store.import_recovery_bundle(&bundle, policy, dry_run, &import_path, format)
            })
            .await
            .map_err(|error| {
                Self::map_err(AppError::db(format!("import_bundle task failed: {error}")))
            })?;
            mutation_guard = Some(guard);
            result.map_err(Self::map_err)?
        } else {
            run_blocking("import_bundle", move || {
                store.import_recovery_bundle(&bundle, policy, dry_run, &import_path, format)
            })
            .await
            .map_err(Self::map_err)?
        };
        report.legacy_bundle_version = legacy_bundle_version;
        report.legacy_reembed_requested = params.reembed_legacy.unwrap_or(false);
        report.embeddings_reembed_planned = embeddings_reembed_planned;
        report.embeddings_reembedded = embeddings_reembedded;
        if report.success
            && !report.dry_run
            && (report.documents_overwritten > 0 || report.chunks_inserted > 0)
        {
            let guard = mutation_guard
                .take()
                .expect("non-dry bundle import owns the mutation lane");
            let outcome = self
                .refresh_fts_after_mutation("import_bundle", guard)
                .await;
            mutation_guard = outcome.guard;
            if let Some(error) = outcome.error {
                report.success = false;
                report.errors.push(error);
            }
        }
        let _mutation_guard = mutation_guard;
        Self::json_result(&report)
    }
}

fn recovery_format(requested: Option<&str>, path: &Path) -> Result<&'static str, AppError> {
    let value = requested
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .or_else(|| {
            path.extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
        })
        .unwrap_or_else(|| "json".into());
    match value.as_str() {
        "json" => Ok("json"),
        "jsonl" | "ndjson" => Ok("jsonl"),
        other => Err(AppError::config(format!(
            "invalid bundle format '{other}': expected json or jsonl"
        ))),
    }
}

async fn prepare_recovery_bundle_for_import(
    mut bundle: RecoveryBundle,
    reembed_legacy: bool,
    dry_run: bool,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
) -> Result<PreparedRecoveryBundle, AppError> {
    preflight_recovery_bundle(&bundle)?;
    if bundle.format != "rag-recovery-bundle" {
        return Err(AppError::config(format!(
            "unsupported recovery bundle format/version: {}/{}",
            bundle.format, bundle.version
        )));
    }
    if bundle.version == BUNDLE_VERSION {
        return Ok(PreparedRecoveryBundle {
            bundle,
            legacy_bundle_version: None,
            embeddings_reembed_planned: 0,
            embeddings_reembedded: 0,
        });
    }
    if bundle.version != LEGACY_RECOVERY_BUNDLE_VERSION {
        return Err(AppError::config(format!(
            "unsupported recovery bundle format/version: {}/{}",
            bundle.format, bundle.version
        )));
    }

    let locations = bundle
        .documents
        .iter()
        .enumerate()
        .flat_map(|(document_index, document)| {
            (0..document.chunks.len()).map(move |chunk_index| (document_index, chunk_index))
        })
        .collect::<Vec<_>>();
    let planned = u64::try_from(locations.len())
        .map_err(|_| AppError::embeddings("legacy recovery bundle has too many chunks"))?;
    if !locations.is_empty() && !reembed_legacy {
        return Err(AppError::embeddings(format!(
            "legacy recovery bundle v1 contains {planned} chunks with unverifiable embedding identity; retry import_bundle with reembed_legacy=true to replace every vector with the live embedding provider"
        )));
    }

    let expected_dims = config.embedding_dims;
    preflight_recovery_bundle_reembed(&bundle, expected_dims)?;
    if !locations.is_empty() && embedder.dimensions() != expected_dims {
        return Err(AppError::embeddings(format!(
            "live embedder reports dims={}, but config requires dims={expected_dims}",
            embedder.dimensions()
        )));
    }
    let mut reembedded = 0u64;
    if dry_run {
        for &(document_index, chunk_index) in &locations {
            bundle.documents[document_index].chunks[chunk_index].embedding =
                vec![0.0; expected_dims];
        }
    } else {
        for batch in locations.chunks(LEGACY_RECOVERY_REEMBED_BATCH_SIZE) {
            let texts = batch
                .iter()
                .map(|&(document_index, chunk_index)| {
                    bundle.documents[document_index].chunks[chunk_index]
                        .content
                        .clone()
                })
                .collect::<Vec<_>>();
            let embeddings = embedder.embed(&texts).await?;
            if embeddings.len() != batch.len() {
                return Err(AppError::embeddings(format!(
                    "legacy bundle re-embed returned {} vectors for {} chunks",
                    embeddings.len(),
                    batch.len()
                )));
            }
            if let Some((index, embedding)) = embeddings
                .iter()
                .enumerate()
                .find(|(_, embedding)| embedding.len() != expected_dims)
            {
                return Err(AppError::embeddings(format!(
                    "legacy bundle re-embed vector {index} has dims={}, expected {expected_dims}",
                    embedding.len()
                )));
            }
            for (&(document_index, chunk_index), embedding) in batch.iter().zip(embeddings) {
                bundle.documents[document_index].chunks[chunk_index].embedding = embedding;
                reembedded += 1;
            }
        }
    }
    bundle.embedding_manifest = if locations.is_empty() {
        None
    } else {
        Some(embedding_manifest_from_config(config))
    };
    bundle.version = BUNDLE_VERSION;

    Ok(PreparedRecoveryBundle {
        bundle,
        legacy_bundle_version: Some(LEGACY_RECOVERY_BUNDLE_VERSION),
        embeddings_reembed_planned: planned,
        embeddings_reembedded: reembedded,
    })
}

fn pack_hit_to_search_hit(h: PackHitParams) -> SearchHit {
    SearchHit::from(h)
}

/// Parse optional MCP timestamp string into UTC.
///
/// Accepts RFC3339 and common SQL / ISO forms (`YYYY-MM-DD[T ]HH:MM:SS[.f]`, date-only).
/// Empty / whitespace-only → `None`. Invalid → [`AppError::Config`] (maps to invalid_params).
pub(crate) fn parse_optional_ts(raw: Option<&str>) -> Result<Option<DateTime<Utc>>, AppError> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(Some(dt.with_timezone(&Utc)));
    }
    const FORMATS: &[&str] = &[
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d",
    ];
    for fmt in FORMATS {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return Ok(Some(naive.and_utc()));
        }
        if *fmt == "%Y-%m-%d" {
            if let Ok(d) = chrono::NaiveDate::parse_from_str(s, fmt) {
                if let Some(ndt) = d.and_hms_opt(0, 0, 0) {
                    return Ok(Some(ndt.and_utc()));
                }
            }
        }
    }
    Err(AppError::config(format!(
        "invalid timestamp (use RFC3339 or YYYY-MM-DD[ HH:MM:SS]): {s}"
    )))
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
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let surface = self.config.tool_surface;
        let tools = self
            .tool_router
            .list_all()
            .into_iter()
            .filter(|t| surface::tool_allowed(surface, t.name.as_ref()))
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

#[derive(Debug, Serialize)]
struct ReadIndexResult {
    entries: Vec<WikiIndexEntry>,
    count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    markdown: Option<String>,
}

#[derive(Debug, Serialize)]
struct RebuildIndexResult {
    count: usize,
}

/// Raw-layer inventory row for `list_sources`.
#[derive(Debug, Serialize)]
struct SourceSummary {
    id: String,
    title: String,
    uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    room: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_file: Option<String>,
    layer: String,
    kind: String,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct ChunkView {
    id: String,
    chunk_index: i32,
    content: String,
    char_start: i32,
    char_end: i32,
}

impl From<retrieval::ChunkText> for ChunkView {
    fn from(chunk: retrieval::ChunkText) -> Self {
        Self {
            id: chunk.id,
            chunk_index: chunk.chunk_index,
            content: chunk.content,
            char_start: chunk.char_start,
            char_end: chunk.char_end,
        }
    }
}

#[derive(Debug, Serialize)]
struct DocumentDetail {
    id: String,
    uri: String,
    title: String,
    content: String,
    metadata_json: String,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_download_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks: Option<Vec<ChunkView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks_truncated: Option<bool>,
}

impl From<DocumentWithChunks> for DocumentDetail {
    fn from(value: DocumentWithChunks) -> Self {
        let document = value.document;
        let source_download_path = document
            .source_file
            .as_ref()
            .map(|_| format!("/v1/source-file?document_id={}", document.id));
        Self {
            id: document.id,
            uri: document.uri,
            title: document.title,
            content: document.content,
            metadata_json: document.metadata_json,
            created_at: document.created_at.to_rfc3339(),
            updated_at: document.updated_at.to_rfc3339(),
            source_file: document.source_file,
            source_download_path,
            chunks_total: value.chunks_total,
            chunks_truncated: value.chunks_truncated,
            chunks: value
                .chunks
                .map(|chunks| chunks.into_iter().map(ChunkView::from).collect()),
        }
    }
}

/// Full raw-layer source for `get_source`.
#[derive(Debug, Serialize)]
struct SourceDetail {
    id: String,
    uri: String,
    title: String,
    content: String,
    metadata_json: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    room: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_download_path: Option<String>,
    layer: String,
    kind: String,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks: Option<Vec<ChunkView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks_truncated: Option<bool>,
}

impl From<DocumentWithChunks> for SourceDetail {
    fn from(value: DocumentWithChunks) -> Self {
        let document = value.document;
        let source_download_path = document
            .source_file
            .as_ref()
            .map(|_| format!("/v1/source-file?document_id={}", document.id));
        Self {
            id: document.id,
            uri: document.uri,
            title: document.title,
            content: document.content,
            metadata_json: document.metadata_json,
            content_hash: document.content_hash,
            wing: document.wing,
            room: document.room,
            source_file: document.source_file,
            source_download_path,
            layer: document.layer,
            kind: document.kind,
            created_at: document.created_at.to_rfc3339(),
            updated_at: document.updated_at.to_rfc3339(),
            chunks_total: value.chunks_total,
            chunks_truncated: value.chunks_truncated,
            chunks: value
                .chunks
                .map(|chunks| chunks.into_iter().map(ChunkView::from).collect()),
        }
    }
}

#[derive(Debug, Serialize)]
struct FindNodeResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    node: Option<GraphNode>,
    matches: Vec<GraphNode>,
}

#[derive(Debug, Serialize)]
struct GraphExpandSearchResult {
    hits: Vec<SearchHit>,
    graph: GraphView,
}

#[derive(Debug, Serialize)]
struct ListTunnelsResult {
    count: u64,
    tunnels: Vec<GraphEdge>,
}

#[derive(Debug, Serialize)]
struct DeleteTunnelResult {
    deleted: bool,
    tunnel_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbedder;
    use crate::models::SearchMode;
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn maintenance_max_docs_never_exceeds_configured_boundary() {
        assert_eq!(clamp_maintenance_max_docs(50, None), 50);
        assert_eq!(clamp_maintenance_max_docs(50, Some(0)), 1);
        assert_eq!(clamp_maintenance_max_docs(50, Some(12)), 12);
        assert_eq!(clamp_maintenance_max_docs(50, Some(500)), 50);
        assert_eq!(clamp_maintenance_max_docs(0, Some(500)), 1);
    }

    struct CountingEmbedder {
        calls: Arc<AtomicUsize>,
        dims: usize,
    }

    struct GatedEmbedder {
        started: Arc<tokio::sync::Semaphore>,
        release: Arc<tokio::sync::Semaphore>,
        dims: usize,
    }

    #[async_trait]
    impl EmbeddingProvider for CountingEmbedder {
        async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(vec![vec![1.0; self.dims]; texts.len()])
        }

        fn dimensions(&self) -> usize {
            self.dims
        }
    }

    #[async_trait]
    impl EmbeddingProvider for GatedEmbedder {
        async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
            self.started.add_permits(1);
            let permit = self
                .release
                .acquire()
                .await
                .expect("release semaphore open");
            permit.forget();
            Ok(vec![vec![0.25; self.dims]; texts.len()])
        }

        fn dimensions(&self) -> usize {
            self.dims
        }
    }

    fn test_config(db_path: PathBuf, roots: Vec<PathBuf>, dims: usize) -> Config {
        Config {
            db_path,
            embedding_provider: crate::config::EmbeddingProviderKind::Mock,
            embedding_base_url: "https://api.openai.com/v1".to_string(),
            embedding_api_key: String::new(),
            embedding_model: "mock".to_string(),
            embedding_dims: dims,
            chunk_size: 800,
            chunk_overlap: 120,
            default_top_k: 5,
            ingest_roots: roots,
            max_context_tokens: 4096,
            max_chunks_per_doc: 3,
            fts_stemmer: "porter".to_string(),
            default_search_mode: SearchMode::Vec,
            llm_base_url: "http://127.0.0.1:11434/v1".to_string(),
            llm_provider: crate::llm::LlmProviderKind::Ollama,
            llm_model: "llama3.2".to_string(),
            llm_api_key: "ollama".to_string(),
            llm_enabled: false,
            llm_timeout_secs: 120,
            llm_max_tokens: 4096,
            maint_max_docs: 50,
            maint_near_dup_threshold: 0.92,
            tool_surface: crate::mcp::ToolSurface::Full,
            http_bind: None,
            wiki_require_if_match: false,
        }
    }

    #[test]
    fn status_and_doctor_empty_corpus() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("status.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 32usize;
        let config = test_config(path.clone(), Vec::new(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);

        let status = server.status_report().expect("status");
        assert_eq!(status.backend, "duckdb");
        assert_eq!(status.schema_version, SCHEMA_VERSION);
        assert_eq!(status.document_count, 0);
        assert_eq!(status.chunk_count, 0);
        assert!(status.wings.is_empty());
        assert!(!status.ready_for_search);
        assert!(!status.ingest_roots_configured);
        assert_eq!(status.embed_dims, dims);

        let doctor = server.doctor_report().expect("doctor");
        assert!(doctor.schema_ok);
        assert!(doctor.embed_ok);
        assert!(doctor.ok);
        assert!(!doctor.ready_for_search);
        assert!(!doctor.ingest_roots_configured);
        assert_eq!(doctor.expected_schema_version, SCHEMA_VERSION);
        // Server start records embedding_manifest from config when missing.
        assert_eq!(doctor.manifest_dims, Some(dims as i32));
    }

    #[tokio::test]
    async fn doctor_repair_rebuilds_missing_chunks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("doctor-repair.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 16usize;
        let config = test_config(path, Vec::new(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);
        let now = Utc::now();
        server
            .store
            .upsert_document(&Document {
                id: "missing-chunks".into(),
                uri: "file://missing.md".into(),
                title: "Missing chunks".into(),
                content: "This document must become searchable after repair.".into(),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                layer: "raw".into(),
                kind: "document".into(),
                ..Default::default()
            })
            .unwrap();
        server.store.ensure_fts(&server.config.fts_stemmer).unwrap();
        assert_eq!(server.doctor_report().unwrap().documents_without_chunks, 1);

        server
            .doctor_repair(Parameters(DoctorRepairParams {
                dry_run: Some(true),
                max_docs: Some(10),
            }))
            .await
            .unwrap();
        assert!(server
            .store
            .list_chunks_for_document("missing-chunks")
            .unwrap()
            .is_empty());

        let search_guard = server.store.corpus_mutation_lane().read_owned().await;
        let busy = server
            .doctor_repair(Parameters(DoctorRepairParams {
                dry_run: Some(false),
                max_docs: Some(10),
            }))
            .await
            .unwrap_err();
        assert_eq!(busy.data.as_ref().unwrap()["code"], "STORE_BUSY");
        drop(search_guard);

        server
            .doctor_repair(Parameters(DoctorRepairParams {
                dry_run: Some(false),
                max_docs: Some(10),
            }))
            .await
            .unwrap();
        assert!(!server
            .store
            .list_chunks_for_document("missing-chunks")
            .unwrap()
            .is_empty());
        assert_eq!(server.doctor_report().unwrap().documents_without_chunks, 0);
        let generation_after_repair = {
            let conn = server.store.lock().unwrap();
            crate::db::fts::fts_generation_state(&conn).unwrap()
        };
        assert!(!generation_after_repair.dirty);
        let hits = crate::db::search::search(
            &server.store,
            &SearchQuery {
                mode: SearchMode::Lex,
                query_text: Some("searchable".into()),
                top_k: 5,
                fts_stemmer: server.config.fts_stemmer.clone(),
                ..SearchQuery::default()
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        let generation_after_search = {
            let conn = server.store.lock().unwrap();
            crate::db::fts::fts_generation_state(&conn).unwrap()
        };
        assert_eq!(generation_after_search, generation_after_repair);
    }

    #[tokio::test]
    async fn import_bundle_finishes_with_generation_clean_fts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("import-target.duckdb");
        let bundle_path = dir.path().join("bundle.json");
        let dims = 8usize;
        let config = test_config(path.clone(), vec![dir.path().to_path_buf()], dims);
        let store = Store::open(&path).expect("open");
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);
        server.store.ensure_fts(&server.config.fts_stemmer).unwrap();
        let body = "portable searchable recovery body";
        let bundle = RecoveryBundle {
            format: "rag-recovery-bundle".into(),
            version: BUNDLE_VERSION,
            exported_at: Utc::now(),
            embedding_manifest: Some(crate::db::store::embedding_manifest_from_config(
                &server.config,
            )),
            documents: vec![BundleDocument {
                document: Document {
                    id: "imported-document".into(),
                    uri: "recovery://imported-document".into(),
                    title: "Imported document".into(),
                    content: body.into(),
                    layer: "wiki".into(),
                    kind: "wiki".into(),
                    ..Document::default()
                },
                chunks: vec![Chunk {
                    id: "imported-chunk".into(),
                    document_id: "imported-document".into(),
                    chunk_index: 0,
                    content: body.into(),
                    embedding: vec![0.2; dims],
                    char_start: 0,
                    char_end: body.len() as i32,
                    metadata_json: "{}".into(),
                }],
            }],
        };
        std::fs::write(
            &bundle_path,
            encode_recovery_bundle(&bundle, "json").unwrap(),
        )
        .unwrap();

        let search_guard = server.store.corpus_mutation_lane().read_owned().await;
        let busy = server
            .import_bundle(Parameters(ImportBundleParams {
                path: bundle_path.display().to_string(),
                format: Some("json".into()),
                conflict_policy: Some("error".into()),
                dry_run: Some(false),
                reembed_legacy: None,
            }))
            .await
            .unwrap_err();
        assert_eq!(busy.data.as_ref().unwrap()["code"], "STORE_BUSY");
        drop(search_guard);

        let result = server
            .import_bundle(Parameters(ImportBundleParams {
                path: bundle_path.display().to_string(),
                format: Some("json".into()),
                conflict_policy: Some("error".into()),
                dry_run: Some(false),
                reembed_legacy: None,
            }))
            .await
            .expect("import bundle");
        let result_json: serde_json::Value =
            serde_json::from_str(&result.content[0].as_text().expect("text result").text).unwrap();
        assert_eq!(result_json["success"], true);
        assert_eq!(result_json["durable_mutation_committed"], true);
        assert_eq!(result_json["documents_inserted"], 1);
        let generation_after_import = {
            let conn = server.store.lock().unwrap();
            crate::db::fts::fts_generation_state(&conn).unwrap()
        };
        assert!(!generation_after_import.dirty);

        let _hits = crate::db::search::search(
            &server.store,
            &SearchQuery {
                mode: SearchMode::Lex,
                query_text: Some("portable".into()),
                top_k: 5,
                fts_stemmer: server.config.fts_stemmer.clone(),
                ..SearchQuery::default()
            },
        )
        .unwrap();
        let generation_after_search = {
            let conn = server.store.lock().unwrap();
            crate::db::fts::fts_generation_state(&conn).unwrap()
        };
        assert_eq!(generation_after_search, generation_after_import);
    }

    #[tokio::test]
    async fn legacy_bundle_requires_opt_in_and_reembeds_before_import() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("legacy-import-target.duckdb");
        let bundle_path = dir.path().join("legacy-v1.json");
        let dims = 8usize;
        let config = test_config(path.clone(), vec![dir.path().to_path_buf()], dims);
        let store = Store::open(&path).expect("open");
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);
        let body = "legacy vector must be replaced";
        let legacy_vector = vec![42.0; dims];
        let bundle = RecoveryBundle {
            format: "rag-recovery-bundle".into(),
            version: LEGACY_RECOVERY_BUNDLE_VERSION,
            exported_at: Utc::now(),
            embedding_manifest: None,
            documents: vec![BundleDocument {
                document: Document {
                    id: "legacy-document".into(),
                    uri: "recovery://legacy-document".into(),
                    title: "Legacy document".into(),
                    content: body.into(),
                    ..Document::default()
                },
                chunks: vec![Chunk {
                    id: "legacy-chunk".into(),
                    document_id: "legacy-document".into(),
                    chunk_index: 0,
                    content: body.into(),
                    embedding: legacy_vector.clone(),
                    char_start: 0,
                    char_end: body.len() as i32,
                    metadata_json: "{}".into(),
                }],
            }],
        };
        std::fs::write(
            &bundle_path,
            encode_recovery_bundle(&bundle, "json").unwrap(),
        )
        .unwrap();

        let refused = server
            .import_bundle(Parameters(ImportBundleParams {
                path: bundle_path.display().to_string(),
                format: Some("json".into()),
                conflict_policy: Some("error".into()),
                dry_run: Some(false),
                reembed_legacy: Some(false),
            }))
            .await
            .unwrap_err();
        assert!(refused.message.contains("reembed_legacy=true"));
        assert!(server
            .store
            .get_document("legacy-document")
            .unwrap()
            .is_none());

        let result = server
            .import_bundle(Parameters(ImportBundleParams {
                path: bundle_path.display().to_string(),
                format: Some("json".into()),
                conflict_policy: Some("error".into()),
                dry_run: Some(false),
                reembed_legacy: Some(true),
            }))
            .await
            .expect("legacy import with explicit re-embed");
        let result_json: serde_json::Value =
            serde_json::from_str(&result.content[0].as_text().expect("text result").text).unwrap();
        assert_eq!(result_json["success"], true);
        assert_eq!(result_json["legacy_bundle_version"], 1);
        assert_eq!(result_json["legacy_reembed_requested"], true);
        assert_eq!(result_json["embeddings_reembed_planned"], 1);
        assert_eq!(result_json["embeddings_reembedded"], 1);
        let chunks = server
            .store
            .list_chunks_for_document("legacy-document")
            .unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].embedding.len(), dims);
        assert_ne!(chunks[0].embedding, legacy_vector);
        assert!(server
            .store
            .embedding_manifest_matches_config(&server.config)
            .unwrap());
    }

    #[tokio::test]
    async fn legacy_reembed_preflight_rejects_oversized_vectors_before_provider_call() {
        let root = tempfile::tempdir().unwrap();
        let dimensions =
            usize::try_from(crate::db::recovery::PORTABLE_RECOVERY_MAX_BYTES / 64 + 1).unwrap();
        let config = test_config(root.path().join("unused.duckdb"), Vec::new(), dimensions);
        let calls = Arc::new(AtomicUsize::new(0));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(CountingEmbedder {
            calls: Arc::clone(&calls),
            dims: dimensions,
        });
        let bundle = RecoveryBundle {
            format: "rag-recovery-bundle".into(),
            version: LEGACY_RECOVERY_BUNDLE_VERSION,
            exported_at: Utc::now(),
            embedding_manifest: None,
            documents: vec![BundleDocument {
                document: Document {
                    id: "legacy-large-vector".into(),
                    uri: "recovery://legacy-large-vector".into(),
                    title: "Legacy".into(),
                    content: "body".into(),
                    ..Document::default()
                },
                chunks: vec![Chunk {
                    id: "legacy-large-vector-chunk".into(),
                    document_id: "legacy-large-vector".into(),
                    chunk_index: 0,
                    content: "body".into(),
                    embedding: Vec::new(),
                    char_start: 0,
                    char_end: 4,
                    metadata_json: "{}".into(),
                }],
            }],
        };

        let error = match prepare_recovery_bundle_for_import(bundle, true, true, &embedder, &config)
            .await
        {
            Ok(_) => panic!("oversized prospective vectors must be refused"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("verified DuckDB backup"));
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn bulk_maintenance_apply_paths_refuse_active_guarded_search() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("maintenance-lane.duckdb");
        let dims = 8usize;
        let config = test_config(path.clone(), Vec::new(), dims);
        let store = Store::open(&path).expect("open");
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);
        let search_guard = server.store.corpus_mutation_lane().read_owned().await;

        let plan_error = server
            .apply_maintenance_plan(Parameters(ApplyMaintenancePlanParams {
                actions: vec![crate::mcp::tools::MaintenanceActionParams {
                    action: "reindex_fts".into(),
                    reason: None,
                    target_id: None,
                    params: None,
                }],
                dry_run: Some(false),
                max_docs: None,
                agent: None,
            }))
            .await
            .unwrap_err();
        assert_eq!(plan_error.data.as_ref().unwrap()["code"], "STORE_BUSY");

        let organize_error = server
            .maintain_organize(Parameters(MaintainOrganizeParams {
                dry_run: Some(false),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(organize_error.data.as_ref().unwrap()["code"], "STORE_BUSY");

        let refresh_error = server
            .maintain_refresh(Parameters(MaintainRefreshParams {
                reindex_fts: Some(true),
                dry_run: Some(false),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(refresh_error.data.as_ref().unwrap()["code"], "STORE_BUSY");

        let compress_error = server
            .maintain_compress(Parameters(MaintainCompressParams {
                level: Some(0),
                dry_run: Some(false),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(compress_error.data.as_ref().unwrap()["code"], "STORE_BUSY");

        drop(search_guard);

        // The write guard moves into the blocking compression task and must
        // not deadlock or outlive a normally completed request.
        server
            .maintain_compress(Parameters(MaintainCompressParams {
                level: Some(0),
                dry_run: Some(false),
                ..Default::default()
            }))
            .await
            .expect("compression after guarded search");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropped_blocking_waiter_keeps_transferred_mutation_guard() {
        let lane = Arc::new(tokio::sync::RwLock::new(()));
        let mutation_guard = lane
            .clone()
            .try_write_owned()
            .expect("initial mutation guard");
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        let waiter = tokio::spawn(async move {
            retain_mutation_guard_while_blocking(mutation_guard, move || {
                let _ = started_tx.send(());
                release_rx.recv().expect("release blocking tail");
            })
            .await
        });
        started_rx.await.expect("blocking tail started");
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(
            lane.clone().try_write_owned().is_err(),
            "detached blocking tail must retain the mutation guard"
        );

        release_tx.send(()).expect("release blocking tail");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(guard) = lane.clone().try_write_owned() {
                    drop(guard);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("mutation guard released after blocking tail");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropped_apply_request_leaves_owned_mutation_workflow_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("owned-maintenance.duckdb");
        let dims = 8usize;
        let store = Store::open(&path).expect("open");
        let config = test_config(path, Vec::new(), dims);
        store.ensure_embedding_manifest(&config).unwrap();
        store
            .upsert_document(&Document {
                id: "owned-doc".into(),
                uri: "wiki://owned-doc".into(),
                title: "Owned document".into(),
                content: "owned workflow searchable body".into(),
                layer: "wiki".into(),
                kind: "wiki".into(),
                ..Document::default()
            })
            .expect("seed document");
        store
            .insert_chunks(&[Chunk {
                id: "owned-chunk".into(),
                document_id: "owned-doc".into(),
                chunk_index: 0,
                content: "owned workflow searchable body".into(),
                embedding: vec![0.0; dims],
                char_start: 0,
                char_end: 30,
                metadata_json: "{}".into(),
            }])
            .expect("seed chunk");
        let started = Arc::new(tokio::sync::Semaphore::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(GatedEmbedder {
            started: started.clone(),
            release: release.clone(),
            dims,
        });
        let server = RagServer::new(store, embedder, config);
        let request_server = server.clone();
        let waiter = tokio::spawn(async move {
            request_server
                .apply_maintenance_plan(Parameters(ApplyMaintenancePlanParams {
                    actions: vec![crate::mcp::tools::MaintenanceActionParams {
                        action: "reembed".into(),
                        reason: None,
                        target_id: Some("owned-doc".into()),
                        params: None,
                    }],
                    dry_run: Some(false),
                    max_docs: None,
                    agent: None,
                }))
                .await
        });
        let started_permit =
            tokio::time::timeout(std::time::Duration::from_secs(2), started.acquire())
                .await
                .expect("embedding started in time")
                .expect("started semaphore open");
        started_permit.forget();

        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        let lane = server.store.corpus_mutation_lane();
        assert!(
            lane.clone().try_write_owned().is_err(),
            "owned child must retain the lane after its request waiter is dropped"
        );

        release.add_permits(1);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(guard) = lane.clone().try_write_owned() {
                    drop(guard);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owned maintenance workflow completed");
        let chunks = server
            .store
            .list_chunks_for_document("owned-doc")
            .expect("reembedded chunks");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].embedding, vec![0.25; dims]);
    }

    #[tokio::test]
    async fn retrieval_helpers_multi_get_expand_and_find_similar() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("retrieval-helpers.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 16usize;
        let config = test_config(path, Vec::new(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);
        let first = server
            .ingest_pipeline(IngestCommand {
                text: "alpha architecture and duckdb reliability ".repeat(80),
                title: Some("Alpha".into()),
                uri: Some("file://alpha.md".into()),
                metadata_json: None,
                wing: Some("rag".into()),
                room: Some("docs".into()),
                source_file: None,
                layer: "raw".into(),
                kind: "document".into(),
                immutable: false,
            })
            .await
            .unwrap();
        let second = server
            .ingest_pipeline(IngestCommand {
                text: "alpha architecture with database recovery ".repeat(80),
                title: Some("Beta".into()),
                uri: Some("file://beta.md".into()),
                metadata_json: None,
                wing: Some("rag".into()),
                room: Some("docs".into()),
                source_file: None,
                layer: "raw".into(),
                kind: "document".into(),
                immutable: false,
            })
            .await
            .unwrap();

        server
            .multi_get(Parameters(MultiGetParams {
                document_ids: vec![
                    first.document_id.clone(),
                    second.document_id.clone(),
                    "missing".into(),
                ],
                include_chunks: Some(false),
                chunk_limit: None,
            }))
            .await
            .unwrap();
        server
            .expand_chunks(Parameters(ExpandChunksParams {
                document_id: first.document_id.clone(),
                chunk_index: 0,
                radius: Some(1),
            }))
            .await
            .unwrap();
        server
            .find_similar(Parameters(FindSimilarParams {
                document_id: first.document_id,
                top_k: Some(2),
                wing: Some("rag".into()),
                room: None,
            }))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn llm_status_reports_config_and_unreachable_when_no_server() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("llm_status.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 32usize;
        // Closed local port so the probe fails fast without a live Ollama.
        let mut config = test_config(path.clone(), Vec::new(), dims);
        config.llm_enabled = false;
        config.llm_timeout_secs = 2;
        config.llm_base_url = "http://127.0.0.1:1/v1".to_string();
        config.llm_model = "test-model".to_string();
        config.embedding_model = "mock-embed".to_string();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);

        assert!(server.llm.is_some(), "ChatClient should wire from config");

        let report = server.llm_status_report().await.expect("llm_status");
        assert!(!report.llm_enabled);
        assert_eq!(report.base_url, "http://127.0.0.1:1/v1");
        assert_eq!(report.model, "test-model");
        assert!(!report.reachable);
        assert!(report.error.is_some());
        assert_eq!(report.embed_provider, "mock");
        assert_eq!(report.embed_model, "mock-embed");
        assert_eq!(report.embed_dims, dims);
        assert!(report.embed_base_url.is_none());
    }

    #[test]
    fn doctor_detects_manifest_dim_mismatch_and_roots() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("doctor.duckdb");
        let store = Store::open(&path).expect("open");
        store
            .set_embedding_manifest(&crate::models::EmbeddingManifest {
                id: "default".into(),
                provider: "mock".into(),
                model: "mock".into(),
                dims: 8,
                base_url: None,
                content_fingerprint: None,
                updated_at: Utc::now(),
            })
            .expect("manifest");

        let dims = 32usize;
        let config = test_config(path.clone(), vec![PathBuf::from("/allowed")], dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);

        let doctor = server.doctor_report().expect("doctor");
        assert!(!doctor.embed_ok);
        assert!(!doctor.ok);
        assert_eq!(doctor.manifest_dims, Some(8));
        assert!(doctor.ingest_roots_configured);
        assert!(!doctor.ready_for_search);

        let status = server.status_report().expect("status");
        assert!(status.ingest_roots_configured);
    }

    #[tokio::test]
    async fn status_ready_for_search_after_ingest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ready.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 32usize;
        let config = test_config(path.clone(), Vec::new(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);

        server
            .ingest_pipeline(IngestCommand {
                text: "hello searchable world".into(),
                title: Some("t".into()),
                uri: Some("text://ready".into()),
                metadata_json: None,
                wing: None,
                room: None,
                source_file: None,
                layer: "raw".into(),
                kind: "document".into(),
                immutable: false,
            })
            .await
            .expect("ingest");

        let status = server.status_report().expect("status");
        assert_eq!(status.backend, "duckdb");
        assert!(status.chunk_count > 0);
        assert!(status.ready_for_search);
        assert_eq!(status.document_count, 1);
        assert!(status.node_count >= 1);

        let doctor = server.doctor_report().expect("doctor");
        assert!(doctor.ready_for_search);
        assert!(doctor.ok);
    }

    #[tokio::test]
    async fn list_wings_rooms_taxonomy_and_status_wings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("taxonomy.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 32usize;
        let config = test_config(path.clone(), Vec::new(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);

        for (text, uri, wing, room) in [
            ("alpha body", "raw://a", "research", "rag"),
            ("beta body", "raw://b", "research", "eval"),
            ("gamma body", "raw://c", "ops", "runbooks"),
        ] {
            server
                .ingest_pipeline(IngestCommand {
                    text: text.into(),
                    title: Some(uri.into()),
                    uri: Some(uri.into()),
                    metadata_json: None,
                    wing: Some(wing.into()),
                    room: Some(room.into()),
                    source_file: None,
                    layer: "raw".into(),
                    kind: "document".into(),
                    immutable: true,
                })
                .await
                .expect("ingest");
        }

        let wings = server.store.list_wings().expect("list_wings");
        assert_eq!(wings.len(), 2);
        assert_eq!(wings[0].wing, "ops");
        assert_eq!(wings[0].document_count, 1);
        assert_eq!(wings[1].wing, "research");
        assert_eq!(wings[1].document_count, 2);

        let rooms = server
            .store
            .list_rooms(Some("research"))
            .expect("list_rooms");
        assert_eq!(rooms.len(), 2);
        assert!(rooms
            .iter()
            .any(|r| r.room == "rag" && r.document_count == 1));
        assert!(rooms
            .iter()
            .any(|r| r.room == "eval" && r.document_count == 1));

        let tax = server.store.get_taxonomy().expect("taxonomy");
        assert_eq!(tax.total_documents, 3);
        assert_eq!(tax.unscoped_count, 0);
        assert_eq!(tax.wings.len(), 2);
        let research = tax.wings.iter().find(|w| w.wing == "research").unwrap();
        assert_eq!(research.document_count, 2);
        assert_eq!(research.rooms.len(), 2);

        let status = server.status_report().expect("status");
        assert_eq!(status.backend, "duckdb");
        assert_eq!(status.document_count, 3);
        assert!(status.chunk_count > 0);
        assert!(status.node_count >= 3);
        assert_eq!(status.wings.len(), 2);
        assert!(status.ready_for_search);
    }

    #[tokio::test]
    async fn update_document_meta_skips_reembed_unless_content_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("meta.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 32usize;
        let config = test_config(path.clone(), Vec::new(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);

        // Wiki layer so body updates are allowed (raw is immutable).
        let ing = server
            .ingest_pipeline(IngestCommand {
                text: "original wiki body about retrieval".into(),
                title: Some("Wiki note".into()),
                uri: Some("wiki://meta-test".into()),
                metadata_json: Some(r#"{"k":1}"#.into()),
                wing: Some("research".into()),
                room: Some("rag".into()),
                source_file: None,
                layer: "wiki".into(),
                kind: "wiki".into(),
                immutable: false,
            })
            .await
            .expect("ingest");
        let chunks_before = server
            .store
            .list_chunks_for_document(&ing.document_id)
            .expect("chunks")
            .len();
        assert!(chunks_before > 0);
        let emb_before = server
            .store
            .list_chunks_for_document(&ing.document_id)
            .expect("chunks")[0]
            .embedding
            .clone();

        // Meta-only: must not re-embed.
        let meta = server
            .update_document_via_service(UpdateDocumentMetaParams {
                document_id: ing.document_id.clone(),
                wing: Some("ops".into()),
                room: Some("runbooks".into()),
                title: Some("Pinned wiki".into()),
                metadata_json: Some(r#"{"k":2,"tag":"x"}"#.into()),
                pinned: Some(true),
                boost: Some(2.5),
                status: Some("active".into()),
                ..Default::default()
            })
            .await
            .expect("meta update");
        assert!(!meta.content_changed);
        assert!(!meta.reembedded);
        assert_eq!(meta.chunk_count, chunks_before);
        assert_eq!(meta.wing.as_deref(), Some("ops"));
        assert_eq!(meta.room.as_deref(), Some("runbooks"));
        assert_eq!(meta.title, "Pinned wiki");
        assert!(meta.pinned);
        assert!((meta.boost - 2.5).abs() < f64::EPSILON);
        assert_eq!(meta.metadata_json, r#"{"k":2,"tag":"x"}"#);

        let emb_after_meta = server
            .store
            .list_chunks_for_document(&ing.document_id)
            .expect("chunks")[0]
            .embedding
            .clone();
        assert_eq!(
            emb_before, emb_after_meta,
            "meta-only update must not change embeddings"
        );

        // Content change: re-embed + rebuild.
        let body = server
            .update_document_via_service(UpdateDocumentMetaParams {
                document_id: ing.document_id.clone(),
                content: Some("revised wiki body about hybrid search and ranking".into()),
                ..Default::default()
            })
            .await
            .expect("content update");
        assert!(body.content_changed);
        assert!(body.reembedded);
        assert!(body.chunk_count > 0);

        // Raw layer refuses content rewrite.
        let raw = server
            .ingest_pipeline(IngestCommand {
                text: "raw immutable body".into(),
                title: Some("raw".into()),
                uri: Some("raw://meta-raw".into()),
                metadata_json: None,
                wing: Some("research".into()),
                room: Some("rag".into()),
                source_file: None,
                layer: "raw".into(),
                kind: "document".into(),
                immutable: true,
            })
            .await
            .expect("raw ingest");
        let refuse = server
            .update_document_via_service(UpdateDocumentMetaParams {
                document_id: raw.document_id,
                content: Some("mutated".into()),
                ..Default::default()
            })
            .await;
        assert!(refuse.is_err());
    }

    #[tokio::test]
    async fn check_duplicate_and_delete_by_source_tools() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("dedupe.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 32usize;
        let config = test_config(path.clone(), Vec::new(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);

        let body = "duplicate candidate body";
        let src = "/vault/shared.md";
        server
            .ingest_pipeline(IngestCommand {
                text: body.into(),
                title: Some("One".into()),
                uri: Some("raw://one".into()),
                metadata_json: None,
                wing: Some("lab".into()),
                room: Some("a".into()),
                source_file: Some(src.into()),
                layer: "raw".into(),
                kind: "document".into(),
                immutable: true,
            })
            .await
            .expect("ingest one");
        server
            .ingest_pipeline(IngestCommand {
                text: "other".into(),
                title: Some("Two".into()),
                uri: Some("raw://two".into()),
                metadata_json: None,
                wing: Some("lab".into()),
                room: Some("b".into()),
                source_file: Some(src.into()),
                layer: "raw".into(),
                kind: "document".into(),
                immutable: true,
            })
            .await
            .expect("ingest two");

        let dup = server
            .store
            .check_duplicate(Some(body), None, None)
            .expect("check_duplicate");
        assert!(dup.is_duplicate);
        assert_eq!(dup.matches.len(), 1);
        assert_eq!(dup.matches[0].uri, "raw://one");

        let by_uri = server
            .store
            .check_duplicate(None, None, Some("raw://two"))
            .expect("uri probe");
        assert!(by_uri.is_duplicate);
        assert_eq!(by_uri.matches[0].match_reason, "uri");

        let deleted = server
            .store
            .delete_by_source(src)
            .expect("delete_by_source");
        assert_eq!(deleted, 2);
        assert_eq!(server.store.list_documents().expect("list").len(), 0);
        assert_eq!(server.store.delete_by_source(src).expect("again"), 0);
    }

    #[tokio::test]
    async fn delete_by_source_preview_matches_archived_and_tombstone_apply_scope() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("delete-source-preview.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 8usize;
        let config = test_config(path, Vec::new(), dims);
        let server = RagServer::new(store, Arc::new(MockEmbedder::new(dims)), config);
        let source = "/vault/all-lifecycle-states.md";
        let now = Utc::now();
        for (id, status) in [
            ("active-source", "active"),
            ("archived-source", "archived"),
            ("tombstone-source", "tombstone"),
        ] {
            server
                .store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("raw://{id}"),
                    title: id.into(),
                    content: format!("body for {id}"),
                    metadata_json: "{}".into(),
                    created_at: now,
                    updated_at: now,
                    source_file: Some(source.into()),
                    layer: "raw".into(),
                    kind: "document".into(),
                    status: status.into(),
                    ..Default::default()
                })
                .unwrap();
        }

        let preview = server
            .delete_by_source(Parameters(DeleteBySourceParams {
                source_file: source.into(),
                dry_run: Some(true),
            }))
            .await
            .expect("preview all lifecycle states");
        let preview: serde_json::Value =
            serde_json::from_str(&preview.content[0].as_text().expect("text result").text).unwrap();
        assert_eq!(preview["match_count"], 3);
        let mut preview_ids = preview["document_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        preview_ids.sort();
        assert_eq!(
            preview_ids,
            vec!["active-source", "archived-source", "tombstone-source"]
        );

        let applied = server
            .delete_by_source(Parameters(DeleteBySourceParams {
                source_file: source.into(),
                dry_run: Some(false),
            }))
            .await
            .expect("apply exact preview scope");
        let applied: serde_json::Value =
            serde_json::from_str(&applied.content[0].as_text().expect("text result").text).unwrap();
        assert_eq!(applied["match_count"], 3);
        assert_eq!(applied["deleted"], 3);
        assert!(server.store.list_documents().unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_by_source_finalization_failure_returns_committed_report() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("delete-source-finalize-failure.duckdb");
        let dims = 8usize;
        let config = test_config(path.clone(), Vec::new(), dims);
        let mut server = RagServer::new(
            Store::open(&path).expect("open"),
            Arc::new(MockEmbedder::new(dims)),
            config,
        );
        let source = "/vault/finalization-failure.md";
        let ingested = server
            .ingest_pipeline(IngestCommand {
                text: "durable deletion before derived-index failure".into(),
                title: Some("Finalization failure".into()),
                uri: Some("raw://finalization-failure".into()),
                metadata_json: None,
                wing: None,
                room: None,
                source_file: Some(source.into()),
                layer: "raw".into(),
                kind: "document".into(),
                immutable: false,
            })
            .await
            .expect("seed document");
        server.store.ensure_fts(&server.config.fts_stemmer).unwrap();
        server.config.fts_stemmer = TEST_FINALIZE_FTS_FAILURE_STEMMER.into();

        let result = server
            .delete_by_source(Parameters(DeleteBySourceParams {
                source_file: source.into(),
                dry_run: Some(false),
            }))
            .await
            .expect("committed mutation returns an aggregate report");
        let result: serde_json::Value =
            serde_json::from_str(&result.content[0].as_text().expect("text result").text).unwrap();
        assert_eq!(result["success"], false);
        assert_eq!(result["durable_mutation_committed"], true);
        assert_eq!(result["deleted"], 1);
        assert!(result["errors"][0]
            .as_str()
            .unwrap()
            .contains("FTS_FINALIZATION_FAILED"));
        assert!(server
            .store
            .get_document(&ingested.document_id)
            .unwrap()
            .is_none());
        let generation = {
            let conn = server.store.lock().unwrap();
            crate::db::fts_generation_state(&conn).unwrap()
        };
        assert!(generation.dirty, "next lexical read must retry FTS");
        assert!(server
            .store
            .list_recent_ops(10)
            .unwrap()
            .iter()
            .any(|entry| entry.op == "fts_finalization_failed"));
    }

    #[tokio::test]
    async fn cleanup_source_duplicates_defaults_safe_requires_confirm_and_respects_lane() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("source-dedupe-tool.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 16usize;
        let config = test_config(path, Vec::new(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);
        let source = "/vault/tool.md";
        let body = "tool duplicate";
        let hash = content_hash(body);
        let now = Utc::now();
        for (id, uri) in [
            ("canonical", "file:///vault/tool.md"),
            ("legacy", "project://tool.md"),
        ] {
            server
                .store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: uri.into(),
                    title: id.into(),
                    content: body.into(),
                    metadata_json: "{}".into(),
                    created_at: now,
                    updated_at: now,
                    source_file: Some(source.into()),
                    content_hash: Some(hash.clone()),
                    layer: "raw".into(),
                    kind: "document".into(),
                    status: "active".into(),
                    ..Default::default()
                })
                .unwrap();
        }
        server.store.ensure_fts(&server.config.fts_stemmer).unwrap();
        let generation_before = {
            let conn = server.store.lock().unwrap();
            crate::db::fts::fts_generation_state(&conn).unwrap()
        };
        assert!(!generation_before.dirty);

        let preview = server
            .cleanup_source_duplicates(Parameters(CleanupSourceDuplicatesParams::default()))
            .await
            .unwrap();
        let preview_json: serde_json::Value =
            serde_json::from_str(&preview.content[0].as_text().expect("text result").text).unwrap();
        assert_eq!(preview_json["dry_run"], true);
        assert_eq!(preview_json["applied"], false);
        assert_eq!(preview_json["candidate_count"], 1);
        assert!(server.store.get_document("legacy").unwrap().is_some());

        let missing_confirm = server
            .cleanup_source_duplicates(Parameters(CleanupSourceDuplicatesParams {
                dry_run: Some(false),
                confirm: Some(false),
                max_candidates: Some(10),
            }))
            .await
            .unwrap_err();
        assert!(missing_confirm.message.contains("confirm=true"));

        let search_guard = server.store.corpus_mutation_lane().read_owned().await;
        let concurrent_preview = server
            .cleanup_source_duplicates(Parameters(CleanupSourceDuplicatesParams::default()))
            .await
            .expect("read-only preview does not own the mutation lane");
        let concurrent_preview: serde_json::Value = serde_json::from_str(
            &concurrent_preview.content[0]
                .as_text()
                .expect("text result")
                .text,
        )
        .unwrap();
        assert_eq!(concurrent_preview["dry_run"], true);
        assert_eq!(concurrent_preview["candidate_count"], 1);

        let busy = server
            .cleanup_source_duplicates(Parameters(CleanupSourceDuplicatesParams {
                dry_run: Some(false),
                confirm: Some(true),
                max_candidates: Some(10),
            }))
            .await
            .unwrap_err();
        assert_eq!(busy.data.as_ref().unwrap()["code"], "STORE_BUSY");
        drop(search_guard);

        let applied = server
            .cleanup_source_duplicates(Parameters(CleanupSourceDuplicatesParams {
                dry_run: Some(false),
                confirm: Some(true),
                max_candidates: Some(10),
            }))
            .await
            .unwrap();
        let applied_json: serde_json::Value =
            serde_json::from_str(&applied.content[0].as_text().expect("text result").text).unwrap();
        assert_eq!(applied_json["applied"], true);
        assert_eq!(applied_json["deleted_documents"], 1);
        assert!(server.store.get_document("legacy").unwrap().is_none());
        let generation_after = {
            let conn = server.store.lock().unwrap();
            crate::db::fts::fts_generation_state(&conn).unwrap()
        };
        assert!(!generation_after.dirty);
        assert_eq!(
            generation_after.chunks_generation,
            generation_before.chunks_generation + 1
        );
        assert_eq!(
            generation_after.index_generation,
            Some(generation_after.chunks_generation)
        );
        assert_eq!(
            generation_after.rebuild_count,
            generation_before.rebuild_count + 1
        );
    }

    #[tokio::test]
    async fn ingest_raw_list_sources_get_source_immutable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("raw.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 32usize;
        let config = test_config(path.clone(), Vec::new(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);

        let r1 = server
            .ingest_pipeline(IngestCommand {
                text: "verbatim source alpha".into(),
                title: Some("Alpha".into()),
                uri: Some("raw://alpha".into()),
                metadata_json: None,
                wing: Some("projects".into()),
                room: Some("lab".into()),
                source_file: Some("/vault/alpha.md".into()),
                layer: "raw".into(),
                kind: "document".into(),
                immutable: true,
            })
            .await
            .expect("ingest_raw");
        assert!(r1.chunk_count >= 1);
        assert!(!r1.document_id.is_empty());

        // Same uri + content: idempotent no-op (stable id).
        let r2 = server
            .ingest_pipeline(IngestCommand {
                text: "verbatim source alpha".into(),
                title: Some("Alpha".into()),
                uri: Some("raw://alpha".into()),
                metadata_json: None,
                wing: Some("projects".into()),
                room: Some("lab".into()),
                source_file: Some("/vault/alpha.md".into()),
                layer: "raw".into(),
                kind: "document".into(),
                immutable: true,
            })
            .await
            .expect("re-ingest same");
        assert_eq!(r2.document_id, r1.document_id);
        assert_eq!(r2.chunk_count, r1.chunk_count);

        // Same uri + different content: conflict under immutable policy.
        let err = server
            .ingest_pipeline(IngestCommand {
                text: "changed content".into(),
                title: Some("Alpha".into()),
                uri: Some("raw://alpha".into()),
                metadata_json: None,
                wing: None,
                room: None,
                source_file: None,
                layer: "raw".into(),
                kind: "document".into(),
                immutable: true,
            })
            .await
            .expect_err("must refuse content change");
        assert!(
            matches!(err, AppError::Conflict(_)),
            "expected Conflict, got {err:?}"
        );

        let sources = server
            .store
            .list_documents_by_layer("raw")
            .expect("list raw");
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].id, r1.document_id);
        assert_eq!(sources[0].layer, "raw");
        assert_eq!(sources[0].wing.as_deref(), Some("projects"));
        assert_eq!(sources[0].room.as_deref(), Some("lab"));

        let got = server
            .store
            .get_document(&r1.document_id)
            .expect("get")
            .expect("present");
        assert_eq!(got.content, "verbatim source alpha");
        assert_eq!(got.layer, "raw");
        assert!(got.content_hash.is_some());
    }

    #[tokio::test]
    async fn ingest_file_respects_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let allowed = dir.path().join("allowed");
        std::fs::create_dir_all(&allowed).expect("mkdir");
        let inside = allowed.join("note.md");
        std::fs::write(&inside, "file body under root").expect("write");

        let outside = dir.path().join("secret.md");
        std::fs::write(&outside, "should not ingest").expect("write outside");

        let db_path = dir.path().join("allow.duckdb");
        let store = Store::open(&db_path).expect("open");
        let dims = 32usize;
        let config = test_config(db_path.clone(), vec![allowed.clone()], dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);

        // Outside root refused before read.
        let err = check_path_allowlist(Path::new(&outside), &server.config.ingest_roots)
            .expect_err("outside");
        assert!(
            err.to_string().contains("outside RAG_INGEST_ROOTS")
                || err.to_string().contains("RAG_INGEST_ROOTS"),
            "{err}"
        );

        // Empty roots refuse all.
        let err_empty = check_path_allowlist(Path::new(&inside), &[]).expect_err("empty");
        assert!(err_empty.to_string().contains("RAG_INGEST_ROOTS"));

        // Inside root is allowed and ingests.
        check_path_allowlist(Path::new(&inside), &server.config.ingest_roots)
            .expect("inside allowed");
        let text = std::fs::read_to_string(&inside).expect("read");
        let result = server
            .ingest_pipeline(IngestCommand {
                text,
                title: Some("note.md".into()),
                uri: Some(format!("file://{}", inside.display())),
                metadata_json: None,
                wing: None,
                room: None,
                source_file: Some(inside.display().to_string()),
                layer: "raw".into(),
                kind: "document".into(),
                immutable: false,
            })
            .await
            .expect("ingest inside");
        assert!(result.chunk_count >= 1);
    }

    #[tokio::test]
    async fn add_drawer_check_duplicate_delete_by_source() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("drawer.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 32usize;
        let config = test_config(path.clone(), Vec::new(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);

        let body = "verbatim drawer body for parity";
        let hash = content_hash(body);

        // Before ingest: not a duplicate.
        let miss = server
            .store
            .check_duplicate(Some(body), None, None)
            .expect("check miss");
        assert!(!miss.is_duplicate);
        assert_eq!(miss.content_hash.as_deref(), Some(hash.as_str()));

        // add_drawer path = ingest with required wing/room + optional source_file.
        let ing = server
            .ingest_pipeline(IngestCommand {
                text: body.into(),
                title: Some("Drawer A".into()),
                uri: Some("drawer://a".into()),
                metadata_json: None,
                wing: Some("projects".into()),
                room: Some("notes".into()),
                source_file: Some("/vault/drawer-a.md".into()),
                layer: "raw".into(),
                kind: "document".into(),
                immutable: false,
            })
            .await
            .expect("add_drawer ingest");
        assert!(ing.chunk_count >= 1);

        let doc = server
            .store
            .get_document(&ing.document_id)
            .expect("get")
            .expect("present");
        assert_eq!(doc.wing.as_deref(), Some("projects"));
        assert_eq!(doc.room.as_deref(), Some("notes"));
        assert_eq!(doc.source_file.as_deref(), Some("/vault/drawer-a.md"));
        assert_eq!(doc.content_hash.as_deref(), Some(hash.as_str()));

        // check_duplicate by content and by hash.
        let by_content = server
            .store
            .check_duplicate(Some(body), None, None)
            .expect("by content");
        assert!(by_content.is_duplicate);
        assert_eq!(by_content.matches.len(), 1);
        assert_eq!(by_content.matches[0].id, ing.document_id);

        let by_hash = server
            .store
            .check_duplicate(None, Some(&hash), None)
            .expect("by hash");
        assert!(by_hash.is_duplicate);

        server.store.ensure_fts(&server.config.fts_stemmer).unwrap();

        // dry_run delete_by_source leaves rows and the clean generation intact.
        let dry = server
            .delete_by_source(Parameters(DeleteBySourceParams {
                source_file: "/vault/drawer-a.md".into(),
                dry_run: Some(true),
            }))
            .await
            .expect("delete_by_source preview");
        let dry_json: serde_json::Value =
            serde_json::from_str(&dry.content[0].as_text().expect("text result").text).unwrap();
        assert_eq!(dry_json["match_count"], 1);
        assert_eq!(dry_json["deleted"], 0);
        assert!(server
            .store
            .get_document(&ing.document_id)
            .expect("get")
            .is_some());

        let search_guard = server.store.corpus_mutation_lane().read_owned().await;
        let busy = server
            .delete_by_source(Parameters(DeleteBySourceParams {
                source_file: "/vault/drawer-a.md".into(),
                dry_run: Some(false),
            }))
            .await
            .unwrap_err();
        assert_eq!(busy.data.as_ref().unwrap()["code"], "STORE_BUSY");
        drop(search_guard);

        let deleted = server
            .delete_by_source(Parameters(DeleteBySourceParams {
                source_file: "/vault/drawer-a.md".into(),
                dry_run: Some(false),
            }))
            .await
            .expect("delete_by_source apply");
        let deleted_json: serde_json::Value =
            serde_json::from_str(&deleted.content[0].as_text().expect("text result").text).unwrap();
        assert_eq!(deleted_json["deleted"], 1);
        assert!(server
            .store
            .get_document(&ing.document_id)
            .expect("get after")
            .is_none());
        assert!(
            !server
                .store
                .check_duplicate(Some(body), None, None)
                .expect("after delete")
                .is_duplicate
        );
        let generation_after_delete = {
            let conn = server.store.lock().unwrap();
            crate::db::fts::fts_generation_state(&conn).unwrap()
        };
        assert!(!generation_after_delete.dirty);
    }

    #[tokio::test]
    async fn single_reembed_refuses_identity_migration_until_full_reembed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("reembed.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 16usize;
        let config = test_config(path.clone(), Vec::new(), dims);
        let calls = Arc::new(AtomicUsize::new(0));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(CountingEmbedder {
            calls: calls.clone(),
            dims,
        });
        let server = RagServer::new(store, embedder, config);

        let ingest = server
            .ingest_pipeline(IngestCommand {
                text: "reembed me please".into(),
                title: Some("re".into()),
                uri: Some("text://reembed".into()),
                metadata_json: None,
                wing: None,
                room: None,
                source_file: None,
                layer: "raw".into(),
                kind: "document".into(),
                immutable: false,
            })
            .await
            .expect("ingest");

        let calls_after_ingest = calls.load(Ordering::Relaxed);
        let chunks_before = server
            .store
            .list_chunks_for_document(&ingest.document_id)
            .expect("chunks before mismatch");
        let generation_before = {
            let conn = server.store.lock().unwrap();
            crate::db::fts::chunks_generation(&conn).unwrap()
        };

        // Same provider/model/dims is still a mismatch when a persisted
        // fingerprint says the vectors came from another endpoint identity.
        let mut stale_manifest = crate::db::store::embedding_manifest_from_config(&server.config);
        stale_manifest.content_fingerprint = Some("stale-fingerprint".into());
        server
            .store
            .set_embedding_manifest(&stale_manifest)
            .expect("force mismatch");

        let err = server.require_vec_compatible().expect_err("must refuse");
        let msg = err.to_string();
        assert!(
            msg.contains("fingerprint") && msg.contains("uncapped reembed_all"),
            "unexpected: {msg}"
        );

        let error = server
            .reembed_document_pipeline(&ingest.document_id)
            .await
            .expect_err("single-document refresh must not migrate identity");
        assert!(error
            .to_string()
            .contains("reembed_document cannot migrate embedding identity"));
        assert_eq!(calls.load(Ordering::Relaxed), calls_after_ingest);
        assert_eq!(
            server
                .store
                .list_chunks_for_document(&ingest.document_id)
                .unwrap()
                .iter()
                .map(|chunk| &chunk.embedding)
                .collect::<Vec<_>>(),
            chunks_before
                .iter()
                .map(|chunk| &chunk.embedding)
                .collect::<Vec<_>>()
        );
        let generation_after_refusal = {
            let conn = server.store.lock().unwrap();
            crate::db::fts::chunks_generation(&conn).unwrap()
        };
        assert_eq!(generation_after_refusal, generation_before);
        assert_eq!(
            server
                .store
                .get_embedding_manifest()
                .unwrap()
                .unwrap()
                .content_fingerprint,
            stale_manifest.content_fingerprint
        );

        let migrated = crate::maintain::reembed_all(
            &server.store,
            &server.embedder,
            &server.config,
            usize::MAX,
        )
        .await
        .expect("complete reembed migration");
        assert_eq!(migrated.documents_failed, 0);
        assert_eq!(migrated.skipped_cap, 0);
        assert_eq!(migrated.documents_succeeded, 1);

        server
            .require_vec_compatible()
            .expect("compatible after complete reembed");
        let chunks = server
            .store
            .list_chunks_for_document(&ingest.document_id)
            .expect("chunks");
        assert!(chunks.iter().all(|c| c.embedding.len() == dims));
    }

    #[test]
    fn parse_optional_ts_accepts_common_forms() {
        assert!(parse_optional_ts(None).unwrap().is_none());
        assert!(parse_optional_ts(Some("  ")).unwrap().is_none());
        let d = parse_optional_ts(Some("2024-03-15")).unwrap().unwrap();
        assert_eq!(d.date_naive().to_string(), "2024-03-15");
        let r = parse_optional_ts(Some("2024-03-15T12:00:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(r.timestamp(), 1_710_504_000);
        let err = parse_optional_ts(Some("not-a-date")).unwrap_err();
        assert!(matches!(err, AppError::Config(_)));
        // Config maps to invalid_params for MCP clients.
        let mcp = RagServer::map_err(err);
        assert_eq!(mcp.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn recovery_jsonl_requires_one_typed_manifest_before_documents() {
        let missing = decode_recovery_bundle("", "jsonl").unwrap_err();
        assert!(missing.to_string().contains("missing its manifest"));

        let document_first = serde_json::json!({
            "record_type": "document",
            "value": {"document": Document::default(), "chunks": []}
        })
        .to_string();
        let error = decode_recovery_bundle(&document_first, "jsonl").unwrap_err();
        assert!(error.to_string().contains("before the manifest"));

        let header = serde_json::json!({
            "record_type": "manifest",
            "format": "rag-recovery-bundle",
            "version": 1,
            "exported_at": Utc::now(),
        })
        .to_string();
        let decoded = decode_recovery_bundle(&header, "jsonl").unwrap();
        assert_eq!(decoded.version, LEGACY_RECOVERY_BUNDLE_VERSION);
        assert!(decoded.embedding_manifest.is_none());

        let duplicate = format!("{header}\n{header}\n");
        let error = decode_recovery_bundle(&duplicate, "jsonl").unwrap_err();
        assert!(error.to_string().contains("duplicate JSONL manifest"));

        let overflow = serde_json::json!({
            "record_type": "manifest",
            "format": "rag-recovery-bundle",
            "version": u64::from(u32::MAX) + 1,
            "exported_at": Utc::now(),
        })
        .to_string();
        let error = decode_recovery_bundle(&overflow, "jsonl").unwrap_err();
        assert!(error.to_string().contains("exceeds u32"));
    }

    #[tokio::test]
    async fn recovery_tools_refuse_oversized_io_without_publishing_an_artifact() {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("bounded.duckdb");
        let store = Store::open(&db_path).unwrap();
        let config = test_config(db_path, vec![root.path().to_path_buf()], 32);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(32));
        let server = RagServer::new(store, embedder, config);

        let oversized_input = root.path().join("oversized.json");
        let file = std::fs::File::create(&oversized_input).unwrap();
        file.set_len(crate::db::recovery::PORTABLE_RECOVERY_MAX_BYTES + 1)
            .unwrap();
        let import_error = server
            .import_bundle(Parameters(ImportBundleParams {
                path: oversized_input.display().to_string(),
                format: Some("json".into()),
                conflict_policy: None,
                dry_run: Some(true),
                reembed_legacy: None,
            }))
            .await
            .unwrap_err();
        assert!(import_error.message.contains("verified DuckDB backup"));

        {
            let conn = server.store.lock().unwrap();
            conn.execute(
                r#"
                INSERT INTO documents (
                    id, uri, title, content, metadata_json, created_at, updated_at
                )
                SELECT
                    'portable-limit-' || CAST(i AS VARCHAR),
                    'recovery://portable-limit/' || CAST(i AS VARCHAR),
                    'limit', '', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
                FROM range(0, ?) AS generated(i)
                "#,
                [
                    i64::try_from(crate::db::recovery::PORTABLE_RECOVERY_MAX_DOCUMENTS + 1)
                        .unwrap(),
                ],
            )
            .unwrap();
        }
        let output = root.path().join("must-not-exist.json");
        let export_error = server
            .export_bundle(Parameters(ExportBundleParams {
                path: output.display().to_string(),
                format: Some("json".into()),
                dry_run: Some(false),
                overwrite: Some(false),
            }))
            .await
            .unwrap_err();
        assert!(export_error.message.contains("contains 10001 documents"));
        assert!(export_error.message.contains("verified DuckDB backup"));
        assert!(!output.exists());
    }

    #[test]
    fn busy_error_is_structured_retryable_and_resource_safe() {
        let mcp = RagServer::map_err(AppError::busy(
            "exclusive corpus mutation is active; retry search after it completes",
        ));
        assert_eq!(mcp.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
        assert_eq!(
            mcp.data,
            Some(serde_json::json!({
                "code": "STORE_BUSY",
                "retryable": true,
                "retry_after_ms": 1_000,
            }))
        );
        assert!(!mcp.message.contains("http"));
        assert!(!mcp.message.contains("/Users/"));
    }

    #[tokio::test]
    async fn multi_query_search_fails_busy_before_embedding_any_rewrite() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("multi-query-busy.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 2;
        let calls = Arc::new(AtomicUsize::new(0));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(CountingEmbedder {
            calls: calls.clone(),
            dims,
        });
        let mut config = test_config(path, Vec::new(), dims);
        config.default_search_mode = SearchMode::Hybrid;
        let server = RagServer::new(store, embedder, config);
        let sync_guard = server.store.corpus_mutation_lane().write_owned().await;

        let error = server
            .multi_query_search(Parameters(MultiQuerySearchParams {
                queries: vec!["original".into(), "rewrite".into()],
                top_k: Some(5),
                mode: Some("hybrid".into()),
                wing: Some("project".into()),
                room: None,
                layer: None,
                source_file: None,
                timeout_ms: Some(5_000),
            }))
            .await
            .unwrap_err();
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(error.data.as_ref().unwrap()["code"], "STORE_BUSY");
        assert_eq!(error.data.as_ref().unwrap()["retryable"], true);
        drop(sync_guard);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn vector_search_holds_corpus_guard_across_async_embedding() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vector-search-guard.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 2;
        let started = Arc::new(tokio::sync::Semaphore::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(GatedEmbedder {
            started: started.clone(),
            release: release.clone(),
            dims,
        });
        let server = RagServer::new(store, embedder, test_config(path, Vec::new(), dims));
        let request_server = server.clone();
        let search_task = tokio::spawn(async move {
            request_server
                .search(Parameters(SearchParams {
                    query: "guarded query".into(),
                    top_k: Some(5),
                    document_id: None,
                    mode: Some("vec".into()),
                    min_score: None,
                    wing: None,
                    room: None,
                    layer: None,
                    source_file: None,
                    include_archived: None,
                    diversity: None,
                    group_by: None,
                    recency_half_life_days: None,
                    max_context_tokens: None,
                    max_chunks_per_document: None,
                    context_expansion: None,
                    neighbor_chunks: None,
                    timeout_ms: Some(5_000),
                    rrf_k: None,
                }))
                .await
        });
        let started_permit =
            tokio::time::timeout(std::time::Duration::from_secs(2), started.acquire())
                .await
                .expect("embedding started in time")
                .expect("started semaphore open");
        started_permit.forget();

        let mutation_error = match server
            .store
            .try_corpus_mutation_guard("embedding migration")
        {
            Ok(_) => panic!("vector search must retain the corpus read guard while embedding"),
            Err(error) => error,
        };
        assert!(matches!(mutation_error, AppError::Busy(_)));

        release.add_permits(1);
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), search_task)
            .await
            .expect("vector search finished in time")
            .expect("vector search task did not panic");
        assert!(result.is_ok());
    }

    #[test]
    fn kg_store_roundtrip_via_server() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("kg_mcp.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 16usize;
        let config = test_config(path.clone(), Vec::new(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);

        let f = server
            .store
            .kg_add(
                "Alice",
                "works_at",
                "Acme",
                parse_optional_ts(Some("2020-01-01")).unwrap(),
                None,
                Some("doc:1"),
                Some(0.95),
                None,
            )
            .expect("kg_add");
        assert_eq!(f.status, "active");

        let hits = server
            .store
            .kg_query(Some("Alice"), Some("works_at"), None, None)
            .expect("kg_query");
        assert_eq!(hits.len(), 1);

        let new_f = server
            .store
            .kg_supersede(
                "Alice",
                "works_at",
                "Acme",
                "BetaCo",
                parse_optional_ts(Some("2024-06-01")).unwrap(),
                None,
                None,
            )
            .expect("kg_supersede");
        assert_eq!(new_f.object, "BetaCo");

        let timeline = server.store.kg_timeline("Alice").expect("timeline");
        assert_eq!(timeline.len(), 2);

        let stats = server.store.kg_stats().expect("stats");
        assert_eq!(stats.total_facts, 2);
        assert_eq!(stats.active_facts, 1);
        assert_eq!(stats.superseded_facts, 1);
    }
}

/// Merge computed `etag` into a document JSON body for tools.
fn document_with_etag(doc: &Document) -> serde_json::Value {
    let mut v = serde_json::to_value(doc).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert("etag".into(), serde_json::json!(doc.etag()));
    }
    v
}

#[derive(Debug, Serialize)]
struct PackContextResult {
    hits: Vec<SearchHit>,
    total_tokens: usize,
    max_tokens: usize,
    omitted_count: usize,
    context_text: String,
}
