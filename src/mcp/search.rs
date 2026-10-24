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
use crate::models::SearchHit;
use crate::retrieval::{self, SimilarDocumentsQuery};
use crate::util::run_blocking;

#[tool_router(router = search_router, vis = "pub(super)")]
impl RagServer {
    #[tool(
        name = "search",
        description = "Search stored chunks (mode=lex|vec|hybrid). Supports filters, diversity, token packing, and optional context_expansion=neighbors|parent_section|parent. Markdown hits include heading_path/section."
    )]
    pub(super) async fn search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let command = retrieval::SearchRequest {
            query: params.query,
            mode: params.mode,
            top_k: params.top_k.map(|value| value as usize),
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
            max_context_tokens: params.max_context_tokens.map(|value| value as usize),
            max_chunks_per_document: params.max_chunks_per_document.map(|value| value as usize),
            context_expansion: params.context_expansion,
            neighbor_chunks: params.neighbor_chunks.map(|value| value as usize),
            timeout_ms: params.timeout_ms,
            rrf_k: params.rrf_k,
            rerank: params.rerank,
            rewrite: params.rewrite,
            hyde: params.hyde,
        }
        .into_command(&self.config);
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
        let requested_top_k = params
            .top_k
            .map(|n| n as usize)
            .unwrap_or(self.config.default_top_k);
        let reranker =
            retrieval::maybe_build_reranker(&self.config, params.rerank.unwrap_or(false));
        let pool_top_k = if reranker.is_some() {
            requested_top_k
                .max(self.config.rerank_top_n)
                .min(crate::db::search::MAX_TOP_K)
        } else {
            requested_top_k
        };
        let rerank_query = params.queries.first().cloned().unwrap_or_default();
        let hits = retrieval::multi_query_search(
            &self.store,
            self.embedder.as_ref(),
            &self.config,
            retrieval::MultiQuerySearchCommand {
                queries: params.queries,
                top_k: Some(pool_top_k),
                mode: params.mode,
                wing: params.wing,
                room: params.room,
                layer: params.layer,
                source_file: params.source_file,
                timeout_ms: params.timeout_ms,
            },
        )
        .await?;
        let hits = match reranker {
            Some(reranker) => {
                retrieval::rerank_hits_or_fallback(
                    reranker,
                    &rerank_query,
                    hits,
                    requested_top_k,
                    Some(self.config.max_context_tokens),
                )
                .await
            }
            None => hits,
        };
        Self::json_result(&hits)
    }

    #[tool(
        name = "get_embedding_manifest",
        description = "Return the corpus embedding identity (provider, model, dims, endpoint fingerprint). Empty object fields when none recorded yet."
    )]
    pub(super) async fn get_embedding_manifest(&self) -> Result<CallToolResult, McpError> {
        crate::util::blocking_section(|| {
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
        })
    }

    #[tool(
        name = "reembed_document",
        description = "Refresh one document's vectors only when the live embedding identity \
        already matches the corpus manifest. Use complete uncapped reembed_all for \
        provider/model/dims/endpoint migration."
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
        description = "Get up to 100 documents by id in request order. Returns found documents \
        plus missing ids. include_chunks returns no embeddings, defaults to 100 chunks per \
        document (hard cap 500 via chunk_limit), and has an 8 MiB aggregate chunk-text budget."
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
        crate::util::blocking_section(|| {
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
        })
    }

    #[tool(
        name = "find_similar",
        description = "Find documents with chunk embeddings similar to a seed document. Uses the \
        normalized mean of its existing chunk vectors and excludes the seed."
    )]
    pub(super) async fn find_similar(
        &self,
        Parameters(params): Parameters<FindSimilarParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::util::blocking_section(|| {
            let hits = retrieval::find_similar(
                &self.store,
                &self.config,
                SimilarDocumentsQuery {
                    document_id: params.document_id,
                    top_k: params.top_k.unwrap_or(self.config.default_top_k as u32) as usize,
                    wing: params.wing,
                    room: params.room,
                    fts_stemmer: self.config.fts_stemmer.clone(),
                },
            )?;
            Self::json_result(&hits)
        })
    }

    #[tool(
        name = "pack_context",
        description = "Pack ranked search hits under a complete citation-block budget (~4 \
        chars/token), including headers and optional neighbors/parent-section expansion. \
        total_tokens never exceeds max_tokens; 0 returns an empty block. Packed content is \
        canonical; duplicate snippet/context text is omitted."
    )]
    pub(super) async fn pack_context(
        &self,
        Parameters(params): Parameters<PackContextParams>,
    ) -> Result<CallToolResult, McpError> {
        let command = retrieval::PackContextCommand {
            hits: params.hits.into_iter().map(SearchHit::from).collect(),
            max_tokens: params.max_tokens.map(|t| t as usize),
            context_expansion: params.context_expansion,
            neighbor_chunks: params.neighbor_chunks.map(|n| n as usize),
        };
        let store = self.store.clone();
        let config = self.config.clone();
        let packed = run_blocking("pack_context", move || {
            retrieval::pack_context(&store, &config, command)
        })
        .await?
        .packed;
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
