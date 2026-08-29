//! One-shot organize pass: suggest (and optionally apply) refiles for docs missing wing.
//!
//! Deterministic heuristics first (path / title / embedding kNN labels). Optional local
//! LLM refinement when enabled. Actions are whitelist-validated (`refile`, …).
//! `dry_run` defaults to true; apply mutates placement metadata only (raw body untouched).
//! Every run appends `ops_log`. See `docs/LOCAL_LLM_MAINTENANCE.md` §3.2 and `docs/ORGANIZE.md`.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;
use crate::db::Store;
use crate::embeddings::cosine_similarity;
use crate::error::{AppError, Result};
use crate::llm::{ChatClient, ChatMessage};
use crate::maintain::plan::{validate_action, MaintenanceAction, MaintenancePlanItem};
use crate::models::{Document, DocumentMetaUpdate, OpsLogEntry, Taxonomy};

/// Layers never auto-refiled (system / catalog).
const SKIP_LAYERS: &[&str] = &["schema", "index", "log"];
/// Cap titles / samples sent to the LLM.
const LLM_SAMPLE_CAP: usize = 40;
/// Max labeled neighbors considered for embedding vote.
const EMBED_NEIGHBOR_K: usize = 8;
/// Min cosine to count an embedding neighbor vote.
const EMBED_MIN_COSINE: f32 = 0.55;

/// How suggestions are produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OrganizeMode {
    /// Path / title / embedding heuristics only (no LLM).
    #[default]
    Heuristic,
    /// Local chat LLM proposes refiles from taxonomy + titles (fallback to heuristic if LLM fails).
    Llm,
    /// Heuristic first; LLM fills gaps (low/no confidence) when client is present.
    Auto,
}

impl OrganizeMode {
    /// Parse tool/env wire name.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "heuristic" | "heuristics" | "rules" => Ok(Self::Heuristic),
            "llm" | "chat" => Ok(Self::Llm),
            "auto" | "hybrid" => Ok(Self::Auto),
            other => Err(AppError::config(format!(
                "invalid organize mode '{other}': expected heuristic|llm|auto"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Heuristic => "heuristic",
            Self::Llm => "llm",
            Self::Auto => "auto",
        }
    }
}

/// Options for [`maintain_organize`].
#[derive(Debug, Clone)]
pub struct OrganizeOptions {
    /// When true (default), report suggestions without mutating placement.
    pub dry_run: bool,
    pub mode: OrganizeMode,
    /// Cap documents considered / applied (`RAG_MAINT_MAX_DOCS`).
    pub max_docs: usize,
    /// Only apply suggestions with `confidence >= min_confidence` (default 0.5).
    pub min_confidence: f64,
    /// When true (and not dry_run), rebuild wiki_index after successful refiles.
    pub rebuild_index: bool,
    /// Optional agent name recorded in ops_log.
    pub agent: Option<String>,
}

impl Default for OrganizeOptions {
    fn default() -> Self {
        Self {
            dry_run: true,
            mode: OrganizeMode::Heuristic,
            max_docs: 50,
            min_confidence: 0.5,
            rebuild_index: false,
            agent: None,
        }
    }
}

impl OrganizeOptions {
    /// Build from config + optional overrides (MCP / CLI).
    pub fn from_config(config: &Config) -> Self {
        Self {
            max_docs: config.maint_max_docs.max(1),
            ..Self::default()
        }
    }
}

/// One proposed refile for a document missing wing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefileSuggestion {
    pub document_id: String,
    pub title: String,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_wing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_room: Option<String>,
    pub suggested_wing: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_room: Option<String>,
    /// `0.0..=1.0` confidence score.
    pub confidence: f64,
    /// `heuristic_path` | `heuristic_title` | `heuristic_embedding` | `llm` | …
    pub method: String,
    pub reason: String,
}

