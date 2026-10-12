//! Deterministic corpus analysis (no LLM).
//!
//! MCP tool: `analyze_corpus` → [`AnalysisReport`] JSON.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;
use crate::db::analytics::{
    collect_counts, collect_size_stats, find_archive_candidates, find_exact_duplicates,
    find_orphans, find_stubs, parse_ts, short_hash,
};
pub use crate::db::analytics::{
    ArchiveCandidate, CorpusCounts, CountBucket, DuplicateGroup, OrphanNode, SizeStats, StubInfo,
};
use crate::db::Store;
use crate::diagnostics::DiagnosticsService;
use crate::diagnostics::DoctorReport;
use crate::embeddings::cosine_similarity;
use crate::error::Result;
use crate::models::OpsLogEntry;

/// Default age (days) after which an unresolved stub is "aging".
const DEFAULT_STUB_AGE_DAYS: u32 = 14;
/// Default age (days) for low-boost archive candidates.
const DEFAULT_ARCHIVE_MIN_AGE_DAYS: u32 = 90;
/// Cap document pairs scanned for near-dup cosine (first-chunk embeddings).
const NEAR_DUP_MAX_DOCS: usize = 200;
use crate::db::analytics::REPORT_LIST_CAP;

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
pub struct NearDuplicatePair {
    pub document_id_a: String,
    pub document_id_b: String,
    pub title_a: String,
    pub title_b: String,
    pub cosine: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaleWikiPage {
    pub wiki_document_id: String,
    pub wiki_title: String,
    pub wiki_updated_at: DateTime<Utc>,
    pub raw_document_id: String,
    pub raw_title: String,
    /// Null when a recorded source has been deleted or is unavailable.
    pub raw_updated_at: Option<DateTime<Utc>>,
    /// Source hash, missing-source, or legacy timestamp evidence.
    #[serde(default)]
    pub link_kind: String,
}

crate::models::finding_codes! {
    /// `analyze_corpus` finding codes.
    AnalysisCode {
        AgingStub => "aging_stub",
        ArchiveCandidate => "archive_candidate",
        DoctorNotOk => "doctor_not_ok",
        EmbedMismatch => "embed_mismatch",
        ExactDup => "exact_dup",
        FtsNotReady => "fts_not_ready",
        NearDup => "near_dup",
        OrphanNode => "orphan_node",
        OrphanWiki => "orphan_wiki",
        SchemaLag => "schema_lag",
        StaleWiki => "stale_wiki",
        UnresolvedStub => "unresolved_stub",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisIssue {
    /// Machine code; unknown codes from a supplied report stay `Other`.
    pub code: AnalysisCode,
    /// `info` | `warn` | `error`
    pub severity: crate::models::IssueSeverity,
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
    let doctor = DiagnosticsService::new(store, config).doctor()?;
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
            message: store
                .require_embedding_manifest_match(config)
                .err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| {
                    "embedding identity changed while corpus analysis was running".into()
                }),
        })
    };

    let exact_duplicates = find_exact_duplicates(store, false)?;
    let near_duplicates = if opts.include_near_dups {
        let corpus_guard =
            crate::db::search::acquire_corpus_search_guard(store, crate::models::SearchMode::Vec)?;
        store.require_embedding_manifest_match(config)?;
        let pairs = near_duplicate_pairs(store, opts.near_dup_threshold, false)?;
        drop(corpus_guard);
        pairs
    } else {
        Vec::new()
    };

    let (orphan_nodes, orphan_wiki_pages) = find_orphans(store)?;
    let (unresolved_stubs, aging_stubs) = find_stubs(store, opts.stub_age_days, generated_at)?;
    let stale_wiki = find_stale_wiki(store)?;
    let archive_candidates =
        find_archive_candidates(store, opts.archive_min_age_days, generated_at)?;

    let mut issues = Vec::new();
    push_doctor_issues(&mut issues, &doctor);
    if let Some(ref m) = embed_mismatch {
        issues.push(AnalysisIssue {
            code: AnalysisCode::EmbedMismatch,
            severity: crate::models::IssueSeverity::Error,
            message: m.message.clone(),
            entity_id: None,
        });
    }
    for g in &exact_duplicates {
        issues.push(AnalysisIssue {
            code: AnalysisCode::ExactDup,
            severity: crate::models::IssueSeverity::Warn,
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
            code: AnalysisCode::NearDup,
            severity: crate::models::IssueSeverity::Info,
            message: format!(
                "near-duplicate cosine={:.3}: '{}' ~ '{}'",
                p.cosine, p.title_a, p.title_b
            ),
            entity_id: Some(p.document_id_a.clone()),
        });
    }
    for o in &orphan_nodes {
        issues.push(AnalysisIssue {
            code: AnalysisCode::OrphanNode,
            severity: crate::models::IssueSeverity::Info,
            message: format!("graph node '{}' ({}) has degree 0", o.label, o.kind),
            entity_id: Some(o.id.clone()),
        });
    }
    for o in &orphan_wiki_pages {
        issues.push(AnalysisIssue {
            code: AnalysisCode::OrphanWiki,
            severity: crate::models::IssueSeverity::Info,
            message: format!("wiki page '{}' has no graph edges", o.label),
            entity_id: o.document_id.clone().or_else(|| Some(o.id.clone())),
        });
    }
    for s in &unresolved_stubs {
        let aging = aging_stubs.iter().any(|a| a.node_id == s.node_id);
        issues.push(AnalysisIssue {
            code: if aging {
                AnalysisCode::AgingStub
            } else {
                AnalysisCode::UnresolvedStub
            },
            severity: if aging {
                crate::models::IssueSeverity::Warn
            } else {
                crate::models::IssueSeverity::Info
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
            code: AnalysisCode::StaleWiki,
            severity: crate::models::IssueSeverity::Warn,
            message: match w.link_kind.as_str() {
                "source_missing" => format!(
                    "wiki '{}' has an unavailable source '{}'; restore or relink the source",
                    w.wiki_title, w.raw_title
                ),
                "source_version" => format!(
                    "wiki '{}' was generated from different source content in '{}'",
                    w.wiki_title, w.raw_title
                ),
                _ => format!(
                    "wiki '{}' older than linked raw '{}'",
                    w.wiki_title, w.raw_title
                ),
            },
            entity_id: Some(w.wiki_document_id.clone()),
        });
    }
    for a in &archive_candidates {
        issues.push(AnalysisIssue {
            code: AnalysisCode::ArchiveCandidate,
            severity: crate::models::IssueSeverity::Info,
            message: format!("{}: {}", a.title, a.reason),
            entity_id: Some(a.document_id.clone()),
        });
    }
    if !fts_ready && chunk_count > 0 {
        issues.push(AnalysisIssue {
            code: AnalysisCode::FtsNotReady,
            severity: crate::models::IssueSeverity::Info,
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

fn push_doctor_issues(issues: &mut Vec<AnalysisIssue>, doctor: &DoctorReport) {
    if !doctor.schema_ok {
        issues.push(AnalysisIssue {
            code: AnalysisCode::SchemaLag,
            severity: crate::models::IssueSeverity::Error,
            message: format!(
                "schema_version {} < expected {}",
                doctor.schema_version, doctor.expected_schema_version
            ),
            entity_id: None,
        });
    }
    if !doctor.ok {
        issues.push(AnalysisIssue {
            code: AnalysisCode::DoctorNotOk,
            severity: crate::models::IssueSeverity::Error,
            message: "doctor integrity gate failed (schema or embed)".into(),
            entity_id: None,
        });
    }
}

/// Document pairs whose representative embeddings reach `threshold` cosine,
/// most similar first. At most `NEAR_DUP_MAX_DOCS` documents are compared (the
/// cap is applied in SQL) and at most `REPORT_LIST_CAP` pairs are returned.
pub(crate) fn near_duplicate_pairs(
    store: &Store,
    threshold: f64,
    active_only: bool,
) -> Result<Vec<NearDuplicatePair>> {
    let threshold = threshold.clamp(0.0, 1.0) as f32;
    let entries =
        store.representative_document_embeddings(NEAR_DUP_MAX_DOCS, active_only, false, false)?;

    let mut pairs = Vec::new();
    'scan: for i in 0..entries.len() {
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
                    break 'scan;
                }
            }
        }
    }
    pairs.sort_by(|a, b| b.cosine.total_cmp(&a.cosine));
    Ok(pairs)
}

