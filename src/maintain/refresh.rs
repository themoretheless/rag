//! Actualize / refresh: FTS reindex, graph rebuild, wiki catalog, optional reembed.
//!
//! Whitelist-only actions (no SQL/shell invention). Logs every apply to `ops_log`.
//! See `docs/LOCAL_LLM_MAINTENANCE.md` §3.5.

use crate::db::store::embedding_manifest_from_config;
use crate::models::OpsLogEntry;
use chrono::Utc;
use std::sync::Arc;

use serde::Serialize;

use crate::config::Config;
use crate::db::store::{embedding_manifest_matches_config, embedding_migration_manifest};
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::error::{AppError, Result};
use crate::graph::rebuild_document_graph;
use crate::models::EmbeddingManifest;

/// Allowed `maintain_refresh` action names (safety whitelist).
pub const REFRESH_ACTION_WHITELIST: &[&str] = &[
    "reindex_fts",
    "rebuild_graph",
    "rebuild_wiki_index",
    "reembed_all",
];

pub(crate) const REEMBED_ALL_INCOMPLETE_CODE: &str = "REEMBED_ALL_INCOMPLETE";
const MAINTENANCE_PAGE_SIZE: usize = 64;
const REPORT_DOCUMENT_CAP: usize = 64;

/// Flags controlling which refresh steps run.
///
/// Defaults (when built via [`MaintainRefreshFlags::from_options`]):
/// - `reindex_fts`, `rebuild_graph`, `rebuild_wiki_index` = **true**
/// - `graph_dirty_only` = **true** (missing document nodes only)
/// - `reembed_all` = **false** (opt-in; expensive)
/// - `dry_run` = **false**
#[derive(Debug, Clone)]
pub struct MaintainRefreshFlags {
    pub reindex_fts: bool,
    pub rebuild_graph: bool,
    /// When true with `rebuild_graph`, only docs without a document graph node.
    /// When false, rebuild graph for every document (capped by `max_docs`).
    pub graph_dirty_only: bool,
    pub rebuild_wiki_index: bool,
    /// Opt-in: re-embed all document chunks with the live embedder.
    pub reembed_all: bool,
    /// When true, report planned work without mutating store.
    pub dry_run: bool,
    /// Cap documents touched by graph rebuild / reembed (`RAG_MAINT_MAX_DOCS`).
    pub max_docs: usize,
}

impl Default for MaintainRefreshFlags {
    fn default() -> Self {
        Self {
            reindex_fts: true,
            rebuild_graph: true,
            graph_dirty_only: true,
            rebuild_wiki_index: true,
            reembed_all: false,
            dry_run: false,
            max_docs: super::DEFAULT_MAX_DOCS,
        }
    }
}

impl MaintainRefreshFlags {
    /// Build flags from optional MCP/tool parameters.
    ///
    /// Any explicitly provided flag overrides the default. When **no** action
    /// flags are provided at all (`None` for all four), safe defaults apply
    /// (fts + dirty graph + wiki index; no reembed). When at least one action
    /// is explicitly set, unspecified actions default to **false** so callers
    /// can run a single step.
    pub fn from_options(
        reindex_fts: Option<bool>,
        rebuild_graph: Option<bool>,
        graph_dirty_only: Option<bool>,
        rebuild_wiki_index: Option<bool>,
        reembed_all: Option<bool>,
        dry_run: Option<bool>,
        max_docs: usize,
    ) -> Self {
        let any_action_set = reindex_fts.is_some()
            || rebuild_graph.is_some()
            || rebuild_wiki_index.is_some()
            || reembed_all.is_some();

        let max_docs = max_docs.max(1);

        if any_action_set {
            Self {
                reindex_fts: reindex_fts.unwrap_or(false),
                rebuild_graph: rebuild_graph.unwrap_or(false),
                graph_dirty_only: graph_dirty_only.unwrap_or(true),
                rebuild_wiki_index: rebuild_wiki_index.unwrap_or(false),
                reembed_all: reembed_all.unwrap_or(false),
                dry_run: dry_run.unwrap_or(false),
                max_docs,
            }
        } else {
            Self {
                reindex_fts: true,
                rebuild_graph: true,
                graph_dirty_only: graph_dirty_only.unwrap_or(true),
                rebuild_wiki_index: true,
                reembed_all: false,
                dry_run: dry_run.unwrap_or(false),
                max_docs,
            }
        }
    }

    /// True when at least one whitelist action is requested.
    pub fn has_work(&self) -> bool {
        self.reindex_fts || self.rebuild_graph || self.rebuild_wiki_index || self.reembed_all
    }

    /// Ordered whitelist action names that will run.
    pub fn selected_actions(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.reindex_fts {
            out.push("reindex_fts");
        }
        if self.rebuild_graph {
            out.push("rebuild_graph");
        }
        if self.rebuild_wiki_index {
            out.push("rebuild_wiki_index");
        }
        if self.reembed_all {
            out.push("reembed_all");
        }
        out
    }
}

/// FTS reindex step result.
#[derive(Debug, Clone, Serialize)]
pub struct ReindexFtsReport {
    pub backend: String,
    pub stemmer: String,
    pub dry_run: bool,
}

