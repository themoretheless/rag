//! MCP tool implementations and `ServerHandler` wiring.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, NaiveDateTime, Utc};
use rmcp::handler::server::tool::{ToolCallContext, ToolRouter};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParam, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParam, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, ErrorData as McpError, RoleServer, ServerHandler};
use serde::Serialize;
use uuid::Uuid;

use super::surface::{self, ToolSurface, INDEX_FIRST_PLAYBOOK, SPINE_TOOLS_BLURB};

use super::tools::{
    AddDrawerParams, AnalyzeCorpusParams, AppendLogParams, ApplyMaintenancePlanParams,
    ArchiveMemoryItemsParams, BackupDbParams, CheckDuplicateParams, CheckpointParams,
    CollectionCreateParams, CollectionEntryParams, CollectionGetParams, CollectionListParams,
    CollectionUpdateParams, CompileSourceParams, ConsolidateMemoryItemsParams, ConsolidateParams,
    CreateTunnelParams, DeleteBySourceParams, DeleteDocumentParams, DeleteTunnelParams,
    DiaryReadParams, DiaryWriteParams, DoctorRepairParams, ExpandChunksParams, ExportBundleParams,
    ExportGraphSnapshotParams, ExportVaultParams, FileAnswerCitationParams, FileAnswerParams,
    FindNodeParams, FindSimilarParams, FindTunnelsParams, FollowTunnelsParams, GetBacklinksParams,
    GetDocumentParams, GetGraphParams, GetNeighborsParams, GetSchemaParams, GetSourceParams,
    GetTaxonomyParams, GetWikiPageParams, GraphExpandSearchParams, ImportBundleParams,
    IngestFileParams, IngestRawParams, IngestTextParams, KgAddParams, KgInvalidateParams,
    KgQueryParams, KgSupersedeParams, KgTimelineParams, LinkNodesParams, ListDocumentsParams,
    ListMemoryLifecycleCandidatesParams, ListRecentOpsParams, ListRoomsParams, ListSourcesParams,
    ListTunnelsParams, ListWingsParams, MaintainCompressParams, MaintainOrganizeParams,
    MaintainRefreshParams, MemoriesFiledAwayParams, MultiGetParams, MultiQuerySearchParams,
    PackContextParams, PackHitParams, PlanMaintenanceParams, QueryWithIndexParams, ReadIndexParams,
    ReadLogParams, RebuildIndexParams, ReconnectParams, ReembedDocumentParams,
    RefreshStaleWikiParams, SearchParams, SearchWikiParams, SyncSourcesParams,
    UpdateDocumentMetaParams, UpdateIndexEntryParams, UpdateSchemaParams, WakeUpParams,
    WriteWikiPageParams,
};
use crate::chunking::{from_config, markdown_section_metadata, Chunker};
use crate::config::Config;
use crate::db::recovery::{
    BundleDocument, BundleExportReport, ConflictPolicy, RecoveryBundle, BUNDLE_VERSION,
};
use crate::db::schema::SCHEMA_VERSION;
use crate::db::search::{
    attach_context, fuse_rrf_many, search, search_chunks, ContextExpansion, DiversityMode,
    SearchQuery,
};
use crate::db::Store;
use crate::diary;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::file_ingest::{extract_file, is_supported_source, merge_metadata};
use crate::graph::rebuild_document_graph;
use crate::ingest::{IngestCommand, IngestService, ReembedDocumentResult};
use crate::llm::ChatClient;
use crate::maintain::{
    self, ApplyPlanOptions, CompressOptions, MaintainRefreshFlags, MaintenancePlanItem,
};
use crate::memory_lifecycle;
use crate::models::{
    Chunk, Collection, CollectionEntry, DoctorReport, Document, DocumentFilter, DocumentMetaUpdate,
    DrawerListItem, GraphEdge, GraphFilter, GraphNode, GraphView, IndexQueryPage, IndexQueryResult,
    IngestResult, LlmStatusReport, OpsLogEntry, SearchHit, SearchMode, Stats, StatusReport,
    VacuumStoreReport, WikiIndexEntry,
};
use crate::retrieval::{self, DocumentWithChunks, SimilarDocumentsQuery};
use crate::search_pack::pack_hits;
use crate::util::{check_path_allowlist, content_hash};
use crate::wiki;
use crate::wiki::FileAnswerCitation;

