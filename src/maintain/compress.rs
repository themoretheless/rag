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
///
/// Phases: guard → L0 checkpoint/FTS → L1 exact merge plan → L2 near-dup
/// plan → one atomic disposition commit → FTS/checkpoint finalization →
/// after-stats → report and audit row.
pub fn maintain_compress(
    store: &Store,
    config: &Config,
    opts: &CompressOptions,
) -> Result<MaintainCompressReport> {
    opts.validate()?;

    // L2 decisions compare stored vectors and may delete or tombstone documents.
    // Refuse the entire workflow before CHECKPOINT, L1 merges, or ops-log writes
    // when a corpus migration is incomplete or the runtime identity differs.
    if opts.level >= 2 {
        store
            .embedding_manifest()
            .require_embedding_manifest_match(config)?;
    }
    let mut run = CompressRun::start(store, config, opts)?;
    run.checkpoint_and_fts()?;
    if opts.level >= 1 {
        run.plan_exact_merges()?;
    }
    if opts.level >= 2 {
        run.plan_near_merges()?;
    }
    run.commit_dispositions()?;
    run.finalize();
    let mut report = run.into_report();
    if opts.log_ops {
        record_compress_audit(store, &mut report);
    }
    Ok(report)
}

/// Mutable state of one `maintain_compress` run, shared by its phases.
struct CompressRun<'a> {
    store: &'a Store,
    config: &'a Config,
    opts: &'a CompressOptions,
    dry_run: bool,
    max_docs: usize,
    bytes_before: Option<u64>,
    docs_before: u64,
    chunks_before: u64,
    actions: Vec<String>,
    merges: Vec<CompressMergeAction>,
    notes: Vec<String>,
    docs_removed: u64,
    docs_tombstoned: u64,
    chunks_removed: u64,
    removed_budget: usize,
    skipped_cap: usize,
    raw_protected: usize,
    errors: Vec<String>,
    exact_dup_groups: Vec<DuplicateGroup>,
    near_duplicates: Vec<NearDuplicatePair>,
    planned_deletes: Vec<String>,
    planned_tombstones: Vec<String>,
    l0_report: Option<CompressL0Report>,
}

impl<'a> CompressRun<'a> {
    fn start(store: &'a Store, config: &'a Config, opts: &'a CompressOptions) -> Result<Self> {
        let max_docs = opts.max_docs.max(1);
        let bytes_before = store.maintenance().db_file_size_bytes();
        let (docs_before, chunks_before, _, _) = store.maintenance().stats()?;
        Ok(Self {
            store,
            config,
            opts,
            dry_run: opts.dry_run,
            max_docs,
            bytes_before,
            docs_before,
            chunks_before,
            actions: Vec::new(),
            merges: Vec::new(),
            notes: Vec::new(),
            docs_removed: 0,
            docs_tombstoned: 0,
            chunks_removed: 0,
            removed_budget: max_docs,
            skipped_cap: 0,
            raw_protected: 0,
            errors: Vec::new(),
            exact_dup_groups: Vec::new(),
            near_duplicates: Vec::new(),
            planned_deletes: Vec::new(),
            planned_tombstones: Vec::new(),
            l0_report: None,
        })
    }

    fn durable_mutation(&self) -> bool {
        self.docs_removed > 0 || self.docs_tombstoned > 0
    }

    fn push_merge(&mut self, merge: CompressMergeAction) {
        if self.merges.len() < REPORT_LIST_CAP {
            self.merges.push(merge);
        }
    }

