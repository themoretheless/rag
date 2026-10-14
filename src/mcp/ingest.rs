//! Ingest, source inventory, document integrity, taxonomy, and health tools.

use std::path::PathBuf;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router, ErrorData as McpError};
use serde::Serialize;

use super::facade::RagServer;
use super::tools::{
    AddDrawerParams, CheckDuplicateParams, DeleteBySourceParams, DeleteDocumentParams,
    GetDocumentParams, GetSourceParams, GetTaxonomyParams, IngestFileParams, IngestRawParams,
    IngestTextParams, ListDocumentsParams, ListRoomsParams, ListSourcesParams, ListWingsParams,
    SyncSourcesParams, UpdateDocumentMetaParams,
};
use super::views::{DocumentDetail, SourceDetail};
use crate::db::{DocumentCatalogFilter, DEFAULT_CATALOG_PAGE_SIZE, MAX_CATALOG_PAGE_SIZE};
use crate::diagnostics::DiagnosticsService;
use crate::error::AppError;
use crate::ingest::{IngestCommand, IngestFileCommand, IngestService};
use crate::models::{DocumentFilter, Stats};
use crate::retrieval::{self};
use crate::source_sync::{
    sync_sources_nonblocking, SourceSyncCommand, SourceSyncControl, SourceSyncOutcome,
};
use crate::util::{non_blank, run_blocking};