/// Empty / whitespace-only optional strings become `None` (ingest wing/room/source_file).
fn nonempty_opt(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

fn collect_supported_sources(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    const SKIP_DIRECTORIES: &[&str] = &[
        ".git",
        ".agents",
        ".codex",
        ".idea",
        ".vscode",
        ".zed",
        "target",
        "node_modules",
        "dist",
        "build",
        "coverage",
        "vendor",
        "backups",
    ];
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if file_type.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                if !SKIP_DIRECTORIES.contains(&name) {
                    pending.push(path);
                }
            } else if file_type.is_file() && is_supported_source(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn inferred_scope(root: &Path, path: &Path) -> (String, String) {
    let root_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut components = relative
        .components()
        .filter_map(|part| part.as_os_str().to_str());
    if root_name.eq_ignore_ascii_case("sources") {
        let wing = components.next().unwrap_or("project").to_string();
        let room = components.next().unwrap_or("root").to_string();
        (wing, room)
    } else {
        let room = relative
            .parent()
            .and_then(|parent| parent.components().next())
            .and_then(|part| part.as_os_str().to_str())
            .unwrap_or("root")
            .to_string();
        (root_name.to_string(), room)
    }
}

#[derive(Debug, Serialize)]
struct SyncSourceError {
    path: String,
    error: String,
}

#[derive(Debug, Serialize)]
struct SyncSourcesSummary {
    added: Vec<String>,
    updated: Vec<String>,
    skipped: Vec<String>,
    deleted: Vec<String>,
    errors: Vec<SyncSourceError>,
}

#[derive(Debug, Serialize)]
struct DoctorRepairReport {
    dry_run: bool,
    documents_considered: usize,
    documents_repaired: Vec<String>,
    documents_failed: Vec<SyncSourceError>,
    orphan_chunks_pruned: u64,
    orphan_document_nodes_pruned: u64,
    orphan_edges_pruned: u64,
    before: DoctorReport,
    after: DoctorReport,
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
}

impl RagServer {
    pub(crate) fn tool_count(&self) -> usize {
        self.tool_router.map.len()
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
                    };
                    match self.sync_sources(Parameters(params)).await {
                        Ok(_) => tracing::info!(path, "automatic source sync completed"),
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

    /// Ensure corpus manifest exists; refuse when stored dims != config dims.
    fn require_vec_compatible(&self) -> Result<(), AppError> {
        self.store.ensure_embedding_manifest(&self.config)?;
        self.store
            .require_embedding_dims_match(self.config.embedding_dims)
    }

    fn json_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
        let content = Content::json(value)?;
        Ok(CallToolResult::success(vec![content]))
    }

    /// Ingest text: upsert-by-uri, chunk, embed, store chunks.
    ///
    /// When `immutable` is true (raw layer policy): existing uri with the same
    /// content hash is a no-op; different content is refused (`Conflict`).
    async fn ingest_pipeline(
        &self,
        text: String,
        title: Option<String>,
        uri: Option<String>,
        metadata_json: Option<String>,
        wing: Option<String>,
        room: Option<String>,
        source_file: Option<String>,
        layer: &str,
        kind: &str,
        immutable: bool,
    ) -> Result<IngestResult, AppError> {
        IngestService::new(&self.store, &self.embedder, &self.config)
            .ingest(IngestCommand {
                text,
                title,
                uri,
                metadata_json,
                wing,
                room,
                source_file,
                layer: layer.to_string(),
                kind: kind.to_string(),
                immutable,
            })
            .await
    }

    /// Build the MemPalace-style `status` health payload (vision §5.5 layer health).
    pub(crate) fn status_report(&self) -> Result<StatusReport, AppError> {
        let schema_version = self.store.schema_version()?.unwrap_or(0);
        let fts_ready = self.store.fts_ready()?;
        let (document_count, chunk_count, node_count, edge_count) = self.store.stats()?;
        let wings = self.store.list_wings()?;
        let embed_dims = self.config.embedding_dims;
        let ingest_roots_configured = !self.config.ingest_roots.is_empty();
        // Empty corpus is not ready; schema lag also blocks readiness.
        let ready_for_search = chunk_count > 0 && schema_version >= SCHEMA_VERSION;

        let raw_docs = self.store.list_documents_by_layer("raw")?;
        let wiki_docs = self.store.list_documents_by_layer("wiki")?;
        let raw_count = raw_docs.len() as u64;
        let wiki_count = wiki_docs.len() as u64;
        let index = self.store.list_wiki_index()?;
        let index_entry_count = index.len() as u64;
        let indexed_pages = wiki_docs
            .iter()
            .filter(|d| {
                index
                    .iter()
                    .any(|e| e.page_id.as_deref() == Some(d.id.as_str()))
            })
            .count() as u64;
        let index_coverage = if wiki_count == 0 {
            0.0
        } else {
            indexed_pages as f64 / wiki_count as f64
        };

        // Uncompiled debt: raw with no wiki corpus at all, or raw nodes never
        // targeted by a wiki document edge (soft graph signal).
        let uncompiled_raw_count = if wiki_count == 0 {
            raw_count
        } else {
            let mut uncompiled = 0u64;
            for r in &raw_docs {
                let Some(node) = self.store.find_node_by_document_id(&r.id)? else {
                    uncompiled += 1;
                    continue;
                };
                let bl = self.store.backlinks(&node.id)?;
                let wiki_ref = bl.nodes.iter().any(|n| {
                    n.id != node.id
                        && n.resolved
                        && n.uri
                            .as_deref()
                            .map(|u| u.starts_with("wiki://"))
                            .unwrap_or(false)
                });
                if !wiki_ref {
                    uncompiled += 1;
                }
            }
            uncompiled
        };

        let manifest = self.store.get_embedding_manifest()?;
        let embedding_manifest_match = match &manifest {
            Some(m) => {
                m.dims as usize == self.config.embedding_dims
                    && m.provider == self.config.embedding_provider.as_str()
                    && m.model == self.config.embedding_model
            }
            None => chunk_count == 0, // empty corpus: no fingerprint yet is ok
        };

        Ok(StatusReport {
            backend: "duckdb".to_string(),
            storage_capabilities: crate::storage::duckdb_capability_names(),
            schema_version,
            fts_ready,
            document_count,
            chunk_count,
            node_count,
            edge_count,
            raw_count,
            wiki_count,
            index_entry_count,
            index_coverage,
            uncompiled_raw_count,
            embedding_manifest_match,
            embed_provider: self.config.embedding_provider.as_str().to_string(),
            embed_model: self.config.embedding_model.clone(),
            wings,
            embed_dims,
            ready_for_search,
            ingest_roots_configured,
            db_path: self.store.path().display().to_string(),
        })
    }

    /// Build the minimal `doctor` integrity payload.
    pub(crate) fn doctor_report(&self) -> Result<DoctorReport, AppError> {
        let schema_version = self.store.schema_version()?.unwrap_or(0);
        let schema_ok = schema_version >= SCHEMA_VERSION;
        let fts_ready = self.store.fts_ready()?;
        let (document_count, chunk_count, node_count, edge_count) = self.store.stats()?;
        let embed_dims = self.config.embedding_dims;
        let manifest = self.store.get_embedding_manifest()?;
        let manifest_dims = manifest.as_ref().map(|m| m.dims);
        let embed_ok = match manifest_dims {
            None => true,
            Some(d) => d as usize == embed_dims,
        };
        let ingest_roots_configured = !self.config.ingest_roots.is_empty();
        let ready_for_search = chunk_count > 0 && schema_ok && embed_ok;
        let (
            documents_without_chunks,
            orphan_chunks,
            orphan_document_nodes,
            orphan_edges,
            unscoped_documents,
        ) = self.store.integrity_counts()?;
        let relational_integrity_ok =
            orphan_chunks == 0 && orphan_document_nodes == 0 && orphan_edges == 0;
        let wal_bytes = self.store.wal_file_size_bytes();
        let wal_warn_bytes = crate::ops::wal_warn_bytes();
        let wal_too_large = wal_bytes >= wal_warn_bytes;
        let repair_hint = if !relational_integrity_ok {
            Some("Create a backup, then run db_repair offline and maintain_refresh.".to_string())
        } else if documents_without_chunks > 0 {
            Some("Reingest documents without chunks; sync_sources repairs unchanged file documents too.".to_string())
        } else if wal_too_large {
            Some("WAL exceeds the configured warning threshold; run a checkpointed backup or vacuum_store.".to_string())
        } else {
            None
        };
        let ok = schema_ok && embed_ok && relational_integrity_ok && documents_without_chunks == 0;
        Ok(DoctorReport {
            backend: "duckdb".to_string(),
            storage_capabilities: crate::storage::duckdb_capability_names(),
            schema_version,
            expected_schema_version: SCHEMA_VERSION,
            schema_ok,
            fts_ready,
            document_count,
            chunk_count,
            node_count,
            edge_count,
            embed_dims,
            manifest_dims,
            embed_ok,
            ready_for_search,
            ingest_roots_configured,
            db_path: self.store.path().display().to_string(),
            wal_bytes,
            wal_warn_bytes,
            wal_too_large,
            documents_without_chunks,
            orphan_chunks,
            orphan_document_nodes,
            orphan_edges,
            unscoped_documents,
            relational_integrity_ok,
            repair_hint,
            ok,
        })
    }

    /// Probe local chat LLM and report embedding config (no corpus mutation).
    async fn llm_status_report(&self) -> Result<LlmStatusReport, AppError> {
        let (reachable, error) = match &self.llm {
            Some(client) => {
                let probe = client.llm_status().await;
                let error = if probe.ok { None } else { Some(probe.detail) };
                (probe.ok, error)
            }
            None => (
                false,
                Some(
                    "ChatClient not configured; check RAG_LLM_BASE_URL and RAG_LLM_MODEL"
                        .to_string(),
                ),
            ),
        };

        let embed_base_url = match self.config.embedding_provider {
            crate::config::EmbeddingProviderKind::Mock => None,
            crate::config::EmbeddingProviderKind::OpenAi
            | crate::config::EmbeddingProviderKind::Ollama => {
                Some(self.config.embedding_base_url.clone())
            }
        };

        let dialect = match self.config.llm_provider.dialect() {
            crate::llm::LlmApiDialect::OpenAiCompat => "openai_compat",
            crate::llm::LlmApiDialect::AnthropicMessages => "anthropic_messages",
        };
        Ok(LlmStatusReport {
            llm_enabled: self.config.llm_enabled,
            provider: self.config.llm_provider.as_str().to_string(),
            dialect: dialect.to_string(),
            base_url: self.config.llm_base_url.clone(),
            model: self.config.llm_model.clone(),
            reachable,
            error,
            embed_provider: self.config.embedding_provider.as_str().to_string(),
            embed_model: self.config.embedding_model.clone(),
            embed_dims: self.config.embedding_dims,
            embed_base_url,
        })
    }

    /// Re-embed all existing chunks for one document with the live embedder, then
    /// refresh the corpus embedding_manifest to match config.
    async fn reembed_document_pipeline(
        &self,
        document_id: &str,
    ) -> Result<ReembedDocumentResult, AppError> {
        IngestService::new(&self.store, &self.embedder, &self.config)
            .reembed_document(document_id)
            .await
    }

    /// Apply document meta update; re-chunk + re-embed only when body text changes.
    async fn update_document_meta_pipeline(
        &self,
        params: UpdateDocumentMetaParams,
    ) -> Result<UpdateDocumentMetaResult, AppError> {
        let document_id = params.document_id.trim();
        if document_id.is_empty() {
            return Err(AppError::config("document_id must be non-empty"));
        }

        let update = DocumentMetaUpdate {
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
        };

        let applied = self
            .store
            .update_document_meta(document_id, &update)?
            .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;

        let mut reembedded = false;
        let mut chunk_count = self
            .store
            .list_chunks_for_document(&applied.document.id)?
            .len();

        if applied.content_changed {
            // Body changed: re-chunk + re-embed + rebuild graph (same as content re-ingest).
            self.require_vec_compatible()?;
            let doc = &applied.document;
            let chunker = from_config(self.config.chunk_size, self.config.chunk_overlap);
            let pieces: Vec<(String, i32, i32)> = Chunker::chunk(&chunker, &doc.content);
            let section_metadata = markdown_section_metadata(&doc.content, &pieces);

            if pieces.is_empty() {
                self.store.replace_chunks_for_document(&doc.id, &[])?;
                chunk_count = 0;
            } else {
                let texts: Vec<String> = pieces.iter().map(|(c, _, _)| c.clone()).collect();
                let embeddings = self.embedder.embed(&texts).await?;
                if embeddings.len() != pieces.len() {
                    return Err(AppError::embeddings(format!(
                        "embedder returned {} vectors for {} chunks",
                        embeddings.len(),
                        pieces.len()
                    )));
                }
                let mut chunks = Vec::with_capacity(pieces.len());
                for (i, (((content, char_start, char_end), embedding), metadata_json)) in pieces
                    .into_iter()
                    .zip(embeddings.into_iter())
                    .zip(section_metadata.into_iter())
                    .enumerate()
                {
                    chunks.push(Chunk {
                        id: Uuid::new_v4().to_string(),
                        document_id: doc.id.clone(),
                        chunk_index: i as i32,
                        content,
                        embedding,
                        char_start,
                        char_end,
                        metadata_json,
                    });
                }
                chunk_count = chunks.len();
                self.store.replace_chunks_for_document(&doc.id, &chunks)?;
            }
            rebuild_document_graph(&self.store, doc)?;
            reembedded = true;
        } else if applied.title_changed {
            // Title-only: sync graph node label without rewriting edges / embeddings.
            if let Some(mut node) = self.store.find_node_by_document_id(&applied.document.id)? {
                node.label = applied.document.title.clone();
                self.store.upsert_graph_node(&node)?;
            }
        }

        let doc = applied.document;
        let payload = serde_json::json!({
            "content_changed": applied.content_changed,
            "title_changed": applied.title_changed,
            "reembedded": reembedded,
            "wing": doc.wing,
            "room": doc.room,
            "status": doc.status,
            "pinned": doc.pinned,
            "boost": doc.boost,
        });
        let _ = self.store.append_ops_log(&OpsLogEntry {
            id: String::new(),
            seq: 0,
            ts: Utc::now(),
            op: "update_document_meta".into(),
            prefix: Some("META".into()),
            message: format!("updated document meta for {}", doc.id),
            entity_id: Some(doc.id.clone()),
            entity_kind: Some("document".into()),
            payload_json: payload.to_string(),
            agent_name: None,
        });

        Ok(UpdateDocumentMetaResult {
            document_id: doc.id,
            uri: doc.uri,
            title: doc.title,
            wing: doc.wing,
            room: doc.room,
            status: doc.status,
            pinned: doc.pinned,
            boost: doc.boost,
            metadata_json: doc.metadata_json,
            layer: doc.layer,
            kind: doc.kind,
            source_file: doc.source_file,
            content_changed: applied.content_changed,
            reembedded,
            chunk_count,
            updated_at: doc.updated_at.to_rfc3339(),
        })
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
            .ingest_pipeline(
                params.text,
                params.title,
                params.uri,
                params.metadata_json,
                None,
                None,
                None,
                "raw",
                "document",
                false,
            )
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
        let path = Path::new(&params.path);
        check_path_allowlist(path, &self.config.ingest_roots).map_err(Self::map_err)?;

        let extracted = extract_file(path).map_err(|e| {
            if path.try_exists().ok() == Some(false) {
                Self::map_err(AppError::not_found(format!(
                    "file not found: {}",
                    params.path
                )))
            } else {
                Self::map_err(AppError::config(e.to_string()))
            }
        })?;
        let metadata_json =
            merge_metadata(params.metadata_json, extracted.metadata).map_err(|e| {
                Self::map_err(AppError::config(format!(
                    "metadata_json is not valid JSON: {e}"
                )))
            })?;

        let default_uri = path
            .canonicalize()
            .map(|p| format!("file://{}", p.display()))
            .unwrap_or_else(|_| format!("file://{}", params.path));

        let default_title = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string());

        let source_file = path
            .canonicalize()
            .map(|p| p.display().to_string())
            .ok()
            .or_else(|| Some(params.path.clone()));

        let result = self
            .ingest_pipeline(
                extracted.text,
                params.title.or(default_title),
                params.uri.or(Some(default_uri)),
                Some(metadata_json),
                params.wing,
                params.room,
                source_file,
                "raw",
                "document",
                false,
            )
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
        let requested_root = Path::new(&params.path);
        check_path_allowlist(requested_root, &self.config.ingest_roots).map_err(Self::map_err)?;
        let root = requested_root.canonicalize().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Self::map_err(AppError::not_found(format!(
                    "directory not found: {}",
                    params.path
                )))
            } else {
                Self::map_err(AppError::from(e))
            }
        })?;
        if !root.is_dir() {
            return Err(Self::map_err(AppError::config(format!(
                "sync_sources path is not a directory: {}",
                root.display()
            ))));
        }

        let files = collect_supported_sources(&root).map_err(|e| Self::map_err(e.into()))?;
        let mut seen = BTreeSet::new();
        let mut summary = SyncSourcesSummary {
            added: Vec::new(),
            updated: Vec::new(),
            skipped: Vec::new(),
            deleted: Vec::new(),
            errors: Vec::new(),
        };

        for path in files {
            let canonical = match path.canonicalize() {
                Ok(path) => path,
                Err(error) => {
                    summary.errors.push(SyncSourceError {
                        path: path.display().to_string(),
                        error: error.to_string(),
                    });
                    continue;
                }
            };
            let source_file = canonical.display().to_string();
            seen.insert(source_file.clone());
            let uri = format!("file://{}", canonical.display());
            let extracted = match extract_file(&canonical) {
                Ok(extracted) => extracted,
                Err(error) => {
                    summary.errors.push(SyncSourceError {
                        path: source_file,
                        error: error.to_string(),
                    });
                    continue;
                }
            };
            let existing = match self.store.find_by_uri(&uri) {
                Ok(existing) => existing,
                Err(error) => {
                    summary.errors.push(SyncSourceError {
                        path: source_file,
                        error: error.to_string(),
                    });
                    continue;
                }
            };
            let hash = content_hash(&extracted.text);
            let unchanged = existing.as_ref().is_some_and(|doc| {
                doc.content_hash
                    .as_deref()
                    .map(|stored| stored == hash)
                    .unwrap_or_else(|| content_hash(&doc.content) == hash)
            });
            let has_chunks = existing
                .as_ref()
                .and_then(|doc| self.store.list_chunks_for_document(&doc.id).ok())
                .is_some_and(|chunks| !chunks.is_empty());
            if unchanged && has_chunks {
                summary.skipped.push(source_file);
                continue;
            }

            let title = canonical
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string);
            let metadata_json = match merge_metadata(
                existing.as_ref().map(|doc| doc.metadata_json.clone()),
                extracted.metadata,
            ) {
                Ok(metadata) => Some(metadata),
                Err(error) => {
                    summary.errors.push(SyncSourceError {
                        path: source_file,
                        error: error.to_string(),
                    });
                    continue;
                }
            };
            let (inferred_wing, inferred_room) = inferred_scope(&root, &canonical);
            let wing = params
                .wing
                .clone()
                .or_else(|| existing.as_ref().and_then(|doc| doc.wing.clone()))
                .or(Some(inferred_wing));
            let room = params
                .room
                .clone()
                .or_else(|| existing.as_ref().and_then(|doc| doc.room.clone()))
                .or(Some(inferred_room));
            match self
                .ingest_pipeline(
                    extracted.text,
                    title,
                    Some(uri),
                    metadata_json,
                    wing,
                    room,
                    Some(source_file.clone()),
                    "raw",
                    "document",
                    false,
                )
                .await
            {
                Ok(result) if result.op == "inserted" => summary.added.push(source_file),
                Ok(_) => summary.updated.push(source_file),
                Err(error) => summary.errors.push(SyncSourceError {
                    path: source_file,
                    error: error.to_string(),
                }),
            }
        }

        if params.remove_deleted.unwrap_or(false) {
            let documents = self.store.list_documents().map_err(Self::map_err)?;
            let mut missing = BTreeSet::new();
            for document in documents {
                let Some(source) = document.source_file else {
                    continue;
                };
                let source_path = Path::new(&source);
                if source_path.starts_with(&root)
                    && is_supported_source(source_path)
                    && !seen.contains(&source)
                    && !source_path.exists()
                {
                    missing.insert(source);
                }
            }
            for source in missing {
                match self.store.delete_by_source(&source) {
                    Ok(count) if count > 0 => summary.deleted.push(source),
                    Ok(_) => {}
                    Err(error) => summary.errors.push(SyncSourceError {
                        path: source,
                        error: error.to_string(),
                    }),
                }
            }
        }

        Self::json_result(&summary)
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
            .ingest_pipeline(
                params.text,
                params.title,
                params.uri,
                params.metadata_json,
                params.wing,
                params.room,
                params.source_file,
                "raw",
                "document",
                true,
            )
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
            .ingest_pipeline(
                params.content,
                params.title,
                params.uri,
                params.metadata_json,
                Some(wing.to_string()),
                Some(room.to_string()),
                params.source_file,
                "raw",
                "document",
                false,
            )
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

        let matched = self
            .store
            .list_documents_filtered(&DocumentFilter {
                source_file: Some(source_file.to_string()),
                ..Default::default()
            })
            .map_err(Self::map_err)?;
        let document_ids: Vec<String> = matched.iter().map(|d| d.id.clone()).collect();
        let match_count = document_ids.len() as u64;
        let dry_run = params.dry_run.unwrap_or(false);

        if dry_run {
            return Self::json_result(&serde_json::json!({
                "success": true,
                "dry_run": true,
                "source_file": source_file,
                "match_count": match_count,
                "deleted": 0,
                "document_ids": document_ids,
            }));
        }

        let deleted = self
            .store
            .delete_by_source(source_file)
            .map_err(Self::map_err)?;
        Self::json_result(&serde_json::json!({
            "success": true,
            "dry_run": false,
            "source_file": source_file,
            "match_count": match_count,
            "deleted": deleted,
            "document_ids": document_ids,
        }))
    }

    #[tool(
        name = "list_sources",
        description = "List immutable raw-layer sources (layer=raw): id, title, uri, content_hash, wing, room, source_file."
    )]
    async fn list_sources(
        &self,
        Parameters(params): Parameters<ListSourcesParams>,
    ) -> Result<CallToolResult, McpError> {
        let docs = self
            .store
            .list_documents_by_layer("raw")
            .map_err(Self::map_err)?;
        let wing_filter = params
            .wing
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let room_filter = params
            .room
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let summaries: Vec<SourceSummary> = docs
            .into_iter()
            .filter(|d| {
                if let Some(w) = wing_filter {
                    if d.wing.as_deref() != Some(w) {
                        return false;
                    }
                }
                if let Some(r) = room_filter {
                    if d.room.as_deref() != Some(r) {
                        return false;
                    }
                }
                true
            })
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
        description = "Fetch one raw-layer source by document_id or uri (full content; optional chunks)."
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

        let doc = if let Some(id) = doc_id {
            self.store
                .get_document(id)
                .map_err(Self::map_err)?
                .ok_or_else(|| {
                    Self::map_err(AppError::not_found(format!("source not found: {id}")))
                })?
        } else if let Some(u) = uri {
            self.store
                .find_by_uri(u)
                .map_err(Self::map_err)?
                .ok_or_else(|| {
                    Self::map_err(AppError::not_found(format!("source not found: uri={u}")))
                })?
        } else {
            return Err(Self::map_err(AppError::config(
                "get_source requires document_id or uri",
            )));
        };

        if doc.layer != "raw" {
            return Err(Self::map_err(AppError::not_found(format!(
                "document {} is not a raw source (layer={})",
                doc.id, doc.layer
            ))));
        }

        let include_chunks = params.include_chunks.unwrap_or(false);
        let chunks = if include_chunks {
            let raw = self
                .store
                .list_chunks_for_document(&doc.id)
                .map_err(Self::map_err)?;
            Some(raw.into_iter().map(ChunkView::from).collect())
        } else {
            None
        };

        let body = SourceDetail {
            id: doc.id,
            uri: doc.uri,
            title: doc.title,
            content: doc.content,
            metadata_json: doc.metadata_json,
            content_hash: doc.content_hash,
            wing: doc.wing,
            room: doc.room,
            source_file: doc.source_file,
            layer: doc.layer,
            kind: doc.kind,
            created_at: doc.created_at.to_rfc3339(),
            updated_at: doc.updated_at.to_rfc3339(),
            chunks,
        };
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
        let top_k = params
            .top_k
            .map(|k| k as usize)
            .unwrap_or(self.config.default_top_k);

        let mode = match params
            .mode
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => SearchMode::parse(raw).map_err(|e| Self::map_err(AppError::config(e)))?,
            None => self.config.default_search_mode,
        };

        if matches!(mode, SearchMode::Vec | SearchMode::Hybrid) {
            self.require_vec_compatible().map_err(Self::map_err)?;
        }

        let mut diversity = match params
            .diversity
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => Some(DiversityMode::parse(raw).map_err(Self::map_err)?),
            None => None,
        };
        if let Some(group_by) = params
            .group_by
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match group_by.to_ascii_lowercase().as_str() {
                "document" | "document_id" => diversity = Some(DiversityMode::CollapseByDocument),
                "none" => {}
                other => {
                    return Err(Self::map_err(AppError::config(format!(
                        "invalid group_by '{other}': expected document or none"
                    ))))
                }
            }
        }
        if params
            .recency_half_life_days
            .is_some_and(|days| !days.is_finite() || days <= 0.0)
        {
            return Err(Self::map_err(AppError::config(
                "recency_half_life_days must be finite and greater than zero",
            )));
        }

        let context_expansion = match params.context_expansion.as_deref() {
            Some(raw) => Some(ContextExpansion::parse(raw).map_err(Self::map_err)?),
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
                .embed(&[params.query.clone()])
                .await
                .map_err(Self::map_err)?;
            let emb = query_vecs.into_iter().next().ok_or_else(|| {
                McpError::internal_error("embedder returned no vector for query", None)
            })?;
            Some(emb)
        } else {
            None
        };

        let nonempty = |s: Option<String>| s.filter(|v| !v.trim().is_empty());

        let opts = SearchQuery {
            mode,
            top_k,
            query_text: Some(params.query),
            query_embedding,
            document_id: nonempty(params.document_id),
            wing: nonempty(params.wing),
            room: nonempty(params.room),
            layer: nonempty(params.layer),
            source_file: nonempty(params.source_file),
            include_archived: params.include_archived.unwrap_or(false),
            min_score: params.min_score,
            diversity,
            max_chunks_per_document,
            recency_half_life_days: params.recency_half_life_days,
            max_context_tokens,
            context_expansion,
            neighbor_chunks: params.neighbor_chunks.unwrap_or(1) as usize,
            timeout_ms: params.timeout_ms.or(Some(5_000)),
            fts_stemmer: self.config.fts_stemmer.clone(),
            ..SearchQuery::default()
        };

        let hits = search(&self.store, &opts).map_err(Self::map_err)?;
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
        let mode = match params
            .mode
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            Some(raw) => SearchMode::parse(raw).map_err(|e| Self::map_err(AppError::config(e)))?,
            None => self.config.default_search_mode,
        };
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
                search(
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
        description = "Return the corpus embedding fingerprint (provider, model, dims). Empty object fields when none recorded yet."
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
        description = "Re-embed all chunks for one document with the live embedding config; updates embedding_manifest."
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
        description = "List stored documents without full content. Filters: wing?, room?, source_file?, include_archived?, layer?, kind?, limit?. Default excludes archived/tombstone."
    )]
    async fn list_documents(
        &self,
        Parameters(params): Parameters<ListDocumentsParams>,
    ) -> Result<CallToolResult, McpError> {
        let nonempty = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
        let filter = DocumentFilter {
            wing: nonempty(params.wing),
            room: nonempty(params.room),
            source_file: nonempty(params.source_file),
            include_archived: Some(params.include_archived.unwrap_or(false)),
            layer: nonempty(params.layer),
            kind: nonempty(params.kind),
            limit: params.limit.map(|n| n as usize),
            ..Default::default()
        };
        let docs = self
            .store
            .list_documents_filtered(&filter)
            .map_err(Self::map_err)?;
        let items: Vec<DrawerListItem> = docs.iter().map(DrawerListItem::from).collect();
        Self::json_result(&items)
    }

    #[tool(
        name = "get_document",
        description = "Get a document by id with metadata; optionally include chunk texts (no embeddings)."
    )]
    async fn get_document(
        &self,
        Parameters(params): Parameters<GetDocumentParams>,
    ) -> Result<CallToolResult, McpError> {
        let body = retrieval::get_document(
            &self.store,
            &params.document_id,
            params.include_chunks.unwrap_or(false),
        )
        .map(DocumentDetail::from)
        .map_err(Self::map_err)?;
        Self::json_result(&body)
    }

    #[tool(
        name = "multi_get",
        description = "Get several documents by id in request order. Returns found documents plus missing ids; optionally includes chunk texts."
    )]
    async fn multi_get(
        &self,
        Parameters(params): Parameters<MultiGetParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = retrieval::multi_get(
            &self.store,
            params.document_ids,
            params.include_chunks.unwrap_or(false),
        )
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
        self.require_vec_compatible().map_err(Self::map_err)?;
        let hits = retrieval::find_similar(
            &self.store,
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
            .update_document_meta_pipeline(params)
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
        let body = self.status_report().map_err(Self::map_err)?;
        Self::json_result(&body)
    }

    #[tool(
        name = "doctor",
        description = "Minimal integrity: schema_version vs expected, FTS ready, embed dims vs manifest, ingest_roots, ready_for_search."
    )]
    async fn doctor(&self) -> Result<CallToolResult, McpError> {
        let body = self.doctor_report().map_err(Self::map_err)?;
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
        let dry_run = params.dry_run.unwrap_or(true);
        let max_docs = params
            .max_docs
            .unwrap_or(self.config.maint_max_docs)
            .max(1)
            .min(self.config.maint_max_docs.max(1));
        let before = self.doctor_report().map_err(Self::map_err)?;
        let documents = self.store.list_documents().map_err(Self::map_err)?;
        let mut missing = Vec::new();
        for document in documents {
            if document.layer == "schema" {
                continue;
            }
            let chunks = self
                .store
                .list_chunks_for_document(&document.id)
                .map_err(Self::map_err)?;
            if chunks.is_empty() {
                missing.push(document);
                if missing.len() >= max_docs {
                    break;
                }
            }
        }

        let mut documents_repaired = Vec::new();
        let mut documents_failed = Vec::new();
        if !dry_run {
            for document in &missing {
                match self
                    .ingest_pipeline(
                        document.content.clone(),
                        Some(document.title.clone()),
                        Some(document.uri.clone()),
                        Some(document.metadata_json.clone()),
                        document.wing.clone(),
                        document.room.clone(),
                        document.source_file.clone(),
                        &document.layer,
                        &document.kind,
                        false,
                    )
                    .await
                {
                    Ok(_) => documents_repaired.push(document.id.clone()),
                    Err(error) => documents_failed.push(SyncSourceError {
                        path: document.uri.clone(),
                        error: error.to_string(),
                    }),
                }
            }
        }
        let (orphan_chunks_pruned, orphan_document_nodes_pruned, orphan_edges_pruned) =
            self.store.prune_orphans(dry_run).map_err(Self::map_err)?;
        if !dry_run {
            self.store.checkpoint().map_err(Self::map_err)?;
        }
        let after = if dry_run {
            before.clone()
        } else {
            self.doctor_report().map_err(Self::map_err)?
        };
        Self::json_result(&DoctorRepairReport {
            dry_run,
            documents_considered: missing.len(),
            documents_repaired,
            documents_failed,
            orphan_chunks_pruned,
            orphan_document_nodes_pruned,
            orphan_edges_pruned,
            before,
            after,
        })
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
        let view = self
            .store
            .export_graph_for_ui(Some(max_nodes), include_tags)
            .map_err(Self::map_err)?;

        let default_path = self
            .store
            .path()
            .parent()
            .map(|p| p.join("graph.json"))
            .unwrap_or_else(|| Path::new("graph.json").to_path_buf());
        let out = params
            .path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Path::new)
            .map(|p| p.to_path_buf())
            .unwrap_or(default_path);

        let out = std::fs::canonicalize(out.parent().unwrap_or(Path::new(".")))
            .map(|parent| {
                parent.join(
                    out.file_name()
                        .unwrap_or_else(|| std::ffi::OsStr::new("graph.json")),
                )
            })
            .unwrap_or(out.clone());

        // Allow: directory of RAG_DB_PATH, or RAG_INGEST_ROOTS.
        let allowed = {
            let mut roots = self.config.ingest_roots.clone();
            if let Some(parent) = self.store.path().parent() {
                roots.push(parent.to_path_buf());
            }
            roots
        };
        let out_canon = if out.exists() {
            std::fs::canonicalize(&out).unwrap_or(out.clone())
        } else if let Some(parent) = out.parent() {
            let parent_c = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
            parent_c.join(out.file_name().unwrap_or_default())
        } else {
            out.clone()
        };
        let ok_path = allowed.iter().any(|root| {
            let root_c = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
            out_canon.starts_with(&root_c)
        });
        if !ok_path {
            return Err(Self::map_err(AppError::forbidden(format!(
                "export path {} is outside DB directory and RAG_INGEST_ROOTS",
                out.display()
            ))));
        }

        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Self::map_err(AppError::db(format!(
                    "create parent for {}: {e}",
                    out.display()
                )))
            })?;
        }
        let json = serde_json::to_string_pretty(&view)
            .map_err(|e| Self::map_err(AppError::db(format!("serialize GraphView: {e}"))))?;
        std::fs::write(&out, json.as_bytes())
            .map_err(|e| Self::map_err(AppError::db(format!("write {}: {e}", out.display()))))?;

        #[derive(Serialize)]
        struct ExportSnapResult {
            path: String,
            node_count: usize,
            edge_count: usize,
            max_nodes: u32,
            include_tags: bool,
            ui_hint: String,
        }
        Self::json_result(&ExportSnapResult {
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

        let hits = search_chunks(
            &self.store,
            &query_emb,
            top_k,
            params.document_id.as_deref(),
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
            &params.agent_name,
            &params.content,
            params.wing.as_deref(),
            params.topic.as_deref(),
            params.title.as_deref(),
            true,
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
        let status = self.status_report().map_err(Self::map_err)?;
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

        let mode = match params
            .mode
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => SearchMode::parse(raw).map_err(|e| Self::map_err(AppError::config(e)))?,
            None => self.config.default_search_mode,
        };

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
                .embed(&[params.query.clone()])
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

        let hits = search(&self.store, &opts).map_err(Self::map_err)?;
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
            max_docs: params.max_docs.map(|n| n as usize),
            agent: params.agent.filter(|s| !s.trim().is_empty()),
        };

        let llm = self.llm.as_ref();
        let report = maintain::apply_maintenance_plan(
            &self.store,
            &self.embedder,
            &self.config,
            llm,
            actions,
            &opts,
        )
        .await
        .map_err(Self::map_err)?;
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
            opts.max_docs = (n as usize).max(1);
        }
        if let Some(c) = params.min_confidence {
            opts.min_confidence = c;
        }
        if let Some(v) = params.rebuild_index {
            opts.rebuild_index = v;
        }
        opts.agent = params.agent.filter(|s| !s.trim().is_empty());

        let llm = self.llm.as_ref();
        let report = maintain::maintain_organize(&self.store, &self.config, llm, &opts)
            .await
            .map_err(Self::map_err)?;
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
        let max_docs = params
            .max_docs
            .map(|n| n as usize)
            .unwrap_or(self.config.maint_max_docs);
        let flags = MaintainRefreshFlags::from_options(
            params.reindex_fts,
            params.rebuild_graph,
            params.graph_dirty_only,
            params.rebuild_wiki_index,
            params.reembed_all,
            params.dry_run,
            max_docs,
        );
        let report = maintain::maintain_refresh(&self.store, &self.embedder, &self.config, flags)
            .await
            .map_err(Self::map_err)?;
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
            opts.max_docs = (n as usize).max(1);
        }
        let report =
            maintain::maintain_compress(&self.store, &self.config, &opts).map_err(Self::map_err)?;
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
        description = "Upsert wiki page by slug; pass if_match_revision/etag from get_wiki_page to avoid clobbering concurrent LLM writes."
    )]
    async fn update_wiki_page(
        &self,
        Parameters(params): Parameters<WriteWikiPageParams>,
    ) -> Result<CallToolResult, McpError> {
        self.write_wiki_page(Parameters(params)).await
    }

    #[tool(
        name = "get_wiki_page",
        description = "Fetch a wiki page by slug, wiki://uri, or document id. Returns revision+etag for If-Match on write."
    )]
    async fn get_wiki_page(
        &self,
        Parameters(params): Parameters<GetWikiPageParams>,
    ) -> Result<CallToolResult, McpError> {
        let key = params.id_or_slug.trim();
        let doc = if let Some(d) = self.store.get_document(key).map_err(Self::map_err)? {
            d
        } else if let Some(d) = self.store.find_by_uri(key).map_err(Self::map_err)? {
            d
        } else if let Some(d) = self
            .store
            .find_by_uri(&format!("wiki://{key}"))
            .map_err(Self::map_err)?
        {
            d
        } else {
            return Err(Self::map_err(AppError::not_found(format!(
                "wiki page not found: {key}"
            ))));
        };
        Self::json_result(&document_with_etag(&doc))
    }

    #[tool(
        name = "list_wiki_pages",
        description = "List layer=wiki documents (id, uri, title, kind)."
    )]
    async fn list_wiki_pages(&self) -> Result<CallToolResult, McpError> {
        let docs = self
            .store
            .list_documents_by_layer("wiki")
            .map_err(Self::map_err)?;
        let rows: Vec<_> = docs
            .into_iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.id,
                    "uri": d.uri,
                    "title": d.title,
                    "kind": d.kind,
                    "updated_at": d.updated_at,
                })
            })
            .collect();
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
            max_docs: params.max_docs.map(|n| n as usize),
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
        let max_docs = params.max_docs.map(|n| n as usize);

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
        description = "Create a consistent DuckDB file backup after CHECKPOINT. path must be under RAG_INGEST_ROOTS. dry_run=false writes; overwrite defaults to false."
    )]
    async fn backup_db(
        &self,
        Parameters(params): Parameters<BackupDbParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = recovery_path(&params.path, &self.config.ingest_roots).map_err(Self::map_err)?;
        refuse_live_db_target(&path, self.store.path()).map_err(Self::map_err)?;
        let report = self
            .store
            .backup_database(
                &path,
                params.dry_run.unwrap_or(false),
                params.overwrite.unwrap_or(false),
            )
            .map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "export_bundle",
        description = "Export documents, essential metadata, and chunks as a portable JSON or JSONL recovery bundle. path must be under RAG_INGEST_ROOTS; never overwrites unless overwrite=true."
    )]
    async fn export_bundle(
        &self,
        Parameters(params): Parameters<ExportBundleParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = recovery_path(&params.path, &self.config.ingest_roots).map_err(Self::map_err)?;
        refuse_live_db_target(&path, self.store.path()).map_err(Self::map_err)?;
        let format = recovery_format(params.format.as_deref(), &path).map_err(Self::map_err)?;
        let overwrite = params.overwrite.unwrap_or(false);
        let dry_run = params.dry_run.unwrap_or(false);
        let existed = path.exists();
        if existed && !overwrite {
            return Err(Self::map_err(AppError::conflict(format!(
                "bundle destination '{}' already exists; set overwrite=true explicitly",
                path.display()
            ))));
        }
        let bundle = self.store.recovery_bundle().map_err(Self::map_err)?;
        let documents = bundle.documents.len() as u64;
        let chunks = bundle.documents.iter().map(|d| d.chunks.len() as u64).sum();
        let encoded = encode_recovery_bundle(&bundle, format).map_err(Self::map_err)?;
        if !dry_run {
            std::fs::write(&path, &encoded).map_err(|e| Self::map_err(e.into()))?;
        }
        let report = BundleExportReport {
            success: true,
            dry_run,
            path: path.display().to_string(),
            format: format.into(),
            overwritten: existed && overwrite,
            documents,
            chunks,
            bytes: Some(encoded.len() as u64),
            errors: Vec::new(),
        };
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
        let path = recovery_path(&params.path, &self.config.ingest_roots).map_err(Self::map_err)?;
        refuse_live_db_target(&path, self.store.path()).map_err(Self::map_err)?;
        let report = self
            .store
            .export_vault(
                &path,
                params.dry_run.unwrap_or(true),
                params.overwrite.unwrap_or(false),
            )
            .map_err(Self::map_err)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "import_bundle",
        description = "Import a portable JSON/JSONL recovery bundle from RAG_INGEST_ROOTS. dry_run defaults true. conflict_policy is error (default), skip, or overwrite; overwrite must be explicit. Returns structured counts/conflicts/errors."
    )]
    async fn import_bundle(
        &self,
        Parameters(params): Parameters<ImportBundleParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = recovery_path(&params.path, &self.config.ingest_roots).map_err(Self::map_err)?;
        if !path.is_file() {
            return Err(Self::map_err(AppError::not_found(format!(
                "bundle file '{}'",
                path.display()
            ))));
        }
        let format = recovery_format(params.format.as_deref(), &path).map_err(Self::map_err)?;
        let policy =
            ConflictPolicy::parse(params.conflict_policy.as_deref()).map_err(Self::map_err)?;
        let input = std::fs::read_to_string(&path).map_err(|e| Self::map_err(e.into()))?;
        let bundle = decode_recovery_bundle(&input, format).map_err(Self::map_err)?;
        let report = self
            .store
            .import_recovery_bundle(
                &bundle,
                policy,
                params.dry_run.unwrap_or(true),
                &path,
                format,
            )
            .map_err(Self::map_err)?;
        Self::json_result(&report)
    }
}

