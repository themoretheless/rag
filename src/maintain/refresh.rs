//! Actualize / refresh: FTS reindex, graph rebuild, wiki catalog, optional reembed.
//!
//! Whitelist-only actions (no SQL/shell invention). Logs every apply to `ops_log`.
//! See `docs/LOCAL_LLM_MAINTENANCE.md` §3.5.

use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::config::Config;
use crate::db::store::{embedding_manifest_matches_config, embedding_migration_manifest};
use crate::db::{self, Store};
use crate::embeddings::EmbeddingProvider;
use crate::error::{AppError, Result};
use crate::graph::rebuild_document_graph;
use crate::models::{EmbeddingManifest, OpsLogEntry};

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
            max_docs: 50,
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
            || report.chunks_reembedded != report.attached_chunks_considered
            || !report.errors.is_empty())
}

pub(crate) fn reembed_all_incomplete_message(report: &ReembedAllReport) -> String {
    format!(
        "{REEMBED_ALL_INCOMPLETE_CODE}: processed={} succeeded={} failed={} skipped={}; chunks_considered={} attached_chunks={} orphan_chunks={} chunks_reembedded={} errors={}; full-corpus completion was not reached",
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
    let conn = store.lock()?;
    let state = match stemmer {
        Some(s) if !s.trim().is_empty() => db::reindex_with_stemmer(&conn, s)?,
        _ => db::reindex(&conn)?,
    };
    Ok(ReindexFtsReport {
        backend: state.backend.as_str().to_string(),
        stemmer: state.stemmer,
        dry_run: false,
    })
}

#[cfg(test)]
const TEST_FINALIZE_FTS_FAILURE_STEMMER: &str = "__test_fail_refresh_finalize_fts__";

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
    if stemmer == TEST_FINALIZE_FTS_FAILURE_STEMMER {
        return Err(AppError::fts("injected refresh FTS finalization failure"));
    }

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
    if stemmer == TEST_FINALIZE_FTS_FAILURE_STEMMER {
        return Err(AppError::fts("injected refresh FTS finalization failure"));
    }

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

    Ok(GraphRebuildReport {
        dirty_only,
        candidate_count,
        processed,
        succeeded,
        failed,
        skipped_cap,
        dry_run: false,
        pruned_orphan_stubs,
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
    let conn = store.lock()?;
    let (total, attached): (i64, i64) = conn.query_row(
        r#"
        SELECT
          COUNT(*)::BIGINT,
          COUNT(d.id)::BIGINT
        FROM chunks c
        LEFT JOIN documents d ON d.id = c.document_id
        "#,
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
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

fn refuse_capped_embedding_migration(
    store: &Store,
    config: &Config,
    document_count: usize,
    max_docs: usize,
) -> Result<()> {
    if document_count <= max_docs || store.embedding_manifest_matches_config(config)? {
        return Ok(());
    }
    let mismatch = store
        .require_embedding_manifest_match(config)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_else(|| "embedding identity changed during migration preflight".into());
    Err(AppError::embeddings(format!(
        "refuse capped embedding migration: corpus has {document_count} documents but max_docs={max_docs}; no vectors were changed. Run a complete uncapped reembed_all. {mismatch}"
    )))
}

/// Re-embed chunks for every document (capped by `max_docs`) with the live embedder.
///
/// A different embedding identity may be migrated only when the whole corpus
/// fits within `max_docs`. The target manifest is published only after an
/// uncapped, zero-failure pass (or for an empty corpus), so a partial migration
/// remains visibly incompatible with vector search.
pub async fn reembed_all(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    max_docs: usize,
) -> Result<ReembedAllReport> {
    let max_docs = max_docs.max(1);
    let documents_considered = store.count_documents()?;
    let skipped_cap = documents_considered.saturating_sub(max_docs);
    refuse_capped_embedding_migration(store, config, documents_considered, max_docs)?;
    let previous_manifest = store.get_embedding_manifest()?;
    let inventory_before = reembed_chunk_inventory(store)?;
    if inventory_before.orphan > 0 {
        let error = format!(
            "{REEMBED_ALL_INCOMPLETE_CODE}: refuse reembed_all because {} orphan chunk(s) are not attached to any document; no vectors were changed and no target embedding manifest was published. Run doctor and repair corpus integrity explicitly before retrying",
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
            document_results_truncated: 0,
            manifest: previous_manifest,
            errors: vec![error],
            documents: Vec::new(),
        });
    }
    let migration_marker = previous_manifest
        .as_ref()
        .is_none_or(|manifest| !embedding_manifest_matches_config(manifest, config))
        .then(|| embedding_migration_manifest(config));
    let mut migration_started = false;
    let target_count = documents_considered.min(max_docs);

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
        let page = store.list_document_ids_after(after_id.as_deref(), page_limit)?;
        if page.is_empty() {
            errors.push(format!(
                "{REEMBED_ALL_INCOMPLETE_CODE}: document inventory changed during reembed_all (expected to process {target_count}, reached {documents_processed})"
            ));
            break;
        }
        for document_id in page {
            after_id = Some(document_id.clone());
            documents_processed += 1;
            let manifest = migration_marker.as_ref().filter(|_| !migration_started);
            let result = match reembed_one_document(
                store,
                embedder,
                &document_id,
                config.embedding_dims,
                manifest,
            )
            .await
            {
                Ok(chunk_count) => {
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
    }

    let inventory_after = reembed_chunk_inventory(store)?;
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
    if chunks_reembedded != inventory_before.attached {
        errors.push(format!(
            "{REEMBED_ALL_INCOMPLETE_CODE}: attached chunk coverage mismatch (expected={}, reembedded={})",
            inventory_before.attached, chunks_reembedded
        ));
    }

    let complete_success = skipped_cap == 0
        && documents_failed == 0
        && documents_processed == documents_considered
        && documents_succeeded == documents_considered
        && errors.is_empty();
    let manifest = if complete_success {
        Some(store.write_embedding_manifest_from_config(config)?)
    } else {
        // The first successful vector mutation publishes an incompatible
        // marker atomically with those vectors. A partial migration therefore
        // remains fail-closed for both the old and target runtime configs.
        store.get_embedding_manifest()?
    };

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
        document_results_truncated,
        manifest,
        errors,
        documents,
    })
}

async fn reembed_one_document(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    document_id: &str,
    expected_dims: usize,
    manifest: Option<&EmbeddingManifest>,
) -> Result<usize> {
    let doc = store
        .get_document(document_id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;

    let chunks = store.list_chunks_for_document(&doc.id)?;
    let chunk_count = chunks.len();
    if chunks.is_empty() {
        return Ok(0);
    }

    let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
    let embeddings = embedder.embed(&texts).await?;
    if embeddings.len() != chunks.len() {
        return Err(AppError::embeddings(format!(
            "embedder returned {} vectors for {} chunks",
            embeddings.len(),
            chunks.len()
        )));
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
    store.update_chunk_embeddings_atomic(&doc.id, doc.revision, &chunks, &embeddings, manifest)?;
    Ok(chunk_count)
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

    if flags.reembed_all && !flags.dry_run {
        let document_count = store.count_documents()?;
        refuse_capped_embedding_migration(store, config, document_count, flags.max_docs)?;
    }

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
            let status = {
                let conn = store.lock()?;
                db::fts_status(&conn)?
            };
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
            let candidate_count = store.count_graph_rebuild_candidates(flags.graph_dirty_only)?;
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
                document_results_truncated: 0,
                documents: Vec::new(),
            });
        } else {
            report.rebuild_graph = Some(rebuild_graph_for_all_or_dirty(
                store,
                flags.graph_dirty_only,
                flags.max_docs,
            )?);
        }
    }

    if flags.rebuild_wiki_index {
        if dry_run {
            let n = store.count_documents_by_layer("wiki")?;
            report.rebuild_wiki_index = Some(WikiIndexRebuildReport {
                entry_count: n,
                dry_run: true,
            });
        } else {
            report.rebuild_wiki_index = Some(rebuild_wiki_index(store)?);
        }
    }

    if flags.reembed_all {
        if dry_run {
            let n = store.count_documents()?;
            let chunk_est = store.count_chunks_for_document_limit(flags.max_docs)?;
            let inventory = reembed_chunk_inventory(store)?;
            let errors = (inventory.orphan > 0)
                .then(|| {
                    format!(
                        "{REEMBED_ALL_INCOMPLETE_CODE}: {} orphan chunk(s) are outside document-driven re-embedding; apply would refuse without changing vectors",
                        inventory.orphan
                    )
                })
                .into_iter()
                .collect();
            report.reembed_all = Some(ReembedAllReport {
                documents_considered: n,
                documents_processed: n.min(flags.max_docs),
                documents_succeeded: 0,
                documents_failed: 0,
                skipped_cap: n.saturating_sub(flags.max_docs),
                chunks_considered: inventory.total,
                attached_chunks_considered: inventory.attached,
                orphan_chunks: inventory.orphan,
                chunks_reembedded: chunk_est,
                dry_run: true,
                document_results_truncated: 0,
                manifest: store.get_embedding_manifest()?,
                errors,
                documents: Vec::new(),
            });
        } else {
            report.reembed_all = Some(reembed_all(store, embedder, config, flags.max_docs).await?);
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
    let op = if dry_run {
        "maintain_refresh_dry_run"
    } else {
        "maintain_refresh"
    };
    let message = if dry_run {
        format!("dry_run maintain_refresh actions=[{}]", actions.join(","))
    } else {
        format!("maintain_refresh actions=[{}]", actions.join(","))
    };
    let payload = serde_json::to_string(&report).unwrap_or_else(|_| "{}".into());
    let written = store.append_ops_log(&OpsLogEntry {
        id: Uuid::new_v4().to_string(),
        seq: 0,
        ts: Utc::now(),
        op: op.into(),
        prefix: Some("MAINT".into()),
        message,
        entity_id: None,
        entity_kind: Some("corpus".into()),
        payload_json: payload,
        agent_name: None,
    })?;
    report.ops_log_id = Some(written.id);
    report.ops_log_seq = Some(written.seq);

    Ok(report)
}

fn fts_finalization_failure(store: &Store, operation: &str, error: &AppError) -> String {
    let marker_error = store
        .mark_fts_dirty_for_retry()
        .err()
        .map(|marker_error| marker_error.to_string());
    let mut message = format!(
        "FTS_FINALIZATION_FAILED: {operation} committed corpus work, but final FTS refresh failed: {error}; retryable=true"
    );
    if let Some(marker_error) = marker_error {
        message.push_str(&format!(
            "; additionally failed to mark FTS dirty for retry: {marker_error}"
        ));
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbedder;
    use crate::models::{Chunk, Document, SearchMode};
    use crate::wiki::{write_wiki_page, LAYER_WIKI};
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    fn test_config(db_path: PathBuf, dims: usize) -> Config {
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
            ingest_roots: Vec::new(),
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

    fn seed_doc(store: &Store, id: &str, title: &str, content: &str, layer: &str) {
        let now = Utc::now();
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("text://{id}"),
                title: title.into(),
                content: content.into(),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                layer: layer.into(),
                kind: if layer == LAYER_WIKI {
                    "wiki".into()
                } else {
                    "document".into()
                },
                ..Default::default()
            })
            .unwrap();
        store
            .insert_chunks(&[Chunk {
                id: format!("{id}-c0"),
                document_id: id.into(),
                chunk_index: 0,
                content: content.into(),
                embedding: vec![0.1; 8],
                char_start: 0,
                char_end: content.len() as i32,
                metadata_json: "{}".into(),
            }])
            .unwrap();
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

        let _mutation_guard = store
            .try_corpus_mutation_guard("paged reembed test")
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
    async fn reembed_all_refuses_capped_identity_migration_before_embedding() {
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

        let before_vectors = store
            .all_chunks_with_embeddings()
            .unwrap()
            .into_iter()
            .map(|chunk| chunk.embedding)
            .collect::<Vec<_>>();
        let before_generation = {
            let conn = store.lock().unwrap();
            crate::db::fts::chunks_generation(&conn).unwrap()
        };
        let before_fingerprint = store
            .get_embedding_manifest()
            .unwrap()
            .unwrap()
            .content_fingerprint;
        let failing = Arc::new(FailAfterFirstEmbedder {
            calls: AtomicUsize::new(0),
            dims: 8,
        });
        let embedder: Arc<dyn EmbeddingProvider> = failing.clone();

        let error = reembed_all(&store, &embedder, &target_config, 1)
            .await
            .expect_err("capped migration must be refused");
        let message = error.to_string();
        assert!(message.contains("refuse capped embedding migration"));
        assert!(message.contains("no vectors were changed"));
        assert_eq!(failing.calls.load(Ordering::Relaxed), 0);
        assert_eq!(
            store
                .all_chunks_with_embeddings()
                .unwrap()
                .into_iter()
                .map(|chunk| chunk.embedding)
                .collect::<Vec<_>>(),
            before_vectors
        );
        let after_generation = {
            let conn = store.lock().unwrap();
            crate::db::fts::chunks_generation(&conn).unwrap()
        };
        assert_eq!(after_generation, before_generation);
        assert_eq!(
            store
                .get_embedding_manifest()
                .unwrap()
                .unwrap()
                .content_fingerprint,
            before_fingerprint
        );
        store
            .require_embedding_manifest_match(&old_config)
            .expect("old identity remains valid after refused migration");
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

        let failing = Arc::new(FailAfterFirstEmbedder {
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
        assert_eq!(failing.calls.load(Ordering::Relaxed), 2);

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
        assert_eq!(recovered.documents_succeeded, 2);
        assert_eq!(recovered.documents_failed, 0);
        assert_eq!(recovered.skipped_cap, 0);
        store
            .require_embedding_manifest_match(&target_config)
            .expect("target identity is published after complete retry");
        assert!(store
            .all_chunks_with_embeddings()
            .unwrap()
            .iter()
            .all(|chunk| chunk.embedding.len() == 16));
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
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(FailAfterFirstEmbedder {
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
        config.fts_stemmer = TEST_FINALIZE_FTS_FAILURE_STEMMER.into();

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
            "alpha-page",
            "Alpha Page",
            "Body with [[Other]]",
            "wiki",
            Some("notes"),
            Some("summary line"),
            None,
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
}
