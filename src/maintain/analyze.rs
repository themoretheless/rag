//! Deterministic corpus analysis (no LLM).
//!
//! MCP tool: `analyze_corpus` → [`AnalysisReport`] JSON.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use duckdb::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;
use crate::db::schema::SCHEMA_VERSION;
use crate::db::Store;
use crate::embeddings::cosine_similarity;
use crate::error::Result;
use crate::models::{DoctorReport, OpsLogEntry};

/// Default age (days) after which an unresolved stub is "aging".
const DEFAULT_STUB_AGE_DAYS: u32 = 14;
/// Default age (days) for low-boost archive candidates.
const DEFAULT_ARCHIVE_MIN_AGE_DAYS: u32 = 90;
/// Cap document pairs scanned for near-dup cosine (first-chunk embeddings).
const NEAR_DUP_MAX_DOCS: usize = 200;
/// Cap reported groups / lists to keep MCP payloads bounded.
const REPORT_LIST_CAP: usize = 100;

/// Tunables for [`analyze_corpus`].
#[derive(Debug, Clone)]
pub struct AnalyzeOptions {
    /// Unresolved stubs older than this many days are `aging_stubs`.
    pub stub_age_days: u32,
    /// When true, compare first-chunk embeddings across docs (capped).
    pub include_near_dups: bool,
    /// Cosine threshold θ for near-dups (from config when unset by caller).
    pub near_dup_threshold: f64,
    /// Active, unpinned, low-boost docs older than this become archive candidates.
    pub archive_min_age_days: u32,
    /// When true, append an `analyze_corpus` row to `ops_log`.
    pub log_ops: bool,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self {
            stub_age_days: DEFAULT_STUB_AGE_DAYS,
            include_near_dups: false,
            near_dup_threshold: 0.92,
            archive_min_age_days: DEFAULT_ARCHIVE_MIN_AGE_DAYS,
            log_ops: true,
        }
    }
}

impl AnalyzeOptions {
    /// Build options from runtime config (near-dup θ) and optional overrides.
    pub fn from_config(config: &Config) -> Self {
        Self {
            near_dup_threshold: config.maint_near_dup_threshold,
            ..Self::default()
        }
    }
}

/// Structured corpus health report for `analyze_corpus`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisReport {
    pub generated_at: DateTime<Utc>,
    pub db_path: String,
    /// On-disk size of the DuckDB file when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_size_bytes: Option<u64>,
    pub counts: CorpusCounts,
    pub size: SizeStats,
    /// Subset of doctor integrity signals.
    pub doctor: DoctorReport,
    pub fts_ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed_mismatch: Option<EmbedMismatchInfo>,
    /// Same `content_hash` on 2+ documents.
    pub exact_duplicates: Vec<DuplicateGroup>,
    /// Embedding cosine ≥ θ (only when `include_near_dups`).
    pub near_duplicates: Vec<NearDuplicatePair>,
    /// Graph nodes with degree 0 (no incident edges).
    pub orphan_nodes: Vec<OrphanNode>,
    /// Wiki pages with no graph edges (never linked).
    pub orphan_wiki_pages: Vec<OrphanNode>,
    /// Unresolved stub nodes.
    pub unresolved_stubs: Vec<StubInfo>,
    /// Unresolved stubs older than `stub_age_days`.
    pub aging_stubs: Vec<StubInfo>,
    /// Wiki pages older than a linked raw parent (or compile payload parent).
    pub stale_wiki: Vec<StaleWikiPage>,
    /// Soft archive suggestions (low boost / old / not pinned).
    pub archive_candidates: Vec<ArchiveCandidate>,
    /// Flattened issue list for planners / UI.
    pub issues: Vec<AnalysisIssue>,
    pub issue_count: usize,
}

