//! Wiki compilation, index, diary, and durable operations-log tools.

use crate::mcp::error::mcp_error;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::Serialize;

use crate::db::store::MAX_OPS_LOG_PAGE;

use super::facade::RagServer;
use super::maintain::clamp_maintenance_max_docs;
use super::tools::{
    AppendLogParams, ArchiveMemoryItemsParams, CheckpointParams, CompileSourceParams,
    ConsolidateMemoryItemsParams, ConsolidateParams, DiaryReadParams, DiaryWriteParams,
    FileAnswerCitationParams, FileAnswerParams, GetSchemaParams, GetWikiPageParams,
    ListMemoryLifecycleCandidatesParams, ListRecentOpsParams, ListWikiPagesParams,
    MemoriesFiledAwayParams, QueryWithIndexParams, ReadIndexParams, ReadLogParams,
    RebuildIndexParams, ReconnectParams, RefreshStaleWikiParams, SearchWikiParams,
    UpdateIndexEntryParams, UpdateSchemaParams, WakeUpParams, WriteWikiPageParams,
};
use crate::db::search::{
    DiversityMode, SearchQuery, acquire_corpus_search_guard, search_with_corpus_guard,
};
use crate::db::store::WikiPageMetaFilter;
use crate::db::{DEFAULT_CATALOG_PAGE_SIZE, MAX_CATALOG_PAGE_SIZE};
use crate::diagnostics::DiagnosticsService;
use crate::diary;
use crate::error::AppError;
use crate::memory_lifecycle;
use crate::models::{
    Document, IndexQueryPage, IndexQueryResult, OpsLogEntry, SearchMode, WikiIndexEntry,
};
use crate::retrieval::{self};
use crate::util::{non_blank, run_blocking};
use crate::wiki;
use crate::wiki::FileAnswerCitation;

/// Diary entries `diary_read` returns when `last_n` is absent.
const DEFAULT_DIARY_LAST_N: u32 = 10;

/// Upper bound for `wake_up` diary / pinned-memory limits.
const MAX_WAKE_UP_LIMIT: usize = 200;
/// Upper bound for `list_memory_lifecycle_candidates` page size.
const MAX_LIFECYCLE_CANDIDATES: usize = 1000;

/// Resolve an optional client-supplied limit to `default`, clamped to `1..=max`.
fn clamp_limit(limit: Option<u32>, default: usize, max: usize) -> usize {
    limit
        .map(|n| usize::try_from(n).unwrap_or(max))
        .unwrap_or(default)
        .clamp(1, max)
}