/// Per-document graph rebuild line (best effort).
#[derive(Debug, Clone, Serialize)]
pub struct GraphDocResult {
    pub document_id: String,
    pub node_id: Option<String>,
    pub edge_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Graph rebuild step result.
#[derive(Debug, Clone, Serialize)]
pub struct GraphRebuildReport {
    pub dirty_only: bool,
    pub candidate_count: usize,
    pub processed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped_cap: usize,
    pub dry_run: bool,
    /// Derived unresolved placeholders removed after a complete successful pass.
    pub pruned_orphan_stubs: usize,
    /// Derived tag nodes removed after a complete successful pass.
    pub pruned_orphan_tags: usize,
    /// Per-document result rows omitted from this bounded report payload.
    pub document_results_truncated: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documents: Vec<GraphDocResult>,
}

/// Wiki index rebuild step result.
#[derive(Debug, Clone, Serialize)]
pub struct WikiIndexRebuildReport {
    pub entry_count: usize,
    pub dry_run: bool,
}

/// One document reembed line.
#[derive(Debug, Clone, Serialize)]
pub struct ReembedDocResult {
    pub document_id: String,
    pub chunk_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Optional full-corpus reembed step result.
#[derive(Debug, Clone, Serialize)]
pub struct ReembedAllReport {
    pub documents_considered: usize,
    pub documents_processed: usize,
    pub documents_succeeded: usize,
    pub documents_failed: usize,
    pub skipped_cap: usize,
    /// All chunk rows observed by the preflight inventory.
    pub chunks_considered: usize,
    /// Preflight chunk rows attached to an existing document and therefore
    /// eligible for this document-driven migration.
    pub attached_chunks_considered: usize,
    /// Chunk rows not owned by any document. These are never deleted here.
    pub orphan_chunks: usize,
    pub chunks_reembedded: usize,
    pub dry_run: bool,
    /// True when this run was an identity migration tracked in
    /// `reembed_progress` (resumable across processes and batches).
    pub resumable: bool,
    /// Documents still carrying the old embedding identity after this run
    /// (migration mode only; 0 in plain refresh mode).
    pub pending_documents: usize,
    /// Per-document result rows omitted from this bounded report payload.
    pub document_results_truncated: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<EmbeddingManifest>,
    /// Corpus-level integrity/coverage failures that prevent manifest publish.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documents: Vec<ReembedDocResult>,
}

pub(crate) fn reembed_all_incomplete(report: &ReembedAllReport) -> bool {
    !report.dry_run
        && (report.documents_failed > 0
            || report.skipped_cap > 0
            || report.orphan_chunks > 0
            || report.pending_documents > 0
            || !report.errors.is_empty())
}

pub(crate) fn reembed_all_incomplete_message(report: &ReembedAllReport) -> String {
    format!(
        "{REEMBED_ALL_INCOMPLETE_CODE}: processed={} succeeded={} failed={} skipped={}; \
        chunks_considered={} attached_chunks={} orphan_chunks={} chunks_reembedded={} errors={}; \
        full-corpus completion was not reached",
        report.documents_processed,
        report.documents_succeeded,
        report.documents_failed,
        report.skipped_cap,
        report.chunks_considered,
        report.attached_chunks_considered,
        report.orphan_chunks,
        report.chunks_reembedded,
        report.errors.len(),
    )
}

/// Aggregate report for `maintain_refresh`.
#[derive(Debug, Clone, Serialize)]
pub struct MaintainRefreshReport {
    pub dry_run: bool,
    pub actions: Vec<String>,
    /// Terminal derived-index failures after already-durable refresh work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reindex_fts: Option<ReindexFtsReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebuild_graph: Option<GraphRebuildReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebuild_wiki_index: Option<WikiIndexRebuildReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reembed_all: Option<ReembedAllReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ops_log_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ops_log_seq: Option<i64>,
}

/// Rebuild the FTS / BM25 index on `chunks` (read-your-writes).
///
/// Uses `stemmer` when provided; otherwise the last meta stemmer / `"porter"`.
pub fn reindex_fts(store: &Store, stemmer: Option<&str>) -> Result<ReindexFtsReport> {
    let state = store.reindex_fts(stemmer)?;
    Ok(ReindexFtsReport {
        backend: state.backend.as_str().to_string(),
        stemmer: state.stemmer,
        dry_run: false,
    })
}

/// Run an explicit FTS rebuild on Tokio's blocking pool.
///
/// The caller remains responsible for holding the corpus-mutation lane across
/// this await when the rebuild is part of a larger write workflow.
pub(crate) async fn reindex_fts_nonblocking(
    store: &Store,
    stemmer: &str,
    operation: &'static str,
) -> Result<ReindexFtsReport> {
    #[cfg(test)]
    crate::db::fts::test_fault::check("refresh", stemmer)?;

    let store = store.clone();
    let stemmer = stemmer.to_string();
    tokio::task::spawn_blocking(move || reindex_fts(&store, Some(&stemmer)))
        .await
        .map_err(|error| AppError::fts(format!("{operation} FTS task failed: {error}")))?
}

/// Bring a stale FTS generation current on Tokio's blocking pool.
///
/// Unlike [`reindex_fts_nonblocking`], a generation-clean index stays on the
/// cheap no-op path.
pub(crate) async fn ensure_fts_nonblocking(
    store: &Store,
    stemmer: &str,
    operation: &'static str,
) -> Result<()> {
    #[cfg(test)]
    crate::db::fts::test_fault::check("refresh", stemmer)?;

    let store = store.clone();
    let stemmer = stemmer.to_string();
    tokio::task::spawn_blocking(move || store.ensure_fts(&stemmer))
        .await
        .map_err(|error| AppError::fts(format!("{operation} FTS task failed: {error}")))??;
    Ok(())
}

/// Rebuild object-graph slices for all documents or only dirty ones (best effort).
///
/// **Dirty** = no graph node with `document_id` matching the document.
/// Individual document failures are recorded and do not abort the batch.
/// Processing is capped at `max_docs` (must be ≥ 1).
pub fn rebuild_graph_for_all_or_dirty(
    store: &Store,
    dirty_only: bool,
    max_docs: usize,
) -> Result<GraphRebuildReport> {
    let max_docs = max_docs.max(1);
    let candidate_count = store.count_graph_rebuild_candidates(dirty_only)?;
    let skipped_cap = candidate_count.saturating_sub(max_docs);
    let target_count = candidate_count.min(max_docs);

    let mut documents = Vec::with_capacity(target_count.min(REPORT_DOCUMENT_CAP));
    let mut document_results_truncated = 0usize;
    let mut processed = 0usize;
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut after_id: Option<String> = None;

    while processed < target_count {
        let page_limit = (target_count - processed).min(MAINTENANCE_PAGE_SIZE);
        let page = store.list_graph_rebuild_candidates_after(
            dirty_only,
            after_id.as_deref(),
            page_limit,
        )?;
        if page.is_empty() {
            break;
        }
        for doc in page {
            after_id = Some(doc.id.clone());
            processed += 1;
            let result = match rebuild_document_graph(store, &doc) {
                Ok((node_id, edge_count)) => {
                    succeeded += 1;
                    GraphDocResult {
                        document_id: doc.id,
                        node_id: Some(node_id),
                        edge_count,
                        error: None,
                    }
                }
                Err(e) => {
                    failed += 1;
                    tracing::warn!(
                        document_id = %doc.id,
                        error = %e,
                        "rebuild_document_graph failed (best effort continue)"
                    );
                    GraphDocResult {
                        document_id: doc.id,
                        node_id: None,
                        edge_count: 0,
                        error: Some(e.to_string()),
                    }
                }
            };
            if documents.len() < REPORT_DOCUMENT_CAP {
                documents.push(result);
            } else {
                document_results_truncated += 1;
            }
        }
    }

    let complete_success =
        !dirty_only && failed == 0 && skipped_cap == 0 && processed == candidate_count;
    let pruned_orphan_stubs = if complete_success {
        store.prune_orphan_stubs()?
    } else {
        0
    };
    let pruned_orphan_tags = if complete_success {
        store.prune_orphan_tags()?
    } else {
        0
    };

    Ok(GraphRebuildReport {
        dirty_only,
        candidate_count,
        processed,
        succeeded,
        failed,
        skipped_cap,
        dry_run: false,
        pruned_orphan_stubs,
        pruned_orphan_tags,
        document_results_truncated,
        documents,
    })
}

/// Clear and rebuild `wiki_index` from all `layer=wiki` documents.
pub fn rebuild_wiki_index(store: &Store) -> Result<WikiIndexRebuildReport> {
    let entry_count = store.rebuild_wiki_index_from_docs()?;
    Ok(WikiIndexRebuildReport {
        entry_count,
        dry_run: false,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReembedChunkInventory {
    total: usize,
    attached: usize,
    orphan: usize,
}

/// Count the vector-bearing rows that a document-driven re-embed can cover.
///
/// Both counts come from one connection snapshot. Orphans are reported rather
/// than pruned: maintenance repair owns deletion policy, while re-embedding
/// must fail closed instead of silently certifying vectors it never replaced.
fn reembed_chunk_inventory(store: &Store) -> Result<ReembedChunkInventory> {
    let (total, attached) = crate::db::analytics::chunk_attachment_counts(store)?;
    let total = usize::try_from(total).map_err(|_| {
        AppError::db(format!(
            "invalid chunk count during reembed preflight: {total}"
        ))
    })?;
    let attached = usize::try_from(attached).map_err(|_| {
        AppError::db(format!(
            "invalid attached chunk count during reembed preflight: {attached}"
        ))
    })?;
    if attached > total {
        return Err(AppError::db(format!(
            "attached chunk count {attached} exceeds total chunk count {total}"
        )));
    }
    Ok(ReembedChunkInventory {
        total,
        attached,
        orphan: total - attached,
    })
}

/// Re-embed chunks for every document (capped by `max_docs`) with the live embedder.
///
/// A different embedding identity may be migrated in resumable batches. Vector
/// reads remain closed between batches. The target manifest is published only after an
/// uncapped, zero-failure pass (or for an empty corpus), so a partial migration
/// remains visibly incompatible with vector search.
pub async fn reembed_all(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    max_docs: usize,
) -> Result<ReembedAllReport> {
    let scoped_store = store.try_corpus_mutation_scope("reembed_all")?;
    let store = &scoped_store;
    let max_docs = max_docs.max(1);
    let documents_considered = store.count_documents()?;
    let previous_manifest = store.get_embedding_manifest()?;
    let identity_matches = store.embedding_manifest_matches_config(config)?;
    let target_fingerprint = (!identity_matches).then(|| {
        embedding_manifest_from_config(config)
            .content_fingerprint
            .unwrap_or_else(|| "unknown".into())
    });
    let pending_before = match target_fingerprint.as_ref() {
        Some(fingerprint) => store.count_reembed_pending(fingerprint)?,
        None => documents_considered,
    };
    let skipped_cap = pending_before.saturating_sub(max_docs);
    let inventory_before = reembed_chunk_inventory(store)?;
    if inventory_before.orphan > 0 {
        let error = format!(
            "{REEMBED_ALL_INCOMPLETE_CODE}: refuse reembed_all because {} orphan chunk(s) are \
            not attached to any document; no vectors were changed and no target embedding \
            manifest was published. Run doctor and repair corpus integrity explicitly before \
            retrying",
            inventory_before.orphan
        );
        return Ok(ReembedAllReport {
            documents_considered,
            documents_processed: 0,
            documents_succeeded: 0,
            documents_failed: 0,
            skipped_cap,
            chunks_considered: inventory_before.total,
            attached_chunks_considered: inventory_before.attached,
            orphan_chunks: inventory_before.orphan,
            chunks_reembedded: 0,
            dry_run: false,
            resumable: target_fingerprint.is_some(),
            pending_documents: pending_before,
            document_results_truncated: 0,
            manifest: previous_manifest,
            errors: vec![error],
            documents: Vec::new(),
        });
    }
    // Fail-closed marker published with the first vector mutation; its value is
    // deterministic, so re-publishing it on later batches is idempotent.
    let migration_marker = target_fingerprint
        .as_ref()
        .map(|_| embedding_migration_manifest(config));
    let mut migration_started = false;
    let target_count = pending_before.min(max_docs);

    let mut documents = Vec::with_capacity(target_count.min(REPORT_DOCUMENT_CAP));
    let mut document_results_truncated = 0usize;
    let mut documents_processed = 0usize;
    let mut documents_succeeded = 0usize;
    let mut documents_failed = 0usize;
    let mut chunks_reembedded = 0usize;
    let mut errors = Vec::new();
    let mut after_id: Option<String> = None;

    while documents_processed < target_count {
        let page_limit = (target_count - documents_processed).min(MAINTENANCE_PAGE_SIZE);
        let page = match target_fingerprint.as_ref() {
            Some(fingerprint) => store.list_reembed_pending_document_ids(
                fingerprint,
                after_id.as_deref(),
                page_limit,
            )?,
            None => store.list_document_ids_after(after_id.as_deref(), page_limit)?,
        };
        if page.is_empty() {
            errors.push(format!(
                "{REEMBED_ALL_INCOMPLETE_CODE}: document inventory changed during reembed_all \
                (expected to process {target_count}, reached {documents_processed})"
            ));
            break;
        }
        after_id = page.last().cloned();
        // Load the page, then embed its chunks in cross-document groups: a
        // corpus of small documents costs ~chunks/REEMBED_GROUP_CHUNKS
        // provider calls instead of one call per document.
        let loaded = crate::util::blocking_section(|| {
            page.into_iter()
                .map(|document_id| {
                    let pending = load_reembed_document(store, &document_id);
                    (document_id, pending)
                })
                .collect::<Vec<_>>()
        });
        let mut embedded = embed_reembed_groups(embedder, &loaded).await.into_iter();
        crate::util::blocking_section(|| {
            for (document_id, pending) in loaded {
                documents_processed += 1;
                let manifest = migration_marker.as_ref().filter(|_| !migration_started);
                let outcome = match pending {
                    Err(e) => Err(e),
                    Ok(pending) => {
                        let vectors = embedded.next().unwrap_or_else(|| {
                            Err(AppError::embeddings("missing embeddings for document"))
                        });
                        vectors.and_then(|vectors| {
                            apply_reembedding(
                                store,
                                &pending,
                                &vectors,
                                config.embedding_dims,
                                manifest,
                            )
                        })
                    }
                };
                let result = match outcome {
                    Ok(chunk_count) => {
                        if let Some(fingerprint) = target_fingerprint.as_ref() {
                            store.mark_reembed_done(fingerprint, &document_id, chunk_count)?;
                        }
                        if chunk_count > 0 && manifest.is_some() {
                            migration_started = true;
                        }
                        documents_succeeded += 1;
                        chunks_reembedded += chunk_count;
                        ReembedDocResult {
                            document_id,
                            chunk_count,
                            error: None,
                        }
                    }
                    Err(e) => {
                        documents_failed += 1;
                        tracing::warn!(
                            document_id = %document_id,
                            error = %e,
                            "reembed document failed (best effort continue)"
                        );
                        ReembedDocResult {
                            document_id,
                            chunk_count: 0,
                            error: Some(e.to_string()),
                        }
                    }
                };
                if documents.len() < REPORT_DOCUMENT_CAP {
                    documents.push(result);
                } else {
                    document_results_truncated += 1;
                }
            }
            Ok::<_, AppError>(())
        })?;
    }

    let inventory_after = crate::util::blocking_section(|| reembed_chunk_inventory(store))?;
    if inventory_after.orphan > 0 {
        errors.push(format!(
            "{REEMBED_ALL_INCOMPLETE_CODE}: {} orphan chunk(s) were observed after document re-embedding",
            inventory_after.orphan
        ));
    }
    if inventory_after.total != inventory_before.total
        || inventory_after.attached != inventory_before.attached
    {
        errors.push(format!(
            "{REEMBED_ALL_INCOMPLETE_CODE}: chunk inventory changed during reembed_all (before total={} attached={}; after total={} attached={})",
            inventory_before.total,
            inventory_before.attached,
            inventory_after.total,
            inventory_after.attached,
        ));
    }
    let pending_after = match target_fingerprint.as_ref() {
        Some(fingerprint) => store.count_reembed_pending(fingerprint)?,
        None => 0,
    };
    if target_fingerprint.is_none()
        && skipped_cap == 0
        && chunks_reembedded != inventory_before.attached
    {
        // Refresh mode processes every document in one pass; coverage must be exact.
        errors.push(format!(
            "{REEMBED_ALL_INCOMPLETE_CODE}: attached chunk coverage mismatch (expected={}, reembedded={})",
            inventory_before.attached, chunks_reembedded
        ));
    }

    let complete_success =
        skipped_cap == 0 && documents_failed == 0 && pending_after == 0 && errors.is_empty();
    let manifest = if complete_success {
        let written = store.write_embedding_manifest_from_config(config)?;
        if let Some(fingerprint) = target_fingerprint.as_ref() {
            // Migration is complete; drop the resume state for this target.
            let _ = store.clear_reembed_progress(fingerprint);
        }
        Some(written)
    } else {
        // The first successful vector mutation publishes an incompatible
        // marker atomically with those vectors. A partial migration therefore
        // remains fail-closed for both the old and target runtime configs.
        store.get_embedding_manifest()?
    };

    if let Some(fingerprint) = target_fingerprint.as_ref() {
        let documents_done = store.count_reembed_done(fingerprint)?;
        let _ = store.append_ops_log(&OpsLogEntry {
            id: String::new(),
            seq: 0,
            ts: Utc::now(),
            op: "reembed_progress".into(),
            prefix: Some("REEMBED".into()),
            message: format!(
                "reembed migration batch: {} done / {} pending (target complete: {complete_success})",
                documents_done, pending_after
            ),
            entity_id: None,
            entity_kind: Some("corpus".into()),
            payload_json: serde_json::json!({
                "target_fingerprint": fingerprint,
                "batch_processed": documents_processed,
                "batch_succeeded": documents_succeeded,
                "batch_failed": documents_failed,
                "batch_chunks_reembedded": chunks_reembedded,
                "documents_done": documents_done,
                "documents_pending": pending_after,
                "documents_total": documents_considered,
                "complete": complete_success,
            })
            .to_string(),
            agent_name: None,
        });
    }

    Ok(ReembedAllReport {
        documents_considered,
        documents_processed,
        documents_succeeded,
        documents_failed,
        skipped_cap,
        chunks_considered: inventory_before.total,
        attached_chunks_considered: inventory_before.attached,
        orphan_chunks: inventory_after.orphan,
        chunks_reembedded,
        dry_run: false,
        resumable: target_fingerprint.is_some(),
        pending_documents: pending_after,
        document_results_truncated,
        manifest,
        errors,
        documents,
    })
}

/// Chunks per provider call when re-embedding across documents.
const REEMBED_GROUP_CHUNKS: usize = 128;

/// A document's revision and current chunks, read before re-embedding.
struct PendingReembed {
    document_id: String,
    revision: i64,
    chunks: Vec<crate::models::Chunk>,
}

fn load_reembed_document(store: &Store, document_id: &str) -> Result<PendingReembed> {
    // The revision is the CAS token; the document body is never needed.
    let revision = store
        .document_revision(document_id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;
    let chunks = store.list_chunks_for_document(document_id)?;
    Ok(PendingReembed {
        document_id: document_id.to_string(),
        revision,
        chunks,
    })
}

/// Embeddings for every successfully loaded document, in order.
///
/// Documents are grouped up to [`REEMBED_GROUP_CHUNKS`] chunks per call. When a
/// group call fails, its documents are retried one by one so a single bad
/// document still fails alone, as before batching.
async fn embed_reembed_groups(
    embedder: &Arc<dyn EmbeddingProvider>,
    loaded: &[(String, Result<PendingReembed>)],
) -> Vec<Result<Vec<Vec<f32>>>> {
    let pending: Vec<&PendingReembed> = loaded
        .iter()
        .filter_map(|(_, pending)| pending.as_ref().ok())
        .collect();
    let mut out = Vec::with_capacity(pending.len());
    let mut start = 0;
    while start < pending.len() {
        let mut end = start;
        let mut chunk_total = 0;
        while end < pending.len()
            && (end == start || chunk_total + pending[end].chunks.len() <= REEMBED_GROUP_CHUNKS)
        {
            chunk_total += pending[end].chunks.len();
            end += 1;
        }
        let group = &pending[start..end];
        let texts: Vec<String> = group
            .iter()
            .flat_map(|doc| doc.chunks.iter().map(|c| c.content.clone()))
            .collect();
        let grouped = if texts.is_empty() {
            Ok(Vec::new())
        } else {
            crate::embeddings::embed_exact(embedder.as_ref(), &texts).await
        };
        match grouped {
            Ok(vectors) => {
                let mut vectors = vectors.into_iter();
                for doc in group {
                    out.push(Ok(vectors.by_ref().take(doc.chunks.len()).collect()));
                }
            }
            Err(_) if group.len() > 1 => {
                for doc in group {
                    let texts: Vec<String> = doc.chunks.iter().map(|c| c.content.clone()).collect();
                    out.push(if texts.is_empty() {
                        Ok(Vec::new())
                    } else {
                        crate::embeddings::embed_exact(embedder.as_ref(), &texts).await
                    });
                }
            }
            Err(e) => out.push(Err(e)),
        }
        start = end;
    }
    out
}

fn apply_reembedding(
    store: &Store,
    pending: &PendingReembed,
    embeddings: &[Vec<f32>],
    expected_dims: usize,
    manifest: Option<&EmbeddingManifest>,
) -> Result<usize> {
    if pending.chunks.is_empty() {
        return Ok(0);
    }
    if let Some((index, embedding)) = embeddings
        .iter()
        .enumerate()
        .find(|(_, embedding)| embedding.len() != expected_dims)
    {
        return Err(AppError::embeddings(format!(
            "embedding {index} has dims={}, expected {expected_dims} from config",
            embedding.len()
        )));
    }
    store.update_chunk_embeddings_atomic(
        &pending.document_id,
        pending.revision,
        &pending.chunks,
        embeddings,
        manifest,
    )?;
    Ok(pending.chunks.len())
}

/// Run whitelist refresh actions according to `flags`.
///
/// Always appends an `ops_log` row on apply (not on pure dry_run of zero work).
/// Dry-run still logs with `op=maintain_refresh_dry_run` so the timeline shows intent.
pub async fn maintain_refresh(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    mut flags: MaintainRefreshFlags,
) -> Result<MaintainRefreshReport> {
    if !flags.has_work() {
        return Err(AppError::config(
            "maintain_refresh: no actions selected; set reindex_fts, rebuild_graph, \
             rebuild_wiki_index, and/or reembed_all",
        ));
    }
    flags.max_docs = flags.max_docs.max(1).min(config.maint_max_docs.max(1));

    // Migration mode is resumable: a capped run is one batch, not a refusal.
    // `RAG_REEMBED_BATCH` bounds each reembed pass independently of the
    // graph-rebuild budget that shares `flags.max_docs`.
    let reembed_max = flags.max_docs.min(config.reembed_batch.max(1));

    // Safety: only whitelist action names appear in the report.
    let actions: Vec<String> = flags
        .selected_actions()
        .into_iter()
        .filter(|a| REFRESH_ACTION_WHITELIST.contains(a))
        .map(|s| s.to_string())
        .collect();

    let dry_run = flags.dry_run;
    let mut report = MaintainRefreshReport {
        dry_run,
        actions: actions.clone(),
        errors: Vec::new(),
        reindex_fts: None,
        rebuild_graph: None,
        rebuild_wiki_index: None,
        reembed_all: None,
        ops_log_id: None,
        ops_log_seq: None,
    };

    if flags.reindex_fts {
        if dry_run {
            let status = crate::util::blocking_section(|| store.fts_status())?;
            report.reindex_fts = Some(ReindexFtsReport {
                backend: status
                    .as_ref()
                    .map(|s| s.backend.as_str().to_string())
                    .unwrap_or_else(|| "unknown".into()),
                stemmer: status
                    .map(|s| s.stemmer)
                    .unwrap_or_else(|| config.fts_stemmer.clone()),
                dry_run: true,
            });
        } else if !flags.reembed_all {
            // When re-embedding is selected, defer the explicit rebuild until
            // after chunk replacement so the refresh performs it only once.
            report.reindex_fts = Some(
                reindex_fts_nonblocking(store, &config.fts_stemmer, "maintain_refresh reindex")
                    .await?,
            );
        }
    }

    if flags.rebuild_graph {
        if dry_run {
            let candidate_count = crate::util::blocking_section(|| {
                store.count_graph_rebuild_candidates(flags.graph_dirty_only)
            })?;
            let processed = candidate_count.min(flags.max_docs);
            report.rebuild_graph = Some(GraphRebuildReport {
                dirty_only: flags.graph_dirty_only,
                candidate_count,
                processed,
                succeeded: 0,
                failed: 0,
                skipped_cap: candidate_count.saturating_sub(flags.max_docs),
                dry_run: true,
                pruned_orphan_stubs: 0,
                pruned_orphan_tags: 0,
                document_results_truncated: 0,
                documents: Vec::new(),
            });
        } else {
            report.rebuild_graph = Some(crate::util::blocking_section(|| {
                rebuild_graph_for_all_or_dirty(store, flags.graph_dirty_only, flags.max_docs)
            })?);
        }
    }

    if flags.rebuild_wiki_index {
        if dry_run {
            let n = crate::util::blocking_section(|| {
                store.count_documents_by_layer(crate::models::Layer::Wiki.as_str())
            })?;
            report.rebuild_wiki_index = Some(WikiIndexRebuildReport {
                entry_count: n,
                dry_run: true,
            });
        } else {
            report.rebuild_wiki_index =
                Some(crate::util::blocking_section(|| rebuild_wiki_index(store))?);
        }
    }

    if flags.reembed_all {
        if dry_run {
            let n = store.count_documents()?;
            let chunk_est = store.count_chunks_for_document_limit(reembed_max)?;
            let inventory = reembed_chunk_inventory(store)?;
            let identity_matches = store.embedding_manifest_matches_config(config)?;
            let pending = if identity_matches {
                n
            } else {
                let fingerprint = embedding_manifest_from_config(config)
                    .content_fingerprint
                    .unwrap_or_else(|| "unknown".into());
                store.count_reembed_pending(&fingerprint)?
            };
            let errors = (inventory.orphan > 0)
                .then(|| {
                    format!(
                        "{REEMBED_ALL_INCOMPLETE_CODE}: {} orphan chunk(s) are outside \
                        document-driven re-embedding; apply would refuse without changing \
                        vectors",
                        inventory.orphan
                    )
                })
                .into_iter()
                .collect();
            report.reembed_all = Some(ReembedAllReport {
                documents_considered: n,
                documents_processed: pending.min(reembed_max),
                documents_succeeded: 0,
                documents_failed: 0,
                skipped_cap: pending.saturating_sub(reembed_max),
                chunks_considered: inventory.total,
                attached_chunks_considered: inventory.attached,
                orphan_chunks: inventory.orphan,
                chunks_reembedded: chunk_est,
                dry_run: true,
                resumable: !identity_matches,
                pending_documents: pending,
                document_results_truncated: 0,
                manifest: store.get_embedding_manifest()?,
                errors,
                documents: Vec::new(),
            });
        } else {
            report.reembed_all = Some(reembed_all(store, embedder, config, reembed_max).await?);
            if let Some(reembed) = report
                .reembed_all
                .as_ref()
                .filter(|reembed| reembed_all_incomplete(reembed))
            {
                report.errors.push(reembed_all_incomplete_message(reembed));
            }
            let chunks_reembedded = report
                .reembed_all
                .as_ref()
                .is_some_and(|reembed| reembed.chunks_reembedded > 0);
            if flags.reindex_fts {
                match reindex_fts_nonblocking(
                    store,
                    &config.fts_stemmer,
                    "maintain_refresh post-reembed reindex",
                )
                .await
                {
                    Ok(reindex) => report.reindex_fts = Some(reindex),
                    Err(error) => report.errors.push(fts_finalization_failure(
                        store,
                        "maintain_refresh post-reembed reindex",
                        &error,
                    )),
                }
            } else if chunks_reembedded {
                // Even when the caller selected only re-embedding, finish the
                // chunk replacement with matching generations so the next
                // lexical request remains a warm read.
                if let Err(error) = ensure_fts_nonblocking(
                    store,
                    &config.fts_stemmer,
                    "maintain_refresh post-reembed refresh",
                )
                .await
                {
                    report.errors.push(fts_finalization_failure(
                        store,
                        "maintain_refresh post-reembed refresh",
                        &error,
                    ));
                }
            }
        }
    }

    // ops_log every run (including dry_run) for auditability.
    let message = if dry_run {
        format!("dry_run maintain_refresh actions=[{}]", actions.join(","))
    } else {
        format!("maintain_refresh actions=[{}]", actions.join(","))
    };
    let written = super::audit::record_run(
        store,
        "maintain_refresh",
        dry_run,
        "corpus",
        message,
        None,
        &report,
    )?;
    report.ops_log_id = Some(written.id);
    report.ops_log_seq = Some(written.seq);

    Ok(report)
}

fn fts_finalization_failure(store: &Store, operation: &str, error: &AppError) -> String {
    crate::util::blocking_section(|| {
        store.record_fts_finalization_failure(operation, true, &error.to_string())
    })
    .message
}

#[derive(Debug, Clone, Serialize)]
pub struct ReembedMigrationStatus {
    pub target_fingerprint: String,
    pub documents_done: usize,
    pub documents_pending: usize,
    pub documents_total: usize,
    /// Batch bound applied per `reembed_all` run (`RAG_REEMBED_BATCH`).
    pub batch_size: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReembedStatus {
    /// True when the persisted manifest matches live config: vector search open.
    pub identity_match: bool,
    pub vector_search_open: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<EmbeddingManifest>,
    /// Identity implied by live config (what a completed migration publishes).
    pub configured: EmbeddingManifest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migration: Option<ReembedMigrationStatus>,
    pub hint: String,
}

pub fn reembed_status(store: &Store, config: &Config) -> Result<ReembedStatus> {
    let configured = embedding_manifest_from_config(config);
    let manifest = store.get_embedding_manifest()?;
    let identity_match = manifest
        .as_ref()
        .is_some_and(|m| embedding_manifest_matches_config(m, config));
    if identity_match {
        return Ok(ReembedStatus {
            identity_match: true,
            vector_search_open: true,
            manifest,
            configured,
            migration: None,
            hint: "embedding identity matches live config; vector search is open".into(),
        });
    }
    let documents_total = store.count_documents()?;
    let target_fingerprint = configured
        .content_fingerprint
        .clone()
        .unwrap_or_else(|| "unknown".into());
    let documents_done = store.count_reembed_done(&target_fingerprint)?;
    let documents_pending = store.count_reembed_pending(&target_fingerprint)?;
    let hint = if documents_pending == 0 {
        "no pending documents; run reembed_all once to publish the target manifest".to_string()
    } else {
        format!(
            "vector search is closed until migration completes; run reembed_all (maintain_refresh reembed_all=true) repeatedly — {documents_pending} document(s) pending, batches of {}",
            config.reembed_batch
        )
    };
    Ok(ReembedStatus {
        identity_match: false,
        vector_search_open: false,
        manifest,
        configured,
        migration: Some(ReembedMigrationStatus {
            target_fingerprint,
            documents_done,
            documents_pending,
            documents_total,
            batch_size: config.reembed_batch,
        }),
        hint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbedder;
    use crate::models::{Chunk, SearchMode};
    use crate::testkit::SeedDoc;
    use crate::wiki::write_wiki_page;
    use async_trait::async_trait;
    use chrono::Utc;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Seeded body the failing embedder refuses.
    const FAILING_TEXT: &str = "second body";

    struct FailAfterFirstEmbedder {
        calls: AtomicUsize,
        dims: usize,
    }

    #[async_trait]
    impl EmbeddingProvider for FailAfterFirstEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            let call = self.calls.fetch_add(1, Ordering::Relaxed);
            if call > 0 {
                return Err(AppError::embeddings("injected provider failure"));
            }
            Ok(texts.iter().map(|_| vec![0.25; self.dims]).collect())
        }

        fn dimensions(&self) -> usize {
            self.dims
        }
    }

    struct FailOnTextEmbedder {
        calls: AtomicUsize,
        dims: usize,
    }

    #[async_trait]
    impl EmbeddingProvider for FailOnTextEmbedder {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            // Re-embedding groups documents into one call; failing on one
            // document's text (not on the Nth call) keeps the failure per
            // document whatever the grouping.
            if texts.iter().any(|text| text.contains(FAILING_TEXT)) {
                return Err(AppError::embeddings("injected provider failure"));
            }
            Ok(texts.iter().map(|_| vec![0.25; self.dims]).collect())
        }

        fn dimensions(&self) -> usize {
            self.dims
        }
    }

    fn test_config(db_path: PathBuf, dims: usize) -> Config {
        Config {
            db_path,
            embedding_model: "mock".to_string(),
            embedding_dims: dims,
            llm_enabled: false,
            ..Config::for_tests()
        }
    }

    fn seed_doc(store: &Store, id: &str, title: &str, content: &str, layer: &str) {
        SeedDoc::new(id, title, content).layer(layer).insert(store);
    }

    #[test]
    fn whitelist_contains_expected_actions() {
        for a in [
            "reindex_fts",
            "rebuild_graph",
            "rebuild_wiki_index",
            "reembed_all",
        ] {
            assert!(REFRESH_ACTION_WHITELIST.contains(&a));
        }
    }

    #[test]
    fn flags_default_safe_suite_when_none_set() {
        let f = MaintainRefreshFlags::from_options(None, None, None, None, None, None, 50);
        assert!(f.reindex_fts);
        assert!(f.rebuild_graph);
        assert!(f.graph_dirty_only);
        assert!(f.rebuild_wiki_index);
        assert!(!f.reembed_all);
    }

    #[test]
    fn flags_explicit_single_action() {
        let f =
            MaintainRefreshFlags::from_options(Some(true), None, None, None, None, Some(true), 10);
        assert!(f.reindex_fts);
        assert!(!f.rebuild_graph);
        assert!(!f.rebuild_wiki_index);
        assert!(!f.reembed_all);
        assert!(f.dry_run);
        assert_eq!(f.max_docs, 10);
    }

    #[test]
    fn reindex_fts_and_wiki_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refresh.duckdb");
        let store = Store::open(&path).unwrap();
        seed_doc(&store, "d1", "Alpha", "hello world [[Beta]] #tag", "raw");
        seed_doc(
            &store,
            "w1",
            "Wiki One",
            "compiled page about alpha",
            "wiki",
        );

        let fts = reindex_fts(&store, Some("porter")).unwrap();
        assert!(!fts.backend.is_empty());
        assert_eq!(fts.stemmer, "porter");

        let idx = rebuild_wiki_index(&store).unwrap();
        assert_eq!(idx.entry_count, 1);
        assert_eq!(store.list_wiki_index().unwrap().len(), 1);
    }

    #[test]
    fn rebuild_graph_dirty_then_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.duckdb");
        let store = Store::open(&path).unwrap();
        seed_doc(&store, "d1", "Alpha", "see [[Beta]] and #topic", "raw");
        seed_doc(&store, "d2", "Gamma", "plain text no links", "raw");

        // Dirty: neither has a document node.
        let r1 = rebuild_graph_for_all_or_dirty(&store, true, 50).unwrap();
        assert_eq!(r1.candidate_count, 2);
        assert_eq!(r1.succeeded, 2);
        assert!(store.find_node_by_document_id("d1").unwrap().is_some());

        // Dirty again: none left.
        let r2 = rebuild_graph_for_all_or_dirty(&store, true, 50).unwrap();
        assert_eq!(r2.candidate_count, 0);
        assert_eq!(r2.processed, 0);

        // All: rebuild both again.
        let r3 = rebuild_graph_for_all_or_dirty(&store, false, 50).unwrap();
        assert_eq!(r3.candidate_count, 2);
        assert_eq!(r3.succeeded, 2);
    }

    #[test]
    fn graph_respects_max_docs_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cap.duckdb");
        let store = Store::open(&path).unwrap();
        for i in 0..5 {
            seed_doc(
                &store,
                &format!("d{i}"),
                &format!("Doc{i}"),
                "content",
                "raw",
            );
        }
        let r = rebuild_graph_for_all_or_dirty(&store, false, 2).unwrap();
        assert_eq!(r.candidate_count, 5);
        assert_eq!(r.processed, 2);
        assert_eq!(r.skipped_cap, 3);
        assert_eq!(r.succeeded, 2);
    }

    #[tokio::test]
    async fn maintain_refresh_dry_run_and_apply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("maint.duckdb");
        let store = Store::open(&path).unwrap();
        let dims = 8usize;
        let config = test_config(path.clone(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));

        seed_doc(&store, "d1", "Alpha", "hello [[Beta]] #x", "raw");
        seed_doc(&store, "w1", "Wiki", "wiki body", "wiki");
        store.write_embedding_manifest_from_config(&config).unwrap();

        let dry_flags = MaintainRefreshFlags {
            reindex_fts: true,
            rebuild_graph: true,
            graph_dirty_only: true,
            rebuild_wiki_index: true,
            reembed_all: false,
            dry_run: true,
            max_docs: 50,
        };
        let dry = maintain_refresh(&store, &embedder, &config, dry_flags)
            .await
            .unwrap();
        assert!(dry.dry_run);
        assert!(dry.reindex_fts.as_ref().unwrap().dry_run);
        assert_eq!(dry.rebuild_graph.as_ref().unwrap().candidate_count, 2);
        // dry_run does not create nodes
        assert!(store.find_node_by_document_id("d1").unwrap().is_none());
        assert!(dry.ops_log_id.is_some());

        let apply_flags = MaintainRefreshFlags {
            reindex_fts: true,
            rebuild_graph: true,
            graph_dirty_only: true,
            rebuild_wiki_index: true,
            reembed_all: true,
            dry_run: false,
            max_docs: 50,
        };
        let applied = maintain_refresh(&store, &embedder, &config, apply_flags)
            .await
            .unwrap();
        assert!(!applied.dry_run);
        assert_eq!(applied.rebuild_graph.as_ref().unwrap().succeeded, 2);
        assert_eq!(applied.rebuild_wiki_index.as_ref().unwrap().entry_count, 1);
        assert!(applied
            .reembed_all
            .as_ref()
            .is_some_and(|report| report.chunks_reembedded > 0));
        assert!(store.find_node_by_document_id("d1").unwrap().is_some());
        assert_eq!(store.list_wiki_index().unwrap().len(), 1);
        let generation_after_refresh = {
            let conn = store.lock().unwrap();
            crate::db::fts::fts_generation_state(&conn).unwrap()
        };
        assert!(!generation_after_refresh.dirty);
        let _hits = crate::db::search::search(
            &store,
            &crate::db::search::SearchQuery {
                mode: SearchMode::Lex,
                query_text: Some("hello".into()),
                top_k: 5,
                fts_stemmer: config.fts_stemmer.clone(),
                ..crate::db::search::SearchQuery::default()
            },
        )
        .unwrap();
        let generation_after_search = {
            let conn = store.lock().unwrap();
            crate::db::fts::fts_generation_state(&conn).unwrap()
        };
        assert_eq!(generation_after_search, generation_after_refresh);

        let ops = store.list_ops_log(10).unwrap();
        assert!(ops.iter().any(|o| o.op == "maintain_refresh"));
        assert!(ops.iter().any(|o| o.op == "maintain_refresh_dry_run"));
    }

    #[tokio::test]
    async fn reembed_all_updates_vectors_and_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reembed.duckdb");
        let store = Store::open(&path).unwrap();
        let dims = 16usize;
        let config = test_config(path.clone(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));

        seed_doc(&store, "d1", "A", "reembed body one", "raw");
        // Wrong-dim chunks from seed (8); reembed should fix to 16.
        store
            .set_embedding_manifest(&EmbeddingManifest {
                id: "default".into(),
                provider: "mock".into(),
                model: "mock".into(),
                dims: 8,
                base_url: None,
                content_fingerprint: None,
                updated_at: Utc::now(),
            })
            .unwrap();

        let report = reembed_all(&store, &embedder, &config, 50).await.unwrap();
        assert_eq!(report.documents_succeeded, 1);
        assert_eq!(report.chunks_considered, 1);
        assert_eq!(report.attached_chunks_considered, 1);
        assert_eq!(report.orphan_chunks, 0);
        assert_eq!(report.chunks_reembedded, report.attached_chunks_considered);
        assert!(report.errors.is_empty());
        let man = report.manifest.unwrap();
        assert_eq!(man.dims, dims as i32);

        let chunks = store.list_chunks_for_document("d1").unwrap();
        assert!(chunks.iter().all(|c| c.embedding.len() == dims));
        store
            .require_embedding_manifest_match(&config)
            .expect("identity matches after reembed_all");
    }

