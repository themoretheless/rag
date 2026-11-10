//! Storage compression: checkpoint/FTS, exact-hash merge, near-dup list/merge.
//!
//! Levels (cumulative):
//! - **0** — DuckDB `CHECKPOINT` + FTS reindex (safe).
//! - **1** — exact `content_hash` merge (keep canonical, remove dups).
//! - **2** — near-duplicate list; merge only when `confirm=true` (and not dry_run).
//!
//! Safety: never hard-deletes `layer=raw` without `allow_raw_delete=true`
//! (tombstones instead). Every run appends `ops_log`. See
//! `docs/LOCAL_LLM_MAINTENANCE.md` §3.4.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::db::{self, Store};
use crate::error::{AppError, Result};
use crate::maintain::analyze::{DuplicateGroup, NearDuplicatePair};
use crate::models::Document;

/// Highest compression level implemented (L0–L2).
pub const COMPRESS_LEVEL_MAX: u8 = 2;

/// Cap reported merge/list lines.
const REPORT_LIST_CAP: usize = 100;

/// Allowed action names that `maintain_compress` may perform (safety whitelist).
pub const COMPRESS_ACTION_WHITELIST: &[&str] = &[
    "checkpoint",
    "reindex_fts",
    "merge_exact_dup",
    "merge_near_dup",
    "list_near_dup",
    "tombstone_raw_dup",
];

/// Options for [`maintain_compress`].
#[derive(Debug, Clone)]
pub struct CompressOptions {
    /// Compression level 0..=2 (higher includes lower steps).
    pub level: u8,
    /// When true, report planned work without mutating store.
    pub dry_run: bool,
    /// Required to **apply** near-dup merges at level ≥ 2 (list always runs).
    pub confirm: bool,
    /// When true, hard-delete `layer=raw` duplicates. Otherwise tombstone them.
    pub allow_raw_delete: bool,
    /// Cosine θ for near-dups (level ≥ 2).
    pub near_dup_threshold: f64,
    /// Max documents removed/tombstoned per run (`RAG_MAINT_MAX_DOCS`).
    pub max_docs: usize,
    /// When true, append `ops_log` for this run.
    pub log_ops: bool,
}

impl Default for CompressOptions {
    fn default() -> Self {
        Self {
            level: 0,
            dry_run: true,
            confirm: false,
            allow_raw_delete: false,
            near_dup_threshold: 0.92,
            max_docs: super::DEFAULT_MAX_DOCS,
            log_ops: true,
        }
    }
}

impl CompressOptions {
    /// Build options from config defaults and optional MCP overrides.
    pub fn from_config(config: &Config) -> Self {
        Self {
            near_dup_threshold: config.maint_near_dup_threshold,
            max_docs: config.maint_max_docs.max(1),
            ..Self::default()
        }
    }

    /// Validate level range.
    pub fn validate(&self) -> Result<()> {
        if self.level > COMPRESS_LEVEL_MAX {
            return Err(AppError::invalid(format!(
                "maintain_compress level {} out of range 0..={}",
                self.level, COMPRESS_LEVEL_MAX
            )));
        }
        if self.max_docs == 0 {
            return Err(AppError::invalid("maintain_compress max_docs must be >= 1"));
        }
        Ok(())
    }
}

/// One planned or applied merge/tombstone line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressMergeAction {
    /// `merge_exact_dup` | `merge_near_dup` | `tombstone_raw_dup` | `skip_cap` | `list_near_dup`.
    pub action: String,
    pub keep_id: String,
    pub remove_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cosine: Option<f32>,
    /// Document layer of the removed side (`raw`, `wiki`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_layer: Option<String>,
    /// How the remove side was handled: `deleted` | `tombstoned` | `planned` | `listed` | `skipped`.
    pub disposition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// L0 step detail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressL0Report {
    pub checkpointed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fts_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fts_stemmer: Option<String>,
    pub dry_run: bool,
}

