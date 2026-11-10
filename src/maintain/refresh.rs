//! Actualize / refresh: FTS reindex, graph rebuild, wiki catalog, optional reembed.
//!
//! Whitelist-only actions (no SQL/shell invention). Logs every apply to `ops_log`.
//! See `docs/LOCAL_LLM_MAINTENANCE.md` §3.5.

use crate::db::store::embedding_manifest_from_config;
use crate::models::OpsLogEntry;
use std::sync::Arc;

use serde::Serialize;

use crate::config::Config;
use crate::db::graph::resolve::rebuild_document_graph;
use crate::db::store::{embedding_manifest_matches_config, embedding_migration_manifest};
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::error::{AppError, Result};
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
    let state = store.maintenance().reindex_fts(stemmer)?;
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
    tokio::task::spawn_blocking(move || store.maintenance().ensure_fts(&stemmer))
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
    let candidate_count = store
        .documents()
        .count_graph_rebuild_candidates(dirty_only)?;
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
        let page = store.documents().list_graph_rebuild_candidates_after(
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
        store.graph().prune_orphan_stubs()?
    } else {
        0
    };
    let pruned_orphan_tags = if complete_success {
        store.graph().prune_orphan_tags()?
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
    let entry_count = store.wiki_index().rebuild_wiki_index_from_docs()?;
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
    let scoped_store = store
        .corpus_lane()
        .try_corpus_mutation_scope("reembed_all")?;
    let store = &scoped_store;
    let max_docs = max_docs.max(1);
    let documents_considered = store.documents().count_documents()?;
    let previous_manifest = store.embedding_manifest().get_embedding_manifest()?;
    let identity_matches = store
        .embedding_manifest()
        .embedding_manifest_matches_config(config)?;
    let target_fingerprint = (!identity_matches).then(|| {
        embedding_manifest_from_config(config)
            .content_fingerprint
            .unwrap_or_else(|| "unknown".into())
    });
    let pending_before = match target_fingerprint.as_ref() {
        Some(fingerprint) => store
            .reembed_progress()
            .count_reembed_pending(fingerprint)?,
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
            Some(fingerprint) => store.reembed_progress().list_reembed_pending_document_ids(
                fingerprint,
                after_id.as_deref(),
                page_limit,
            )?,
            None => store
                .documents()
                .list_document_ids_after(after_id.as_deref(), page_limit)?,
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
                            store.reembed_progress().mark_reembed_done(
                                fingerprint,
                                &document_id,
                                chunk_count,
                            )?;
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
        Some(fingerprint) => store
            .reembed_progress()
            .count_reembed_pending(fingerprint)?,
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
        let written = store
            .embedding_manifest()
            .write_embedding_manifest_from_config(config)?;
        if let Some(fingerprint) = target_fingerprint.as_ref() {
            // Migration is complete; drop the resume state for this target.
            let _ = store.reembed_progress().clear_reembed_progress(fingerprint);
        }
        Some(written)
    } else {
        // The first successful vector mutation publishes an incompatible
        // marker atomically with those vectors. A partial migration therefore
        // remains fail-closed for both the old and target runtime configs.
        store.embedding_manifest().get_embedding_manifest()?
    };

    if let Some(fingerprint) = target_fingerprint.as_ref() {
        let documents_done = store.reembed_progress().count_reembed_done(fingerprint)?;
        store.ops_log().record(&OpsLogEntry::new("reembed_progress", "REEMBED", format!(
                "reembed migration batch: {} done / {} pending (target complete: {complete_success})",
                documents_done, pending_after
            ))
.entity(None, "corpus")
.payload(serde_json::json!({
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
            .to_string()));
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
        .documents()
        .document_revision(document_id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;
    let chunks = store.chunks().list_chunks_for_document(document_id)?;
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
    store.chunks().update_chunk_embeddings_atomic(
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
        return Err(AppError::invalid(
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
            let status = crate::util::blocking_section(|| store.maintenance().fts_status())?;
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
                store
                    .documents()
                    .count_graph_rebuild_candidates(flags.graph_dirty_only)
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
                store
                    .documents()
                    .count_documents_by_layer(crate::models::Layer::Wiki.as_str())
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
            let n = store.documents().count_documents()?;
            let chunk_est = store
                .documents()
                .count_chunks_for_document_limit(reembed_max)?;
            let inventory = reembed_chunk_inventory(store)?;
            let identity_matches = store
                .embedding_manifest()
                .embedding_manifest_matches_config(config)?;
            let pending = if identity_matches {
                n
            } else {
                let fingerprint = embedding_manifest_from_config(config)
                    .content_fingerprint
                    .unwrap_or_else(|| "unknown".into());
                store
                    .reembed_progress()
                    .count_reembed_pending(&fingerprint)?
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
                manifest: store.embedding_manifest().get_embedding_manifest()?,
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
        store
            .corpus_lane()
            .record_fts_finalization_failure(operation, true, &error.to_string())
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
    let manifest = store.embedding_manifest().get_embedding_manifest()?;
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
    let documents_total = store.documents().count_documents()?;
    let target_fingerprint = configured
        .content_fingerprint
        .clone()
        .unwrap_or_else(|| "unknown".into());
    let documents_done = store
        .reembed_progress()
        .count_reembed_done(&target_fingerprint)?;
    let documents_pending = store
        .reembed_progress()
        .count_reembed_pending(&target_fingerprint)?;
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
mod tests;
