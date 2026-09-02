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

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;
use crate::db::{self, Store};
use crate::embeddings::cosine_similarity;
use crate::error::{AppError, Result};
use crate::maintain::analyze::{DuplicateGroup, NearDuplicatePair};
use crate::models::{Document, OpsLogEntry};

/// Highest compression level implemented (L0–L2).
pub const COMPRESS_LEVEL_MAX: u8 = 2;

/// Cap document pairs scanned for near-dup cosine (first-chunk embeddings).
const NEAR_DUP_MAX_DOCS: usize = 200;
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
            max_docs: 50,
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
            return Err(AppError::config(format!(
                "maintain_compress level {} out of range 0..={}",
                self.level, COMPRESS_LEVEL_MAX
            )));
        }
        if self.max_docs == 0 {
            return Err(AppError::config("maintain_compress max_docs must be >= 1"));
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

#[cfg(test)]
const TEST_FINALIZE_FTS_FAILURE_STEMMER: &str = "__test_fail_finalize_fts__";
#[cfg(test)]
const TEST_FINALIZE_FTS_PANIC_STEMMER: &str = "__test_panic_finalize_fts__";

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
        store.require_embedding_manifest_match(config)?;
    }
    let dry_run = opts.dry_run;
    let max_docs = opts.max_docs.max(1);
    let bytes_before = store.db_file_size_bytes();
    let (docs_before, chunks_before, _, _) = store.stats()?;

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
        let status = {
            let conn = store.lock()?;
            db::fts_status(&conn)?
        };
        Some(CompressL0Report {
            checkpointed: false,
            fts_backend: status.as_ref().map(|s| s.backend.as_str().to_string()),
            fts_stemmer: status
                .map(|s| s.stemmer)
                .or_else(|| Some(config.fts_stemmer.clone())),
            dry_run: true,
        })
    } else {
        {
            let conn = store.lock()?;
            conn.execute_batch("CHECKPOINT")?;
        }
        let fts = if level == 0 {
            let conn = store.lock()?;
            Some(db::reindex_with_stemmer(&conn, &config.fts_stemmer)?)
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
        exact_dup_groups = find_exact_duplicate_groups(store)?;

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
                            remove_layer: Some(doc.layer.clone()),
                            disposition: "skipped".into(),
                            reason: Some("max_docs budget exhausted".into()),
                        });
                    }
                    continue;
                }

                let is_raw = doc.layer.eq_ignore_ascii_case("raw");
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
                        let chunk_n = store.count_chunks_for_document(&doc.id)?;
                        planned_deletes.push(doc.id.clone());
                        docs_removed += 1;
                        chunks_removed += chunk_n;
                    }
                } else if is_raw && !opts.allow_raw_delete {
                    raw_protected += 1;
                    docs_tombstoned += 1; // planned
                } else {
                    let chunk_n = store.count_chunks_for_document(&doc.id)?;
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
                        remove_layer: Some(doc.layer.clone()),
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
        near_duplicates = find_near_duplicate_pairs(store, opts.near_dup_threshold)?;

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

        for pair in &near_duplicates {
            if consumed.contains(&pair.document_id_a) || consumed.contains(&pair.document_id_b) {
                continue;
            }
            let Some(doc_a) = store.get_document(&pair.document_id_a)? else {
                continue;
            };
            let Some(doc_b) = store.get_document(&pair.document_id_b)? else {
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
                        remove_layer: Some(remove.layer.clone()),
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
                        remove_layer: Some(remove.layer.clone()),
                        disposition: "skipped".into(),
                        reason: Some("max_docs budget exhausted".into()),
                    });
                }
                continue;
            }

            let is_raw = remove.layer.eq_ignore_ascii_case("raw");
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
                        remove_layer: Some(remove.layer.clone()),
                        disposition: "tombstoned".into(),
                        reason: Some("raw layer protected; allow_raw_delete=false".into()),
                    });
                }
            } else {
                let chunk_n = store.count_chunks_for_document(&remove.id)?;
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
                        remove_layer: Some(remove.layer.clone()),
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
        let (deleted, tombstoned, deleted_chunks) =
            store.apply_document_dispositions_atomic(&planned_deletes, &planned_tombstones)?;
        if deleted != docs_removed
            || tombstoned != docs_tombstoned
            || deleted_chunks != chunks_removed
        {
            return Err(AppError::db(format!(
                "compression disposition counts changed during atomic apply: planned deleted={docs_removed} tombstoned={docs_tombstoned} chunks={chunks_removed}, applied deleted={deleted} tombstoned={tombstoned} chunks={deleted_chunks}"
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
                let failure = store.record_fts_finalization_failure(
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
        let checkpoint = store
            .lock()
            .and_then(|conn| conn.execute_batch("CHECKPOINT").map_err(Into::into));
        if let Err(error) = checkpoint {
            errors.push(format!(
                "CHECKPOINT_FINALIZATION_FAILED retryable=true durable_mutation_committed=true: {error}"
            ));
        }
    }

    let bytes_after = store.db_file_size_bytes();
    let bytes_delta = match (bytes_before, bytes_after) {
        (Some(b), Some(a)) => Some(a as i64 - b as i64),
        _ => None,
    };
    let (docs_after, chunks_after) = match store.stats() {
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
        let op = if dry_run {
            "maintain_compress_dry_run"
        } else {
            "maintain_compress"
        };
        let message = format!(
            "maintain_compress level={} dry_run={} removed={} tombstoned={} near_pairs={} errors={}",
            level,
            dry_run,
            report.docs_removed,
            report.docs_tombstoned,
            report.near_duplicates.len(),
            report.errors.len()
        );
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
        });
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
    if stemmer == TEST_FINALIZE_FTS_FAILURE_STEMMER {
        return Err(AppError::fts("injected final FTS reindex failure"));
    }
    #[cfg(test)]
    if stemmer == TEST_FINALIZE_FTS_PANIC_STEMMER {
        panic!("injected final FTS reindex panic");
    }

    let conn = store.lock()?;
    db::reindex_with_stemmer(&conn, stemmer)
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
        if let Some(d) = store.get_document(id)? {
            if !is_inactive(&d) {
                out.push(d);
            }
        }
    }
    Ok(out)
}

fn find_exact_duplicate_groups(store: &Store) -> Result<Vec<DuplicateGroup>> {
    let conn = store.lock()?;
    let mut stmt = conn.prepare(
        r#"
        SELECT content_hash, COUNT(*)::BIGINT
        FROM documents
        WHERE content_hash IS NOT NULL AND TRIM(content_hash) <> ''
          AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone')
        GROUP BY content_hash
        HAVING COUNT(*) > 1
        ORDER BY COUNT(*) DESC
        LIMIT 50
        "#,
    )?;
    let mut rows = stmt.query([])?;
    let mut hashes: Vec<(String, i64)> = Vec::new();
    while let Some(row) = rows.next()? {
        hashes.push((row.get(0)?, row.get(1)?));
    }
    drop(rows);
    drop(stmt);

    let mut groups = Vec::new();
    for (hash, _n) in hashes {
        let mut stmt = conn.prepare(
            r#"
            SELECT id, title FROM documents
            WHERE content_hash = ?
              AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone')
            ORDER BY created_at ASC
            "#,
        )?;
        let mut rows = stmt.query(duckdb::params![hash.as_str()])?;
        let mut ids = Vec::new();
        let mut titles = Vec::new();
        while let Some(row) = rows.next()? {
            ids.push(row.get::<_, String>(0)?);
            titles.push(row.get::<_, String>(1)?);
        }
        let count = ids.len();
        if count > 1 {
            groups.push(DuplicateGroup {
                content_hash: hash,
                document_ids: ids,
                titles,
                count,
            });
        }
        if groups.len() >= REPORT_LIST_CAP {
            break;
        }
    }
    Ok(groups)
}

fn find_near_duplicate_pairs(store: &Store, threshold: f64) -> Result<Vec<NearDuplicatePair>> {
    let threshold = threshold.clamp(0.0, 1.0) as f32;
    // Keep the advertised cap in the data query rather than materializing the
    // entire vector corpus before truncation.
    let entries =
        store.representative_document_embeddings(NEAR_DUP_MAX_DOCS, true, false, false)?;

    let mut pairs = Vec::new();
    for i in 0..entries.len() {
        for j in (i + 1)..entries.len() {
            let cos = cosine_similarity(&entries[i].embedding, &entries[j].embedding);
            if cos >= threshold {
                pairs.push(NearDuplicatePair {
                    document_id_a: entries[i].document_id.clone(),
                    document_id_b: entries[j].document_id.clone(),
                    title_a: entries[i].title.clone(),
                    title_b: entries[j].title.clone(),
                    cosine: cos,
                });
                if pairs.len() >= REPORT_LIST_CAP {
                    break;
                }
            }
        }
        if pairs.len() >= REPORT_LIST_CAP {
            break;
        }
    }
    pairs.sort_by(|a, b| {
        b.cosine
            .partial_cmp(&a.cosine)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Chunk, SearchMode};
    use crate::util::content_hash;
    use std::path::PathBuf;

    fn test_config(db_path: PathBuf) -> Config {
        Config {
            db_path,
            embedding_provider: crate::config::EmbeddingProviderKind::Mock,
            embedding_base_url: "https://api.openai.com/v1".into(),
            embedding_api_key: String::new(),
            embedding_model: "mock".into(),
            embedding_dims: 8,
            chunk_size: 800,
            chunk_overlap: 120,
            default_top_k: 5,
            ingest_roots: vec![],
            max_context_tokens: 4096,
            max_chunks_per_doc: 3,
            fts_stemmer: "porter".into(),
            default_search_mode: SearchMode::Vec,
            llm_base_url: "http://127.0.0.1:11434/v1".into(),
            llm_provider: crate::llm::LlmProviderKind::Ollama,
            llm_model: "llama3.2".into(),
            llm_api_key: "ollama".into(),
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

    fn seed_doc(store: &Store, id: &str, title: &str, content: &str, layer: &str, emb: Vec<f32>) {
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
                kind: if layer == "wiki" {
                    "wiki".into()
                } else {
                    "document".into()
                },
                content_hash: Some(content_hash(content)),
                ..Default::default()
            })
            .unwrap();
        store
            .insert_chunks(&[Chunk {
                id: format!("{id}-c0"),
                document_id: id.into(),
                chunk_index: 0,
                content: content.into(),
                embedding: emb,
                char_start: 0,
                char_end: content.len() as i32,
                metadata_json: "{}".into(),
            }])
            .unwrap();
    }

    fn assert_finalization_failure_audit(
        store: &Store,
        report: &MaintainCompressReport,
        expected_error: &str,
        expected_durable: bool,
    ) {
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].contains("FTS_FINALIZATION_FAILED"));
        if expected_durable {
            assert!(report.errors[0].contains("durable corpus work was committed"));
        } else {
            assert!(report.errors[0].contains("no durable corpus mutation was detected"));
        }
        assert!(report.errors[0].contains("retryable=true"));
        assert!(report.errors[0].contains(expected_error));

        let entries = store.list_ops_log(10).unwrap();
        let summary_id = report.ops_log_id.as_deref().expect("terminal summary id");
        let summary = entries
            .iter()
            .find(|entry| entry.id == summary_id)
            .expect("terminal summary ops_log row");
        assert_eq!(summary.op, "maintain_compress");
        let summary_payload: serde_json::Value =
            serde_json::from_str(&summary.payload_json).unwrap();
        assert_eq!(summary_payload["errors"].as_array().unwrap().len(), 1);
        assert!(summary_payload["errors"][0]
            .as_str()
            .unwrap()
            .contains(expected_error));

        let failure = entries
            .iter()
            .find(|entry| entry.op == "fts_finalization_failed")
            .expect("durable FTS failure ops_log row");
        let failure_payload: serde_json::Value =
            serde_json::from_str(&failure.payload_json).unwrap();
        assert_eq!(failure_payload["operation"], "maintain_compress");
        assert_eq!(failure_payload["code"], "FTS_FINALIZATION_FAILED");
        assert_eq!(
            failure_payload["durable_mutation_committed"],
            expected_durable
        );
        assert_eq!(failure_payload["retryable"], true);
        assert_eq!(failure_payload["dirty_marker_written"], true);
    }

    #[test]
    fn level_out_of_range_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);
        let mut opts = CompressOptions::from_config(&cfg);
        opts.level = 9;
        assert!(maintain_compress(&store, &cfg, &opts).is_err());
    }

    #[test]
    fn l0_dry_run_and_apply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l0.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);
        seed_doc(&store, "d1", "A", "hello world body", "wiki", vec![0.1; 8]);

        let mut opts = CompressOptions::from_config(&cfg);
        opts.level = 0;
        opts.dry_run = true;
        let dry = maintain_compress(&store, &cfg, &opts).unwrap();
        assert!(dry.dry_run);
        assert!(dry.l0.as_ref().unwrap().dry_run);
        assert_eq!(dry.docs_removed, 0);
        assert!(dry.ops_log_id.is_some());

        opts.dry_run = false;
        let applied = maintain_compress(&store, &cfg, &opts).unwrap();
        assert!(!applied.dry_run);
        assert!(applied.l0.as_ref().unwrap().checkpointed);
        assert!(store
            .list_ops_log(10)
            .unwrap()
            .iter()
            .any(|o| o.op == "maintain_compress"));
    }

    #[test]
    fn l1_merges_exact_hash_wiki_keeps_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l1.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);
        let body = "identical wiki body for compress";
        seed_doc(&store, "w1", "Keep", body, "wiki", vec![0.2; 8]);
        // slight delay not needed; ids order: pick_canonical uses created_at then id
        seed_doc(&store, "w2", "Dup", body, "wiki", vec![0.2; 8]);

        let mut opts = CompressOptions::from_config(&cfg);
        opts.level = 1;
        opts.dry_run = false;
        opts.allow_raw_delete = false;
        let r = maintain_compress(&store, &cfg, &opts).unwrap();
        assert_eq!(r.docs_removed, 1);
        assert_eq!(r.exact_dup_groups.len(), 1);
        let remaining = store.list_documents().unwrap();
        assert_eq!(remaining.len(), 1);
        // oldest / smaller id kept
        assert_eq!(remaining[0].id, "w1");
        let generation_after_compress = {
            let conn = store.lock().unwrap();
            crate::db::fts::fts_generation_state(&conn).unwrap()
        };
        assert!(!generation_after_compress.dirty);
        assert_eq!(
            generation_after_compress.index_generation,
            Some(generation_after_compress.chunks_generation)
        );

        let hits = crate::db::search::search(
            &store,
            &crate::db::search::SearchQuery {
                mode: SearchMode::Lex,
                query_text: Some("identical wiki".into()),
                top_k: 5,
                fts_stemmer: cfg.fts_stemmer.clone(),
                ..crate::db::search::SearchQuery::default()
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        let generation_after_search = {
            let conn = store.lock().unwrap();
            crate::db::fts::fts_generation_state(&conn).unwrap()
        };
        assert_eq!(generation_after_search, generation_after_compress);
    }

    #[test]
    fn l1_raw_dups_tombstone_without_allow_raw_delete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("raw.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);
        let body = "raw twin content";
        seed_doc(&store, "r1", "RawA", body, "raw", vec![0.3; 8]);
        seed_doc(&store, "r2", "RawB", body, "raw", vec![0.3; 8]);

        let mut opts = CompressOptions::from_config(&cfg);
        opts.level = 1;
        opts.dry_run = false;
        opts.allow_raw_delete = false;
        let r = maintain_compress(&store, &cfg, &opts).unwrap();
        assert_eq!(r.docs_removed, 0);
        assert_eq!(r.docs_tombstoned, 1);
        assert!(r.raw_protected >= 1);
        assert!(store.get_document("r1").unwrap().is_some());
        assert!(store.get_document("r2").unwrap().is_some());
        let statuses: Vec<_> = store
            .list_documents()
            .unwrap()
            .into_iter()
            .map(|d| (d.id, d.status))
            .collect();
        // one active, one tombstone
        let active = statuses.iter().filter(|(_, s)| s != "tombstone").count();
        let tomb = statuses.iter().filter(|(_, s)| s == "tombstone").count();
        assert_eq!(active, 1);
        assert_eq!(tomb, 1);
    }

    #[test]
    fn l1_raw_hard_delete_with_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rawdel.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);
        let body = "raw twin hard delete";
        seed_doc(&store, "r1", "RawA", body, "raw", vec![0.3; 8]);
        seed_doc(&store, "r2", "RawB", body, "raw", vec![0.3; 8]);

        let mut opts = CompressOptions::from_config(&cfg);
        opts.level = 1;
        opts.dry_run = false;
        opts.allow_raw_delete = true;
        let r = maintain_compress(&store, &cfg, &opts).unwrap();
        assert_eq!(r.docs_removed, 1);
        assert_eq!(store.list_documents().unwrap().len(), 1);
    }

    #[test]
    fn l2_lists_near_dups_without_confirm() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l2list.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);
        // Same embedding → cosine 1.0
        let emb = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        seed_doc(
            &store,
            "a",
            "Alpha",
            "body alpha unique",
            "wiki",
            emb.clone(),
        );
        seed_doc(&store, "b", "Beta", "body beta unique", "wiki", emb);
        store.write_embedding_manifest_from_config(&cfg).unwrap();

        let mut opts = CompressOptions::from_config(&cfg);
        opts.level = 2;
        opts.dry_run = false;
        opts.confirm = false;
        opts.near_dup_threshold = 0.9;
        let r = maintain_compress(&store, &cfg, &opts).unwrap();
        assert!(!r.near_duplicates.is_empty());
        assert_eq!(r.docs_removed, 0);
        assert!(r.notes.iter().any(|n| n.contains("confirm")));
        assert_eq!(store.list_documents().unwrap().len(), 2);
        assert!(r.merges.iter().any(|m| m.disposition == "listed"));
    }

    #[test]
    fn l2_merges_near_dups_with_confirm() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l2merge.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);
        let emb = vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        seed_doc(
            &store,
            "a",
            "Alpha",
            "body alpha unique",
            "wiki",
            emb.clone(),
        );
        seed_doc(&store, "b", "Beta", "body beta unique", "wiki", emb);
        store.write_embedding_manifest_from_config(&cfg).unwrap();

        let mut opts = CompressOptions::from_config(&cfg);
        opts.level = 2;
        opts.dry_run = false;
        opts.confirm = true;
        opts.near_dup_threshold = 0.9;
        let r = maintain_compress(&store, &cfg, &opts).unwrap();
        assert_eq!(r.docs_removed, 1);
        assert_eq!(store.list_documents().unwrap().len(), 1);
        assert!(r.merges.iter().any(|m| m.action == "merge_near_dup"));
    }

    #[test]
    fn l2_apply_fails_before_any_mutation_during_embedding_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l2-migration-incomplete.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);
        let emb = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        seed_doc(
            &store,
            "keep",
            "Keep",
            "unique body one",
            "wiki",
            emb.clone(),
        );
        seed_doc(&store, "drop", "Drop", "unique body two", "wiki", emb);
        let migration = crate::db::store::embedding_migration_manifest(&cfg);
        store.set_embedding_manifest(&migration).unwrap();
        let stats_before = store.stats().unwrap();
        let documents_before = serde_json::to_value(store.list_documents().unwrap()).unwrap();
        let chunks_before =
            serde_json::to_value(store.all_chunks_with_embeddings().unwrap()).unwrap();
        assert!(store.list_ops_log(10).unwrap().is_empty());

        let mut opts = CompressOptions::from_config(&cfg);
        opts.level = 2;
        opts.dry_run = false;
        opts.confirm = true;
        opts.allow_raw_delete = true;
        opts.near_dup_threshold = 0.9;
        let error = maintain_compress(&store, &cfg, &opts).unwrap_err();

        assert!(matches!(error, AppError::Embeddings(_)));
        assert!(error.to_string().contains("incomplete corpus migration"));
        assert_eq!(store.stats().unwrap(), stats_before);
        assert_eq!(
            serde_json::to_value(store.list_documents().unwrap()).unwrap(),
            documents_before
        );
        assert_eq!(
            serde_json::to_value(store.all_chunks_with_embeddings().unwrap()).unwrap(),
            chunks_before
        );
        assert!(store.list_ops_log(10).unwrap().is_empty());
        assert_eq!(
            serde_json::to_value(store.get_embedding_manifest().unwrap()).unwrap(),
            serde_json::to_value(Some(migration)).unwrap()
        );
    }

    #[test]
    fn l1_dry_run_does_not_delete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l1dry.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);
        let body = "dup dry run body";
        seed_doc(&store, "w1", "A", body, "wiki", vec![0.1; 8]);
        seed_doc(&store, "w2", "B", body, "wiki", vec![0.1; 8]);

        let mut opts = CompressOptions::from_config(&cfg);
        opts.level = 1;
        opts.dry_run = true;
        let r = maintain_compress(&store, &cfg, &opts).unwrap();
        assert!(r.dry_run);
        assert_eq!(r.docs_removed, 1); // planned
        assert_eq!(store.list_documents().unwrap().len(), 2);
        assert!(r.merges.iter().any(|m| m.disposition == "planned"));
        assert!(store
            .list_ops_log(5)
            .unwrap()
            .iter()
            .any(|o| o.op == "maintain_compress_dry_run"));
    }

    #[test]
    fn final_fts_failure_returns_durable_merge_report_and_terminal_ops_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("final-fts-failure.duckdb");
        let store = Store::open(&path).unwrap();
        let mut cfg = test_config(path);
        let body = "same durable compression body";
        seed_doc(&store, "keep", "Keep", body, "wiki", vec![0.2; 8]);
        seed_doc(&store, "drop", "Drop", body, "wiki", vec![0.2; 8]);
        store.ensure_fts(&cfg.fts_stemmer).unwrap();
        cfg.fts_stemmer = TEST_FINALIZE_FTS_FAILURE_STEMMER.into();

        let mut opts = CompressOptions::from_config(&cfg);
        opts.level = 1;
        opts.dry_run = false;
        let report = maintain_compress(&store, &cfg, &opts)
            .expect("aggregate report survives final FTS failure");

        assert_eq!(report.docs_removed, 1);
        assert_eq!(store.list_documents().unwrap().len(), 1);
        assert_finalization_failure_audit(
            &store,
            &report,
            "injected final FTS reindex failure",
            true,
        );
        let generation = {
            let conn = store.lock().unwrap();
            db::fts_generation_state(&conn).unwrap()
        };
        assert!(generation.dirty, "next lexical read must retry FTS");

        let mut legacy_payload = serde_json::to_value(&report).unwrap();
        legacy_payload.as_object_mut().unwrap().remove("errors");
        let decoded: MaintainCompressReport = serde_json::from_value(legacy_payload).unwrap();
        assert!(decoded.errors.is_empty());
    }

    #[test]
    fn final_fts_panic_returns_durable_merge_report_and_terminal_ops_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("final-fts-panic.duckdb");
        let store = Store::open(&path).unwrap();
        let mut cfg = test_config(path);
        let body = "same durable compression panic body";
        seed_doc(&store, "keep", "Keep", body, "wiki", vec![0.2; 8]);
        seed_doc(&store, "drop", "Drop", body, "wiki", vec![0.2; 8]);
        store.ensure_fts(&cfg.fts_stemmer).unwrap();
        cfg.fts_stemmer = TEST_FINALIZE_FTS_PANIC_STEMMER.into();

        let mut opts = CompressOptions::from_config(&cfg);
        opts.level = 1;
        opts.dry_run = false;
        let report = maintain_compress(&store, &cfg, &opts)
            .expect("aggregate report survives final FTS panic");

        assert_eq!(report.docs_removed, 1);
        assert_eq!(store.list_documents().unwrap().len(), 1);
        assert_finalization_failure_audit(
            &store,
            &report,
            "maintain_compress FTS finalization panicked",
            true,
        );
        let generation = {
            let conn = store.lock().unwrap();
            db::fts_generation_state(&conn).unwrap()
        };
        assert!(generation.dirty, "next lexical read must retry FTS");
    }

    #[test]
    fn final_fts_panic_without_merge_preserves_noop_report_and_audit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("final-fts-panic-noop.duckdb");
        let store = Store::open(&path).unwrap();
        let mut cfg = test_config(path);
        seed_doc(
            &store,
            "only",
            "Only",
            "unique compression body",
            "wiki",
            vec![0.2; 8],
        );
        store.ensure_fts(&cfg.fts_stemmer).unwrap();
        cfg.fts_stemmer = TEST_FINALIZE_FTS_PANIC_STEMMER.into();

        let mut opts = CompressOptions::from_config(&cfg);
        opts.level = 1;
        opts.dry_run = false;
        let report = maintain_compress(&store, &cfg, &opts)
            .expect("no-op aggregate survives final FTS panic");

        assert_eq!(report.docs_removed, 0);
        assert_eq!(report.docs_tombstoned, 0);
        assert_eq!(store.list_documents().unwrap().len(), 1);
        assert_finalization_failure_audit(
            &store,
            &report,
            "maintain_compress FTS finalization panicked",
            false,
        );
    }
}