    #[tokio::test]
    async fn graph_and_uncapped_reembed_use_multiple_bounded_pages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paged-refresh.duckdb");
        let store = Store::open(&path).unwrap();
        let dims = 8usize;
        let config = test_config(path, dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        for index in 0..70 {
            let id = format!("doc-{index:03}");
            seed_doc(&store, &id, &id, "paged body", "wiki");
        }

        let graph = rebuild_graph_for_all_or_dirty(&store, false, usize::MAX).unwrap();
        assert_eq!(graph.candidate_count, 70);
        assert_eq!(graph.processed, 70);
        assert_eq!(graph.succeeded, 70);
        assert_eq!(graph.documents.len(), REPORT_DOCUMENT_CAP);
        assert_eq!(graph.document_results_truncated, 6);

        let store = store
            .try_corpus_mutation_scope("paged reembed test")
            .unwrap();
        let reembed = reembed_all(&store, &embedder, &config, usize::MAX)
            .await
            .unwrap();
        assert_eq!(reembed.documents_considered, 70);
        assert_eq!(reembed.documents_processed, 70);
        assert_eq!(reembed.documents_succeeded, 70);
        assert_eq!(reembed.skipped_cap, 0);
        assert_eq!(reembed.documents.len(), REPORT_DOCUMENT_CAP);
        assert_eq!(reembed.document_results_truncated, 6);
        assert!(reembed.errors.is_empty());
        store
            .require_embedding_manifest_match(&config)
            .expect("complete paged migration publishes target identity");
    }