fn find_stale_wiki(store: &Store) -> Result<Vec<StaleWikiPage>> {
    // Reuse wiki-layer detection (graph related, metadata, citations, body markers).
    let items = crate::wiki::find_stale_wiki(store)?;
    let mut out = Vec::with_capacity(items.len().min(REPORT_LIST_CAP));
    for item in items.into_iter().take(REPORT_LIST_CAP) {
        let wiki_updated_at = parse_ts(&item.wiki_updated_at).unwrap_or_else(Utc::now);
        let raw_updated_at = item.raw_updated_at.as_deref().and_then(parse_ts);
        out.push(StaleWikiPage {
            wiki_document_id: item.wiki_id,
            wiki_title: item.wiki_title,
            wiki_updated_at,
            raw_document_id: item.raw_id,
            raw_title: item.raw_title,
            raw_updated_at,
            link_kind: item.link_kind,
        });
    }
    Ok(out)
}

fn file_size_bytes(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_codes_keep_wire_strings_and_accept_unknown_codes() {
        let issue: AnalysisIssue = serde_json::from_value(serde_json::json!({
            "code": "custom_client_code", "severity": "info", "message": "m"
        }))
        .unwrap();
        assert_eq!(issue.code, AnalysisCode::Other("custom_client_code".into()));
        assert_eq!(serde_json::to_value(&issue).unwrap()["code"], "custom_client_code");
        assert_eq!(serde_json::to_value(AnalysisCode::ExactDup).unwrap(), "exact_dup");
        assert_eq!(AnalysisCode::from("stale_wiki"), AnalysisCode::StaleWiki);
    }
    use crate::models::{Chunk, Document, GraphEdge, GraphNode};
    use crate::util::content_hash;
    use chrono::Duration;
    use std::path::PathBuf;

    fn test_config(db_path: PathBuf) -> Config {
        Config {
            db_path,
            embedding_model: "mock".into(),
            embedding_dims: 8,
            llm_enabled: false,
            ..Config::for_tests()
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
            kind: if crate::models::Layer::Wiki.is(layer) {
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
    fn near_duplicate_analysis_fails_closed_during_embedding_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("near-dups-migration-incomplete.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);
        for id in ["a", "b"] {
            let document = doc(
                id,
                &format!("raw://{id}"),
                &format!("Document {id}"),
                &format!("unique body {id}"),
                "raw",
            );
            store.upsert_document(&document).unwrap();
            store
                .insert_chunks(&[Chunk {
                    id: format!("{id}-c0"),
                    document_id: id.into(),
                    chunk_index: 0,
                    content: document.content,
                    embedding: vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    char_start: 0,
                    char_end: 13,
                    metadata_json: "{}".into(),
                }])
                .unwrap();
        }
        store
            .set_embedding_manifest(&crate::db::store::embedding_migration_manifest(&cfg))
            .unwrap();
        let mut opts = AnalyzeOptions::from_config(&cfg);
        opts.include_near_dups = true;

        let error = analyze_corpus(&store, &cfg, &opts).unwrap_err();

        assert!(matches!(error, crate::error::AppError::Embeddings(_)));
        assert!(error.to_string().contains("incomplete corpus migration"));
        assert!(store.list_ops_log(10).unwrap().is_empty());
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
        wiki.kind = crate::models::DocumentKind::SourceSummary.as_str().into();
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
    fn missing_source_diagnostics_keep_null_timestamp_and_require_restoration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing-source.duckdb");
        let store = Store::open(&path).unwrap();
        let config = test_config(path);
        let mut wiki = doc(
            "wiki1",
            "wiki://summary",
            "Summary",
            "Source synthesis",
            "wiki",
        );
        wiki.metadata_json = serde_json::json!({"source_versions":[{
            "document_id":"deleted-source", "uri":"raw://deleted", "content_hash":crate::util::content_hash("original source body")
        }]})
        .to_string();
        store.upsert_document(&wiki).unwrap();
        let mut options = AnalyzeOptions::from_config(&config);
        options.log_ops = false;
        let report = analyze_corpus(&store, &config, &options).unwrap();
        assert_eq!(report.stale_wiki.len(), 1);
        assert!(report.stale_wiki[0].raw_updated_at.is_none());
        let value = serde_json::to_value(&report.stale_wiki[0]).unwrap();
        assert!(value["raw_updated_at"].is_null());
        assert_eq!(value["link_kind"], "source_missing");
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.message.contains("unavailable source")));
        let plan = crate::maintain::heuristic_plan(&report, 50);
        assert!(!plan
            .actions
            .iter()
            .any(|action| action.action == crate::maintain::MaintenanceAction::RefreshStaleWiki));
        assert!(plan.actions.iter().any(|action| action.action
            == crate::maintain::MaintenanceAction::Noop
            && action
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("restore or relink"))));
    }

    #[test]
    fn parse_ts_duckdb_and_rfc3339() {
        assert!(parse_ts("2024-01-02 03:04:05").is_some());
        assert!(parse_ts("2024-01-02T03:04:05Z").is_some());
        assert!(parse_ts("").is_none());
    }
}
