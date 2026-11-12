//! `SearchUseCase`: search, `pack_context` and `multi_get` for both transports.

use std::sync::Arc;

use serde::Serialize;

use crate::config::Config;
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::eval::{RetrievalTrace, SourceVersion, SpanBuilder, source_versions_json};
use crate::models::{Document, SearchHit};
use crate::retrieval::{self, ChunkText, DocumentWithChunks, PackContextCommand, SearchRequest};
use crate::util::run_blocking;

/// Retrieval use cases over one store, embedder and configuration.
#[derive(Clone)]
pub struct SearchUseCase {
    store: Arc<Store>,
    embedder: Arc<dyn EmbeddingProvider>,
    config: Arc<Config>,
}

/// Optional retrieval trace for `pack_context` (same options on both edges).
#[derive(Debug, Clone, Default)]
pub struct TraceOptions {
    /// Return a local retrieval trace with spans and source versions.
    pub include: bool,
    /// Also persist the trace into `eval_traces` (implies nothing without a trace).
    pub persist: bool,
    /// A search trace to extend (search → pack path); implies `include`.
    pub prior: Option<RetrievalTrace>,
}

/// Typed `pack_context` command.
#[derive(Debug, Clone, Default)]
pub struct PackContextRequest {
    pub hits: Vec<SearchHit>,
    pub max_tokens: Option<usize>,
    pub context_expansion: Option<String>,
    pub neighbor_chunks: Option<usize>,
    pub trace: TraceOptions,
}

/// `pack_context` result; `trace` only when requested.
#[derive(Debug, Serialize)]
pub struct PackContextResponse {
    pub hits: Vec<SearchHit>,
    pub total_tokens: usize,
    pub max_tokens: usize,
    pub omitted_count: usize,
    pub context_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<RetrievalTrace>,
}

/// Typed `multi_get` command.
#[derive(Debug, Clone, Default)]
pub struct MultiGetCommand {
    pub document_ids: Vec<String>,
    pub include_chunks: bool,
    pub chunk_limit: Option<usize>,
}

/// One `multi_get` document, the same on both transports.
///
/// `document` is the full stored row (layer, kind, status, revision, …). The
/// flat fields beside it are the earlier MCP shape (`id`, `uri`, `title`,
/// `content`, `metadata_json`, `created_at`, `updated_at`, and the optional
/// `source_file` / `source_download_path`), kept so existing MCP clients that
/// read them keep working.
///
/// The body is serialised once, as the flat `content`: `document` carries
/// every other column of the row but omits `content`.
#[derive(Debug, Serialize)]
pub struct MultiGetItem {
    #[serde(serialize_with = "serialize_document_without_content")]
    pub document: Document,
    pub id: String,
    pub uri: String,
    pub title: String,
    pub content: String,
    pub metadata_json: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_download_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Vec<ChunkText>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks_truncated: Option<bool>,
}

fn serialize_document_without_content<S>(
    document: &Document,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let mut row = serde_json::to_value(document).map_err(serde::ser::Error::custom)?;
    if let Some(object) = row.as_object_mut() {
        object.remove("content");
    }
    row.serialize(serializer)
}

impl From<DocumentWithChunks> for MultiGetItem {
    fn from(value: DocumentWithChunks) -> Self {
        let document = value.document;
        let source_download_path = document
            .source_file
            .as_ref()
            .map(|_| format!("/v1/source-file?document_id={}", document.id));
        Self {
            id: document.id.clone(),
            uri: document.uri.clone(),
            title: document.title.clone(),
            content: document.content.clone(),
            metadata_json: document.metadata_json.clone(),
            created_at: document.created_at.to_rfc3339(),
            updated_at: document.updated_at.to_rfc3339(),
            source_file: document.source_file.clone(),
            source_download_path,
            document,
            chunks: value.chunks,
            chunks_total: value.chunks_total,
            chunks_truncated: value.chunks_truncated,
        }
    }
}

/// `multi_get` result: found documents in request order plus missing ids.
#[derive(Debug, Serialize)]
pub struct MultiGetResponse {
    pub items: Vec<MultiGetItem>,
    pub missing: Vec<String>,
}