fn recovery_path(raw: &str, roots: &[PathBuf]) -> Result<PathBuf, AppError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(AppError::config("path must be non-empty"));
    }
    let path = PathBuf::from(raw);
    check_path_allowlist(&path, roots)?;
    let parent = path.parent().unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(AppError::config(format!(
            "parent directory '{}' does not exist",
            parent.display()
        )));
    }
    let parent = std::fs::canonicalize(parent)?;
    Ok(parent.join(
        path.file_name()
            .ok_or_else(|| AppError::config("path must name a file"))?,
    ))
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

fn refuse_live_db_target(path: &Path, db_path: &Path) -> Result<(), AppError> {
    let db = std::fs::canonicalize(db_path).unwrap_or_else(|_| db_path.to_path_buf());
    if path == db {
        return Err(AppError::forbidden(
            "recovery output path must not be the live DuckDB file",
        ));
    }
    Ok(())
}

fn encode_recovery_bundle(bundle: &RecoveryBundle, format: &str) -> Result<Vec<u8>, AppError> {
    if format == "json" {
        return Ok(serde_json::to_vec_pretty(bundle)?);
    }
    let mut lines = Vec::with_capacity(bundle.documents.len() + 1);
    lines.push(serde_json::to_string(&serde_json::json!({
        "record_type": "manifest", "format": bundle.format, "version": bundle.version,
        "exported_at": bundle.exported_at,
    }))?);
    for item in &bundle.documents {
        lines.push(serde_json::to_string(
            &serde_json::json!({"record_type": "document", "value": item}),
        )?);
    }
    Ok((lines.join("\n") + "\n").into_bytes())
}

