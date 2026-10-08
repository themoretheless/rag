//! Retrieval, embedding lifecycle, and context-packing tools.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router, ErrorData as McpError};
use serde::Serialize;

use super::facade::RagServer;
use super::tools::{
    ExpandChunksParams, FindSimilarParams, MultiGetParams, MultiQuerySearchParams,
    PackContextParams, ReembedDocumentParams, SearchParams,
};
use super::views::{ChunkView, DocumentDetail};
use crate::db::search::{
    acquire_corpus_search_guard, attach_context, fuse_rrf_many, search_with_corpus_guard,
    ContextExpansion, SearchQuery,
};
use crate::error::AppError;
use crate::models::{SearchHit, SearchMode};
use crate::retrieval::SearchCommand;
use crate::retrieval::{self, SimilarDocumentsQuery};
use crate::search_pack::pack_hits;
use crate::util::{non_blank, run_blocking};

#[tool_router(router = search_router, vis = "pub(super)")]
impl RagServer {
    #[tool(
        name = "search",
        description = "Search stored chunks (mode=lex|vec|hybrid). Supports filters, diversity, token packing, and optional context_expansion=neighbors|parent_section. Markdown hits include heading_path/section."
    )]
    pub(super) async fn search(
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
        .await?;
        Self::json_result(&hits)
    }

    #[tool(
        name = "multi_query_search",
        description = "Fuse an original query and caller-supplied rewrites with RRF. No LLM rewrite is performed implicitly."
    )]
    pub(super) async fn multi_query_search(
        &self,
        Parameters(params): Parameters<MultiQuerySearchParams>,
    ) -> Result<CallToolResult, McpError> {
        if params.queries.is_empty() || params.queries.len() > 16 {
            return Err(McpError::from(AppError::config(
                "queries must contain 1..=16 entries",
            )));
        }
        let top_k = params
            .top_k
            .map(|v| v as usize)
            .unwrap_or(self.config.default_top_k);
        let mode = retrieval::resolve_search_mode(
            params.mode.as_deref(),
            self.config.default_search_mode,
        )?;
        let corpus_guard = acquire_corpus_search_guard(&self.store, mode)?;
        if matches!(mode, SearchMode::Vec | SearchMode::Hybrid) {
            self.require_vec_compatible()?;
        }
        let mut lists = Vec::with_capacity(params.queries.len());
        for text in params.queries {
            let embedding = if matches!(mode, SearchMode::Vec | SearchMode::Hybrid) {
                Some(
                    self.embedder
                        .embed(std::slice::from_ref(&text))
                        .await?
                        .into_iter()
                        .next()
                        .ok_or_else(|| {
                            McpError::internal_error("embedder returned no vector for query", None)
                        })?,
                )
            } else {
                None
            };
            lists.push(search_with_corpus_guard(
                &self.store,
                &SearchQuery {
                    mode,
                    top_k,
                    query_text: Some(text),
                    query_embedding: embedding,
                    wing: non_blank(params.wing.clone()),
                    room: non_blank(params.room.clone()),
                    layer: non_blank(params.layer.clone()),
                    source_file: non_blank(params.source_file.clone()),
                    timeout_ms: params.timeout_ms.or(Some(5_000)),
                    fts_stemmer: self.config.fts_stemmer.clone(),
                    ..SearchQuery::default()
                },
                &corpus_guard,
            )?);
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
    pub(super) async fn get_embedding_manifest(&self) -> Result<CallToolResult, McpError> {
        let manifest = self.store.get_embedding_manifest()?;
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
    pub(super) async fn reembed_document(
        &self,
        Parameters(params): Parameters<ReembedDocumentParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .reembed_document_pipeline(params.document_id.trim())
            .await?;
        Self::json_result(&result)
    }

    #[tool(
        name = "multi_get",
        description = "Get up to 100 documents by id in request order. Returns found documents plus missing ids. include_chunks returns no embeddings, defaults to 100 chunks per document (hard cap 500 via chunk_limit), and has an 8 MiB aggregate chunk-text budget."
    )]
    pub(super) async fn multi_get(
        &self,
        Parameters(params): Parameters<MultiGetParams>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let include_chunks = params.include_chunks.unwrap_or(false);
        let chunk_limit = params.chunk_limit.map(|value| value as usize);
        let result = run_blocking("multi_get", move || {
            retrieval::multi_get(&store, params.document_ids, include_chunks, chunk_limit)
        })
        .await?;
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
    pub(super) async fn expand_chunks(
        &self,
        Parameters(params): Parameters<ExpandChunksParams>,
    ) -> Result<CallToolResult, McpError> {
        let chunks = retrieval::expand_chunks(
            &self.store,
            &params.document_id,
            params.chunk_index,
            params.radius.unwrap_or(1),
        )?
        .into_iter()
        .map(ChunkView::from)
        .collect::<Vec<_>>();
        Self::json_result(&chunks)
    }

    #[tool(
        name = "find_similar",
        description = "Find documents with chunk embeddings similar to a seed document. Uses the normalized mean of its existing chunk vectors and excludes the seed."
    )]
    pub(super) async fn find_similar(
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
        )?;
        Self::json_result(&hits)
    }

    #[tool(
        name = "pack_context",
        description = "Pack ranked search hits under a complete citation-block budget (~4 chars/token), including headers and optional neighbors/parent-section expansion. total_tokens never exceeds max_tokens; 0 returns an empty block. Packed content is canonical; duplicate snippet/context text is omitted."
    )]
    pub(super) async fn pack_context(
        &self,
        Parameters(params): Parameters<PackContextParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_tokens = params
            .max_tokens
            .map(|t| t as usize)
            .unwrap_or(self.config.max_context_tokens);

        let expansion = match params.context_expansion.as_deref() {
            Some(raw) => Some(ContextExpansion::parse(raw)?),
            None => None,
        };
        let mut hits: Vec<SearchHit> = params.hits.into_iter().map(SearchHit::from).collect();
        attach_context(
            &self.store,
            &mut hits,
            expansion,
            params.neighbor_chunks.unwrap_or(1) as usize,
        )?;
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
}

#[derive(Debug, Serialize)]
struct PackContextResult {
    hits: Vec<SearchHit>,
    total_tokens: usize,
    max_tokens: usize,
    omitted_count: usize,
    context_text: String,
}