    /// L0: checkpoint + FTS (level 0 reindexes now; 1 and 2 after merging).
    fn checkpoint_and_fts(&mut self) -> Result<()> {
        let (store, config) = (self.store, self.config);
        self.actions.push("checkpoint".into());
        self.actions.push("reindex_fts".into());
        self.l0_report = if self.dry_run {
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
            let fts = if self.opts.level == 0 {
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
        Ok(())
    }

    /// L1: plan exact `content_hash` merges (keep the canonical document).
    fn plan_exact_merges(&mut self) -> Result<()> {
        let store = self.store;
        let dry_run = self.dry_run;
        let allow_raw_delete = self.opts.allow_raw_delete;
        self.actions.push("merge_exact_dup".into());
        self.exact_dup_groups = crate::db::analytics::find_exact_duplicates(store, true)?;
        let groups = self.exact_dup_groups.clone();

        for group in &groups {
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
                let merge = |action: &str, disposition: &str, reason: Option<String>| {
                    CompressMergeAction {
                        action: action.into(),
                        keep_id: keep.id.clone(),
                        remove_id: doc.id.clone(),
                        keep_title: Some(keep.title.clone()),
                        remove_title: Some(doc.title.clone()),
                        content_hash: Some(group.content_hash.clone()),
                        cosine: None,
                        remove_layer: Some(doc.layer.to_string()),
                        disposition: disposition.into(),
                        reason,
                    }
                };
                if self.removed_budget == 0 {
                    self.skipped_cap += 1;
                    self.push_merge(merge(
                        "merge_exact_dup",
                        "skipped",
                        Some("max_docs budget exhausted".into()),
                    ));
                    continue;
                }

                let is_raw = crate::models::Layer::Raw.is(&doc.layer);
                let (action_name, disposition, reason) = if is_raw && !allow_raw_delete {
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
                        self.planned_tombstones.push(doc.id.clone());
                        self.docs_tombstoned += 1;
                        self.raw_protected += 1;
                        // Chunks stay; no chunk removal.
                    } else {
                        let chunk_n = store.chunks().count_chunks_for_document(&doc.id)?;
                        self.planned_deletes.push(doc.id.clone());
                        self.docs_removed += 1;
                        self.chunks_removed += chunk_n;
                    }
                } else if is_raw && !allow_raw_delete {
                    self.raw_protected += 1;
                    self.docs_tombstoned += 1; // planned
                } else {
                    let chunk_n = store.chunks().count_chunks_for_document(&doc.id)?;
                    self.docs_removed += 1; // planned
                    self.chunks_removed += chunk_n;
                }

                self.removed_budget = self.removed_budget.saturating_sub(1);
                self.push_merge(merge(action_name, disposition, reason));
            }
        }
        Ok(())
    }