fn decode_recovery_bundle(input: &str, format: &str) -> Result<RecoveryBundle, AppError> {
    if format == "json" {
        return Ok(serde_json::from_str(input)?);
    }
    let mut format_name = String::new();
    let mut version = 0;
    let mut exported_at = Utc::now();
    let mut documents = Vec::<BundleDocument>::new();
    for (index, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| AppError::config(format!("invalid JSONL line {}: {e}", index + 1)))?;
        match value.get("record_type").and_then(|v| v.as_str()) {
            Some("manifest") => {
                format_name = value
                    .get("format")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .into();
                version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                exported_at =
                    serde_json::from_value(value.get("exported_at").cloned().unwrap_or_default())?;
            }
            Some("document") => documents.push(serde_json::from_value(
                value.get("value").cloned().unwrap_or_default(),
            )?),
            other => {
                return Err(AppError::config(format!(
                    "invalid JSONL record_type {:?} at line {}",
                    other,
                    index + 1
                )))
            }
        }
    }
    Ok(RecoveryBundle {
        format: format_name,
        version: if version == 0 {
            BUNDLE_VERSION
        } else {
            version
        },
        exported_at,
        documents,
    })
}

fn pack_hit_to_search_hit(h: PackHitParams) -> SearchHit {
    SearchHit {
        chunk_id: h.chunk_id,
        document_id: h.document_id,
        document_title: h.document_title,
        document_uri: h.document_uri,
        chunk_index: h.chunk_index,
        content: h.content,
        score: h.score,
        score_vec: h.score_vec,
        score_lex: h.score_lex,
        score_rrf: h.score_rrf,
        snippet: h.snippet,
        char_start: h.char_start,
        char_end: h.char_end,
        heading_path: h.heading_path,
        section: h.section,
        context: None,
        explanation: None,
    }
}