/// One applied (or dry-run preview) refile line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefileApplyResult {
    pub document_id: String,
    pub wing: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    pub dry_run: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Aggregate report for `maintain_organize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizeReport {
    pub dry_run: bool,
    pub mode: String,
    /// Documents missing wing that were eligible (before cap).
    pub unscoped_total: usize,
    /// Unscoped considered after `max_docs` cap.
    pub unscoped_considered: usize,
    pub skipped_cap: usize,
    pub suggestions: Vec<RefileSuggestion>,
    /// Whitelist-validated action list (primarily `refile`; optional `rebuild_index`).
    pub actions: Vec<MaintenancePlanItem>,
    /// Apply / dry-run outcomes for accepted suggestions.
    pub applied: Vec<RefileApplyResult>,
    pub applied_ok: usize,
    pub applied_failed: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebuild_index_entries: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops_log_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops_log_seq: Option<i64>,
}

/// Suggest refiles for docs missing wing; dry-run or apply placement updates.
///
/// Safety:
/// - Only whitelist actions (`refile`, optional `rebuild_index`).
/// - Does not mutate raw document bodies.
/// - Caps work with `max_docs`.
/// - Always logs to `ops_log` (dry_run uses `maintain_organize_dry_run`).
pub async fn maintain_organize(
    store: &Store,
    config: &Config,
    llm: Option<&ChatClient>,
    opts: &OrganizeOptions,
) -> Result<OrganizeReport> {
    let max_docs = opts.max_docs.max(1);
    let min_confidence = opts.min_confidence.clamp(0.0, 1.0);
    let dry_run = opts.dry_run;
    let taxonomy = store.get_taxonomy()?;
    let unscoped = list_unscoped_documents(store)?;
    let unscoped_total = unscoped.len();
    let skipped_cap = unscoped_total.saturating_sub(max_docs);
    let considered: Vec<Document> = unscoped.into_iter().take(max_docs).collect();
    let unscoped_considered = considered.len();

    let mut suggestions = match opts.mode {
        OrganizeMode::Heuristic => suggest_heuristic(store, &taxonomy, &considered)?,
        OrganizeMode::Llm => {
            if let Some(client) = llm.filter(|_| config.llm_enabled) {
                match suggest_llm(client, &taxonomy, &considered).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "organize LLM failed; falling back to heuristic");
                        let mut s = suggest_heuristic(store, &taxonomy, &considered)?;
                        for item in &mut s {
                            item.reason =
                                format!("llm_fallback: {}; {}", e, item.reason);
                        }
                        s
                    }
                }
            } else {
                let mut s = suggest_heuristic(store, &taxonomy, &considered)?;
                for item in &mut s {
                    item.reason = format!(
                        "llm unavailable (enabled={}, client={}); {}",
                        config.llm_enabled,
                        llm.is_some(),
                        item.reason
                    );
                }
                s
            }
        }
        OrganizeMode::Auto => {
            let mut s = suggest_heuristic(store, &taxonomy, &considered)?;
            let need_llm: Vec<Document> = considered
                .iter()
                .filter(|d| {
                    !s.iter().any(|x| {
                        x.document_id == d.id && x.confidence >= min_confidence
                    })
                })
                .cloned()
                .collect();
            if !need_llm.is_empty() {
                if let Some(client) = llm.filter(|_| config.llm_enabled) {
                    match suggest_llm(client, &taxonomy, &need_llm).await {
                        Ok(llm_sugs) => {
                            let covered: HashSet<String> =
                                s.iter().map(|x| x.document_id.clone()).collect();
                            for ls in llm_sugs {
                                if let Some(existing) =
                                    s.iter_mut().find(|x| x.document_id == ls.document_id)
                                {
                                    if ls.confidence > existing.confidence {
                                        *existing = ls;
                                    }
                                } else if !covered.contains(&ls.document_id) {
                                    s.push(ls);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "organize auto LLM fill-in failed; keeping heuristics"
                            );
                        }
                    }
                }
            }
            s
        }
    };

    // Stable order: confidence desc, title asc.
    suggestions.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.title.cmp(&b.title))
    });

    // Build whitelist actions from suggestions above confidence floor.
    let mut actions = Vec::new();
    for s in &suggestions {
        if s.confidence < min_confidence {
            continue;
        }
        let mut params = serde_json::Map::new();
        params.insert(
            "wing".into(),
            serde_json::Value::String(s.suggested_wing.clone()),
        );
        if let Some(ref room) = s.suggested_room {
            params.insert("room".into(), serde_json::Value::String(room.clone()));
        }
        params.insert(
            "confidence".into(),
            serde_json::Value::from(s.confidence),
        );
        params.insert(
            "method".into(),
            serde_json::Value::String(s.method.clone()),
        );
        let action = MaintenancePlanItem {
            action: MaintenanceAction::Refile,
            reason: Some(s.reason.clone()),
            target_id: Some(s.document_id.clone()),
            params: serde_json::Value::Object(params),
        };
        actions.push(validate_action(action)?);
    }
    if opts.rebuild_index {
        actions.push(validate_action(MaintenancePlanItem {
            action: MaintenanceAction::RebuildIndex,
            reason: Some("post-organize catalog refresh".into()),
            target_id: None,
            params: serde_json::json!({}),
        })?);
    }

    let mut applied = Vec::new();
    let mut applied_ok = 0usize;
    let mut applied_failed = 0usize;
    let mut rebuild_index_entries = None;

    for action in &actions {
        if action.action == MaintenanceAction::RebuildIndex {
            continue; // handled after refiles
        }
        if action.action != MaintenanceAction::Refile {
            // Defensive: only refile is applied here.
            applied_failed += 1;
            applied.push(RefileApplyResult {
                document_id: action.target_id.clone().unwrap_or_default(),
                wing: String::new(),
                room: None,
                dry_run,
                error: Some(format!(
                    "organize apply ignores non-refile action '{}'",
                    action.action
                )),
            });
            continue;
        }
        let Some(doc_id) = action.target_id.as_deref() else {
            applied_failed += 1;
            applied.push(RefileApplyResult {
                document_id: String::new(),
                wing: String::new(),
                room: None,
                dry_run,
                error: Some("refile missing target_id".into()),
            });
            continue;
        };
        let wing = action
            .params
            .get("wing")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("")
            .to_string();
        if wing.is_empty() {
            applied_failed += 1;
            applied.push(RefileApplyResult {
                document_id: doc_id.into(),
                wing: String::new(),
                room: None,
                dry_run,
                error: Some("refile params.wing must be non-empty".into()),
            });
            continue;
        }
        let room = action
            .params
            .get("room")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        if dry_run {
            applied_ok += 1;
            applied.push(RefileApplyResult {
                document_id: doc_id.into(),
                wing: wing.clone(),
                room: room.clone(),
                dry_run: true,
                error: None,
            });
            continue;
        }

        let update = DocumentMetaUpdate {
            wing: Some(wing.clone()),
            room: room.clone(),
            ..Default::default()
        };
        match store.update_document_meta(doc_id, &update) {
            Ok(Some(_)) => {
                applied_ok += 1;
                applied.push(RefileApplyResult {
                    document_id: doc_id.into(),
                    wing,
                    room,
                    dry_run: false,
                    error: None,
                });
            }
            Ok(None) => {
                applied_failed += 1;
                applied.push(RefileApplyResult {
                    document_id: doc_id.into(),
                    wing,
                    room,
                    dry_run: false,
                    error: Some("document not found".into()),
                });
            }
            Err(e) => {
                applied_failed += 1;
                applied.push(RefileApplyResult {
                    document_id: doc_id.into(),
                    wing,
                    room,
                    dry_run: false,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    if opts.rebuild_index && actions.iter().any(|a| a.action == MaintenanceAction::RebuildIndex) {
        if dry_run {
            let n = store.list_documents_by_layer("wiki")?.len();
            rebuild_index_entries = Some(n);
        } else if applied_ok > 0 || suggestions.is_empty() {
            let n = store.rebuild_wiki_index_from_docs()?;
            rebuild_index_entries = Some(n);
        }
    }

    let notes = build_notes(
        opts.mode,
        unscoped_total,
        &suggestions,
        min_confidence,
        dry_run,
        llm.is_some() && config.llm_enabled,
    );

    let mut report = OrganizeReport {
        dry_run,
        mode: opts.mode.as_str().into(),
        unscoped_total,
        unscoped_considered,
        skipped_cap,
        suggestions,
        actions,
        applied,
        applied_ok,
        applied_failed,
        rebuild_index_entries,
        notes: Some(notes),
        ops_log_id: None,
        ops_log_seq: None,
    };

    let op = if dry_run {
        "maintain_organize_dry_run"
    } else {
        "maintain_organize"
    };
    let message = if dry_run {
        format!(
            "dry_run maintain_organize: {} suggestions, {} actions, {} unscoped",
            report.suggestions.len(),
            report.actions.len(),
            unscoped_total
        )
    } else {
        format!(
            "maintain_organize: applied_ok={} failed={} suggestions={}",
            applied_ok,
            applied_failed,
            report.suggestions.len()
        )
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
        entity_kind: Some("maintenance".into()),
        payload_json: payload,
        agent_name: opts.agent.clone(),
    })?;
    report.ops_log_id = Some(written.id);
    report.ops_log_seq = Some(written.seq);

    Ok(report)
}

fn build_notes(
    mode: OrganizeMode,
    unscoped_total: usize,
    suggestions: &[RefileSuggestion],
    min_confidence: f64,
    dry_run: bool,
    llm_ready: bool,
) -> String {
    let above: usize = suggestions
        .iter()
        .filter(|s| s.confidence >= min_confidence)
        .count();
    format!(
        "mode={} dry_run={} llm_ready={} unscoped={} suggestions={} actionable>={:.2}:{}{}",
        mode.as_str(),
        dry_run,
        llm_ready,
        unscoped_total,
        suggestions.len(),
        min_confidence,
        above,
        if dry_run {
            "; re-run with dry_run=false to apply refiles"
        } else {
            ""
        }
    )
}

/// Active (non-archived) documents with empty/null wing, excluding system layers.
fn list_unscoped_documents(store: &Store) -> Result<Vec<Document>> {
    let docs = store.list_documents()?;
    let mut out = Vec::new();
    for d in docs {
        if SKIP_LAYERS.iter().any(|l| d.layer == *l) {
            continue;
        }
        let status = d.status.trim().to_ascii_lowercase();
        if status == "archived" || status == "tombstone" {
            continue;
        }
        let wing_empty = d
            .wing
            .as_ref()
            .map(|w| w.trim().is_empty())
            .unwrap_or(true);
        if wing_empty {
            out.push(d);
        }
    }
    // Prefer recently updated first so maintain touches hot docs under cap.
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.title.cmp(&b.title)));
    Ok(out)
}

fn suggest_heuristic(
    store: &Store,
    taxonomy: &Taxonomy,
    unscoped: &[Document],
) -> Result<Vec<RefileSuggestion>> {
    if unscoped.is_empty() {
        return Ok(Vec::new());
    }

    let wing_names: Vec<String> = taxonomy.wings.iter().map(|w| w.wing.clone()).collect();
    let room_by_wing: HashMap<String, Vec<String>> = taxonomy
        .wings
        .iter()
        .map(|w| {
            (
                w.wing.clone(),
                w.rooms.iter().map(|r| r.room.clone()).collect(),
            )
        })
        .collect();

    // Preload embeddings for kNN labels when corpus has labeled docs.
    let labeled = store.list_documents()?.into_iter().filter(|d| {
        d.wing
            .as_ref()
            .map(|w| !w.trim().is_empty())
            .unwrap_or(false)
            && !SKIP_LAYERS.iter().any(|l| d.layer == *l)
    });
    let first_emb = first_chunk_embeddings(store)?;
    let labeled_emb: Vec<(String, String, Option<String>, Vec<f32>)> = labeled
        .filter_map(|d| {
            let emb = first_emb.get(&d.id)?.clone();
            let wing = d.wing?;
            Some((d.id, wing, d.room, emb))
        })
        .collect();

    let mut out = Vec::with_capacity(unscoped.len());
    for doc in unscoped {
        if let Some(s) = heuristic_path(doc, &wing_names, &room_by_wing) {
            out.push(s);
            continue;
        }
        if let Some(s) = heuristic_title(doc, &wing_names, &room_by_wing) {
            out.push(s);
            continue;
        }
        if let Some(s) = heuristic_embedding(doc, &first_emb, &labeled_emb) {
            out.push(s);
            continue;
        }
        // No signal: skip (no forced default wing).
    }
    Ok(out)
}

fn heuristic_path(
    doc: &Document,
    wing_names: &[String],
    room_by_wing: &HashMap<String, Vec<String>>,
) -> Option<RefileSuggestion> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(ref src) = doc.source_file {
        candidates.extend(path_segments(src));
    }
    candidates.extend(path_segments(&doc.uri));

    if candidates.is_empty() {
        return None;
    }

    // Prefer known taxonomy wings appearing in path segments.
    for seg in &candidates {
        if let Some(wing) = match_known(seg, wing_names) {
            let room = candidates
                .iter()
                .filter(|s| s.as_str() != wing.as_str())
                .find_map(|s| {
                    room_by_wing
                        .get(&wing)
                        .and_then(|rooms| match_known(s, rooms))
                });
            return Some(RefileSuggestion {
                document_id: doc.id.clone(),
                title: doc.title.clone(),
                uri: doc.uri.clone(),
                current_wing: doc.wing.clone(),
                current_room: doc.room.clone(),
                suggested_wing: wing,
                suggested_room: room.or_else(|| doc.room.clone()),
                confidence: 0.9,
                method: "heuristic_path".into(),
                reason: format!("path/uri segment matched existing wing"),
            });
        }
    }

    // Propose first meaningful segment as a new wing (lower confidence).
    if let Some(wing) = candidates.first() {
        if is_meaningful_segment(wing) {
            return Some(RefileSuggestion {
                document_id: doc.id.clone(),
                title: doc.title.clone(),
                uri: doc.uri.clone(),
                current_wing: doc.wing.clone(),
                current_room: doc.room.clone(),
                suggested_wing: wing.clone(),
                suggested_room: candidates.get(1).filter(|s| is_meaningful_segment(s)).cloned(),
                confidence: 0.55,
                method: "heuristic_path".into(),
                reason: "path/uri first segment proposed as wing (not in taxonomy yet)".into(),
            });
        }
    }
    None
}

fn heuristic_title(
    doc: &Document,
    wing_names: &[String],
    room_by_wing: &HashMap<String, Vec<String>>,
) -> Option<RefileSuggestion> {
    if wing_names.is_empty() {
        return None;
    }
    let title_l = doc.title.to_ascii_lowercase();
    let mut best_wing: Option<(String, usize)> = None;
    for w in wing_names {
        let wl = w.to_ascii_lowercase();
        if wl.len() >= 2 && title_l.contains(&wl) {
            let score = wl.len();
            if best_wing.as_ref().map(|(_, s)| *s).unwrap_or(0) < score {
                best_wing = Some((w.clone(), score));
            }
        }
    }
    let (wing, _) = best_wing?;
    let room = room_by_wing.get(&wing).and_then(|rooms| {
        rooms.iter().find_map(|r| {
            let rl = r.to_ascii_lowercase();
            if rl.len() >= 2 && title_l.contains(&rl) {
                Some(r.clone())
            } else {
                None
            }
        })
    });
    Some(RefileSuggestion {
        document_id: doc.id.clone(),
        title: doc.title.clone(),
        uri: doc.uri.clone(),
        current_wing: doc.wing.clone(),
        current_room: doc.room.clone(),
        suggested_wing: wing,
        suggested_room: room.or_else(|| doc.room.clone()),
        confidence: 0.7,
        method: "heuristic_title".into(),
        reason: "document title contains an existing wing name".into(),
    })
}

fn heuristic_embedding(
    doc: &Document,
    first_emb: &BTreeMap<String, Vec<f32>>,
    labeled_emb: &[(String, String, Option<String>, Vec<f32>)],
) -> Option<RefileSuggestion> {
    if labeled_emb.is_empty() {
        return None;
    }
    let emb = first_emb.get(&doc.id)?;
    let mut scored: Vec<(f32, &str, Option<&str>)> = Vec::new();
    for (_id, wing, room, v) in labeled_emb {
        let cos = cosine_similarity(emb, v);
        if cos >= EMBED_MIN_COSINE {
            scored.push((cos, wing.as_str(), room.as_deref()));
        }
    }
    if scored.is_empty() {
        return None;
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(EMBED_NEIGHBOR_K);

    let mut wing_votes: HashMap<&str, f32> = HashMap::new();
    let mut room_votes: HashMap<(&str, &str), f32> = HashMap::new();
    for (cos, wing, room) in &scored {
        *wing_votes.entry(*wing).or_default() += *cos;
        if let Some(r) = room {
            *room_votes.entry((*wing, *r)).or_default() += *cos;
        }
    }
    let (best_wing, _wing_score) = wing_votes.into_iter().max_by(|a, b| {
        a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
    })?;
    let best_room = room_votes
        .into_iter()
        .filter(|((w, _), _)| *w == best_wing)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|((_, r), _)| r.to_string());

    let top_cos = scored.first().map(|s| s.0).unwrap_or(0.0);
    let confidence = (0.45 + 0.45 * f64::from(top_cos)).clamp(0.0, 0.92);

    Some(RefileSuggestion {
        document_id: doc.id.clone(),
        title: doc.title.clone(),
        uri: doc.uri.clone(),
        current_wing: doc.wing.clone(),
        current_room: doc.room.clone(),
        suggested_wing: best_wing.to_string(),
        suggested_room: best_room.or_else(|| doc.room.clone()),
        confidence,
        method: "heuristic_embedding".into(),
        reason: format!(
            "embedding kNN vote wing={} (top_cos={:.3}, neighbors={})",
            best_wing,
            top_cos,
            scored.len()
        ),
    })
}

fn first_chunk_embeddings(store: &Store) -> Result<BTreeMap<String, Vec<f32>>> {
    let chunks = store.all_chunks_with_embeddings()?;
    let mut first: BTreeMap<String, Vec<f32>> = BTreeMap::new();
    for c in chunks {
        first.entry(c.document_id).or_insert(c.embedding);
    }
    Ok(first)
}

fn path_segments(path: &str) -> Vec<String> {
    let path = path.trim();
    if path.is_empty() {
        return Vec::new();
    }
    // Strip scheme for uris like file:///… or raw://…
    let stripped = path
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(path);
    let stripped = stripped.trim_start_matches('/');
    stripped
        .split(['/', '\\'])
        .filter_map(|s| {
            let s = s.trim();
            if s.is_empty() || !is_meaningful_segment(s) {
                None
            } else {
                // Drop file extension on last segment-like tokens later; keep as-is for matching.
                let no_ext = s.rsplit_once('.').map(|(stem, ext)| {
                    if ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
                        stem
                    } else {
                        s
                    }
                }).unwrap_or(s);
                if is_meaningful_segment(no_ext) {
                    Some(no_ext.to_string())
                } else {
                    None
                }
            }
        })
        .collect()
}

fn is_meaningful_segment(s: &str) -> bool {
    let s = s.trim();
    if s.len() < 2 {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    let noise = matches!(
        lower.as_str(),
        "src"
            | "lib"
            | "bin"
            | "docs"
            | "doc"
            | "test"
            | "tests"
            | "tmp"
            | "temp"
            | "home"
            | "users"
            | "user"
            | "var"
            | "etc"
            | "opt"
            | "usr"
            | "dev"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | "out"
            | "vendor"
            | "raw"
            | "wiki"
            | "diary"
            | "file"
            | "text"
            | "http"
            | "https"
            | "localhost"
            | "documents"
            | "downloads"
            | "desktop"
    );
    !noise && !s.chars().all(|c| c.is_ascii_digit())
}

fn match_known(seg: &str, known: &[String]) -> Option<String> {
    let sl = seg.to_ascii_lowercase();
    known
        .iter()
        .find(|k| k.to_ascii_lowercase() == sl)
        .cloned()
}

/// LLM JSON shape for organize suggestions.
#[derive(Debug, Clone, Deserialize)]
struct LlmOrganizePayload {
    #[serde(default)]
    suggestions: Vec<LlmRefileItem>,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct LlmRefileItem {
    document_id: String,
    wing: String,
    #[serde(default)]
    room: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    confidence: Option<f64>,
}

async fn suggest_llm(
    client: &ChatClient,
    taxonomy: &Taxonomy,
    unscoped: &[Document],
) -> Result<Vec<RefileSuggestion>> {
    if unscoped.is_empty() {
        return Ok(Vec::new());
    }
    let tax_json = serde_json::to_string_pretty(taxonomy).unwrap_or_else(|_| "{}".into());
    let samples: Vec<serde_json::Value> = unscoped
        .iter()
        .take(LLM_SAMPLE_CAP)
        .map(|d| {
            serde_json::json!({
                "document_id": d.id,
                "title": d.title,
                "uri": d.uri,
                "layer": d.layer,
                "kind": d.kind,
                "source_file": d.source_file,
                "room": d.room,
            })
        })
        .collect();
    let sample_json = serde_json::to_string_pretty(&samples).unwrap_or_else(|_| "[]".into());

    let system = r#"You organize a personal knowledge base into wings (top shelves) and rooms (sub-areas).
Given taxonomy and unscoped documents (missing wing), propose refile targets.

Rules:
- Prefer existing wing/room names from the taxonomy when they fit.
- You may propose a new wing only when nothing fits; use short kebab-case or single words.
- Never invent document_ids; only use ids from the input list.
- Return ONLY valid JSON (no markdown fences):
{
  "suggestions": [
    {
      "document_id": "...",
      "wing": "...",
      "room": "optional",
      "reason": "short",
      "confidence": 0.0
    }
  ],
  "notes": "optional"
}
confidence is 0..1.
"#;
    let user = format!(
        "## Taxonomy\n\n{tax}\n\n## Unscoped documents (missing wing)\n\n{docs}\n",
        tax = truncate(&tax_json, 8000),
        docs = truncate(&sample_json, 16_000),
    );
    let messages = [
        ChatMessage {
            role: "system".into(),
            content: system.into(),
        },
        ChatMessage {
            role: "user".into(),
            content: user,
        },
    ];
    let raw = client.complete(&messages).await?;
    let payload = parse_organize_json(&raw)?;
    let by_id: HashMap<&str, &Document> =
        unscoped.iter().map(|d| (d.id.as_str(), d)).collect();

    let mut out = Vec::new();
    for item in payload.suggestions {
        let id = item.document_id.trim();
        let wing = item.wing.trim();
        if id.is_empty() || wing.is_empty() {
            continue;
        }
        let Some(doc) = by_id.get(id) else {
            continue;
        };
        let conf = item.confidence.unwrap_or(0.75).clamp(0.0, 1.0);
        let room = item
            .room
            .as_ref()
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty());
        out.push(RefileSuggestion {
            document_id: doc.id.clone(),
            title: doc.title.clone(),
            uri: doc.uri.clone(),
            current_wing: doc.wing.clone(),
            current_room: doc.room.clone(),
            suggested_wing: wing.to_string(),
            suggested_room: room.or_else(|| doc.room.clone()),
            confidence: conf,
            method: "llm".into(),
            reason: item
                .reason
                .unwrap_or_else(|| "llm organize suggestion".into()),
        });
    }
    if let Some(notes) = payload.notes {
        tracing::debug!(%notes, "organize llm notes");
    }
    Ok(out)
}

fn parse_organize_json(raw: &str) -> Result<LlmOrganizePayload> {
    let trimmed = raw.trim();
    let unfenced = if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest
            .strip_prefix("json")
            .or_else(|| rest.strip_prefix("JSON"))
            .unwrap_or(rest);
        let rest = rest.trim_start_matches('\n');
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else {
        trimmed
    };
    if let Ok(v) = serde_json::from_str::<LlmOrganizePayload>(unfenced) {
        return Ok(v);
    }
    if let (Some(start), Some(end)) = (unfenced.find('{'), unfenced.rfind('}')) {
        if end > start {
            let slice = &unfenced[start..=end];
            if let Ok(v) = serde_json::from_str::<LlmOrganizePayload>(slice) {
                return Ok(v);
            }
        }
    }
    Err(AppError::llm(format!(
        "failed to parse organize JSON from model output: {}",
        truncate(raw, 400)
    )))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Chunk, SearchMode};
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

    fn seed_doc(
        store: &Store,
        id: &str,
        title: &str,
        content: &str,
        wing: Option<&str>,
        room: Option<&str>,
        source_file: Option<&str>,
        uri: &str,
    ) {
        let now = Utc::now();
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: uri.into(),
                title: title.into(),
                content: content.into(),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                wing: wing.map(|s| s.into()),
                room: room.map(|s| s.into()),
                source_file: source_file.map(|s| s.into()),
                layer: "raw".into(),
                kind: "document".into(),
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
    fn path_segments_extracts_meaningful() {
        let segs = path_segments("/Users/me/Documents/Sources/research/rag/note.md");
        assert!(segs.iter().any(|s| s == "research"));
        assert!(segs.iter().any(|s| s == "rag"));
        assert!(segs.iter().any(|s| s == "note"));
        assert!(!segs.iter().any(|s| s == "Users" || s == "Documents"));
    }

    #[test]
    fn mode_parse() {
        assert_eq!(OrganizeMode::parse("heuristic").unwrap(), OrganizeMode::Heuristic);
        assert_eq!(OrganizeMode::parse("LLM").unwrap(), OrganizeMode::Llm);
        assert_eq!(OrganizeMode::parse("auto").unwrap(), OrganizeMode::Auto);
        assert!(OrganizeMode::parse("nope").is_err());
    }

    #[tokio::test]
    async fn dry_run_suggests_path_refile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("org.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);

        seed_doc(
            &store,
            "placed",
            "Placed",
            "already filed",
            Some("research"),
            Some("rag"),
            Some("/vault/research/rag/placed.md"),
            "file:///vault/research/rag/placed.md",
        );
        seed_doc(
            &store,
            "loose",
            "Loose note",
            "needs a wing",
            None,
            None,
            Some("/vault/research/rag/loose.md"),
            "file:///vault/research/rag/loose.md",
        );

        let opts = OrganizeOptions {
            dry_run: true,
            mode: OrganizeMode::Heuristic,
            max_docs: 50,
            min_confidence: 0.5,
            rebuild_index: false,
            agent: None,
        };
        let report = maintain_organize(&store, &cfg, None, &opts)
            .await
            .unwrap();
        assert!(report.dry_run);
        assert_eq!(report.unscoped_total, 1);
        assert!(!report.suggestions.is_empty());
        let s = &report.suggestions[0];
        assert_eq!(s.document_id, "loose");
        assert_eq!(s.suggested_wing, "research");
        assert_eq!(s.suggested_room.as_deref(), Some("rag"));
        assert!(s.confidence >= 0.5);
        assert!(report.actions.iter().all(|a| a.action == MaintenanceAction::Refile));
        assert_eq!(report.applied_ok, report.actions.len());
        // dry_run: wing still empty
        let doc = store.get_document("loose").unwrap().unwrap();
        assert!(doc.wing.is_none());
        let ops = store.list_ops_log(5).unwrap();
        assert!(ops.iter().any(|o| o.op == "maintain_organize_dry_run"));
    }

    #[tokio::test]
    async fn apply_refiles_unscoped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("org_apply.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);

        seed_doc(
            &store,
            "a",
            "Alpha research notes",
            "body a",
            Some("research"),
            Some("core"),
            None,
            "text://a",
        );
        seed_doc(
            &store,
            "b",
            "research follow-up",
            "body b",
            None,
            None,
            None,
            "text://b",
        );

        let opts = OrganizeOptions {
            dry_run: false,
            mode: OrganizeMode::Heuristic,
            max_docs: 50,
            min_confidence: 0.5,
            rebuild_index: false,
            agent: Some("test-agent".into()),
        };
        let report = maintain_organize(&store, &cfg, None, &opts)
            .await
            .unwrap();
        assert!(!report.dry_run);
        assert!(report.applied_ok >= 1);
        let doc = store.get_document("b").unwrap().unwrap();
        assert_eq!(doc.wing.as_deref(), Some("research"));
        let ops = store.list_ops_log(5).unwrap();
        assert!(ops.iter().any(|o| o.op == "maintain_organize"));
    }

    #[tokio::test]
    async fn no_unscoped_is_empty_report() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty_org.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);
        seed_doc(
            &store,
            "x",
            "X",
            "body",
            Some("ops"),
            Some("run"),
            None,
            "text://x",
        );
        let opts = OrganizeOptions::from_config(&cfg);
        let report = maintain_organize(&store, &cfg, None, &opts)
            .await
            .unwrap();
        assert_eq!(report.unscoped_total, 0);
        assert!(report.suggestions.is_empty());
        assert!(report.actions.is_empty());
    }

    #[tokio::test]
    async fn respects_max_docs_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cap_org.duckdb");
        let store = Store::open(&path).unwrap();
        let cfg = test_config(path);
        seed_doc(
            &store,
            "seed",
            "research seed",
            "s",
            Some("research"),
            None,
            None,
            "text://seed",
        );
        for i in 0..5 {
            seed_doc(
                &store,
                &format!("u{i}"),
                &format!("research item {i}"),
                "body",
                None,
                None,
                None,
                &format!("text://u{i}"),
            );
        }
        let opts = OrganizeOptions {
            dry_run: true,
            mode: OrganizeMode::Heuristic,
            max_docs: 2,
            min_confidence: 0.5,
            rebuild_index: false,
            agent: None,
        };
        let report = maintain_organize(&store, &cfg, None, &opts)
            .await
            .unwrap();
        assert_eq!(report.unscoped_total, 5);
        assert_eq!(report.unscoped_considered, 2);
        assert_eq!(report.skipped_cap, 3);
    }

    #[test]
    fn parse_organize_json_fenced() {
        let raw = r#"```json
{"suggestions":[{"document_id":"d1","wing":"research","room":"rag","confidence":0.8}]}
```"#;
        let p = parse_organize_json(raw).unwrap();
        assert_eq!(p.suggestions.len(), 1);
        assert_eq!(p.suggestions[0].wing, "research");
    }
}