#[tool_router(router = ingest_router, vis = "pub(super)")]
impl RagServer {
    #[tool(
        name = "ingest_text",
        description = "Ingest raw text: chunk, embed, and store. Upserts by uri when provided."
    )]
    pub(super) async fn ingest_text(
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
            .await?;
        Self::json_result(&result)
    }

    #[tool(
        name = "ingest_file",
        description = "Read text, Markdown, HTML, PDF, or source code from disk and ingest it. Path must be under RAG_INGEST_ROOTS. Upserts by uri."
    )]
    pub(super) async fn ingest_file(
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
            .await?;
        Self::json_result(&result)
    }

    #[tool(
        name = "sync_sources",
        description = "Recursively sync supported text, Markdown, HTML, PDF, and source-code \
        files from an allowlisted directory. Adds or updates changed files and removes missing \
        sources only when requested."
    )]
    pub(super) async fn sync_sources(
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
        .await?;
        let report = match outcome {
            SourceSyncOutcome::Completed(report) | SourceSyncOutcome::Cancelled(report) => report,
        };
        Self::json_result(&report)
    }

    #[tool(
        name = "ingest_raw",
        description = "Register an immutable raw source (layer=raw): chunk, embed, store. Same uri+content is a no-op; content change is refused."
    )]
    pub(super) async fn ingest_raw(
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
            .await?;
        Self::json_result(&result)
    }

    #[tool(
        name = "add_drawer",
        description = "File verbatim content as a drawer (document): chunk, embed, store. \
        Requires wing and room; optional source_file/title/uri. Does not summarize on ingest."
    )]
    pub(super) async fn add_drawer(
        &self,
        Parameters(params): Parameters<AddDrawerParams>,
    ) -> Result<CallToolResult, McpError> {
        let wing = params.wing.trim();
        let room = params.room.trim();
        if wing.is_empty() {
            return Err(McpError::from(AppError::invalid("wing must be non-empty")));
        }
        if room.is_empty() {
            return Err(McpError::from(AppError::invalid("room must be non-empty")));
        }
        if params.content.is_empty() {
            return Err(McpError::from(AppError::invalid(
                "content must be non-empty",
            )));
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
            .await?;
        Self::json_result(&result)
    }

    #[tool(
        name = "check_duplicate",
        description = "Exact content-hash / uri dedupe probe before filing. Pass content and/or \
        hash (or content_hash), and/or uri. Returns is_duplicate, content_hash, and matches."
    )]
    pub(super) async fn check_duplicate(
        &self,
        Parameters(params): Parameters<CheckDuplicateParams>,
    ) -> Result<CallToolResult, McpError> {
        let body = self
            .blocking("check_duplicate", move |store| {
                store.check_duplicate(
                    params.content.as_deref(),
                    params.content_hash.as_deref(),
                    params.uri.as_deref(),
                )
            })
            .await?;
        Self::json_result(&body)
    }

    #[tool(
        name = "delete_by_source",
        description = "Bulk-delete all documents (and chunks/graph) whose source_file matches exactly. dry_run=true reports ids without deleting."
    )]
    pub(super) async fn delete_by_source(
        &self,
        Parameters(params): Parameters<DeleteBySourceParams>,
    ) -> Result<CallToolResult, McpError> {
        let source_file = params.source_file.trim();
        if source_file.is_empty() {
            return Err(McpError::from(AppError::invalid(
                "source_file (or source) must be non-empty",
            )));
        }
        let dry_run = params.dry_run.unwrap_or(false);
        if dry_run {
            let matched = crate::util::blocking_section(|| {
                self.store.list_documents_filtered(&DocumentFilter {
                    source_file: Some(source_file.to_string()),
                    include_archived: Some(true),
                    ..Default::default()
                })
            })?;
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

        let mut mutation_guard = Some(self.store.try_corpus_mutation_guard("source deletion")?);
        let document_ids =
            crate::util::blocking_section(|| self.store.delete_by_source_ids(source_file))?;
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
        name = "list_sources",
        description = "List immutable raw-layer sources without loading bodies. Filters: wing?, room?; limit defaults 50 (hard cap 200), offset defaults 0."
    )]
    pub(super) async fn list_sources(
        &self,
        Parameters(params): Parameters<ListSourcesParams>,
    ) -> Result<CallToolResult, McpError> {
        let filter = DocumentCatalogFilter {
            wing: non_blank(params.wing),
            room: non_blank(params.room),
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
        let docs =
            run_blocking("list_sources", move || store.list_document_catalog(&filter)).await?;
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
        description = "Fetch one raw-layer source by document_id or uri. include_chunks returns \
        text-only chunks, default 100 and hard cap 500 via chunk_limit; one response has an 8 \
        MiB aggregate chunk-text budget."
    )]
    pub(super) async fn get_source(
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
            return Err(McpError::from(AppError::invalid(
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
            if !crate::models::Layer::Raw.is(&doc.layer) {
                return Err(AppError::not_found(format!(
                    "document {} is not a raw source (layer={})",
                    doc.id, doc.layer
                )));
            }
            retrieval::document_with_chunks(&store, doc, include_chunks, chunk_limit)
                .map(SourceDetail::from)
        })
        .await?;
        Self::json_result(&body)
    }

    #[tool(
        name = "list_documents",
        description = "List stored documents without loading full content. Filters: wing?, \
        room?, source_file?, include_archived?, layer?, kind?, compiled? (false = raw compile \
        debt, true = sources a wiki page links to); limit defaults 50 (hard cap 200), offset \
        defaults 0. Default excludes archived/tombstone."
    )]
    pub(super) async fn list_documents(
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
            compiled: params.compiled,
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
        .await?;
        Self::json_result(&page.items)
    }

    #[tool(
        name = "get_document",
        description = "Get a document by id with metadata. include_chunks returns text-only \
        chunks, default 100 and hard cap 500 via chunk_limit; one response has an 8 MiB \
        aggregate chunk-text budget."
    )]
    pub(super) async fn get_document(
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
        .await?;
        Self::json_result(&body)
    }

    #[tool(
        name = "delete_document",
        description = "Delete a document, its chunks, and its graph node edges by document_id."
    )]
    pub(super) async fn delete_document(
        &self,
        Parameters(params): Parameters<DeleteDocumentParams>,
    ) -> Result<CallToolResult, McpError> {
        let document_id = params.document_id.clone();
        let deleted = self
            .blocking("delete_document", move |store| store.delete_document(&document_id))
            .await?;
        if !deleted {
            return Err(McpError::from(AppError::not_found(format!(
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
        description = "Update document meta without re-embedding: wing, room, title, \
        metadata_json, pinned, boost, status (and layer/kind/source_file). Optional content \
        triggers re-chunk+re-embed only when body changes (refused for layer=raw). MemPalace \
        update_drawer analogue."
    )]
    pub(super) async fn update_document_meta(
        &self,
        Parameters(params): Parameters<UpdateDocumentMetaParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = self.update_document_via_service(params).await?;
        Self::json_result(&result)
    }

    #[tool(
        name = "list_wings",
        description = "List distinct document wings with document counts (MemPalace taxonomy)."
    )]
    pub(super) async fn list_wings(
        &self,
        Parameters(_params): Parameters<ListWingsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::util::blocking_section(|| {
            let wings = self.store.list_wings()?;
            Self::json_result(&wings)
        })
    }

    #[tool(
        name = "list_rooms",
        description = "List distinct document rooms with counts; optional wing filter (MemPalace taxonomy)."
    )]
    pub(super) async fn list_rooms(
        &self,
        Parameters(params): Parameters<ListRoomsParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::util::blocking_section(|| {
            let wing = params
                .wing
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let rooms = self.store.list_rooms(wing)?;
            Self::json_result(&rooms)
        })
    }

    #[tool(
        name = "get_taxonomy",
        description = "Full wing → room tree with document counts plus unscoped/total totals."
    )]
    pub(super) async fn get_taxonomy(
        &self,
        Parameters(_params): Parameters<GetTaxonomyParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::util::blocking_section(|| {
            let tax = self.store.get_taxonomy()?;
            Self::json_result(&tax)
        })
    }

    #[tool(
        name = "stats",
        description = "Return store counts (documents, chunks, graph nodes/edges) and db path."
    )]
    pub(super) async fn stats(&self) -> Result<CallToolResult, McpError> {
        crate::util::blocking_section(|| {
            let (document_count, chunk_count, node_count, edge_count) = self.store.stats()?;
            let body = Stats {
                document_count,
                chunk_count,
                node_count,
                edge_count,
                db_path: self.store.path().display().to_string(),
            };
            Self::json_result(&body)
        })
    }

    #[tool(
        name = "status",
        description = "Index health: backend=duckdb, docs/chunks/nodes/edges, wings summary, \
        ready_for_search, fts_ready, embed_dims, db path, db/WAL sizes, and the automatic \
        snapshot footprint (count, bytes, retention, free disk) when configured."
    )]
    pub(super) async fn status(&self) -> Result<CallToolResult, McpError> {
        crate::util::blocking_section(|| {
            let body = DiagnosticsService::new(&self.store, &self.config).status()?;
            Self::json_result(&body)
        })
    }

    #[tool(
        name = "doctor",
        description = "Minimal integrity: schema_version vs expected, FTS ready, embed dims vs manifest, ingest_roots, ready_for_search."
    )]
    pub(super) async fn doctor(&self) -> Result<CallToolResult, McpError> {
        crate::util::blocking_section(|| {
            let body = DiagnosticsService::new(&self.store, &self.config).doctor()?;
            Self::json_result(&body)
        })
    }

    #[tool(
        name = "llm_status",
        description = "Local chat LLM + embeddings: llm_enabled, base_url, model, reachable \
        (short probe), embed provider/model/dims. Does not hang long when Ollama is down."
    )]
    pub(super) async fn llm_status(&self) -> Result<CallToolResult, McpError> {
        let body = self.llm_status_report().await?;
        Self::json_result(&body)
    }
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