/// Aggregate report for `maintain_compress`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintainCompressReport {
    pub level: u8,
    pub dry_run: bool,
    pub confirm: bool,
    pub allow_raw_delete: bool,
    pub max_docs: usize,
    /// Whitelist action names that ran (or would run).
    pub actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_before: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_after: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_delta: Option<i64>,
    pub docs_before: u64,
    pub chunks_before: u64,
    pub docs_after: u64,
    pub chunks_after: u64,
    pub docs_removed: u64,
    pub docs_tombstoned: u64,
    pub chunks_removed: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l0: Option<CompressL0Report>,
    /// Exact-hash groups considered at level ≥ 1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exact_dup_groups: Vec<DuplicateGroup>,
    /// Near-dup pairs listed at level ≥ 2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub near_duplicates: Vec<NearDuplicatePair>,
    /// Planned or applied merge/tombstone lines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merges: Vec<CompressMergeAction>,
    /// Docs that would have been touched beyond `max_docs`.
    pub skipped_cap: usize,
    /// Raw dups that were tombstoned (or planned) because `allow_raw_delete=false`.
    pub raw_protected: usize,
    /// Terminal finalization failures after compression phases.
    ///
    /// These are additive so older consumers can continue reading the report,
    /// while callers can distinguish already-durable corpus mutations from a
    /// clean derived-index publication.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops_log_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops_log_seq: Option<i64>,
    /// Human-readable notes (e.g. L2 needs confirm).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Run compression according to `opts.level`.