    /// L2: list near-duplicate pairs; plan merges only with `confirm` and not
    /// `dry_run`.
    fn plan_near_merges(&mut self) -> Result<()> {
        let store = self.store;
        let opts = self.opts;
        self.actions.push("list_near_dup".into());
        self.near_duplicates =
            super::analyze::near_duplicate_pairs(store, opts.near_dup_threshold, true)?;

        let apply_near = opts.confirm && !self.dry_run;
        if apply_near {
            self.actions.push("merge_near_dup".into());
        } else if !opts.confirm {
            self.notes.push(
                "level 2 near-dup merge not applied: set confirm=true (and dry_run=false) to merge"
                    .into(),
            );
        } else if self.dry_run {
            self.notes.push(
                "level 2 near-dup pairs listed only (dry_run); confirm is set but no mutations"
                    .into(),
            );
        }

        // Build undirected merge plan: greedy, highest cosine first, each id once.
        let mut consumed: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Already removed/tombstoned ids this run (exact phase).
        for m in &self.merges {
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
        let pairs = self.near_duplicates.clone();
        for pair in &pairs {
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
            let merge = |action: &str, disposition: &str, reason: Option<String>| {
                CompressMergeAction {
                    action: action.into(),
                    keep_id: keep.id.clone(),
                    remove_id: remove.id.clone(),
                    keep_title: Some(keep.title.clone()),
                    remove_title: Some(remove.title.clone()),
                    content_hash: None,
                    cosine: Some(pair.cosine),
                    remove_layer: Some(remove.layer.to_string()),
                    disposition: disposition.into(),
                    reason,
                }
            };

            if !apply_near {
                self.push_merge(merge(
                    "list_near_dup",
                    "listed",
                    Some(format!("cosine={:.3}", pair.cosine)),
                ));
                continue;
            }

            if self.removed_budget == 0 {
                self.skipped_cap += 1;
                self.push_merge(merge(
                    "merge_near_dup",
                    "skipped",
                    Some("max_docs budget exhausted".into()),
                ));
                continue;
            }

            let is_raw = crate::models::Layer::Raw.is(&remove.layer);
            if is_raw && !opts.allow_raw_delete {
                self.planned_tombstones.push(remove.id.clone());
                self.docs_tombstoned += 1;
                self.raw_protected += 1;
                self.removed_budget = self.removed_budget.saturating_sub(1);
                consumed.insert(remove.id.clone());
                self.push_merge(merge(
                    "tombstone_raw_dup",
                    "tombstoned",
                    Some("raw layer protected; allow_raw_delete=false".into()),
                ));
            } else {
                let chunk_n = store.chunks().count_chunks_for_document(&remove.id)?;
                self.planned_deletes.push(remove.id.clone());
                self.docs_removed += 1;
                self.chunks_removed += chunk_n;
                self.removed_budget = self.removed_budget.saturating_sub(1);
                consumed.insert(remove.id.clone());
                self.push_merge(merge(
                    "merge_near_dup",
                    "deleted",
                    Some(format!("cosine={:.3}", pair.cosine)),
                ));
            }
        }
        Ok(())
    }

    /// Commit every planned disposition in one transaction.
    ///
    /// Destructive dispositions are planned completely before this point. A
    /// later target/storage failure therefore cannot leave an invisible
    /// partial prefix of deleted documents.
    fn commit_dispositions(&mut self) -> Result<()> {
        if self.dry_run || (self.planned_deletes.is_empty() && self.planned_tombstones.is_empty())
        {
            return Ok(());
        }
        let (deleted, tombstoned, deleted_chunks) = self
            .store
            .documents()
            .apply_document_dispositions_atomic(&self.planned_deletes, &self.planned_tombstones)?;
        let (docs_removed, docs_tombstoned, chunks_removed) =
            (self.docs_removed, self.docs_tombstoned, self.chunks_removed);
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
        Ok(())
    }

    /// FTS rebuild (levels 1 and 2, exactly once after every merge phase) and
    /// the final checkpoint; failures are additive report errors.
    fn finalize(&mut self) {
        let store = self.store;
        if self.dry_run || self.opts.level < 1 {
            return;
        }
        let durable_mutation_committed = self.durable_mutation();
        let finalization = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            finalize_compress_fts(store, &self.config.fts_stemmer)
        }))
        .unwrap_or_else(|_| Err(AppError::fts("maintain_compress FTS finalization panicked")));
        match finalization {
            Ok(fts) => {
                if let Some(report) = self.l0_report.as_mut() {
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
                self.errors.push(failure.message);
            }
        }

        // Final checkpoint after mutations so size stats are meaningful.
        if durable_mutation_committed {
            if let Err(error) = store.maintenance().checkpoint() {
                self.errors.push(format!(
                    "CHECKPOINT_FINALIZATION_FAILED retryable=true durable_mutation_committed=true: {error}"
                ));
            }
        }
    }

    /// Read after-stats and assemble the report (dry runs report after == before).
    fn into_report(mut self) -> MaintainCompressReport {
        let store = self.store;
        let dry_run = self.dry_run;
        let bytes_before = self.bytes_before;
        let bytes_after = store.maintenance().db_file_size_bytes();
        let bytes_delta = match (bytes_before, bytes_after) {
            (Some(b), Some(a)) => Some(a as i64 - b as i64),
            _ => None,
        };
        let (docs_after, chunks_after) = match store.maintenance().stats() {
            Ok((documents, chunks, _, _)) => (documents, chunks),
            Err(error) => {
                self.errors.push(format!(
                    "POST_MUTATION_STATS_FAILED retryable=true durable_mutation_committed={}: {error}",
                    self.durable_mutation()
                ));
                (
                    self.docs_before.saturating_sub(self.docs_removed),
                    self.chunks_before.saturating_sub(self.chunks_removed),
                )
            }
        };

        // dry_run planned counts are synthetic; report live stats as after == before.
        let (docs_after_out, chunks_after_out) = if dry_run {
            (self.docs_before, self.chunks_before)
        } else {
            (docs_after, chunks_after)
        };

        let opts = self.opts;
        MaintainCompressReport {
            level: opts.level,
            dry_run,
            confirm: opts.confirm,
            allow_raw_delete: opts.allow_raw_delete,
            max_docs: self.max_docs,
            actions: self
                .actions
                .into_iter()
                .filter(|a| COMPRESS_ACTION_WHITELIST.contains(&a.as_str()) || a == "list_near_dup")
                .collect(),
            bytes_before,
            bytes_after: if dry_run { bytes_before } else { bytes_after },
            bytes_delta: if dry_run { Some(0) } else { bytes_delta },
            docs_before: self.docs_before,
            chunks_before: self.chunks_before,
            docs_after: docs_after_out,
            chunks_after: chunks_after_out,
            docs_removed: self.docs_removed,
            docs_tombstoned: self.docs_tombstoned,
            chunks_removed: self.chunks_removed,
            l0: self.l0_report,
            exact_dup_groups: self.exact_dup_groups,
            near_duplicates: self.near_duplicates,
            merges: self.merges,
            skipped_cap: self.skipped_cap,
            raw_protected: self.raw_protected,
            errors: self.errors,
            ops_log_id: None,
            ops_log_seq: None,
            notes: self.notes,
        }
    }
}

/// Best-effort terminal audit row: the mutation already committed, so a
/// failed summary write is reported instead of failing the run.
fn record_compress_audit(store: &Store, report: &mut MaintainCompressReport) {
    let message = format!(
        "maintain_compress level={} dry_run={} removed={} tombstoned={} near_pairs={} errors={}",
        report.level,
        report.dry_run,
        report.docs_removed,
        report.docs_tombstoned,
        report.near_duplicates.len(),
        report.errors.len()
    );
    let written = super::audit::record_run(
        store,
        "maintain_compress",
        report.dry_run,
        "corpus",
        message,
        None,
        &*report,
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