#[tool_router(router = wiki_router, vis = "pub(super)")]
impl RagServer {
    #[tool(
        name = "get_schema",
        description = "Read the agent conventions schema document at uri schema://agents (layer=schema). Seeds a default when missing unless no_default=true."
    )]
    pub(super) async fn get_schema(
        &self,
        Parameters(params): Parameters<GetSchemaParams>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking_tool("get_schema", move |store| {
            let no_default = params.no_default.unwrap_or(false);
            if no_default {
                let doc = store
                    .documents()
                    .get_schema_document()
                    .map_err(mcp_error)?
                    .ok_or_else(|| {
                        mcp_error(AppError::not_found(
                            "schema document not found at schema://agents",
                        ))
                    })?;
                let body = crate::wiki::SchemaDocumentView {
                    document_id: doc.id,
                    uri: doc.uri,
                    title: doc.title,
                    content: doc.content,
                    layer: doc.layer.to_string(),
                    kind: doc.kind.to_string(),
                    content_hash: doc.content_hash,
                    created_at: doc.created_at.to_rfc3339(),
                    updated_at: doc.updated_at.to_rfc3339(),
                    created: false,
                };
                return Self::json_result(&body);
            }
            let body = crate::wiki::get_schema(store).map_err(mcp_error)?;
            Self::json_result(&body)
        })
        .await
    }

    #[tool(
        name = "update_schema",
        description = "Create or replace the agent conventions schema document at uri schema://agents (layer=schema, kind=schema)."
    )]
    pub(super) async fn update_schema(
        &self,
        Parameters(params): Parameters<UpdateSchemaParams>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking_tool("update_schema", move |store| {
            let body = crate::wiki::update_schema(
                store,
                &params.content,
                params.title.as_deref(),
                params.agent.as_deref(),
            )
            .map_err(mcp_error)?;
            Self::json_result(&body)
        })
        .await
    }

    #[tool(
        name = "diary_write",
        description = "Append a verbatim per-agent diary note (layer=diary, kind=diary, \
        wing=agents/<name> or wing?). Chunks+embeds for search; logs ops_log diary_write."
    )]
    pub(super) async fn diary_write(
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
        .map_err(mcp_error)?;
        Self::json_result(&res)
    }

    #[tool(
        name = "diary_read",
        description = "Read recent diary entries for an agent (newest first). Filters kind=diary \
        under wing agents/<name> (or room=agent). last_n/limit default 10."
    )]
    pub(super) async fn diary_read(
        &self,
        Parameters(params): Parameters<DiaryReadParams>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking_tool("diary_read", move |store| {
            let last_n = params.last_n.unwrap_or(DEFAULT_DIARY_LAST_N) as usize;
            let entries =
                diary::diary_read(store, &params.agent_name, last_n).map_err(mcp_error)?;
            Self::json_result(&entries)
        })
        .await
    }

    #[tool(
        name = "wake_up",
        description = "Session bootstrap: status + last N diary entries (agent_name?) + pinned \
        docs + schema snippet only if schema document already exists (does not seed default)."
    )]
    pub(super) async fn wake_up(
        &self,
        Parameters(params): Parameters<WakeUpParams>,
    ) -> Result<CallToolResult, McpError> {
        let config = self.config.clone();
        self.blocking_tool("wake_up", move |store| {
            let status = DiagnosticsService::new(store, &config)
                .status()
                .map_err(mcp_error)?;
            let diary_limit = clamp_limit(params.diary_limit, 5, MAX_WAKE_UP_LIMIT);
            let pinned_limit = clamp_limit(params.pinned_limit, 20, MAX_WAKE_UP_LIMIT);
            let body = diary::wake_up(
                store,
                status,
                params.agent_name.as_deref(),
                diary_limit,
                pinned_limit,
            )
            .map_err(mcp_error)?;
            Self::json_result(&body)
        })
        .await
    }

    #[tool(
        name = "checkpoint",
        description = "Session savepoint: always append ops_log (op=checkpoint, \
        message=summary). Optional diary content writes a diary entry for agent_name (default \
        agent). Prefer this over separate append_log + diary_write at session boundaries."
    )]
    pub(super) async fn checkpoint(
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
        .map_err(mcp_error)?;
        Self::json_result(&res)
    }

    #[tool(
        name = "append_log",
        description = "Append an ops_log timeline entry (Karpathy log.md). Auto-assigns id/seq/ts."
    )]
    pub(super) async fn append_log(
        &self,
        Parameters(params): Parameters<AppendLogParams>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking_tool("append_log", move |store| {
            let payload_json = match params.payload_json {
                Some(s) if !s.trim().is_empty() => {
                    if let Err(e) = serde_json::from_str::<serde_json::Value>(&s) {
                        return Err(mcp_error(AppError::invalid(format!(
                            "payload_json is not valid JSON: {e}"
                        ))));
                    }
                    s
                }
                _ => "{}".to_string(),
            };
            // The caller's entry verbatim: an optional prefix and entity kind.
            let entry = OpsLogEntry {
                op: params.op,
                prefix: params.prefix,
                message: params.message.unwrap_or_default(),
                entity_id: params.entity_id,
                entity_kind: params.entity_kind,
                payload_json,
                agent_name: params.agent_name,
                ..OpsLogEntry::default()
            };
            let written = store.ops_log().append(&entry).map_err(mcp_error)?;
            Self::json_result(&written)
        })
        .await
    }

    #[tool(
        name = "read_log",
        description = "Read ops_log by id or seq; without either, list recent entries (limit default 50)."
    )]
    pub(super) async fn read_log(
        &self,
        Parameters(params): Parameters<ReadLogParams>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking_tool("read_log", move |store| {
            let id = params
                .id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let has_key = id.is_some() || params.seq.is_some();
            let rows = store
                .ops_log()
                .read_ops_log(id, params.seq, params.limit.map(|n| n as usize))
                .map_err(mcp_error)?;
            if has_key {
                let entry = rows.into_iter().next().ok_or_else(|| {
                    mcp_error(AppError::not_found(format!(
                        "ops_log entry not found (id={:?}, seq={:?})",
                        params.id, params.seq
                    )))
                })?;
                return Self::json_result(&entry);
            }
            Self::json_result(&rows)
        })
        .await
    }

    #[tool(
        name = "list_recent_ops",
        description = "List recent ops_log entries newest-first (default limit 20). Empty list if ops_log table is missing."
    )]
    pub(super) async fn list_recent_ops(
        &self,
        Parameters(params): Parameters<ListRecentOpsParams>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking_tool("list_recent_ops", move |store| {
            let limit = clamp_limit(params.limit, 20, MAX_OPS_LOG_PAGE);
            let ops = store.ops_log().list_recent_ops(limit).map_err(mcp_error)?;
            Self::json_result(&ops)
        })
        .await
    }

    #[tool(
        name = "memories_filed_away",
        description = "Recent memory-filing ops from ops_log \
        (ingest/drawer/wiki/diary/checkpoint), newest first. Empty/quiet when ops_log missing or \
        no matches. Optional limit (default 20)."
    )]
    pub(super) async fn memories_filed_away(
        &self,
        Parameters(params): Parameters<MemoriesFiledAwayParams>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking_tool("memories_filed_away", move |store| {
            let limit = clamp_limit(params.limit, 20, MAX_OPS_LOG_PAGE);
            let ops_log_available = store.ops_log().ops_log_exists().map_err(mcp_error)?;
            let ops = if ops_log_available {
                store
                    .ops_log()
                    .memories_filed_away(limit)
                    .map_err(mcp_error)?
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
        })
        .await
    }

    #[tool(
        name = "reconnect",
        description = "Force reconnect / cache clear after external DB writes. DuckDB \
        single-process has no client cache: always returns ok=true (no-op success)."
    )]
    pub(super) async fn reconnect(
        &self,
        Parameters(_params): Parameters<ReconnectParams>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking_tool("reconnect", move |store| {
            // DuckDB is opened once per process under a mutex; there is no Chroma-style
            // HNSW client cache to drop. Success is always reported for MemPalace parity.
            Self::json_result(&serde_json::json!({
                "ok": true,
                "success": true,
                "backend": "duckdb",
                "message": "DuckDB has no client cache; reconnect is a no-op",
                "db_path": store.path().display().to_string(),
            }))
        })
        .await
    }

    #[tool(
        name = "query_with_index",
        description = "Index-first wiki navigation: match query against wiki_index catalog (slug/title/summary/kind), optionally load page bodies."
    )]
    pub(super) async fn query_with_index(
        &self,
        Parameters(params): Parameters<QueryWithIndexParams>,
    ) -> Result<CallToolResult, McpError> {
        let config = self.config.clone();
        self.blocking_tool("query_with_index", move |store| {
            let top_k = params
                .top_k
                .map(|k| k as usize)
                .unwrap_or(config.default_top_k);
            let include_content = params.include_content.unwrap_or(false);

            let matches = store
                .wiki_index()
                .query_wiki_index(params.query.trim(), top_k)
                .map_err(mcp_error)?;

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
                    if let Some(doc) = store.documents().get_document(pid).map_err(mcp_error)? {
                        pages.push(IndexQueryPage {
                            document_id: doc.id,
                            uri: doc.uri,
                            title: doc.title,
                            layer: doc.layer.to_string(),
                            kind: doc.kind.to_string(),
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
        })
        .await
    }

    #[tool(
        name = "search_wiki",
        description = "Search compiled wiki layer only (layer=wiki). Same modes as search: lex|vec|hybrid with filters, min_score, diversity."
    )]
    pub(super) async fn search_wiki(
        &self,
        Parameters(params): Parameters<SearchWikiParams>,
    ) -> Result<CallToolResult, McpError> {
        let top_k = params
            .top_k
            .map(|k| k as usize)
            .unwrap_or(self.config.default_top_k);

        // Rerank stage (opt-in): expand the candidate pool, rerank, cut back.
        let reranker =
            retrieval::maybe_build_reranker(&self.config, params.rerank.unwrap_or(false));
        let pool_k = match reranker.is_some() {
            true => top_k
                .max(self.config.rerank_top_n)
                .min(crate::db::search::MAX_TOP_K),
            false => top_k,
        };

        let mode =
            retrieval::resolve_search_mode(params.mode.as_deref(), self.config.default_search_mode)
                .map_err(Self::map_err)?;
        let corpus_guard = acquire_corpus_search_guard(&self.store, mode).map_err(Self::map_err)?;

        if matches!(mode, SearchMode::Vec | SearchMode::Hybrid) {
            self.require_vec_compatible().map_err(mcp_error)?;
        }

        let diversity = match params
            .diversity
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => Some(DiversityMode::parse(raw).map_err(mcp_error)?),
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
                .map_err(mcp_error)?;
            let emb = query_vecs.into_iter().next().ok_or_else(|| {
                McpError::internal_error("embedder returned no vector for query", None)
            })?;
            Some(emb)
        } else {
            None
        };

        let opts = SearchQuery {
            mode,
            top_k: pool_k,
            query_text: Some(params.query.clone()),
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

        let hits = crate::util::blocking_section(|| {
            search_with_corpus_guard(&self.store, &opts, &corpus_guard)
        })
        .map_err(mcp_error)?;
        let hits = match reranker {
            Some(reranker) => {
                retrieval::rerank_hits_or_fallback(
                    reranker,
                    &params.query,
                    hits,
                    top_k,
                    max_context_tokens,
                )
                .await
            }
            None => hits,
        };
        Self::json_result(&hits)
    }

    #[tool(
        name = "file_answer",
        description = "Persist a cited answer as a wiki page (layer=wiki, category=answers): \
        write body + citations metadata, rebuild graph, append ops_log, touch wiki_index."
    )]
    pub(super) async fn file_answer(
        &self,
        Parameters(params): Parameters<FileAnswerParams>,
    ) -> Result<CallToolResult, McpError> {
        self.require_vec_compatible().map_err(mcp_error)?;

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
            wiki::FileAnswerRequest {
                title: &params.title,
                body: &params.body,
                slug: params.slug.as_deref(),
                citations,
                agent: params.agent.as_deref(),
            },
        )
        .await
        .map_err(mcp_error)?;

        Self::json_result(&result)
    }

    #[tool(
        name = "read_index",
        description = "Read the wiki content catalog (index.md analogue): one row per page with \
        slug, title, kind, category, one-line summary. format=json|markdown."
    )]
    pub(super) async fn read_index(
        &self,
        Parameters(params): Parameters<ReadIndexParams>,
    ) -> Result<CallToolResult, McpError> {
        let format = params
            .format
            .as_deref()
            .unwrap_or("json")
            .trim()
            .to_ascii_lowercase();
        let want_markdown = format == "markdown" || format == "md";
        // Catalog and markdown come from one blocking task so both describe
        // the same catalog state.
        let (entries, markdown) = self
            .blocking("read_index", move |store| {
                let entries = store.wiki_index().list_wiki_index()?;
                let markdown = if want_markdown {
                    Some(store.wiki_index().render_wiki_index_markdown()?)
                } else {
                    None
                };
                Ok((entries, markdown))
            })
            .await
            .map_err(mcp_error)?;
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
    pub(super) async fn update_index_entry(
        &self,
        Parameters(params): Parameters<UpdateIndexEntryParams>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking_tool("update_index_entry", move |store| {
            let entry = store
                .wiki_index()
                .update_wiki_index_entry_fields(
                    &params.slug,
                    params.title,
                    params.summary,
                    params.kind,
                    params.category,
                    params.page_id,
                )
                .map_err(mcp_error)?;

            let _ = store
                .ops_log()
                .append(
                    &OpsLogEntry::new(
                        "index_update",
                        "INDEX",
                        format!("updated index entry {}", entry.slug),
                    )
                    .entity(Some(entry.id.clone()), "wiki_index")
                    .payload(
                        serde_json::json!({
                            "slug": entry.slug,
                            "title": entry.title,
                            "kind": entry.kind,
                        })
                        .to_string(),
                    ),
                )
                .map_err(mcp_error)?;

            Self::json_result(&entry)
        })
        .await
    }

    #[tool(
        name = "rebuild_index",
        description = "Clear and rebuild the wiki content catalog from all layer=wiki documents (one-line summaries from content or metadata)."
    )]
    pub(super) async fn rebuild_index(
        &self,
        Parameters(_params): Parameters<RebuildIndexParams>,
    ) -> Result<CallToolResult, McpError> {
        // Rebuild and its ops-log row share one blocking task: a cancelled
        // request cannot commit the rebuild and skip the log entry.
        let count = self
            .blocking("rebuild_index", |store| {
                let count = store.wiki_index().rebuild_wiki_index_from_docs()?;
                store.ops_log().append(
                    &OpsLogEntry::new(
                        "index_rebuild",
                        "INDEX",
                        format!("rebuilt wiki_index ({count} entries)"),
                    )
                    .entity(None, "wiki_index")
                    .payload(serde_json::json!({ "count": count }).to_string()),
                )?;
                Ok(count)
            })
            .await
            .map_err(mcp_error)?;

        Self::json_result(&RebuildIndexResult { count })
    }

    #[tool(
        name = "write_wiki_page",
        description = "Create or update a compiled wiki page (layer=wiki, uri wiki://slug); \
        re-embeds and rebuilds graph + index entry. On an existing page omitted \
        kind/category/summary/placement are kept; create_only=true refuses an existing page \
        (CONFLICT). if_match_revision/etag enforce CAS."
    )]
    pub(super) async fn write_wiki_page(
        &self,
        Parameters(params): Parameters<WriteWikiPageParams>,
    ) -> Result<CallToolResult, McpError> {
        let mode = if params.create_only.unwrap_or(false) {
            crate::app::WikiWriteMode::Create
        } else {
            crate::app::WikiWriteMode::Upsert
        };
        self.write_wiki(mode, params).await
    }

    #[tool(
        name = "update_wiki_page",
        description = "Update an existing wiki page by slug, wiki://uri or document id. Omitted \
        kind/category/summary preserve their current values, as do project placement, status, \
        pin/boost state, source ownership and unrelated metadata. Pass if_match_revision/etag \
        from get_wiki_page to avoid clobbering concurrent writes; use write_wiki_page to create \
        a page."
    )]
    pub(super) async fn update_wiki_page(
        &self,
        Parameters(params): Parameters<WriteWikiPageParams>,
    ) -> Result<CallToolResult, McpError> {
        if params.create_only == Some(true) {
            return Err(mcp_error(AppError::invalid(
                "update_wiki_page does not accept create_only; use write_wiki_page with create_only=true",
            )));
        }
        self.write_wiki(crate::app::WikiWriteMode::Cas, params)
            .await
    }

    #[tool(
        name = "get_wiki_page",
        description = "Fetch a wiki page by slug, wiki://uri, or document id. Returns revision+etag for If-Match on write."
    )]
    pub(super) async fn get_wiki_page(
        &self,
        Parameters(params): Parameters<GetWikiPageParams>,
    ) -> Result<CallToolResult, McpError> {
        let key = params.id_or_slug.trim().to_owned();
        let store = self.store.clone();
        let doc = run_blocking("get_wiki_page", move || {
            if let Some(document) = store.documents().get_document(&key)? {
                return Ok(document);
            }
            if let Some(document) = store.documents().find_by_uri(&key)? {
                return Ok(document);
            }
            store
                .documents()
                .find_by_uri(&format!("wiki://{key}"))?
                .ok_or_else(|| AppError::not_found(format!("wiki page not found: {key}")))
        })
        .await
        .map_err(mcp_error)?;
        Self::json_result(&document_with_etag(&doc))
    }

    #[tool(
        name = "list_wiki_pages",
        description = "List lean layer=wiki catalog rows without page bodies. Filters: q?, \
        kind?, category?, wing?, room?; limit defaults 50 (hard cap 200), offset defaults 0."
    )]
    pub(super) async fn list_wiki_pages(
        &self,
        Parameters(params): Parameters<ListWikiPagesParams>,
    ) -> Result<CallToolResult, McpError> {
        let filter = WikiPageMetaFilter {
            q: non_blank(params.q),
            limit: Some(
                params
                    .limit
                    .map(|value| value as usize)
                    .unwrap_or(DEFAULT_CATALOG_PAGE_SIZE)
                    .clamp(1, MAX_CATALOG_PAGE_SIZE),
            ),
            offset: Some(params.offset.unwrap_or(0) as usize),
            kind: non_blank(params.kind),
            category: non_blank(params.category),
            wing: non_blank(params.wing),
            room: non_blank(params.room),
        };
        let store = self.store.clone();
        let (rows, _) = run_blocking("list_wiki_pages", move || {
            store.documents().list_wiki_page_metas_filtered(&filter)
        })
        .await
        .map_err(mcp_error)?;
        Self::json_result(&rows)
    }

    #[tool(
        name = "compile_source",
        description = "Local LLM (Ollama chat) compiles a raw document into wiki pages. dry_run proposes without write."
    )]
    pub(super) async fn compile_source(
        &self,
        Parameters(params): Parameters<CompileSourceParams>,
    ) -> Result<CallToolResult, McpError> {
        self.require_vec_compatible().map_err(mcp_error)?;
        if !self.config.llm_enabled {
            return Err(mcp_error(AppError::config(
                "LLM disabled (RAG_LLM_ENABLED=false)",
            )));
        }
        // The configured client: provider dialect, timeout and max_tokens all
        // come from `Config`, exactly as for every other LLM tool.
        let llm = self.require_llm().map_err(mcp_error)?;
        let res = wiki::compile_source(
            &self.store,
            &self.embedder,
            &self.config,
            llm,
            &params.source_id_or_uri,
            params.dry_run.unwrap_or(false),
            params.agent.as_deref(),
        )
        .await
        .map_err(mcp_error)?;
        Self::json_result(&res)
    }

    #[tool(
        name = "consolidate",
        description = "Local LLM merges N document texts into one wiki page proposal \
        (concept/entity). apply=false (default) returns proposal only; apply=true writes wiki \
        page, rebuilds graph+index, links sources via related edges, ops_log. Capped by \
        RAG_MAINT_MAX_DOCS / max_docs. Optional slug/title/kind/category overrides."
    )]
    pub(super) async fn consolidate(
        &self,
        Parameters(params): Parameters<ConsolidateParams>,
    ) -> Result<CallToolResult, McpError> {
        let apply = params.apply.unwrap_or(false);
        if apply {
            self.require_vec_compatible().map_err(mcp_error)?;
        }
        let llm = self.require_llm().map_err(mcp_error)?;
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
            wiki::ConsolidateRequest {
                document_ids: &params.document_ids,
                apply,
                opts,
                agent: params.agent.as_deref(),
            },
        )
        .await
        .map_err(mcp_error)?;
        Self::json_result(&res)
    }

    #[tool(
        name = "list_memory_lifecycle_candidates",
        description = "List durable memory lifecycle candidates without content. Defaults to \
        active items; optional status/layer/kind filters and limit (default 100, capped at 1000). Deterministic and does not \
        require an LLM."
    )]
    pub(super) async fn list_memory_lifecycle_candidates(
        &self,
        Parameters(params): Parameters<ListMemoryLifecycleCandidatesParams>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking_tool("list_memory_lifecycle_candidates", move |store| {
            let rows = memory_lifecycle::list_candidates(
                store,
                params.status.as_deref(),
                params.layer.as_deref(),
                params.kind.as_deref(),
                clamp_limit(params.limit, 100, MAX_LIFECYCLE_CANDIDATES),
            )
            .map_err(mcp_error)?;
            Self::json_result(&rows)
        })
        .await
    }

    #[tool(
        name = "consolidate_memory_items",
        description = "Idempotently mark selected memory documents consolidated into an existing \
        output document. Adds structured provenance to the output and source metadata; no LLM or \
        content rewrite."
    )]
    pub(super) async fn consolidate_memory_items(
        &self,
        Parameters(params): Parameters<ConsolidateMemoryItemsParams>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking_tool("consolidate_memory_items", move |store| {
            let result = memory_lifecycle::consolidate_selected(
                store,
                &params.document_ids,
                &params.output_document_id,
                params.agent.as_deref(),
            )
            .map_err(mcp_error)?;
            Self::json_result(&result)
        })
        .await
    }

    #[tool(
        name = "archive_memory_items",
        description = "Idempotently archive selected memory documents. Already archived ids are \
        reported as skipped; missing ids are structured in the result. No LLM required."
    )]
    pub(super) async fn archive_memory_items(
        &self,
        Parameters(params): Parameters<ArchiveMemoryItemsParams>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking_tool("archive_memory_items", move |store| {
            let result = memory_lifecycle::archive_selected(
                store,
                &params.document_ids,
                params.agent.as_deref(),
            )
            .map_err(mcp_error)?;
            Self::json_result(&result)
        })
        .await
    }

    #[tool(
        name = "lint_wiki",
        description = "Lint compiled wiki and graph link health: missing/stale index, broken or \
        duplicate wikilinks, unresolved stubs, orphan pages/documents, self-links, uncompiled \
        raw, and aggregate health counts."
    )]
    pub(super) async fn lint_wiki(&self) -> Result<CallToolResult, McpError> {
        let report = self.admin_use_case().lint_wiki().await.map_err(mcp_error)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "refresh_stale_wiki",
        description = "Find wiki pages older than linked raw parents (graph related / metadata \
        source_* / citations / source: lines). dry_run=true (default) lists for the agent; \
        dry_run=false re-runs compile_source per unique raw when LLM is enabled and ChatClient \
        is present (capped by max_docs / RAG_MAINT_MAX_DOCS). Always logs ops_log."
    )]
    pub(super) async fn refresh_stale_wiki(
        &self,
        Parameters(params): Parameters<RefreshStaleWikiParams>,
    ) -> Result<CallToolResult, McpError> {
        let dry_run = params.dry_run.unwrap_or(true);
        let max_docs = Some(clamp_maintenance_max_docs(
            self.config.maint_max_docs,
            params.max_docs.map(|n| n as usize),
        ));

        if !dry_run {
            self.require_vec_compatible().map_err(mcp_error)?;
        }

        let llm_ref = if !dry_run && self.config.llm_enabled {
            self.completer()
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
        .map_err(mcp_error)?;
        Self::json_result(&res)
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

impl RagServer {
    /// Shared adapter for the wiki write tools: one use case, one mode each.
    async fn write_wiki(
        &self,
        mode: crate::app::WikiWriteMode,
        params: WriteWikiPageParams,
    ) -> Result<CallToolResult, McpError> {
        self.require_vec_compatible().map_err(mcp_error)?;
        let res = self
            .wiki_use_case()
            .write(crate::app::WikiWriteCommand {
                mode,
                slug: Some(params.slug),
                title: params.title,
                content: params.content,
                wing: params.wing,
                room: params.room,
                kind: params.kind,
                category: params.category,
                summary: params.summary,
                agent: params.agent,
                if_match_revision: params.if_match_revision,
                if_match_etag: params.if_match_etag,
                // MCP write_wiki_page keeps its historical rule: a blank title
                // on an existing page becomes the slug.
                blank_title_is_slug: mode == crate::app::WikiWriteMode::Upsert,
                ..Default::default()
            })
            .await
            .map_err(mcp_error)?;
        Self::json_result(&res)
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn clamp_limit_defaults_and_bounds() {
        assert_eq!(clamp_limit(None, 20, MAX_OPS_LOG_PAGE), 20);
        assert_eq!(clamp_limit(Some(0), 20, MAX_OPS_LOG_PAGE), 1);
        assert_eq!(clamp_limit(Some(7), 20, MAX_OPS_LOG_PAGE), 7);
        assert_eq!(
            clamp_limit(Some(u32::MAX), 20, MAX_OPS_LOG_PAGE),
            MAX_OPS_LOG_PAGE
        );
        assert_eq!(
            clamp_limit(Some(u32::MAX), 100, MAX_LIFECYCLE_CANDIDATES),
            MAX_LIFECYCLE_CANDIDATES
        );
    }
}