///
/// Never hard-deletes `layer=raw` without `allow_raw_delete`. Level 2 merges
/// apply only when `confirm=true` and `dry_run=false`; otherwise pairs are listed.
pub fn maintain_compress(
    store: &Store,
    config: &Config,
    opts: &CompressOptions,
) -> Result<MaintainCompressReport> {
    opts.validate()?;

    let level = opts.level;
    // L2 decisions compare stored vectors and may delete or tombstone documents.
    // Refuse the entire workflow before CHECKPOINT, L1 merges, or ops-log writes
    // when a corpus migration is incomplete or the runtime identity differs.
    if level >= 2 {
        store
            .embedding_manifest()
            .require_embedding_manifest_match(config)?;
    }
    let dry_run = opts.dry_run;
    let max_docs = opts.max_docs.max(1);
    let bytes_before = store.maintenance().db_file_size_bytes();
    let (docs_before, chunks_before, _, _) = store.maintenance().stats()?;

    let mut actions: Vec<String> = Vec::new();
    let mut merges: Vec<CompressMergeAction> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut docs_removed: u64 = 0;
    let mut docs_tombstoned: u64 = 0;
    let mut chunks_removed: u64 = 0;
    let mut removed_budget = max_docs;
    let mut skipped_cap = 0usize;
    let mut raw_protected = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let mut exact_dup_groups: Vec<DuplicateGroup> = Vec::new();
    let mut near_duplicates: Vec<NearDuplicatePair> = Vec::new();
    let mut planned_deletes: Vec<String> = Vec::new();
    let mut planned_tombstones: Vec<String> = Vec::new();
    // --- L0: checkpoint + FTS ---
    actions.push("checkpoint".into());
    actions.push("reindex_fts".into());
    let mut l0_report = if dry_run {
        let status = store.maintenance().fts_status()?;
        Some(CompressL0Report {
            checkpointed: false,
            fts_backend: status.as_ref().map(|s| s.backend.as_str().to_string()),
            fts_stemmer: status
                .map(|s| s.stemmer)
                .or_else(|| Some(config.fts_stemmer.clone())),
            dry_run: true,
        })
    } else {
        store.maintenance().checkpoint()?;
        let fts = if level == 0 {
            Some(store.maintenance().reindex_fts(Some(&config.fts_stemmer))?)
        } else {
            // Levels 1 and 2 rebuild once after their merge phases.
            None
        };
        Some(CompressL0Report {
            checkpointed: true,
            fts_backend: fts.as_ref().map(|state| state.backend.as_str().to_string()),
            fts_stemmer: fts
                .map(|state| state.stemmer)
                .or_else(|| Some(config.fts_stemmer.clone())),
            dry_run: false,
        })
    };

    // --- L1: exact hash merge ---
    if level >= 1 {
        actions.push("merge_exact_dup".into());
        exact_dup_groups = crate::db::analytics::find_exact_duplicates(store, true)?;

        for group in &exact_dup_groups {
            if group.document_ids.len() < 2 {
                continue;
            }
            let docs = load_docs_ordered(store, &group.document_ids)?;
            if docs.len() < 2 {
                continue;
            }
            let keep = pick_canonical(&docs);
            for doc in &docs {
                if doc.id == keep.id {
                    continue;
                }
                if removed_budget == 0 {
                    skipped_cap += 1;
                    if merges.len() < REPORT_LIST_CAP {
                        merges.push(CompressMergeAction {
                            action: "merge_exact_dup".into(),
                            keep_id: keep.id.clone(),
                            remove_id: doc.id.clone(),
                            keep_title: Some(keep.title.clone()),
                            remove_title: Some(doc.title.clone()),
                            content_hash: Some(group.content_hash.clone()),
                            cosine: None,
                            remove_layer: Some(doc.layer.to_string()),
                            disposition: "skipped".into(),
                            reason: Some("max_docs budget exhausted".into()),
                        });
                    }
                    continue;
                }

                let is_raw = crate::models::Layer::Raw.is(&doc.layer);
                let (action_name, disposition, reason) = if is_raw && !opts.allow_raw_delete {
                    (
                        "tombstone_raw_dup",
                        if dry_run { "planned" } else { "tombstoned" },
                        Some("raw layer protected; allow_raw_delete=false".to_string()),
                    )
                } else {
                    (
                        "merge_exact_dup",
                        if dry_run { "planned" } else { "deleted" },
                        None,
                    )
                };

                if !dry_run {
                    if action_name == "tombstone_raw_dup" {
                        planned_tombstones.push(doc.id.clone());
                        docs_tombstoned += 1;
                        raw_protected += 1;
                        // Chunks stay; no chunk removal.
                    } else {
                        let chunk_n = store.chunks().count_chunks_for_document(&doc.id)?;
                        planned_deletes.push(doc.id.clone());
                        docs_removed += 1;
                        chunks_removed += chunk_n;
                    }
                } else if is_raw && !opts.allow_raw_delete {
                    raw_protected += 1;
                    docs_tombstoned += 1; // planned
                } else {
                    let chunk_n = store.chunks().count_chunks_for_document(&doc.id)?;
                    docs_removed += 1; // planned
                    chunks_removed += chunk_n;
                }

                removed_budget = removed_budget.saturating_sub(1);

                if merges.len() < REPORT_LIST_CAP {
                    merges.push(CompressMergeAction {
                        action: action_name.into(),
                        keep_id: keep.id.clone(),
                        remove_id: doc.id.clone(),
                        keep_title: Some(keep.title.clone()),
                        remove_title: Some(doc.title.clone()),
                        content_hash: Some(group.content_hash.clone()),
                        cosine: None,
                        remove_layer: Some(doc.layer.to_string()),
                        disposition: disposition.into(),
                        reason,
                    });
                }
            }
        }
    }

    // --- L2: near-dup list (+ merge with confirm) ---
    if level >= 2 {
        actions.push("list_near_dup".into());
        near_duplicates =
            super::analyze::near_duplicate_pairs(store, opts.near_dup_threshold, true)?;

        let apply_near = opts.confirm && !dry_run;
        if apply_near {
            actions.push("merge_near_dup".into());
        } else if !opts.confirm {
            notes.push(
                "level 2 near-dup merge not applied: set confirm=true (and dry_run=false) to merge"
                    .into(),
            );
        } else if dry_run {
            notes.push(
                "level 2 near-dup pairs listed only (dry_run); confirm is set but no mutations"
                    .into(),
            );
        }

        // Build undirected merge plan: greedy, highest cosine first, each id once.
        let mut consumed: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Already removed/tombstoned ids this run (exact phase).
        for m in &merges {
            if m.disposition == "deleted" || m.disposition == "tombstoned" {
                consumed.insert(m.remove_id.clone());
            }
        }

        // A document often appears in several pairs; load each one once.
        let mut loaded: std::collections::HashMap<String, Option<Document>> =
            std::collections::HashMap::new();
        let mut load = |id: &str| -> Result<Option<Document>> {
            if !loaded.contains_key(id) {
                loaded.insert(id.to_string(), store.documents().get_document(id)?);
            }
            Ok(loaded[id].clone())
        };
        for pair in &near_duplicates {
            if consumed.contains(&pair.document_id_a) || consumed.contains(&pair.document_id_b) {
                continue;
            }
            let Some(doc_a) = load(&pair.document_id_a)? else {
                continue;
            };
            let Some(doc_b) = load(&pair.document_id_b)? else {
                continue;
            };
            // Skip if either is already non-active (e.g. tombstoned earlier).
            if is_inactive(&doc_a) || is_inactive(&doc_b) {
                continue;
            }

            let keep = pick_canonical(&[doc_a.clone(), doc_b.clone()]);
            let remove = if keep.id == doc_a.id { &doc_b } else { &doc_a };

            if !apply_near {
                if merges.len() < REPORT_LIST_CAP {
                    merges.push(CompressMergeAction {
                        action: "list_near_dup".into(),
                        keep_id: keep.id.clone(),
                        remove_id: remove.id.clone(),
                        keep_title: Some(keep.title.clone()),
                        remove_title: Some(remove.title.clone()),
                        content_hash: None,
                        cosine: Some(pair.cosine),
                        remove_layer: Some(remove.layer.to_string()),
                        disposition: "listed".into(),
                        reason: Some(format!("cosine={:.3}", pair.cosine)),
                    });
                }
                continue;
            }

            if removed_budget == 0 {
                skipped_cap += 1;
                if merges.len() < REPORT_LIST_CAP {
                    merges.push(CompressMergeAction {
                        action: "merge_near_dup".into(),
                        keep_id: keep.id.clone(),
                        remove_id: remove.id.clone(),
                        keep_title: Some(keep.title.clone()),
                        remove_title: Some(remove.title.clone()),
                        content_hash: None,
                        cosine: Some(pair.cosine),
                        remove_layer: Some(remove.layer.to_string()),
                        disposition: "skipped".into(),
                        reason: Some("max_docs budget exhausted".into()),
                    });
                }
                continue;
            }

            let is_raw = crate::models::Layer::Raw.is(&remove.layer);
            if is_raw && !opts.allow_raw_delete {
                planned_tombstones.push(remove.id.clone());
                docs_tombstoned += 1;
                raw_protected += 1;
                removed_budget = removed_budget.saturating_sub(1);
                consumed.insert(remove.id.clone());
                if merges.len() < REPORT_LIST_CAP {
                    merges.push(CompressMergeAction {
                        action: "tombstone_raw_dup".into(),
                        keep_id: keep.id.clone(),
                        remove_id: remove.id.clone(),
                        keep_title: Some(keep.title.clone()),
                        remove_title: Some(remove.title.clone()),
                        content_hash: None,
                        cosine: Some(pair.cosine),
                        remove_layer: Some(remove.layer.to_string()),
                        disposition: "tombstoned".into(),
                        reason: Some("raw layer protected; allow_raw_delete=false".into()),
                    });
                }
            } else {
                let chunk_n = store.chunks().count_chunks_for_document(&remove.id)?;
                planned_deletes.push(remove.id.clone());
                docs_removed += 1;
                chunks_removed += chunk_n;
                removed_budget = removed_budget.saturating_sub(1);
                consumed.insert(remove.id.clone());
                if merges.len() < REPORT_LIST_CAP {
                    merges.push(CompressMergeAction {
                        action: "merge_near_dup".into(),
                        keep_id: keep.id.clone(),
                        remove_id: remove.id.clone(),
                        keep_title: Some(keep.title.clone()),
                        remove_title: Some(remove.title.clone()),
                        content_hash: None,
                        cosine: Some(pair.cosine),
                        remove_layer: Some(remove.layer.to_string()),
                        disposition: "deleted".into(),
                        reason: Some(format!("cosine={:.3}", pair.cosine)),
                    });
                }
            }
        }
    }

    // Destructive dispositions are planned completely before this point and
    // committed as one transaction. A later target/storage failure therefore
    // cannot leave an invisible partial prefix of deleted documents.
    if !dry_run && (!planned_deletes.is_empty() || !planned_tombstones.is_empty()) {
        let (deleted, tombstoned, deleted_chunks) = store
            .documents()
            .apply_document_dispositions_atomic(&planned_deletes, &planned_tombstones)?;
        if deleted != docs_removed
            || tombstoned != docs_tombstoned
            || deleted_chunks != chunks_removed
        {
            return Err(AppError::db(format!(
                "compression disposition counts changed during atomic apply: planned \
                deleted={docs_removed} tombstoned={docs_tombstoned} chunks={chunks_removed}, \
                applied deleted={deleted} tombstoned={tombstoned} chunks={deleted_chunks}"
            )));
        }
    }

    // Levels 1 and 2 rebuild exactly once after every merge phase. This keeps
    // the terminal generation clean without rebuilding both before and after
    // hard deletes.
    if !dry_run && level >= 1 {
        let durable_mutation_committed = docs_removed > 0 || docs_tombstoned > 0;
        let finalization = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            finalize_compress_fts(store, &config.fts_stemmer)
        }))
        .unwrap_or_else(|_| Err(AppError::fts("maintain_compress FTS finalization panicked")));
        match finalization {
            Ok(fts) => {
                if let Some(report) = l0_report.as_mut() {
                    report.fts_backend = Some(fts.backend.as_str().to_string());
                    report.fts_stemmer = Some(fts.stemmer);
                }
            }
            Err(error) => {
                // Compression work may already be durable, and even a no-op
                // run can encounter pre-existing FTS debt. Preserve the
                // aggregate in both cases, persist a retry marker/audit row,
                // and still append the terminal maintain_compress summary.
                let failure = store.corpus_lane().record_fts_finalization_failure(
                    "maintain_compress",
                    durable_mutation_committed,
                    &error.to_string(),
                );
                errors.push(failure.message);
            }
        }
    }

    // Final checkpoint after mutations so size stats are meaningful.
    if !dry_run && level >= 1 && (docs_removed > 0 || docs_tombstoned > 0) {
        if let Err(error) = store.maintenance().checkpoint() {
            errors.push(format!(
                "CHECKPOINT_FINALIZATION_FAILED retryable=true durable_mutation_committed=true: {error}"
            ));
        }
    }

    let bytes_after = store.maintenance().db_file_size_bytes();
    let bytes_delta = match (bytes_before, bytes_after) {
        (Some(b), Some(a)) => Some(a as i64 - b as i64),
        _ => None,
    };
    let (docs_after, chunks_after) = match store.maintenance().stats() {
        Ok((documents, chunks, _, _)) => (documents, chunks),
        Err(error) => {
            errors.push(format!(
                "POST_MUTATION_STATS_FAILED retryable=true durable_mutation_committed={}: {error}",
                docs_removed > 0 || docs_tombstoned > 0
            ));
            (
                docs_before.saturating_sub(docs_removed),
                chunks_before.saturating_sub(chunks_removed),
            )
        }
    };

    // dry_run planned counts are synthetic; report live stats as after == before.
    let (docs_after_out, chunks_after_out) = if dry_run {
        (docs_before, chunks_before)
    } else {
        (docs_after, chunks_after)
    };

    let mut report = MaintainCompressReport {
        level,
        dry_run,
        confirm: opts.confirm,
        allow_raw_delete: opts.allow_raw_delete,
        max_docs,
        actions: actions
            .into_iter()
            .filter(|a| COMPRESS_ACTION_WHITELIST.contains(&a.as_str()) || a == "list_near_dup")
            .collect(),
        bytes_before,
        bytes_after: if dry_run { bytes_before } else { bytes_after },
        bytes_delta: if dry_run { Some(0) } else { bytes_delta },
        docs_before,
        chunks_before,
        docs_after: docs_after_out,
        chunks_after: chunks_after_out,
        docs_removed,
        docs_tombstoned,
        chunks_removed,
        l0: l0_report,
        exact_dup_groups,
        near_duplicates,
        merges,
        skipped_cap,
        raw_protected,
        errors,
        ops_log_id: None,
        ops_log_seq: None,
        notes,
    };

    if opts.log_ops {
        let message = format!(
            "maintain_compress level={} dry_run={} removed={} tombstoned={} near_pairs={} errors={}",
            level,
            dry_run,
            report.docs_removed,
            report.docs_tombstoned,
            report.near_duplicates.len(),
            report.errors.len()
        );
        // Best-effort audit: the mutation already committed, so a failed
        // summary write is reported instead of failing the run.
        let written = super::audit::record_run(
            store,
            "maintain_compress",
            dry_run,
            "corpus",
            message,
            None,
            &report,
        );
        match written {
            Ok(written) => {
                report.ops_log_id = Some(written.id);
                report.ops_log_seq = Some(written.seq);
            }
            Err(error) => report.errors.push(format!(
                "TERMINAL_AUDIT_FAILED durable_mutation_committed={}: {error}",
                report.docs_removed > 0 || report.docs_tombstoned > 0
            )),
        }
    }

    Ok(report)
}

fn finalize_compress_fts(store: &Store, stemmer: &str) -> Result<db::FtsState> {
    #[cfg(test)]
    db::fts::test_fault::check("compress", stemmer)?;

    store.maintenance().reindex_fts(Some(stemmer))
}

fn is_inactive(doc: &Document) -> bool {
    let s = doc.status.to_ascii_lowercase();
    s == "archived" || s == "tombstone"
}

/// Prefer pinned, then higher boost, then older created_at, then smaller id.
fn pick_canonical(docs: &[Document]) -> Document {
    docs.iter()
        .min_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| {
                    b.boost
                        .partial_cmp(&a.boost)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.id.cmp(&b.id))
        })
        .cloned()
        .expect("pick_canonical on non-empty")
}

fn load_docs_ordered(store: &Store, ids: &[String]) -> Result<Vec<Document>> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(d) = store.documents().get_document(id)? {
            if !is_inactive(&d) {
                out.push(d);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