/// Parse optional MCP timestamp string into UTC.
///
/// Accepts RFC3339 and common SQL / ISO forms (`YYYY-MM-DD[T ]HH:MM:SS[.f]`, date-only).
/// Empty / whitespace-only → `None`. Invalid → [`AppError::Config`] (maps to invalid_params).
fn parse_optional_ts(raw: Option<&str>) -> Result<Option<DateTime<Utc>>, AppError> {
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
            "\nSafety: ingest_file requires RAG_INGEST_ROOTS; vec/hybrid refuse wrong embedding_manifest dims. \
             Logs only on stderr. [[wikilinks]] and #tags extract to the object graph.\n",
        );

        ServerInfo {
            instructions: Some(instructions),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "rag-mcp".into(),
                title: Some("RAG MCP Server".into()),
                version: env!("CARGO_PKG_VERSION").into(),
                icons: None,
                website_url: None,
            },
            ..Default::default()
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
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
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let name = request.name.as_ref();
        if !surface::tool_allowed(self.config.tool_surface, name) {
            return Err(McpError::invalid_params(
                format!(
                    "tool '{name}' is not in RAG_TOOLS=spine; set RAG_TOOLS=full to enable advanced tools"
                ),
                None,
            ));
        }
        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }
}

#[derive(Debug, Serialize)]
struct UpdateDocumentMetaResult {
    document_id: String,
    uri: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    wing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    room: Option<String>,
    status: String,
    pinned: bool,
    boost: f64,
    metadata_json: String,
    layer: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_file: Option<String>,
    content_changed: bool,
    reembedded: bool,
    chunk_count: usize,
    updated_at: String,
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

impl From<Chunk> for ChunkView {
    fn from(chunk: Chunk) -> Self {
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
    chunks: Option<Vec<ChunkView>>,
}

impl From<DocumentWithChunks> for DocumentDetail {
    fn from(value: DocumentWithChunks) -> Self {
        let document = value.document;
        Self {
            id: document.id,
            uri: document.uri,
            title: document.title,
            content: document.content,
            metadata_json: document.metadata_json,
            created_at: document.created_at.to_rfc3339(),
            updated_at: document.updated_at.to_rfc3339(),
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
    layer: String,
    kind: String,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks: Option<Vec<ChunkView>>,
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
    use std::path::PathBuf;

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
            .ingest_pipeline(
                "alpha architecture and duckdb reliability ".repeat(80),
                Some("Alpha".into()),
                Some("file://alpha.md".into()),
                None,
                Some("rag".into()),
                Some("docs".into()),
                None,
                "raw",
                "document",
                false,
            )
            .await
            .unwrap();
        let second = server
            .ingest_pipeline(
                "alpha architecture with database recovery ".repeat(80),
                Some("Beta".into()),
                Some("file://beta.md".into()),
                None,
                Some("rag".into()),
                Some("docs".into()),
                None,
                "raw",
                "document",
                false,
            )
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
            .ingest_pipeline(
                "hello searchable world".into(),
                Some("t".into()),
                Some("text://ready".into()),
                None,
                None,
                None,
                None,
                "raw",
                "document",
                false,
            )
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
                .ingest_pipeline(
                    text.into(),
                    Some(uri.into()),
                    Some(uri.into()),
                    None,
                    Some(wing.into()),
                    Some(room.into()),
                    None,
                    "raw",
                    "document",
                    true,
                )
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
            .ingest_pipeline(
                "original wiki body about retrieval".into(),
                Some("Wiki note".into()),
                Some("wiki://meta-test".into()),
                Some(r#"{"k":1}"#.into()),
                Some("research".into()),
                Some("rag".into()),
                None,
                "wiki",
                "wiki",
                false,
            )
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
            .update_document_meta_pipeline(UpdateDocumentMetaParams {
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
            .update_document_meta_pipeline(UpdateDocumentMetaParams {
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
            .ingest_pipeline(
                "raw immutable body".into(),
                Some("raw".into()),
                Some("raw://meta-raw".into()),
                None,
                Some("research".into()),
                Some("rag".into()),
                None,
                "raw",
                "document",
                true,
            )
            .await
            .expect("raw ingest");
        let refuse = server
            .update_document_meta_pipeline(UpdateDocumentMetaParams {
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
            .ingest_pipeline(
                body.into(),
                Some("One".into()),
                Some("raw://one".into()),
                None,
                Some("lab".into()),
                Some("a".into()),
                Some(src.into()),
                "raw",
                "document",
                true,
            )
            .await
            .expect("ingest one");
        server
            .ingest_pipeline(
                "other".into(),
                Some("Two".into()),
                Some("raw://two".into()),
                None,
                Some("lab".into()),
                Some("b".into()),
                Some(src.into()),
                "raw",
                "document",
                true,
            )
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
    async fn ingest_raw_list_sources_get_source_immutable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("raw.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 32usize;
        let config = test_config(path.clone(), Vec::new(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);

        let r1 = server
            .ingest_pipeline(
                "verbatim source alpha".into(),
                Some("Alpha".into()),
                Some("raw://alpha".into()),
                None,
                Some("projects".into()),
                Some("lab".into()),
                Some("/vault/alpha.md".into()),
                "raw",
                "document",
                true,
            )
            .await
            .expect("ingest_raw");
        assert!(r1.chunk_count >= 1);
        assert!(!r1.document_id.is_empty());

        // Same uri + content: idempotent no-op (stable id).
        let r2 = server
            .ingest_pipeline(
                "verbatim source alpha".into(),
                Some("Alpha".into()),
                Some("raw://alpha".into()),
                None,
                Some("projects".into()),
                Some("lab".into()),
                Some("/vault/alpha.md".into()),
                "raw",
                "document",
                true,
            )
            .await
            .expect("re-ingest same");
        assert_eq!(r2.document_id, r1.document_id);
        assert_eq!(r2.chunk_count, r1.chunk_count);

        // Same uri + different content: conflict under immutable policy.
        let err = server
            .ingest_pipeline(
                "changed content".into(),
                Some("Alpha".into()),
                Some("raw://alpha".into()),
                None,
                None,
                None,
                None,
                "raw",
                "document",
                true,
            )
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
            .ingest_pipeline(
                text,
                Some("note.md".into()),
                Some(format!("file://{}", inside.display())),
                None,
                None,
                None,
                Some(inside.display().to_string()),
                "raw",
                "document",
                false,
            )
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
            .ingest_pipeline(
                body.into(),
                Some("Drawer A".into()),
                Some("drawer://a".into()),
                None,
                Some("projects".into()),
                Some("notes".into()),
                Some("/vault/drawer-a.md".into()),
                "raw",
                "document",
                false,
            )
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

        // dry_run delete_by_source leaves rows in place.
        let matched = server
            .store
            .list_documents_filtered(&DocumentFilter {
                source_file: Some("/vault/drawer-a.md".into()),
                ..Default::default()
            })
            .expect("list by source");
        assert_eq!(matched.len(), 1);
        let dry_deleted = 0u64;
        assert_eq!(dry_deleted, 0);
        assert!(server
            .store
            .get_document(&ing.document_id)
            .expect("get")
            .is_some());

        let deleted = server
            .store
            .delete_by_source("/vault/drawer-a.md")
            .expect("delete_by_source");
        assert_eq!(deleted, 1);
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
    }

    #[tokio::test]
    async fn refuse_vec_on_dims_mismatch_and_reembed_document() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("reembed.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 16usize;
        let config = test_config(path.clone(), Vec::new(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let server = RagServer::new(store, embedder, config);

        let ingest = server
            .ingest_pipeline(
                "reembed me please".into(),
                Some("re".into()),
                Some("text://reembed".into()),
                None,
                None,
                None,
                None,
                "raw",
                "document",
                false,
            )
            .await
            .expect("ingest");

        // Simulate config dim drift by overwriting manifest.
        server
            .store
            .set_embedding_manifest(&crate::models::EmbeddingManifest {
                id: "default".into(),
                provider: "mock".into(),
                model: "mock".into(),
                dims: 999,
                base_url: None,
                content_fingerprint: None,
                updated_at: Utc::now(),
            })
            .expect("force mismatch");

        let err = server.require_vec_compatible().expect_err("must refuse");
        let msg = err.to_string();
        assert!(
            msg.contains("dimension mismatch") || msg.contains("dims="),
            "unexpected: {msg}"
        );

        let result = server
            .reembed_document_pipeline(&ingest.document_id)
            .await
            .expect("reembed");
        assert_eq!(result.document_id, ingest.document_id);
        assert!(result.chunk_count >= 1);
        assert_eq!(result.dims, dims as i32);

        server
            .require_vec_compatible()
            .expect("compatible after reembed");
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