impl SearchUseCase {
    pub fn new(
        store: Arc<Store>,
        embedder: Arc<dyn EmbeddingProvider>,
        config: Arc<Config>,
    ) -> Self {
        Self {
            store,
            embedder,
            config,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Ranked hits for one request; defaults come from the configuration.
    pub async fn search(&self, request: SearchRequest) -> Result<Vec<SearchHit>, AppError> {
        self.search_command(request.into_command(&self.config))
            .await
    }

    /// Ranked hits for an already-normalized command (HTTP resolves the mode
    /// first to report it).
    pub async fn search_command(
        &self,
        command: retrieval::SearchCommand,
    ) -> Result<Vec<SearchHit>, AppError> {
        retrieval::execute_search(&self.store, self.embedder.as_ref(), &self.config, command).await
    }

    /// Source versions for a trace span (one hash lookup for all hits).
    pub fn source_versions(&self, hits: &[SearchHit]) -> Vec<SourceVersion> {
        source_versions_for_hits(&self.store, hits)
    }

    /// Persist a retrieval trace into `eval_traces`; failures are logged.
    pub fn persist_trace(&self, trace: &RetrievalTrace) {
        if let Err(error) = crate::db::payload_kv::upsert_payload(
            &self.store,
            crate::db::payload_kv::PayloadTable::EvalTraces,
            &trace.id,
            trace,
        ) {
            tracing::warn!(%error, "failed to persist retrieval trace");
        }
    }

    /// Expand and pack hits under the citation-block budget.
    pub async fn pack_context(
        &self,
        request: PackContextRequest,
    ) -> Result<PackContextResponse, AppError> {
        let trace_options = request.trace;
        let include_trace = trace_options.include || trace_options.prior.is_some();
        let command = PackContextCommand {
            hits: request.hits,
            max_tokens: request.max_tokens,
            context_expansion: request.context_expansion,
            neighbor_chunks: request.neighbor_chunks,
        };
        let store = self.store.clone();
        let config = self.config.clone();
        let span = include_trace.then(|| SpanBuilder::start("pack_context"));
        let outcome = run_blocking("pack_context", move || {
            retrieval::pack_context(&store, &config, command)
        })
        .await?;
        let packed = outcome.packed;
        let trace = match span {
            Some(builder) => {
                let store = self.store.clone();
                let hits = outcome.expanded_hits;
                let versions = run_blocking("pack_context trace", move || {
                    Ok(source_versions_for_hits(&store, &hits))
                })
                .await?;
                let span = builder
                    .attr("total_tokens", packed.total_tokens as u64)
                    .attr("omitted_count", packed.omitted_count as u64)
                    .attr("source_versions", source_versions_json(&versions))
                    .finish();
                let mut trace = trace_options
                    .prior
                    .unwrap_or_else(|| RetrievalTrace::new("search_pack"));
                if trace.path.as_deref() == Some("search") {
                    trace.path = Some("search_pack".into());
                }
                trace.push(span);
                if trace_options.persist {
                    self.persist_trace(&trace);
                }
                Some(trace)
            }
            None => None,
        };
        Ok(PackContextResponse {
            hits: packed.hits,
            total_tokens: packed.total_tokens,
            max_tokens: packed.max_tokens,
            omitted_count: packed.omitted_count,
            context_text: packed.context_text,
            trace,
        })
    }

    /// Up to 100 documents by id, in request order.
    pub async fn multi_get(&self, command: MultiGetCommand) -> Result<MultiGetResponse, AppError> {
        let store = self.store.clone();
        let result = run_blocking("multi_get", move || {
            retrieval::multi_get(
                &store,
                command.document_ids,
                command.include_chunks,
                command.chunk_limit,
            )
        })
        .await?;
        Ok(MultiGetResponse {
            items: result
                .documents
                .into_iter()
                .map(MultiGetItem::from)
                .collect(),
            missing: result.missing,
        })
    }
}

/// One `SourceVersion` per distinct hit document (blocking: one DB query).
pub fn source_versions_for_hits(store: &Store, hits: &[SearchHit]) -> Vec<SourceVersion> {
    let ids: Vec<&str> = hits.iter().map(|hit| hit.document_id.as_str()).collect();
    let hashes = store.documents().content_hashes(&ids).unwrap_or_default();
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for hit in hits {
        if !seen.insert(hit.document_id.clone()) {
            continue;
        }
        let content_hash = hashes.get(&hit.document_id).cloned().flatten();
        out.push(SourceVersion {
            document_id: hit.document_id.clone(),
            uri: Some(hit.document_uri.clone()),
            content_hash,
        });
    }
    out
}
