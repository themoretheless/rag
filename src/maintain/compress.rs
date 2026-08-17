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
use crate::models::{Document, DocumentMetaUpdate, OpsLogEntry};
use crate::maintain::analyze::{DuplicateGroup, NearDuplicatePair};

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
            return Err(AppError::config(
                "maintain_compress max_docs must be >= 1",
            ));
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
    let mut exact_dup_groups: Vec<DuplicateGroup> = Vec::new();
    let mut near_duplicates: Vec<NearDuplicatePair> = Vec::new();
    // --- L0: checkpoint + FTS ---
    actions.push("checkpoint".into());
    actions.push("reindex_fts".into());
    let l0_report = if dry_run {
        let status = {
            let conn = store.lock()?;
            db::fts_status(&conn)?
        };
        Some(CompressL0Report {
            checkpointed: false,
            fts_backend: status
                .as_ref()
                .map(|s| s.backend.as_str().to_string()),
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
        let fts = {
            let conn = store.lock()?;
            db::reindex_with_stemmer(&conn, &config.fts_stemmer)?
        };
        Some(CompressL0Report {
            checkpointed: true,
            fts_backend: Some(fts.backend.as_str().to_string()),
            fts_stemmer: Some(fts.stemmer),
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
                    let chunk_n = store.list_chunks_for_document(&doc.id)?.len() as u64;
                    if action_name == "tombstone_raw_dup" {
                        tombstone_document(store, &doc.id)?;
                        docs_tombstoned += 1;
                        raw_protected += 1;
                        // Chunks stay; no chunk removal.
                    } else {
                        store.delete_document(&doc.id)?;
                        cleanup_wiki_index_for_doc(store, doc)?;
                        docs_removed += 1;
                        chunks_removed += chunk_n;
                    }
                } else if is_raw && !opts.allow_raw_delete {
                    raw_protected += 1;
                    docs_tombstoned += 1; // planned
                } else {
                    let chunk_n = store.list_chunks_for_document(&doc.id)?.len() as u64;
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
        near_duplicates =
            find_near_duplicate_pairs(store, opts.near_dup_threshold)?;

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
        let mut consumed: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // Already removed/tombstoned ids this run (exact phase).
        for m in &merges {
            if m.disposition == "deleted" || m.disposition == "tombstoned" {
                consumed.insert(m.remove_id.clone());
            }
        }

        for pair in &near_duplicates {
            if consumed.contains(&pair.document_id_a) || consumed.contains(&pair.document_id_b)
            {
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
                tombstone_document(store, &remove.id)?;
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
                let chunk_n = store.list_chunks_for_document(&remove.id)?.len() as u64;
                store.delete_document(&remove.id)?;
                cleanup_wiki_index_for_doc(store, remove)?;
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

        // After bulk deletes, reindex FTS again so lex stays consistent.
        if apply_near && (docs_removed > 0 || docs_tombstoned > 0) && !dry_run {
            let conn = store.lock()?;
            let _ = db::reindex_with_stemmer(&conn, &config.fts_stemmer)?;
        }
    }

    // Final checkpoint after mutations so size stats are meaningful.
    if !dry_run && level >= 1 && (docs_removed > 0 || docs_tombstoned > 0) {
        let conn = store.lock()?;
        conn.execute_batch("CHECKPOINT")?;
    }

    let bytes_after = store.db_file_size_bytes();
    let bytes_delta = match (bytes_before, bytes_after) {
        (Some(b), Some(a)) => Some(a as i64 - b as i64),
        _ => None,
    };
    let (docs_after, chunks_after, _, _) = store.stats()?;

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
            "maintain_compress level={} dry_run={} removed={} tombstoned={} near_pairs={}",
            level,
            dry_run,
            report.docs_removed,
            report.docs_tombstoned,
            report.near_duplicates.len()
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
        })?;
        report.ops_log_id = Some(written.id);
        report.ops_log_seq = Some(written.seq);
    }

    Ok(report)
}

fn is_inactive(doc: &Document) -> bool {
    let s = doc.status.to_ascii_lowercase();
    s == "archived" || s == "tombstone"
}

fn tombstone_document(store: &Store, id: &str) -> Result<()> {
    let update = DocumentMetaUpdate {
        status: Some("tombstone".into()),
        ..Default::default()
    };
    store
        .update_document_meta(id, &update)?
        .ok_or_else(|| AppError::not_found(format!("document not found for tombstone: {id}")))?;
    Ok(())
}

fn cleanup_wiki_index_for_doc(store: &Store, doc: &Document) -> Result<()> {
    if !doc.layer.eq_ignore_ascii_case("wiki") {
        return Ok(());
    }
    // Best-effort: drop catalog rows pointing at this page id.
    let entries = store.list_wiki_index()?;
    for e in entries {
        let page_match = e.page_id.as_deref() == Some(doc.id.as_str());
        let id_match = e.id == doc.id;
        if page_match || id_match {
            let _ = store.delete_wiki_index_entry(&e.id);
        }
    }
    Ok(())
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
    use std::collections::{BTreeMap, HashMap};

    let threshold = threshold.clamp(0.0, 1.0) as f32;
    let chunks = store.all_chunks_with_embeddings()?;
    let docs = store.list_documents()?;
    let title_by_id: HashMap<String, String> = docs
        .iter()
        .filter(|d| !is_inactive(d))
        .map(|d| (d.id.clone(), d.title.clone()))
        .collect();
    let active: std::collections::HashSet<String> = title_by_id.keys().cloned().collect();

    let mut first_chunk: BTreeMap<String, Vec<f32>> = BTreeMap::new();
    for c in chunks {
        if !active.contains(&c.document_id) {
            continue;
        }
        first_chunk
            .entry(c.document_id)
            .or_insert_with(|| c.embedding);
    }

    let mut entries: Vec<(String, Vec<f32>)> = first_chunk.into_iter().collect();
    if entries.len() > NEAR_DUP_MAX_DOCS {
        entries.truncate(NEAR_DUP_MAX_DOCS);
    }

    let mut pairs = Vec::new();
    for i in 0..entries.len() {
        for j in (i + 1)..entries.len() {
            let cos = cosine_similarity(&entries[i].1, &entries[j].1);
            if cos >= threshold {
                let id_a = &entries[i].0;
                let id_b = &entries[j].0;
                pairs.push(NearDuplicatePair {
                    document_id_a: id_a.clone(),
                    document_id_b: id_b.clone(),
                    title_a: title_by_id.get(id_a).cloned().unwrap_or_default(),
                    title_b: title_by_id.get(id_b).cloned().unwrap_or_default(),
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
            }])
            .unwrap();
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
        seed_doc(
            &store,
            "d1",
            "A",
            "hello world body",
            "wiki",
            vec![0.1; 8],
        );

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
        let active = statuses
            .iter()
            .filter(|(_, s)| s != "tombstone")
            .count();
        let tomb = statuses
            .iter()
            .filter(|(_, s)| s == "tombstone")
            .count();
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
        seed_doc(&store, "a", "Alpha", "body alpha unique", "wiki", emb.clone());
        seed_doc(&store, "b", "Beta", "body beta unique", "wiki", emb);

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
        seed_doc(&store, "a", "Alpha", "body alpha unique", "wiki", emb.clone());
        seed_doc(&store, "b", "Beta", "body beta unique", "wiki", emb);

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
        assert!(r
            .merges
            .iter()
            .any(|m| m.disposition == "planned"));
        assert!(store
            .list_ops_log(5)
            .unwrap()
            .iter()
            .any(|o| o.op == "maintain_compress_dry_run"));
    }
}