/// Document / graph / layer tallies.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CorpusCounts {
    pub documents: u64,
    pub chunks: u64,
    pub nodes: u64,
    pub edges: u64,
    pub by_layer: Vec<CountBucket>,
    pub by_kind: Vec<CountBucket>,
    pub by_status: Vec<CountBucket>,
    pub by_wing: Vec<CountBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountBucket {
    pub key: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SizeStats {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_size_bytes: Option<u64>,
    pub avg_chunk_chars: f64,
    pub avg_document_chars: f64,
    pub total_chunk_chars: u64,
    pub total_document_chars: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedMismatchInfo {
    pub config_dims: usize,
    pub config_provider: String,
    pub config_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_dims: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_model: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub content_hash: String,
    pub document_ids: Vec<String>,
    pub titles: Vec<String>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NearDuplicatePair {
    pub document_id_a: String,
    pub document_id_b: String,
    pub title_a: String,
    pub title_b: String,
    pub cosine: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrphanNode {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    pub kind: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StubInfo {
    pub node_id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// Whole days since `created_at` (when known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_days: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaleWikiPage {
    pub wiki_document_id: String,
    pub wiki_title: String,
    pub wiki_updated_at: DateTime<Utc>,
    pub raw_document_id: String,
    pub raw_title: String,
    pub raw_updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveCandidate {
    pub document_id: String,
    pub title: String,
    pub layer: String,
    pub boost: f64,
    pub updated_at: DateTime<Utc>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisIssue {
    /// Machine code: `exact_dup`, `orphan_node`, `unresolved_stub`, …
    pub code: String,
    /// `info` | `warn` | `error`
    pub severity: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
}

/// Run deterministic corpus analysis and return a structured report.
pub fn analyze_corpus(
    store: &Store,
    config: &Config,
    opts: &AnalyzeOptions,
) -> Result<AnalysisReport> {
    let generated_at = Utc::now();
    let db_path = store.path().display().to_string();
    let db_size_bytes = file_size_bytes(store.path());

    let (document_count, chunk_count, node_count, edge_count) = store.stats()?;
    let counts = collect_counts(store, document_count, chunk_count, node_count, edge_count)?;
    let size = collect_size_stats(store, db_size_bytes)?;
    let doctor = build_doctor(store, config)?;
    let fts_ready = doctor.fts_ready;

    let embed_mismatch = if doctor.embed_ok {
        None
    } else {
        let manifest = store.get_embedding_manifest()?;
        Some(EmbedMismatchInfo {
            config_dims: config.embedding_dims,
            config_provider: config.embedding_provider.as_str().to_string(),
            config_model: config.embedding_model.clone(),
            manifest_dims: manifest.as_ref().map(|m| m.dims),
            manifest_provider: manifest.as_ref().map(|m| m.provider.clone()),
            manifest_model: manifest.as_ref().map(|m| m.model.clone()),
            message: format!(
                "embedding_manifest dims/provider/model disagree with config (manifest_dims={:?}, config_dims={})",
                manifest.as_ref().map(|m| m.dims),
                config.embedding_dims
            ),
        })
    };

    let exact_duplicates = find_exact_duplicates(store)?;
    let near_duplicates = if opts.include_near_dups {
        find_near_duplicates(store, opts.near_dup_threshold)?
    } else {
        Vec::new()
    };

    let (orphan_nodes, orphan_wiki_pages) = find_orphans(store)?;
    let (unresolved_stubs, aging_stubs) = find_stubs(store, opts.stub_age_days, generated_at)?;
    let stale_wiki = find_stale_wiki(store)?;
    let archive_candidates = find_archive_candidates(store, opts.archive_min_age_days, generated_at)?;

    let mut issues = Vec::new();
    push_doctor_issues(&mut issues, &doctor);
    if let Some(ref m) = embed_mismatch {
        issues.push(AnalysisIssue {
            code: "embed_mismatch".into(),
            severity: "error".into(),
            message: m.message.clone(),
            entity_id: None,
        });
    }
    for g in &exact_duplicates {
        issues.push(AnalysisIssue {
            code: "exact_dup".into(),
            severity: "warn".into(),
            message: format!(
                "{} documents share content_hash {} ({})",
                g.count,
                short_hash(&g.content_hash),
                g.titles.join(", ")
            ),
            entity_id: g.document_ids.first().cloned(),
        });
    }
    for p in &near_duplicates {
        issues.push(AnalysisIssue {
            code: "near_dup".into(),
            severity: "info".into(),
            message: format!(
                "near-duplicate cosine={:.3}: '{}' ~ '{}'",
                p.cosine, p.title_a, p.title_b
            ),
            entity_id: Some(p.document_id_a.clone()),
        });
    }
    for o in &orphan_nodes {
        issues.push(AnalysisIssue {
            code: "orphan_node".into(),
            severity: "info".into(),
            message: format!("graph node '{}' ({}) has degree 0", o.label, o.kind),
            entity_id: Some(o.id.clone()),
        });
    }
    for o in &orphan_wiki_pages {
        issues.push(AnalysisIssue {
            code: "orphan_wiki".into(),
            severity: "info".into(),
            message: format!("wiki page '{}' has no graph edges", o.label),
            entity_id: o.document_id.clone().or_else(|| Some(o.id.clone())),
        });
    }
    for s in &unresolved_stubs {
        let aging = aging_stubs.iter().any(|a| a.node_id == s.node_id);
        issues.push(AnalysisIssue {
            code: if aging {
                "aging_stub".into()
            } else {
                "unresolved_stub".into()
            },
            severity: if aging {
                "warn".into()
            } else {
                "info".into()
            },
            message: format!(
                "unresolved stub '{}'{}",
                s.label,
                s.age_days
                    .map(|d| format!(" (age {d}d)"))
                    .unwrap_or_default()
            ),
            entity_id: Some(s.node_id.clone()),
        });
    }
    for w in &stale_wiki {
        issues.push(AnalysisIssue {
            code: "stale_wiki".into(),
            severity: "warn".into(),
            message: format!(
                "wiki '{}' older than linked raw '{}'",
                w.wiki_title, w.raw_title
            ),
            entity_id: Some(w.wiki_document_id.clone()),
        });
    }
    for a in &archive_candidates {
        issues.push(AnalysisIssue {
            code: "archive_candidate".into(),
            severity: "info".into(),
            message: format!("{}: {}", a.title, a.reason),
            entity_id: Some(a.document_id.clone()),
        });
    }
    if !fts_ready && chunk_count > 0 {
        issues.push(AnalysisIssue {
            code: "fts_not_ready".into(),
            severity: "info".into(),
            message: "FTS index not ready; lex search uses term-frequency fallback".into(),
            entity_id: None,
        });
    }

    let issue_count = issues.len();
    let report = AnalysisReport {
        generated_at,
        db_path: db_path.clone(),
        db_size_bytes,
        counts,
        size,
        doctor,
        fts_ready,
        embed_mismatch,
        exact_duplicates,
        near_duplicates,
        orphan_nodes,
        orphan_wiki_pages,
        unresolved_stubs,
        aging_stubs,
        stale_wiki,
        archive_candidates,
        issues,
        issue_count,
    };

    if opts.log_ops {
        let _ = store.append_ops_log(&OpsLogEntry {
            id: Uuid::new_v4().to_string(),
            seq: 0,
            ts: generated_at,
            op: "analyze_corpus".into(),
            prefix: Some("MAINT".into()),
            message: format!(
                "analyze_corpus: {} issues, {} docs, {} stubs",
                report.issue_count,
                report.counts.documents,
                report.unresolved_stubs.len()
            ),
            entity_id: None,
            entity_kind: Some("maintenance".into()),
            payload_json: serde_json::json!({
                "issue_count": report.issue_count,
                "document_count": report.counts.documents,
                "chunk_count": report.counts.chunks,
                "exact_dup_groups": report.exact_duplicates.len(),
                "near_dup_pairs": report.near_duplicates.len(),
                "orphan_nodes": report.orphan_nodes.len(),
                "stale_wiki": report.stale_wiki.len(),
                "unresolved_stubs": report.unresolved_stubs.len(),
            })
            .to_string(),
            agent_name: None,
        });
    }

    Ok(report)
}

fn build_doctor(store: &Store, config: &Config) -> Result<DoctorReport> {
    let schema_version = store.schema_version()?.unwrap_or(0);
    let schema_ok = schema_version >= SCHEMA_VERSION;
    let fts_ready = store.fts_ready()?;
    let (document_count, chunk_count, node_count, edge_count) = store.stats()?;
    let embed_dims = config.embedding_dims;
    let manifest = store.get_embedding_manifest()?;
    let manifest_dims = manifest.as_ref().map(|m| m.dims);
    let embed_ok = match manifest_dims {
        None => true,
        Some(d) => d as usize == embed_dims,
    };
    let ingest_roots_configured = !config.ingest_roots.is_empty();
    let ready_for_search = chunk_count > 0 && schema_ok && embed_ok;
    let (documents_without_chunks, orphan_chunks, orphan_document_nodes, orphan_edges, unscoped_documents) =
        store.integrity_counts()?;
    let relational_integrity_ok = orphan_chunks == 0 && orphan_document_nodes == 0 && orphan_edges == 0;
    let wal_bytes = store.wal_file_size_bytes();
    let wal_warn_bytes = crate::ops::wal_warn_bytes();
    let wal_too_large = wal_bytes >= wal_warn_bytes;
    let repair_hint = if !relational_integrity_ok {
        Some("Create a backup, run maintain_refresh, and use offline db_repair for fatal DuckDB index errors.".to_string())
    } else if documents_without_chunks > 0 {
        Some("Reingest documents without chunks before relying on retrieval.".to_string())
    } else if wal_too_large {
        Some("WAL exceeds the configured warning threshold; checkpoint the store.".to_string())
    } else { None };
    let ok = schema_ok && embed_ok && relational_integrity_ok && documents_without_chunks == 0;
    Ok(DoctorReport {
        backend: "duckdb".to_string(),
        storage_capabilities: crate::storage::duckdb_capability_names(),
        schema_version,
        expected_schema_version: SCHEMA_VERSION,
        schema_ok,
        fts_ready,
        document_count,
        chunk_count,
        node_count,
        edge_count,
        embed_dims,
        manifest_dims,
        embed_ok,
        ready_for_search,
        ingest_roots_configured,
        db_path: store.path().display().to_string(),
        wal_bytes,
        wal_warn_bytes,
        wal_too_large,
        documents_without_chunks,
        orphan_chunks,
        orphan_document_nodes,
        orphan_edges,
        unscoped_documents,
        relational_integrity_ok,
        repair_hint,
        ok,
    })
}

fn push_doctor_issues(issues: &mut Vec<AnalysisIssue>, doctor: &DoctorReport) {
    if !doctor.schema_ok {
        issues.push(AnalysisIssue {
            code: "schema_lag".into(),
            severity: "error".into(),
            message: format!(
                "schema_version {} < expected {}",
                doctor.schema_version, doctor.expected_schema_version
            ),
            entity_id: None,
        });
    }
    if !doctor.ok {
        issues.push(AnalysisIssue {
            code: "doctor_not_ok".into(),
            severity: "error".into(),
            message: "doctor integrity gate failed (schema or embed)".into(),
            entity_id: None,
        });
    }
}

fn collect_counts(
    store: &Store,
    documents: u64,
    chunks: u64,
    nodes: u64,
    edges: u64,
) -> Result<CorpusCounts> {
    let conn = store.lock()?;
    Ok(CorpusCounts {
        documents,
        chunks,
        nodes,
        edges,
        by_layer: group_count_locked(&conn, "SELECT COALESCE(layer, 'raw'), COUNT(*)::BIGINT FROM documents GROUP BY 1 ORDER BY 2 DESC")?,
        by_kind: group_count_locked(&conn, "SELECT COALESCE(kind, 'document'), COUNT(*)::BIGINT FROM documents GROUP BY 1 ORDER BY 2 DESC")?,
        by_status: group_count_locked(&conn, "SELECT COALESCE(status, 'active'), COUNT(*)::BIGINT FROM documents GROUP BY 1 ORDER BY 2 DESC")?,
        by_wing: group_count_locked(
            &conn,
            r#"
            SELECT CASE
                     WHEN wing IS NULL OR TRIM(wing) = '' THEN '(unscoped)'
                     ELSE wing
                   END,
                   COUNT(*)::BIGINT
            FROM documents
            GROUP BY 1
            ORDER BY 2 DESC
            "#,
        )?,
    })
}

fn group_count_locked(conn: &duckdb::Connection, sql: &str) -> Result<Vec<CountBucket>> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        out.push(CountBucket {
            key,
            count: count as u64,
        });
    }
    Ok(out)
}

fn collect_size_stats(store: &Store, db_size_bytes: Option<u64>) -> Result<SizeStats> {
    let conn = store.lock()?;
    let (chunk_n, chunk_chars): (i64, i64) = conn.query_row(
        "SELECT COUNT(*)::BIGINT, COALESCE(SUM(LENGTH(content)), 0)::BIGINT FROM chunks",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let (doc_n, doc_chars): (i64, i64) = conn.query_row(
        "SELECT COUNT(*)::BIGINT, COALESCE(SUM(LENGTH(content)), 0)::BIGINT FROM documents",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let avg_chunk = if chunk_n > 0 {
        chunk_chars as f64 / chunk_n as f64
    } else {
        0.0
    };
    let avg_doc = if doc_n > 0 {
        doc_chars as f64 / doc_n as f64
    } else {
        0.0
    };
    Ok(SizeStats {
        db_size_bytes,
        avg_chunk_chars: avg_chunk,
        avg_document_chars: avg_doc,
        total_chunk_chars: chunk_chars as u64,
        total_document_chars: doc_chars as u64,
    })
}

fn find_exact_duplicates(store: &Store) -> Result<Vec<DuplicateGroup>> {
    let conn = store.lock()?;
    let mut stmt = conn.prepare(
        r#"
        SELECT content_hash, COUNT(*)::BIGINT
        FROM documents
        WHERE content_hash IS NOT NULL AND TRIM(content_hash) <> ''
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
            ORDER BY created_at ASC
            "#,
        )?;
        let mut rows = stmt.query(params![hash.as_str()])?;
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

fn find_near_duplicates(store: &Store, threshold: f64) -> Result<Vec<NearDuplicatePair>> {
    let threshold = threshold.clamp(0.0, 1.0) as f32;
    // One representative vector per document (lowest chunk_index).
    let chunks = store.all_chunks_with_embeddings()?;
    let docs = store.list_documents()?;
    let title_by_id: HashMap<String, String> = docs
        .iter()
        .map(|d| (d.id.clone(), d.title.clone()))
        .collect();

    let mut first_chunk: BTreeMap<String, Vec<f32>> = BTreeMap::new();
    for c in chunks {
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
                    return Ok(pairs);
                }
            }
        }
    }
    // Highest similarity first.
    pairs.sort_by(|a, b| {
        b.cosine
            .partial_cmp(&a.cosine)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(pairs)
}


fn find_orphans(store: &Store) -> Result<(Vec<OrphanNode>, Vec<OrphanNode>)> {
    let conn = store.lock()?;

    // Nodes with no incident edges.
    let mut stmt = conn.prepare(
        r#"
        SELECT n.id, n.document_id, n.kind, n.label
        FROM graph_nodes n
        WHERE NOT EXISTS (
            SELECT 1 FROM graph_edges e
            WHERE e.source_id = n.id OR e.target_id = n.id
        )
        ORDER BY n.kind ASC, n.label ASC
        LIMIT 200
        "#,
    )?;
    let mut rows = stmt.query([])?;
    let mut orphan_nodes = Vec::new();
    while let Some(row) = rows.next()? {
        orphan_nodes.push(OrphanNode {
            id: row.get(0)?,
            document_id: row.get(1)?,
            kind: row.get(2)?,
            label: row.get(3)?,
            layer: None,
        });
        if orphan_nodes.len() >= REPORT_LIST_CAP {
            break;
        }
    }
    drop(rows);
    drop(stmt);

    // Wiki documents whose graph node (if any) has degree 0, or no node at all.
    let mut stmt = conn.prepare(
        r#"
        SELECT d.id, d.title, d.layer, n.id
        FROM documents d
        LEFT JOIN graph_nodes n ON n.document_id = d.id
        WHERE d.layer = 'wiki'
          AND (
            n.id IS NULL
            OR NOT EXISTS (
              SELECT 1 FROM graph_edges e
              WHERE e.source_id = n.id OR e.target_id = n.id
            )
          )
        ORDER BY d.title ASC
        LIMIT 200
        "#,
    )?;
    let mut rows = stmt.query([])?;
    let mut orphan_wiki = Vec::new();
    while let Some(row) = rows.next()? {
        let doc_id: String = row.get(0)?;
        let title: String = row.get(1)?;
        let layer: String = row.get(2)?;
        let node_id: Option<String> = row.get(3)?;
        orphan_wiki.push(OrphanNode {
            id: node_id.unwrap_or_else(|| doc_id.clone()),
            document_id: Some(doc_id),
            kind: "wiki".into(),
            label: title,
            layer: Some(layer),
        });
        if orphan_wiki.len() >= REPORT_LIST_CAP {
            break;
        }
    }
    Ok((orphan_nodes, orphan_wiki))
}

fn find_stubs(
    store: &Store,
    stub_age_days: u32,
    now: DateTime<Utc>,
) -> Result<(Vec<StubInfo>, Vec<StubInfo>)> {
    let conn = store.lock()?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, label, CAST(created_at AS VARCHAR)
        FROM graph_nodes
        WHERE kind = 'stub' AND resolved = false
        ORDER BY created_at ASC
        LIMIT 200
        "#,
    )?;
    let mut rows = stmt.query([])?;
    let mut unresolved = Vec::new();
    let mut aging = Vec::new();
    let age_cutoff = now - Duration::days(i64::from(stub_age_days));

    while let Some(row) = rows.next()? {
        let node_id: String = row.get(0)?;
        let label: String = row.get(1)?;
        let created_raw: Option<String> = row.get(2)?;
        let created_at = created_raw
            .as_deref()
            .and_then(parse_ts);
        let age_days = created_at.map(|t| (now - t).num_days());
        let info = StubInfo {
            node_id,
            label,
            created_at,
            age_days,
        };
        if created_at.map(|t| t <= age_cutoff).unwrap_or(false) {
            aging.push(info.clone());
        }
        unresolved.push(info);
        if unresolved.len() >= REPORT_LIST_CAP {
            break;
        }
    }
    Ok((unresolved, aging))
}

fn find_stale_wiki(store: &Store) -> Result<Vec<StaleWikiPage>> {
    // Reuse wiki-layer detection (graph related, metadata, citations, body markers).
    let items = crate::wiki::find_stale_wiki(store)?;
    let mut out = Vec::with_capacity(items.len().min(REPORT_LIST_CAP));
    for item in items.into_iter().take(REPORT_LIST_CAP) {
        let wiki_updated_at = parse_ts(&item.wiki_updated_at).unwrap_or_else(Utc::now);
        let raw_updated_at = parse_ts(&item.raw_updated_at).unwrap_or_else(Utc::now);
        out.push(StaleWikiPage {
            wiki_document_id: item.wiki_id,
            wiki_title: item.wiki_title,
            wiki_updated_at,
            raw_document_id: item.raw_id,
            raw_title: item.raw_title,
            raw_updated_at,
        });
    }
    Ok(out)
}

fn find_archive_candidates(
    store: &Store,
    min_age_days: u32,
    now: DateTime<Utc>,
) -> Result<Vec<ArchiveCandidate>> {
    let cutoff = now - Duration::days(i64::from(min_age_days));
    let conn = store.lock()?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, title, COALESCE(layer, 'raw'), COALESCE(boost, 1.0), CAST(updated_at AS VARCHAR)
        FROM documents
        WHERE COALESCE(status, 'active') = 'active'
          AND COALESCE(pinned, false) = false
          AND COALESCE(boost, 1.0) < 1.0
          AND layer NOT IN ('schema', 'index', 'log')
        ORDER BY boost ASC, updated_at ASC
        LIMIT 200
        "#,
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let title: String = row.get(1)?;
        let layer: String = row.get(2)?;
        let boost: f64 = row.get(3)?;
        let ts: String = row.get(4)?;
        let Some(updated_at) = parse_ts(&ts) else {
            continue;
        };
        if updated_at > cutoff && boost >= 0.5 {
            // Only flag recent docs when boost is clearly suppressed.
            continue;
        }
        let reason = if updated_at <= cutoff && boost < 1.0 {
            format!(
                "boost={boost:.2}, updated {}d ago",
                (now - updated_at).num_days()
            )
        } else if boost < 0.5 {
            format!("low boost={boost:.2}")
        } else {
            continue;
        };
        out.push(ArchiveCandidate {
            document_id: id,
            title,
            layer,
            boost,
            updated_at,
            reason,
        });
        if out.len() >= REPORT_LIST_CAP {
            break;
        }
    }
    Ok(out)
}

fn file_size_bytes(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

fn short_hash(h: &str) -> String {
    if h.chars().count() <= 12 {
        h.to_string()
    } else {
        let mut s: String = h.chars().take(12).collect();
        s.push('…');
        s
    }
}

fn parse_ts(raw: &str) -> Option<DateTime<Utc>> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // DuckDB CAST(ts AS VARCHAR) often yields "YYYY-MM-DD HH:MM:SS" or with fractional.
    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f"))
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"));
    naive
        .ok()
        .map(|n| DateTime::<Utc>::from_naive_utc_and_offset(n, Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Document, GraphEdge, GraphNode, SearchMode};
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

    fn doc(id: &str, uri: &str, title: &str, content: &str, layer: &str) -> Document {
        let now = Utc::now();
        Document {
            id: id.into(),
            uri: uri.into(),
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
        }
    }

    #[test]
    fn analyze_empty_corpus() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);
        let mut opts = AnalyzeOptions::from_config(&cfg);
        opts.log_ops = true;
        let report = analyze_corpus(&store, &cfg, &opts).unwrap();
        assert_eq!(report.counts.documents, 0);
        assert_eq!(report.counts.chunks, 0);
        assert!(report.doctor.schema_ok);
        assert!(report.exact_duplicates.is_empty());
        // ops_log row written
        let ops = store.list_ops_log(5).unwrap();
        assert!(ops.iter().any(|o| o.op == "analyze_corpus"));
    }

    #[test]
    fn analyze_finds_exact_dups_and_stubs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dups.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);

        let body = "identical body for dedupe";
        store
            .upsert_document(&doc("d1", "raw://a", "A", body, "raw"))
            .unwrap();
        store
            .upsert_document(&doc("d2", "raw://b", "B", body, "raw"))
            .unwrap();

        store
            .upsert_graph_node(&GraphNode {
                id: "stub-1".into(),
                kind: "stub".into(),
                label: "Missing Page".into(),
                document_id: None,
                uri: None,
                resolved: false,
                metadata_json: "{}".into(),
            })
            .unwrap();

        let mut opts = AnalyzeOptions::from_config(&cfg);
        opts.log_ops = false;
        let report = analyze_corpus(&store, &cfg, &opts).unwrap();
        assert_eq!(report.counts.documents, 2);
        assert_eq!(report.exact_duplicates.len(), 1);
        assert_eq!(report.exact_duplicates[0].count, 2);
        assert_eq!(report.unresolved_stubs.len(), 1);
        assert!(report.issues.iter().any(|i| i.code == "exact_dup"));
        assert!(report.issues.iter().any(|i| i.code == "unresolved_stub"));
    }

    #[test]
    fn analyze_stale_wiki_via_related_edge() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);

        let older = Utc::now() - Duration::days(5);
        let newer = Utc::now();

        let mut raw = doc("raw1", "raw://src", "Source", "raw body v2", "raw");
        raw.updated_at = newer;
        store.upsert_document(&raw).unwrap();

        let mut wiki = doc("wiki1", "wiki://sum", "Summary", "old summary", "wiki");
        wiki.updated_at = older;
        wiki.kind = "source_summary".into();
        store.upsert_document(&wiki).unwrap();

        store
            .upsert_graph_node(&GraphNode {
                id: "n-raw".into(),
                kind: "document".into(),
                label: "Source".into(),
                document_id: Some("raw1".into()),
                uri: Some("raw://src".into()),
                resolved: true,
                metadata_json: "{}".into(),
            })
            .unwrap();
        store
            .upsert_graph_node(&GraphNode {
                id: "n-wiki".into(),
                kind: "document".into(),
                label: "Summary".into(),
                document_id: Some("wiki1".into()),
                uri: Some("wiki://sum".into()),
                resolved: true,
                metadata_json: "{}".into(),
            })
            .unwrap();
        store
            .insert_graph_edges(&[GraphEdge {
                id: "e1".into(),
                source_id: "n-raw".into(),
                target_id: "n-wiki".into(),
                rel_type: "related".into(),
                weight: 1.0,
                context: None,
            }])
            .unwrap();

        let mut opts = AnalyzeOptions::from_config(&cfg);
        opts.log_ops = false;
        let report = analyze_corpus(&store, &cfg, &opts).unwrap();
        assert_eq!(report.stale_wiki.len(), 1);
        assert_eq!(report.stale_wiki[0].wiki_document_id, "wiki1");
        assert!(report.issues.iter().any(|i| i.code == "stale_wiki"));
    }

    #[test]
    fn parse_ts_duckdb_and_rfc3339() {
        assert!(parse_ts("2024-01-02 03:04:05").is_some());
        assert!(parse_ts("2024-01-02T03:04:05Z").is_some());
        assert!(parse_ts("").is_none());
    }
}