    #[tokio::test]
    async fn maintain_refresh_clamps_user_max_docs_to_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refresh-service-cap.duckdb");
        let store = Store::open(&path).unwrap();
        let mut config = test_config(path, 8);
        config.maint_max_docs = 2;
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
        for index in 0..3 {
            let id = format!("doc-{index}");
            seed_doc(&store, &id, &id, "body", "wiki");
        }

        let report = maintain_refresh(
            &store,
            &embedder,
            &config,
            MaintainRefreshFlags {
                reindex_fts: false,
                rebuild_graph: true,
                graph_dirty_only: false,
                rebuild_wiki_index: false,
                reembed_all: false,
                dry_run: true,
                max_docs: 999,
            },
        )
        .await
        .unwrap();
        let graph = report.rebuild_graph.unwrap();
        assert_eq!(graph.candidate_count, 3);
        assert_eq!(graph.processed, 2);
        assert_eq!(graph.skipped_cap, 1);
    }

    #[tokio::test]
    async fn reembed_all_refuses_orphan_chunk_without_publishing_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reembed-orphan.duckdb");
        let store = Store::open(&path).unwrap();
        let dims = 16usize;
        let config = test_config(path, dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let legacy_vector = vec![0.75; 8];
        store
            .insert_chunks(&[Chunk {
                id: "orphan-c0".into(),
                document_id: "missing-document".into(),
                chunk_index: 0,
                content: "unowned legacy vector".into(),
                embedding: legacy_vector.clone(),
                char_start: 0,
                char_end: 21,
                metadata_json: "{}".into(),
            }])
            .unwrap();

        let refresh = maintain_refresh(
            &store,
            &embedder,
            &config,
            MaintainRefreshFlags {
                reindex_fts: false,
                rebuild_graph: false,
                graph_dirty_only: true,
                rebuild_wiki_index: false,
                reembed_all: true,
                dry_run: false,
                max_docs: 50,
            },
        )
        .await
        .expect("orphan integrity refusal is a structured report");
        assert_eq!(refresh.errors.len(), 1);
        assert!(refresh.errors[0].starts_with(REEMBED_ALL_INCOMPLETE_CODE));
        assert!(refresh.errors[0].contains("orphan_chunks=1"));
        let summary_id = refresh.ops_log_id.clone().expect("terminal ops log");
        let report = refresh.reembed_all.expect("nested reembed report");

        assert_eq!(report.documents_considered, 0);
        assert_eq!(report.documents_processed, 0);
        assert_eq!(report.chunks_considered, 1);
        assert_eq!(report.attached_chunks_considered, 0);
        assert_eq!(report.orphan_chunks, 1);
        assert_eq!(report.chunks_reembedded, 0);
        assert!(reembed_all_incomplete(&report));
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].starts_with(REEMBED_ALL_INCOMPLETE_CODE));
        assert!(report.errors[0].contains("no target embedding manifest was published"));
        assert!(report.manifest.is_none());
        assert!(store.get_embedding_manifest().unwrap().is_none());

        let summary = store
            .list_ops_log(10)
            .unwrap()
            .into_iter()
            .find(|entry| entry.id == summary_id)
            .expect("maintain_refresh summary");
        let payload: serde_json::Value = serde_json::from_str(&summary.payload_json).unwrap();
        assert_eq!(payload["errors"].as_array().unwrap().len(), 1);
        assert_eq!(payload["reembed_all"]["orphan_chunks"], 1);

        let chunks = store.all_chunks_with_embeddings().unwrap();
        assert_eq!(chunks.len(), 1, "reembed_all must not prune the orphan");
        assert_eq!(chunks[0].embedding, legacy_vector);
    }

    #[tokio::test]
    async fn capped_same_dimension_identity_migration_remains_closed_until_resumed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reembed-cap.duckdb");
        let store = Store::open(&path).unwrap();
        let old_config = test_config(path, 8);
        let mut target_config = old_config.clone();
        target_config.embedding_model = "next-model-same-dims".into();
        store
            .write_embedding_manifest_from_config(&old_config)
            .unwrap();
        seed_doc(&store, "d1", "A", "first body", "raw");
        seed_doc(&store, "d2", "B", "second body", "raw");
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
        let first = reembed_all(&store, &embedder, &target_config, 1)
            .await
            .unwrap();
        assert_eq!(first.documents_processed, 1);
        assert_eq!(first.pending_documents, 1);
        assert!(first.resumable);
        assert!(store.require_embedding_manifest_match(&old_config).is_err());
        assert!(store
            .require_embedding_manifest_match(&target_config)
            .is_err());
        let resumed = reembed_all(&store, &embedder, &target_config, 1)
            .await
            .unwrap();
        assert_eq!(resumed.documents_processed, 1);
        assert_eq!(resumed.pending_documents, 0);
        assert!(resumed.errors.is_empty());
        store
            .require_embedding_manifest_match(&target_config)
            .unwrap();
        assert!(store.require_embedding_manifest_match(&old_config).is_err());
    }

    #[tokio::test]
    async fn reembed_all_groups_small_documents_into_one_provider_call() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reembed-grouped.duckdb");
        let store = Store::open(&path).unwrap();
        let config = test_config(path, 16);
        for (id, body) in [
            ("g1", "alpha body"),
            ("g2", "beta body"),
            ("g3", "gamma body"),
        ] {
            seed_doc(&store, id, id, body, "raw");
        }
        let counting = Arc::new(FailOnTextEmbedder {
            calls: AtomicUsize::new(0),
            dims: 16,
        });
        let embedder: Arc<dyn EmbeddingProvider> = counting.clone();

        let report = reembed_all(&store, &embedder, &config, 3).await.unwrap();

        assert_eq!(report.documents_succeeded, 3);
        assert_eq!(report.documents_failed, 0);
        assert_eq!(counting.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn partial_migration_marker_refuses_old_and_target_until_full_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reembed-partial.duckdb");
        let store = Store::open(&path).unwrap();
        let old_config = test_config(path.clone(), 8);
        let target_config = test_config(path, 16);
        store
            .write_embedding_manifest_from_config(&old_config)
            .unwrap();
        seed_doc(&store, "d1", "A", "first body", "raw");
        seed_doc(&store, "d2", "B", "second body", "raw");

        let failing = Arc::new(FailOnTextEmbedder {
            calls: AtomicUsize::new(0),
            dims: 16,
        });
        let failing_embedder: Arc<dyn EmbeddingProvider> = failing.clone();
        let partial = reembed_all(&store, &failing_embedder, &target_config, 2)
            .await
            .expect("best-effort reembed report");
        assert_eq!(partial.documents_succeeded, 1);
        assert_eq!(partial.documents_failed, 1);
        assert_eq!(partial.skipped_cap, 0);
        // One grouped call fails, then each document is retried alone.
        assert_eq!(failing.calls.load(Ordering::Relaxed), 3);

        drop(store);
        let store = Store::open(&target_config.db_path).expect("reopen partial migration");
        let marker = store.get_embedding_manifest().unwrap().unwrap();
        assert!(marker
            .content_fingerprint
            .as_deref()
            .is_some_and(|fingerprint| fingerprint.starts_with("migration-incomplete:")));
        let target_error = store
            .require_embedding_manifest_match(&target_config)
            .expect_err("target identity must remain refused");
        assert!(target_error
            .to_string()
            .contains("incomplete corpus migration"));
        let old_error = store
            .require_embedding_manifest_match(&old_config)
            .expect_err("old identity must remain refused for mixed vectors");
        assert!(old_error
            .to_string()
            .contains("incomplete corpus migration"));

        let mut dimensions = store
            .all_chunks_with_embeddings()
            .unwrap()
            .into_iter()
            .map(|chunk| chunk.embedding.len())
            .collect::<Vec<_>>();
        dimensions.sort_unstable();
        assert_eq!(dimensions, vec![8, 16]);

        let recovery_embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
        let recovered = reembed_all(&store, &recovery_embedder, &target_config, 2)
            .await
            .expect("complete retry");
        // Resume skips the already-migrated document (progress survived reopen).
        assert_eq!(recovered.documents_processed, 1);
        assert_eq!(recovered.documents_succeeded, 1);
        assert_eq!(recovered.documents_failed, 0);
        assert_eq!(recovered.skipped_cap, 0);
        assert_eq!(recovered.pending_documents, 0);
        store
            .require_embedding_manifest_match(&target_config)
            .expect("target identity is published after complete retry");
        assert!(store
            .all_chunks_with_embeddings()
            .unwrap()
            .iter()
            .all(|chunk| chunk.embedding.len() == 16));
        // Completion cleared the resume state and logged a complete marker.
        let fingerprint = crate::db::store::embedding_manifest_from_config(&target_config)
            .content_fingerprint
            .unwrap();
        assert_eq!(store.count_reembed_done(&fingerprint).unwrap(), 0);
        assert!(store
            .list_ops_log(10)
            .unwrap()
            .iter()
            .any(|entry| entry.op == "reembed_progress"
                && entry.payload_json.contains("\"complete\":true")));
    }

    #[tokio::test]
    async fn direct_refresh_reports_partial_reembed_as_top_level_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refresh-partial-reembed.duckdb");
        let store = Store::open(&path).unwrap();
        let old_config = test_config(path.clone(), 8);
        let target_config = test_config(path, 16);
        store
            .write_embedding_manifest_from_config(&old_config)
            .unwrap();
        seed_doc(&store, "d1", "A", "first body", "raw");
        seed_doc(&store, "d2", "B", "second body", "raw");
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(FailOnTextEmbedder {
            calls: AtomicUsize::new(0),
            dims: 16,
        });

        let report = maintain_refresh(
            &store,
            &embedder,
            &target_config,
            MaintainRefreshFlags {
                reindex_fts: false,
                rebuild_graph: false,
                graph_dirty_only: true,
                rebuild_wiki_index: false,
                reembed_all: true,
                dry_run: false,
                max_docs: 2,
            },
        )
        .await
        .expect("best-effort refresh report");

        let reembed = report.reembed_all.expect("reembed report");
        assert_eq!(reembed.documents_succeeded, 1);
        assert_eq!(reembed.documents_failed, 1);
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].starts_with(REEMBED_ALL_INCOMPLETE_CODE));
        store
            .require_embedding_manifest_match(&target_config)
            .expect_err("partial refresh must leave vector reads fail-closed");
    }

    #[tokio::test]
    async fn refresh_final_fts_failure_returns_report_and_terminal_ops_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refresh-finalize-failure.duckdb");
        let store = Store::open(&path).unwrap();
        let dims = 8usize;
        let mut config = test_config(path, dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        store.write_embedding_manifest_from_config(&config).unwrap();
        seed_doc(
            &store,
            "d1",
            "Refresh",
            "durable reembed before FTS failure",
            "raw",
        );
        store.ensure_fts(&config.fts_stemmer).unwrap();
        config.fts_stemmer = crate::db::fts::test_fault::fail("refresh");

        let report = maintain_refresh(
            &store,
            &embedder,
            &config,
            MaintainRefreshFlags {
                reindex_fts: false,
                rebuild_graph: false,
                graph_dirty_only: true,
                rebuild_wiki_index: false,
                reembed_all: true,
                dry_run: false,
                max_docs: 50,
            },
        )
        .await
        .expect("aggregate report survives terminal FTS failure");

        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].contains("FTS_FINALIZATION_FAILED"));
        assert!(report
            .reembed_all
            .as_ref()
            .is_some_and(|result| result.chunks_reembedded > 0));
        let generation = {
            let conn = store.lock().unwrap();
            crate::db::fts_generation_state(&conn).unwrap()
        };
        assert!(generation.dirty, "next lexical read must retry FTS");
        let summary_id = report.ops_log_id.as_deref().expect("terminal summary id");
        let summary = store
            .list_ops_log(10)
            .unwrap()
            .into_iter()
            .find(|entry| entry.id == summary_id)
            .expect("terminal summary ops_log row");
        let payload: serde_json::Value = serde_json::from_str(&summary.payload_json).unwrap();
        assert_eq!(payload["errors"].as_array().unwrap().len(), 1);
        assert!(store
            .list_ops_log(10)
            .unwrap()
            .iter()
            .any(|entry| entry.op == "fts_finalization_failed"));
    }

    #[tokio::test]
    async fn write_wiki_then_refresh_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wiki.duckdb");
        let store = Store::open(&path).unwrap();
        let dims = 8usize;
        let config = test_config(path.clone(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        store.write_embedding_manifest_from_config(&config).unwrap();

        write_wiki_page(
            &store,
            &embedder,
            &config,
            crate::wiki::WikiPageWrite {
                slug: "alpha-page",
                title: "Alpha Page",
                content: "Body with [[Other]]",
                kind: "wiki",
                category: Some("notes"),
                summary: Some("summary line"),
                agent: None,
            },
        )
        .await
        .unwrap();

        // Wipe index and rebuild via refresh.
        {
            let conn = store.lock().unwrap();
            conn.execute("DELETE FROM wiki_index", []).unwrap();
        }
        assert!(store.list_wiki_index().unwrap().is_empty());

        let flags = MaintainRefreshFlags {
            reindex_fts: false,
            rebuild_graph: false,
            graph_dirty_only: true,
            rebuild_wiki_index: true,
            reembed_all: false,
            dry_run: false,
            max_docs: 50,
        };
        let r = maintain_refresh(&store, &embedder, &config, flags)
            .await
            .unwrap();
        assert_eq!(r.rebuild_wiki_index.unwrap().entry_count, 1);
    }

    #[tokio::test]
    async fn reembed_all_capped_migration_runs_one_resumable_batch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reembed-cap.duckdb");
        let store = Store::open(&path).unwrap();
        let old_config = test_config(path.clone(), 8);
        let mut target_config = old_config.clone();
        target_config.embedding_model = "next-model-same-dims".into();
        store
            .write_embedding_manifest_from_config(&old_config)
            .unwrap();
        seed_doc(&store, "d1", "A", "first body", "raw");
        seed_doc(&store, "d2", "B", "second body", "raw");

        let failing = Arc::new(FailAfterFirstEmbedder {
            calls: AtomicUsize::new(0),
            dims: 8,
        });
        let embedder: Arc<dyn EmbeddingProvider> = failing.clone();

        // A capped migration is no longer refused: it processes one batch.
        let batch = reembed_all(&store, &embedder, &target_config, 1)
            .await
            .expect("capped migration runs one resumable batch");
        assert!(batch.resumable);
        assert_eq!(batch.documents_processed, 1);
        assert_eq!(batch.documents_succeeded, 1);
        assert_eq!(batch.skipped_cap, 1);
        assert_eq!(batch.pending_documents, 1);
        assert!(reembed_all_incomplete(&batch));
        assert_eq!(failing.calls.load(Ordering::Relaxed), 1);

        // Fail-closed while partial: neither old nor target identity matches.
        store
            .require_embedding_manifest_match(&old_config)
            .expect_err("old identity refused once mixed vectors exist");
        store
            .require_embedding_manifest_match(&target_config)
            .expect_err("target identity refused until migration completes");

        // ops_log carries resumable progress for operators.
        let ops = store.list_ops_log(10).unwrap();
        let progress = ops
            .iter()
            .find(|entry| entry.op == "reembed_progress")
            .expect("reembed progress entry");
        let payload: serde_json::Value = serde_json::from_str(&progress.payload_json).unwrap();
        assert_eq!(payload["documents_done"], 1);
        assert_eq!(payload["documents_pending"], 1);
        assert_eq!(payload["complete"], false);
    }

    #[tokio::test]
    async fn reembed_status_reports_match_pending_and_completion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reembed-status.duckdb");
        let store = Store::open(&path).unwrap();
        let old_config = test_config(path.clone(), 8);
        let target_config = test_config(path, 16);
        store
            .write_embedding_manifest_from_config(&old_config)
            .unwrap();
        for id in ["d1", "d2", "d3"] {
            seed_doc(&store, id, id, "status body", "raw");
        }

        // Identity match: search open, no migration section.
        let status = reembed_status(&store, &old_config).unwrap();
        assert!(status.identity_match);
        assert!(status.vector_search_open);
        assert!(status.migration.is_none());

        // Mismatch: all three documents pending, search closed.
        let status = reembed_status(&store, &target_config).unwrap();
        assert!(!status.identity_match);
        assert!(!status.vector_search_open);
        let migration = status.migration.expect("migration status");
        assert_eq!(migration.documents_done, 0);
        assert_eq!(migration.documents_pending, 3);
        assert_eq!(migration.documents_total, 3);
        assert_eq!(migration.batch_size, target_config.reembed_batch);

        // Batch 1: two documents migrate; status reflects the remaining one.
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(16));
        let batch = reembed_all(&store, &embedder, &target_config, 2)
            .await
            .unwrap();
        assert!(batch.resumable);
        assert_eq!(batch.documents_succeeded, 2);
        assert_eq!(batch.pending_documents, 1);
        store
            .require_embedding_manifest_match(&target_config)
            .expect_err("search stays closed while one document is pending");
        let status = reembed_status(&store, &target_config).unwrap();
        let migration = status.migration.expect("migration status");
        assert_eq!(migration.documents_done, 2);
        assert_eq!(migration.documents_pending, 1);

        // Simulate a restart between batches: reopen the same database file.
        drop(store);
        let store = Store::open(&target_config.db_path).unwrap();
        let status = reembed_status(&store, &target_config).unwrap();
        assert_eq!(status.migration.unwrap().documents_pending, 1);

        // Batch 2 completes the migration: manifest flips atomically, search opens.
        let final_batch = reembed_all(&store, &embedder, &target_config, 2)
            .await
            .unwrap();
        assert_eq!(final_batch.documents_succeeded, 1);
        assert_eq!(final_batch.pending_documents, 0);
        assert!(!reembed_all_incomplete(&final_batch));
        store
            .require_embedding_manifest_match(&target_config)
            .expect("manifest flipped after final batch");
        assert!(store
            .all_chunks_with_embeddings()
            .unwrap()
            .iter()
            .all(|chunk| chunk.embedding.len() == 16));
        let status = reembed_status(&store, &target_config).unwrap();
        assert!(status.identity_match);
        assert!(status.vector_search_open);
        assert!(status.migration.is_none());
    }
}
