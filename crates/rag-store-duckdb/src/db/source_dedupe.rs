//! Precise cleanup for legacy source-document duplicates over `lin`.
//!
//! This is intentionally narrower than content-hash deduplication: a group is
//! actionable only when active raw documents share the same non-empty
//! `(source_file, content_hash)` and exactly one document has the canonical
//! `file://<source_file>` URI. All discovery, graph rewiring, manifest repair,
//! and deletion happens in one `lin` transaction.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::lin_engine::{
    LinReader, LinWriter, RecBuilder, cell_f64, cell_nonempty_string, cell_opt_string, cell_str,
    cell_string, cell_ts_or_now,
};
use super::rows;
use super::store::{Store, delete_document_locked};
use crate::error::{AppError, Result};
use crate::models::Document;
use crate::util::format_db_timestamp;

/// Keep MCP responses bounded even when a damaged store contains a very large
/// duplicate population. Totals in [`SourceDuplicateCleanupReport`] always
/// cover the full scan; only the diagnostic detail vectors are truncated.
const REPORT_GROUP_LIMIT: usize = 50;
const REPORT_MEMBER_LIMIT: usize = 200;

/// One member of a same-source, same-hash duplicate group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDuplicateDocument {
    pub document_id: String,
    pub uri: String,
    pub title: String,
    pub canonical_uri_match: bool,
    pub chunks: u64,
    pub graph_nodes: u64,
    pub incident_graph_edges: u64,
    pub wiki_references: u64,
    pub collection_references: u64,
    pub foreign_source_manifest_references: u64,
    pub revisions: u64,
}

/// One duplicate group and the safety decision made for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDuplicateGroup {
    pub source_file: String,
    pub content_hash: String,
    pub document_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub survivor_document_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_manifest_document_id: Option<String>,
    pub candidate_count: u64,
    /// Candidates selected for this bounded batch (or the next batch in a preview).
    pub batch_candidate_count: u64,
    /// Candidates that remain in this group after the selected batch.
    pub remaining_candidate_count: u64,
    pub actionable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip_reasons: Vec<String>,
    pub members: Vec<SourceDuplicateDocument>,
    /// True when `document_count` is larger than the returned `members` detail.
    pub members_truncated: bool,
}

/// Transactional result for a source duplicate preview or apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDuplicateCleanupReport {
    pub success: bool,
    /// Terminal derived-index failures after committed cleanup work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    pub dry_run: bool,
    pub confirmed: bool,
    pub max_candidates: usize,
    pub scanned_active_raw_documents: u64,
    pub matched_groups: usize,
    pub actionable_groups: usize,
    pub skipped_groups: usize,
    pub candidate_count: u64,
    /// Number of actionable groups touched by this batch (or next preview batch).
    pub batch_groups: usize,
    /// Number of documents selected by this bounded batch.
    pub batch_candidates: u64,
    /// Actionable groups left after this batch (or that would be left by preview).
    pub remaining_groups: usize,
    /// Actionable documents left after this batch (or that would be left by preview).
    pub remaining_candidates: u64,
    pub cap_exceeded: bool,
    pub applied: bool,
    pub deleted_documents: u64,
    pub rewired_graph_edge_endpoints: u64,
    pub deduplicated_graph_edges: u64,
    /// Edges removed because endpoint collapse would otherwise create a new self-loop.
    pub removed_graph_self_edges: u64,
    pub rewired_kg_facts: u64,
    pub rebound_source_manifests: u64,
    /// Number of group details actually included in `groups`.
    pub reported_groups: usize,
    /// True when `matched_groups` is larger than `reported_groups`.
    pub groups_truncated: bool,
    /// Number of member details actually included across all reported groups.
    pub reported_members: usize,
    /// True when any matched member detail was omitted.
    pub members_truncated: bool,
    pub groups: Vec<SourceDuplicateGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GroupKey {
    source_file: String,
    content_hash: String,
}

#[derive(Debug, Clone)]
struct RawMemberBase {
    id: String,
    uri: String,
    title: String,
    source_file: String,
    content_hash: String,
    canonical_uri_match: bool,
}

#[derive(Debug, Clone)]
struct EnrichedMember {
    base: RawMemberBase,
    chunks: u64,
    wiki_references: u64,
    collection_references: u64,
    foreign_source_manifest_references: u64,
    revisions: u64,
}

#[derive(Debug, Clone)]
struct GroupMetrics {
    key: GroupKey,
    document_count: u64,
    canonical_count: u64,
    survivor_document_id: Option<String>,
    source_manifest_document_id: Option<String>,
    canonical_chunks: u64,
    candidate_chunks: u64,
    candidate_wiki_references: u64,
    candidate_collection_references: u64,
    candidate_foreign_manifest_references: u64,
    candidate_revisions: u64,
    source_manifest_owner_is_member: bool,
    actionable: bool,
    candidate_count: u64,
    members: Vec<EnrichedMember>,
}

#[derive(Debug, Clone)]
struct BatchSelection {
    metrics: GroupMetrics,
    candidate_ids: Vec<String>,
}

fn is_active_raw_source_doc(row: &lin::Row) -> Option<RawMemberBase> {
    let layer = cell_str(row, "layer")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("raw");
    if !layer.eq_ignore_ascii_case("raw") {
        return None;
    }
    let status = cell_str(row, "status")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("active");
    if !status.eq_ignore_ascii_case("active") {
        return None;
    }
    let source_file = cell_str(row, "source_file").filter(|s| !s.trim().is_empty())?;
    let content_hash = cell_str(row, "content_hash").filter(|s| !s.trim().is_empty())?;
    let uri = cell_string(row, "uri");
    let canonical_uri_match = uri == format!("file://{source_file}");
    Some(RawMemberBase {
        id: cell_string(row, "id"),
        uri,
        title: cell_string(row, "title"),
        source_file: source_file.to_string(),
        content_hash: content_hash.to_string(),
        canonical_uri_match,
    })
}

fn classify_all_groups(r: &impl LinReader) -> Result<(u64, Vec<GroupMetrics>)> {
    let mut scanned_active_raw_documents = 0u64;
    let mut by_key: BTreeMap<GroupKey, Vec<RawMemberBase>> = BTreeMap::new();

    r.scan_each_projected(
        "documents",
        &[
            "id",
            "uri",
            "title",
            "layer",
            "status",
            "source_file",
            "content_hash",
        ],
        |row| {
            if let Some(member) = is_active_raw_source_doc(row) {
                scanned_active_raw_documents += 1;
                let key = GroupKey {
                    source_file: member.source_file.clone(),
                    content_hash: member.content_hash.clone(),
                };
                by_key.entry(key).or_default().push(member);
            }
            Ok(true)
        },
    )?;

    by_key.retain(|_, members| members.len() > 1);
    if by_key.is_empty() {
        return Ok((scanned_active_raw_documents, Vec::new()));
    }

    let mut duplicate_ids: HashSet<String> = HashSet::new();
    for members in by_key.values() {
        for m in members {
            duplicate_ids.insert(m.id.clone());
        }
    }

    // Wiki references (UNION over row_id + referenced_document_id)
    let mut wiki_refs: HashMap<String, HashSet<String>> = HashMap::new();
    r.scan_each("wiki_index", |row| {
        let rid = cell_string(row, "id");
        if let Some(did) = cell_nonempty_string(row, "document_id") {
            if duplicate_ids.contains(&did) {
                wiki_refs.entry(did).or_default().insert(rid.clone());
            }
        }
        if let Some(pid) = cell_nonempty_string(row, "page_id") {
            if duplicate_ids.contains(&pid) {
                wiki_refs.entry(pid).or_default().insert(rid);
            }
        }
        Ok(true)
    })?;

    // Collection entry references
    let mut col_entry_refs: HashMap<String, HashSet<(String, String)>> = HashMap::new();
    r.scan_each("collection_entries", |row| {
        let cid = cell_string(row, "collection_id");
        let edid = cell_string(row, "document_id");
        if duplicate_ids.contains(&edid) {
            col_entry_refs
                .entry(edid.clone())
                .or_default()
                .insert((cid.clone(), edid.clone()));
        }
        if let Some(parent_id) = cell_nonempty_string(row, "parent_document_id") {
            if duplicate_ids.contains(&parent_id) {
                col_entry_refs
                    .entry(parent_id)
                    .or_default()
                    .insert((cid, edid));
            }
        }
        Ok(true)
    })?;

    // Collection dependency references
    let mut col_dep_refs: HashMap<String, HashSet<(String, String, String)>> = HashMap::new();
    r.scan_each("collection_dependencies", |row| {
        let cid = cell_string(row, "collection_id");
        let did = cell_string(row, "document_id");
        let dep = cell_string(row, "depends_on_document_id");
        if duplicate_ids.contains(&did) {
            col_dep_refs.entry(did.clone()).or_default().insert((
                cid.clone(),
                did.clone(),
                dep.clone(),
            ));
        }
        if duplicate_ids.contains(&dep) {
            col_dep_refs.entry(dep).or_default().insert((
                cid,
                did,
                cell_string(row, "depends_on_document_id"),
            ));
        }
        Ok(true)
    })?;

    // Source manifest map (canonical_path -> document_id, and document_id -> paths)
    let mut manifest_by_path: HashMap<String, String> = HashMap::new();
    let mut manifest_paths_by_doc: HashMap<String, Vec<String>> = HashMap::new();
    r.scan_each("source_manifest", |row| {
        let cp = cell_string(row, "canonical_path");
        let did = cell_string(row, "document_id");
        manifest_by_path.insert(cp.clone(), did.clone());
        if duplicate_ids.contains(&did) {
            manifest_paths_by_doc.entry(did).or_default().push(cp);
        }
        Ok(true)
    })?;

    let mut all_metrics = Vec::with_capacity(by_key.len());
    for (key, mut raw_members) in by_key {
        raw_members.sort_by(|a, b| {
            b.canonical_uri_match
                .cmp(&a.canonical_uri_match)
                .then_with(|| a.id.cmp(&b.id))
        });

        let source_manifest_document_id = manifest_by_path.get(&key.source_file).cloned();
        let mut enriched = Vec::with_capacity(raw_members.len());
        for base in raw_members {
            let mut chunks = 0u64;
            r.for_each_by_eq_projected("chunks", "document_id", &base.id, Some(&["id"]), |_| {
                chunks += 1;
                Ok(true)
            })?;
            let mut revisions = 0u64;
            r.for_each_by_eq_projected(
                "document_revisions",
                "document_id",
                &base.id,
                Some(&["id"]),
                |_| {
                    revisions += 1;
                    Ok(true)
                },
            )?;
            let wiki_references = wiki_refs.get(&base.id).map_or(0, |s| s.len() as u64);
            let collection_references = col_entry_refs.get(&base.id).map_or(0, |s| s.len() as u64)
                + col_dep_refs.get(&base.id).map_or(0, |s| s.len() as u64);
            let foreign_source_manifest_references =
                manifest_paths_by_doc.get(&base.id).map_or(0, |paths| {
                    paths.iter().filter(|p| *p != &base.source_file).count() as u64
                });

            enriched.push(EnrichedMember {
                base,
                chunks,
                wiki_references,
                collection_references,
                foreign_source_manifest_references,
                revisions,
            });
        }

        let document_count = enriched.len() as u64;
        let mut canonical_count = 0u64;
        let mut survivor_document_id: Option<String> = None;
        let mut raw_candidate_count = 0u64;
        let mut canonical_chunks = 0u64;
        let mut candidate_chunks = 0u64;
        let mut candidate_wiki_references = 0u64;
        let mut candidate_collection_references = 0u64;
        let mut candidate_foreign_manifest_references = 0u64;
        let mut candidate_revisions = 0u64;
        let mut source_manifest_owner_members = 0u64;

        for m in &enriched {
            if m.base.canonical_uri_match {
                canonical_count += 1;
                if survivor_document_id
                    .as_ref()
                    .is_none_or(|prev| &m.base.id > prev)
                {
                    survivor_document_id = Some(m.base.id.clone());
                }
                canonical_chunks += m.chunks;
            } else {
                raw_candidate_count += 1;
                candidate_chunks += m.chunks;
                candidate_wiki_references += m.wiki_references;
                candidate_collection_references += m.collection_references;
                candidate_foreign_manifest_references += m.foreign_source_manifest_references;
                candidate_revisions += m.revisions;
            }
            if source_manifest_document_id.as_deref() == Some(m.base.id.as_str()) {
                source_manifest_owner_members += 1;
            }
        }

        let source_manifest_owner_is_member =
            source_manifest_document_id.is_none() || source_manifest_owner_members > 0;
        let actionable = canonical_count == 1
            && raw_candidate_count > 0
            && !(canonical_chunks == 0 && candidate_chunks > 0)
            && candidate_wiki_references == 0
            && candidate_collection_references == 0
            && candidate_foreign_manifest_references == 0
            && candidate_revisions == 0
            && source_manifest_owner_is_member;
        let candidate_count = if actionable { raw_candidate_count } else { 0 };

        all_metrics.push(GroupMetrics {
            key,
            document_count,
            canonical_count,
            survivor_document_id,
            source_manifest_document_id,
            canonical_chunks,
            candidate_chunks,
            candidate_wiki_references,
            candidate_collection_references,
            candidate_foreign_manifest_references,
            candidate_revisions,
            source_manifest_owner_is_member,
            actionable,
            candidate_count,
            members: enriched,
        });
    }

    Ok((scanned_active_raw_documents, all_metrics))
}

impl Store {
    /// Preview or atomically remove legacy raw documents that duplicate the
    /// canonical `file://<source_file>` document.
    pub fn cleanup_source_duplicates(
        &self,
        dry_run: bool,
        confirm: bool,
        max_candidates: usize,
    ) -> Result<SourceDuplicateCleanupReport> {
        if max_candidates == 0 {
            return Err(AppError::config(
                "cleanup_source_duplicates max_candidates must be >= 1",
            ));
        }
        if !dry_run && !confirm {
            return Err(AppError::config(
                "cleanup_source_duplicates apply requires confirm=true",
            ));
        }

        self.with_tx(|tx| {
            let (scanned_active_raw_documents, all_metrics) = classify_all_groups(tx)?;
            let matched_groups = all_metrics.len();
            let actionable_groups = all_metrics.iter().filter(|m| m.actionable).count();
            let total_candidate_count: u64 = all_metrics.iter().map(|m| m.candidate_count).sum();

            // Select bounded batch in (source_file, content_hash, member.id) order.
            let mut batch: Vec<BatchSelection> = Vec::new();
            let mut remaining_budget = max_candidates;
            for metric in &all_metrics {
                if !metric.actionable || remaining_budget == 0 {
                    continue;
                }
                let mut cands: Vec<String> = metric
                    .members
                    .iter()
                    .filter(|m| !m.base.canonical_uri_match)
                    .map(|m| m.base.id.clone())
                    .collect();
                cands.sort();
                if cands.len() > remaining_budget {
                    cands.truncate(remaining_budget);
                }
                if !cands.is_empty() {
                    remaining_budget -= cands.len();
                    batch.push(BatchSelection {
                        metrics: metric.clone(),
                        candidate_ids: cands,
                    });
                }
            }

            let batch_candidates: u64 = batch.iter().map(|s| s.candidate_ids.len() as u64).sum();
            let fully_drained_groups = batch
                .iter()
                .filter(|s| s.candidate_ids.len() as u64 >= s.metrics.candidate_count)
                .count();
            let remaining_candidates = total_candidate_count.saturating_sub(batch_candidates);
            let remaining_groups = actionable_groups.saturating_sub(fully_drained_groups);
            let cap_exceeded = remaining_candidates > 0;

            let groups = build_report_groups(tx, &batch, &all_metrics)?;
            let reported_groups = groups.len();
            let groups_truncated = reported_groups < matched_groups;
            let reported_members = groups.iter().map(|g| g.members.len()).sum();
            let members_truncated = groups_truncated || groups.iter().any(|g| g.members_truncated);

            let mut report = SourceDuplicateCleanupReport {
                success: true,
                errors: Vec::new(),
                dry_run,
                confirmed: confirm,
                max_candidates,
                scanned_active_raw_documents,
                matched_groups,
                actionable_groups,
                skipped_groups: matched_groups.saturating_sub(actionable_groups),
                candidate_count: total_candidate_count,
                batch_groups: batch.len(),
                batch_candidates,
                remaining_groups,
                remaining_candidates,
                cap_exceeded,
                applied: false,
                deleted_documents: 0,
                rewired_graph_edge_endpoints: 0,
                deduplicated_graph_edges: 0,
                removed_graph_self_edges: 0,
                rewired_kg_facts: 0,
                rebound_source_manifests: 0,
                reported_groups,
                groups_truncated,
                reported_members,
                members_truncated,
                groups,
            };

            let now_s = format_db_timestamp(Utc::now());
            for selection in if dry_run { &[][..] } else { batch.as_slice() } {
                let survivor_id = selection
                    .metrics
                    .survivor_document_id
                    .as_deref()
                    .expect("actionable group has one survivor");
                let survivor = load_document_locked(tx, survivor_id)?.ok_or_else(|| {
                    AppError::conflict(format!(
                        "canonical source document disappeared during cleanup: {survivor_id}"
                    ))
                })?;
                let candidate_ids: Vec<&str> =
                    selection.candidate_ids.iter().map(String::as_str).collect();

                let graph = rewire_graph_to_survivor(tx, &survivor, &candidate_ids)?;
                report.rewired_graph_edge_endpoints += graph.rewired_endpoints;
                report.deduplicated_graph_edges += graph.deduplicated_edges;
                report.removed_graph_self_edges += graph.removed_self_edges;

                for &candidate_id in &candidate_ids {
                    report.rewired_kg_facts += tx.update_where(
                        "kg_facts",
                        lin::query::pred::eq("source_document_id", candidate_id),
                        RecBuilder::new()
                            .str("source_document_id", survivor_id)
                            .str("updated_at", &now_s)
                            .build(),
                    )? as u64;
                }

                let sm_pred = lin::query::pred::and(
                    lin::query::pred::eq(
                        "canonical_path",
                        selection.metrics.key.source_file.as_str(),
                    ),
                    lin::query::pred::ne("document_id", survivor_id),
                );
                report.rebound_source_manifests += tx.update_where(
                    "source_manifest",
                    sm_pred,
                    RecBuilder::new().str("document_id", survivor_id).build(),
                )? as u64;

                for candidate_id in candidate_ids {
                    if delete_document_locked(tx, candidate_id)? {
                        report.deleted_documents += 1;
                    } else {
                        return Err(AppError::conflict(format!(
                            "duplicate candidate disappeared during cleanup: {candidate_id}"
                        )));
                    }
                }
            }

            report.applied = !dry_run && report.deleted_documents > 0;
            Ok(report)
        })
    }
}

fn build_report_groups(
    r: &impl LinReader,
    batch: &[BatchSelection],
    all_metrics: &[GroupMetrics],
) -> Result<Vec<SourceDuplicateGroup>> {
    let mut selected_metrics: Vec<GroupMetrics> = batch
        .iter()
        .take(REPORT_GROUP_LIMIT)
        .map(|s| s.metrics.clone())
        .collect();
    if selected_metrics.len() < REPORT_GROUP_LIMIT && selected_metrics.len() < all_metrics.len() {
        for diagnostic in all_metrics.iter().take(REPORT_GROUP_LIMIT) {
            if selected_metrics.iter().any(|e| e.key == diagnostic.key) {
                continue;
            }
            selected_metrics.push(diagnostic.clone());
            if selected_metrics.len() == REPORT_GROUP_LIMIT {
                break;
            }
        }
    }

    let mut member_budget = REPORT_MEMBER_LIMIT;
    let mut groups = Vec::with_capacity(selected_metrics.len());
    for metric in selected_metrics {
        let batch_candidate_count = batch
            .iter()
            .find(|s| s.metrics.key == metric.key)
            .map(|s| s.candidate_ids.len() as u64)
            .unwrap_or(0);
        let members = materialize_group_members(r, &metric.members, member_budget)?;
        member_budget = member_budget.saturating_sub(members.len());
        groups.push(group_from_metrics(metric, batch_candidate_count, members));
    }
    Ok(groups)
}

fn materialize_group_members(
    r: &impl LinReader,
    enriched_members: &[EnrichedMember],
    limit: usize,
) -> Result<Vec<SourceDuplicateDocument>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for m in enriched_members.iter().take(limit) {
        let node_rows = r.find_by_eq("graph_nodes", "document_id", &m.base.id)?;
        let graph_nodes = node_rows.len() as u64;
        let mut incident_edge_ids: HashSet<String> = HashSet::new();
        for nr in &node_rows {
            if let Some(nid) = cell_str(nr, "id") {
                for er in r.find_by_eq("graph_edges", "source_id", nid)? {
                    incident_edge_ids.insert(cell_string(&er, "id"));
                }
                for er in r.find_by_eq("graph_edges", "target_id", nid)? {
                    incident_edge_ids.insert(cell_string(&er, "id"));
                }
            }
        }
        out.push(SourceDuplicateDocument {
            document_id: m.base.id.clone(),
            uri: m.base.uri.clone(),
            title: m.base.title.clone(),
            canonical_uri_match: m.base.canonical_uri_match,
            chunks: m.chunks,
            graph_nodes,
            incident_graph_edges: incident_edge_ids.len() as u64,
            wiki_references: m.wiki_references,
            collection_references: m.collection_references,
            foreign_source_manifest_references: m.foreign_source_manifest_references,
            revisions: m.revisions,
        });
    }
    Ok(out)
}

fn group_from_metrics(
    metrics: GroupMetrics,
    batch_candidate_count: u64,
    members: Vec<SourceDuplicateDocument>,
) -> SourceDuplicateGroup {
    let mut skip_reasons = Vec::new();
    if metrics.canonical_count != 1 {
        skip_reasons.push(format!(
            "expected exactly one canonical URI file://{}, found {}",
            metrics.key.source_file, metrics.canonical_count
        ));
    } else {
        if metrics.canonical_chunks == 0 && metrics.candidate_chunks > 0 {
            skip_reasons.push(format!(
                "canonical survivor has no chunks while legacy candidates have {}; reindex before cleanup",
                metrics.candidate_chunks
            ));
        }
        if metrics.candidate_wiki_references > 0 {
            skip_reasons.push(format!(
                "legacy candidates have {} wiki_index references",
                metrics.candidate_wiki_references
            ));
        }
        if metrics.candidate_collection_references > 0 {
            skip_reasons.push(format!(
                "legacy candidates have {} collection references",
                metrics.candidate_collection_references
            ));
        }
        if metrics.candidate_foreign_manifest_references > 0 {
            skip_reasons.push(format!(
                "legacy candidates own {} foreign source_manifest references",
                metrics.candidate_foreign_manifest_references
            ));
        }
        if metrics.candidate_revisions > 0 {
            skip_reasons.push(format!(
                "legacy candidates have {} document revisions requiring manual preservation",
                metrics.candidate_revisions
            ));
        }
    }
    if let Some(owner) = metrics.source_manifest_document_id.as_deref() {
        if !metrics.source_manifest_owner_is_member {
            skip_reasons.push(format!(
                "canonical source_manifest path is owned by non-member document {owner}"
            ));
        }
    }
    debug_assert_eq!(metrics.actionable, skip_reasons.is_empty());
    let members_truncated = (members.len() as u64) < metrics.document_count;
    SourceDuplicateGroup {
        source_file: metrics.key.source_file,
        content_hash: metrics.key.content_hash,
        document_count: metrics.document_count,
        survivor_document_id: metrics.survivor_document_id,
        source_manifest_document_id: metrics.source_manifest_document_id,
        candidate_count: metrics.candidate_count,
        batch_candidate_count,
        remaining_candidate_count: metrics
            .candidate_count
            .saturating_sub(batch_candidate_count),
        actionable: metrics.actionable,
        skip_reasons,
        members,
        members_truncated,
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct GraphRewireReport {
    rewired_endpoints: u64,
    deduplicated_edges: u64,
    removed_self_edges: u64,
}

fn rewire_graph_to_survivor(
    w: &mut impl LinWriter,
    survivor: &Document,
    candidate_ids: &[&str],
) -> Result<GraphRewireReport> {
    let mut survivor_node_ids = graph_node_ids(w, &survivor.id)?;
    let mut candidate_node_ids = Vec::new();
    for candidate_id in candidate_ids {
        candidate_node_ids.extend(graph_node_ids(w, candidate_id)?);
    }
    candidate_node_ids.sort();
    candidate_node_ids.dedup();
    if survivor_node_ids.is_empty() && candidate_node_ids.is_empty() {
        return Ok(GraphRewireReport::default());
    }
    let canonical_node_id = if survivor_node_ids.is_empty() {
        let node_id = candidate_node_ids.remove(0);
        promote_node_to_survivor(w, &node_id, survivor)?;
        node_id
    } else {
        let node_id = survivor_node_ids.remove(0);
        promote_node_to_survivor(w, &node_id, survivor)?;
        node_id
    };

    let mut merged_node_ids = survivor_node_ids;
    merged_node_ids.extend(candidate_node_ids);
    merged_node_ids.sort();
    merged_node_ids.dedup();
    merged_node_ids.retain(|node_id| node_id != &canonical_node_id);

    let removed_self_edges =
        delete_edges_that_would_collapse_to_self(w, &canonical_node_id, &merged_node_ids)?;
    let deduplicated_edges =
        deduplicate_prospective_edges(w, &canonical_node_id, &merged_node_ids)?;

    let mut rewired_endpoints = 0u64;
    for old_node_id in &merged_node_ids {
        rewired_endpoints += w.update_where(
            "graph_edges",
            lin::query::pred::eq("source_id", old_node_id.as_str()),
            RecBuilder::new()
                .str("source_id", &canonical_node_id)
                .build(),
        )? as u64;
        rewired_endpoints += w.update_where(
            "graph_edges",
            lin::query::pred::eq("target_id", old_node_id.as_str()),
            RecBuilder::new()
                .str("target_id", &canonical_node_id)
                .build(),
        )? as u64;
        let _ = w.delete_by_id("graph_nodes", old_node_id)?;
    }

    Ok(GraphRewireReport {
        rewired_endpoints,
        deduplicated_edges,
        removed_self_edges,
    })
}

fn collect_incident_edges_for_collapse(
    r: &impl LinReader,
    canonical_node_id: &str,
    merged_node_ids: &[String],
) -> Result<Vec<lin::Row>> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for nid in std::iter::once(canonical_node_id).chain(merged_node_ids.iter().map(String::as_str))
    {
        for er in r.find_by_eq("graph_edges", "source_id", nid)? {
            if seen.insert(cell_string(&er, "id")) {
                out.push(er);
            }
        }
        for er in r.find_by_eq("graph_edges", "target_id", nid)? {
            if seen.insert(cell_string(&er, "id")) {
                out.push(er);
            }
        }
    }
    Ok(out)
}

fn delete_edges_that_would_collapse_to_self(
    w: &mut impl LinWriter,
    canonical_node_id: &str,
    merged_node_ids: &[String],
) -> Result<u64> {
    if merged_node_ids.is_empty() {
        return Ok(0);
    }
    let merged_set: HashSet<&str> = merged_node_ids.iter().map(String::as_str).collect();
    let mut collapse_set = merged_set.clone();
    collapse_set.insert(canonical_node_id);

    let edges = collect_incident_edges_for_collapse(w, canonical_node_id, merged_node_ids)?;
    let mut removed = 0u64;
    for er in &edges {
        let sid = cell_str(er, "source_id").unwrap_or_default();
        let tid = cell_str(er, "target_id").unwrap_or_default();
        if collapse_set.contains(sid)
            && collapse_set.contains(tid)
            && (merged_set.contains(sid) || merged_set.contains(tid))
        {
            if w.delete_by_id("graph_edges", cell_str(er, "id").unwrap_or_default())? {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

fn deduplicate_prospective_edges(
    w: &mut impl LinWriter,
    canonical_node_id: &str,
    merged_node_ids: &[String],
) -> Result<u64> {
    if merged_node_ids.is_empty() {
        return Ok(0);
    }
    let merged_set: HashSet<&str> = merged_node_ids.iter().map(String::as_str).collect();
    let edges = collect_incident_edges_for_collapse(w, canonical_node_id, merged_node_ids)?;

    type PartitionKey = (String, String, String, u64, Option<String>);
    let mut partitions: HashMap<PartitionKey, Vec<(u8, chrono::DateTime<Utc>, String)>> =
        HashMap::new();

    for er in &edges {
        let raw_s = cell_str(er, "source_id").unwrap_or_default();
        let raw_t = cell_str(er, "target_id").unwrap_or_default();
        let eff_s = if merged_set.contains(raw_s) {
            canonical_node_id.to_string()
        } else {
            raw_s.to_string()
        };
        let eff_t = if merged_set.contains(raw_t) {
            canonical_node_id.to_string()
        } else {
            raw_t.to_string()
        };
        let rel_type = cell_string(er, "rel_type");
        let weight_bits = cell_f64(er, "weight", 1.0).to_bits();
        let context = cell_opt_string(er, "context");
        let is_merged = u8::from(merged_set.contains(raw_s) || merged_set.contains(raw_t));
        let created_at = cell_ts_or_now(er, "created_at");
        let eid = cell_string(er, "id");

        partitions
            .entry((eff_s, eff_t, rel_type, weight_bits, context))
            .or_default()
            .push((is_merged, created_at, eid));
    }

    let mut deleted = 0u64;
    for (_, mut members) in partitions {
        if members.len() <= 1 {
            continue;
        }
        members.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        for (_, _, eid) in members.into_iter().skip(1) {
            if w.delete_by_id("graph_edges", &eid)? {
                deleted += 1;
            }
        }
    }
    Ok(deleted)
}

fn promote_node_to_survivor(
    w: &mut impl LinWriter,
    node_id: &str,
    survivor: &Document,
) -> Result<()> {
    let now_s = format_db_timestamp(Utc::now());
    let label_key = crate::graph::normalize::label_key(&survivor.title);
    w.update_where(
        "graph_nodes",
        lin::query::pred::eq("id", node_id),
        RecBuilder::new()
            .str("kind", "document")
            .str("label", &survivor.title)
            .str("label_key", &label_key)
            .str("document_id", &survivor.id)
            .str("uri", &survivor.uri)
            .bool("resolved", true)
            .str("updated_at", &now_s)
            .build(),
    )?;
    Ok(())
}

fn graph_node_ids(r: &impl LinReader, document_id: &str) -> Result<Vec<String>> {
    let mut ids: Vec<String> = r
        .find_by_eq("graph_nodes", "document_id", document_id)?
        .iter()
        .map(|row| cell_string(row, "id"))
        .collect();
    ids.sort();
    Ok(ids)
}

fn load_document_locked(r: &impl LinReader, document_id: &str) -> Result<Option<Document>> {
    r.get_by_id("documents", document_id)?
        .as_ref()
        .map(rows::document)
        .transpose()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::db::{KgFactInput, SourceManifestWrite};
    use crate::models::{Chunk, Collection, CollectionEntry, GraphEdge, GraphNode};
    use crate::util::content_hash;

    fn document(
        id: &str,
        uri: &str,
        source_file: &str,
        body: &str,
        layer: &str,
        status: &str,
    ) -> Document {
        let now = Utc::now();
        Document {
            id: id.into(),
            uri: uri.into(),
            title: id.into(),
            content: body.into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            source_file: Some(source_file.into()),
            content_hash: Some(content_hash(body)),
            layer: layer.into(),
            status: status.into(),
            ..Default::default()
        }
    }

    fn graph_node(id: &str, document_id: Option<&str>) -> GraphNode {
        GraphNode {
            id: id.into(),
            kind: if document_id.is_some() {
                "document".into()
            } else {
                "entity".into()
            },
            label: id.into(),
            document_id: document_id.map(str::to_string),
            uri: None,
            resolved: true,
            metadata_json: "{}".into(),
        }
    }

    fn edge(id: &str, source_id: &str, target_id: &str, context: &str) -> GraphEdge {
        GraphEdge {
            id: id.into(),
            source_id: source_id.into(),
            target_id: target_id.into(),
            rel_type: "related".into(),
            weight: 1.0,
            context: Some(context.into()),
        }
    }

    #[test]
    fn precise_cleanup_is_dry_run_then_atomic_apply() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe.lin")).unwrap();
        let source = "/vault/project/a.md";
        let body = "same source body";
        let canonical = document(
            "canonical",
            "file:///vault/project/a.md",
            source,
            body,
            "raw",
            "active",
        );
        let legacy = document(
            "legacy",
            "project://project/a.md",
            source,
            body,
            "raw",
            "active",
        );
        let other_source = document(
            "other-source",
            "project://other/a.md",
            "/vault/other/a.md",
            body,
            "raw",
            "active",
        );
        let wiki = document("wiki", "wiki://same-body", source, body, "wiki", "active");
        let archived = document(
            "archived",
            "project://archive/a.md",
            source,
            body,
            "raw",
            "archived",
        );
        for doc in [&canonical, &legacy, &other_source, &wiki, &archived] {
            store.upsert_document(doc).unwrap();
        }
        store
            .insert_chunks(&[
                Chunk {
                    id: "canonical-chunk".into(),
                    document_id: canonical.id.clone(),
                    chunk_index: 0,
                    content: body.into(),
                    embedding: vec![1.0, 0.0],
                    char_start: 0,
                    char_end: body.len() as i32,
                    metadata_json: "{}".into(),
                },
                Chunk {
                    id: "legacy-chunk".into(),
                    document_id: legacy.id.clone(),
                    chunk_index: 0,
                    content: body.into(),
                    embedding: vec![1.0, 0.0],
                    char_start: 0,
                    char_end: body.len() as i32,
                    metadata_json: "{}".into(),
                },
            ])
            .unwrap();
        for node in [
            graph_node("canonical-node", Some("canonical")),
            graph_node("legacy-node", Some("legacy")),
            graph_node("external-node", None),
        ] {
            store.upsert_graph_node(&node).unwrap();
        }
        store
            .insert_graph_edges(&[
                edge("z-canonical-out", "canonical-node", "external-node", "same"),
                edge("a-legacy-out", "legacy-node", "external-node", "same"),
                edge("legacy-in", "external-node", "legacy-node", "unique"),
            ])
            .unwrap();
        let hash = content_hash(body);
        store
            .upsert_source_manifest(SourceManifestWrite {
                canonical_path: source,
                canonical_root: "/vault/project",
                size_bytes: body.len() as u64,
                mtime_ns: 1,
                content_hash: &hash,
                document_id: "legacy",
            })
            .unwrap();
        let fact = store
            .kg_add(KgFactInput {
                subject: "a",
                predicate: "rel",
                object: "b",
                valid_from: None,
                valid_to: None,
                source_document_id: Some("legacy"),
                confidence: None,
                metadata_json: None,
            })
            .unwrap();

        let preview = store.cleanup_source_duplicates(true, false, 10).unwrap();
        assert!(preview.dry_run);
        assert!(!preview.applied);
        assert_eq!(preview.candidate_count, 1);
        assert_eq!(preview.deleted_documents, 0);
        assert!(store.get_document("legacy").unwrap().is_some());
        assert_eq!(preview.groups[0].members.len(), 2);

        let applied = store.cleanup_source_duplicates(false, true, 10).unwrap();
        assert!(applied.applied);
        assert_eq!(applied.deleted_documents, 1);
        assert_eq!(applied.rewired_graph_edge_endpoints, 1);
        assert_eq!(applied.deduplicated_graph_edges, 1);
        assert_eq!(applied.rewired_kg_facts, 1);
        assert_eq!(applied.rebound_source_manifests, 1);
        assert!(store.get_document("canonical").unwrap().is_some());
        assert!(store.get_document("legacy").unwrap().is_none());
        assert!(store.get_document("other-source").unwrap().is_some());
        assert!(store.get_document("wiki").unwrap().is_some());
        assert!(store.get_document("archived").unwrap().is_some());
        assert!(store.list_chunks_for_document("legacy").unwrap().is_empty());
        assert_eq!(
            store.list_chunks_for_document("canonical").unwrap().len(),
            1
        );

        let r = store.read().unwrap();
        let manifest_document = cell_string(
            &r.get_by_id("source_manifest", source).unwrap().unwrap(),
            "document_id",
        );
        assert_eq!(manifest_document, "canonical");
        drop(r);

        let updated_fact = store.kg_get(&fact.id).unwrap().unwrap();
        assert_eq!(
            updated_fact.source_document_id.as_deref(),
            Some("canonical")
        );

        assert!(store.find_node_by_document_id("legacy").unwrap().is_none());
        let all_edges = store.list_graph_edges().unwrap();
        let canonical_edges = all_edges
            .iter()
            .filter(|e| e.source_id == "canonical-node" || e.target_id == "canonical-node")
            .count();
        assert_eq!(canonical_edges, 2);
        assert!(all_edges.iter().any(|e| e.id == "z-canonical-out"));
        assert!(!all_edges.iter().any(|e| e.id == "a-legacy-out"));
        assert!(
            !all_edges
                .iter()
                .any(|e| e.source_id == "legacy-node" || e.target_id == "legacy-node")
        );
    }

    #[test]
    fn cleanup_requires_confirm_and_drains_an_oversized_group_in_bounded_batches() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-cap.lin")).unwrap();
        let source = "/vault/cap.md";
        let body = "cap body";
        for doc in [
            document(
                "canonical",
                "file:///vault/cap.md",
                source,
                body,
                "raw",
                "active",
            ),
            document("legacy-a", "project://cap/a", source, body, "raw", "active"),
            document("legacy-b", "project://cap/b", source, body, "raw", "active"),
        ] {
            store.upsert_document(&doc).unwrap();
        }

        let error = store
            .cleanup_source_duplicates(false, false, 10)
            .unwrap_err();
        assert!(error.to_string().contains("confirm=true"));

        let first = store.cleanup_source_duplicates(false, true, 1).unwrap();
        assert!(first.cap_exceeded);
        assert!(first.applied);
        assert_eq!(first.candidate_count, 2);
        assert_eq!(first.batch_groups, 1);
        assert_eq!(first.batch_candidates, 1);
        assert_eq!(first.remaining_groups, 1);
        assert_eq!(first.remaining_candidates, 1);
        assert_eq!(first.deleted_documents, 1);
        assert_eq!(first.groups[0].batch_candidate_count, 1);
        assert_eq!(first.groups[0].remaining_candidate_count, 1);
        assert!(store.get_document("legacy-a").unwrap().is_none());
        assert!(store.get_document("legacy-b").unwrap().is_some());

        let second = store.cleanup_source_duplicates(false, true, 1).unwrap();
        assert!(!second.cap_exceeded);
        assert!(second.applied);
        assert_eq!(second.batch_candidates, 1);
        assert_eq!(second.remaining_groups, 0);
        assert_eq!(second.remaining_candidates, 0);
        assert!(store.get_document("legacy-b").unwrap().is_none());

        let converged = store.cleanup_source_duplicates(false, true, 1).unwrap();
        assert_eq!(converged.candidate_count, 0);
        assert_eq!(converged.batch_candidates, 0);
        assert!(!converged.applied);
    }

    #[test]
    fn cleanup_promotes_a_legacy_graph_node_when_survivor_has_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-promote.lin")).unwrap();
        let source = "/vault/promote.md";
        let body = "promote body";
        for doc in [
            document(
                "canonical",
                "file:///vault/promote.md",
                source,
                body,
                "raw",
                "active",
            ),
            document(
                "legacy",
                "project://promote.md",
                source,
                body,
                "raw",
                "active",
            ),
        ] {
            store.upsert_document(&doc).unwrap();
        }
        let mut legacy_node = graph_node("legacy-node", Some("legacy"));
        legacy_node.metadata_json = r#"{"preserved":true}"#.into();
        store.upsert_graph_node(&legacy_node).unwrap();
        store
            .upsert_graph_node(&graph_node("external", None))
            .unwrap();
        store
            .insert_graph_edges(&[edge("incoming", "external", "legacy-node", "keep")])
            .unwrap();

        let report = store.cleanup_source_duplicates(false, true, 10).unwrap();
        assert!(report.applied);
        assert_eq!(report.deleted_documents, 1);

        let r = store.read().unwrap();
        let node_row = r.get_by_id("graph_nodes", "legacy-node").unwrap().unwrap();
        assert_eq!(cell_str(&node_row, "document_id"), Some("canonical"));
        assert_eq!(cell_str(&node_row, "uri"), Some("file:///vault/promote.md"));
        assert_eq!(
            cell_str(&node_row, "metadata_json"),
            Some(r#"{"preserved":true}"#)
        );
        let label = cell_string(&node_row, "label");
        let label_key = cell_opt_string(&node_row, "label_key");
        assert_eq!(
            label_key.as_deref(),
            Some(crate::graph::normalize::label_key(&label).as_str())
        );
        let edge_row = r.get_by_id("graph_edges", "incoming").unwrap().unwrap();
        assert_eq!(cell_str(&edge_row, "target_id"), Some("legacy-node"));
    }

    #[test]
    fn cleanup_does_not_create_graph_state_when_duplicates_have_no_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-no-graph.lin")).unwrap();
        let source = "/vault/no-graph.md";
        let body = "no graph body";
        for doc in [
            document(
                "canonical",
                "file:///vault/no-graph.md",
                source,
                body,
                "raw",
                "active",
            ),
            document(
                "legacy",
                "project://no-graph.md",
                source,
                body,
                "raw",
                "active",
            ),
        ] {
            store.upsert_document(&doc).unwrap();
        }

        let report = store.cleanup_source_duplicates(false, true, 10).unwrap();
        assert!(report.applied);
        assert_eq!(report.deleted_documents, 1);
        assert_eq!(report.rewired_graph_edge_endpoints, 0);
        assert_eq!(store.list_graph_nodes().unwrap().len(), 0);
    }

    #[test]
    fn cleanup_removes_only_edges_that_its_node_collapse_would_make_self_loops() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-self-edges.lin")).unwrap();
        let source = "/vault/self-edges.md";
        let body = "same graph body";
        for doc in [
            document(
                "canonical",
                "file:///vault/self-edges.md",
                source,
                body,
                "raw",
                "active",
            ),
            document(
                "legacy",
                "project://self-edges.md",
                source,
                body,
                "raw",
                "active",
            ),
        ] {
            store.upsert_document(&doc).unwrap();
        }
        for node in [
            graph_node("canonical-node", Some("canonical")),
            graph_node("legacy-node", Some("legacy")),
            graph_node("external-node", None),
        ] {
            store.upsert_graph_node(&node).unwrap();
        }
        store
            .insert_graph_edges(&[
                edge(
                    "preexisting-self",
                    "canonical-node",
                    "canonical-node",
                    "preexisting",
                ),
                edge(
                    "canonical-to-legacy",
                    "canonical-node",
                    "legacy-node",
                    "collapse",
                ),
                edge(
                    "legacy-to-canonical",
                    "legacy-node",
                    "canonical-node",
                    "collapse",
                ),
                edge("legacy-self", "legacy-node", "legacy-node", "collapse"),
                edge(
                    "legacy-external",
                    "legacy-node",
                    "external-node",
                    "preserve",
                ),
            ])
            .unwrap();

        let report = store.cleanup_source_duplicates(false, true, 10).unwrap();
        assert!(report.applied);
        assert_eq!(report.removed_graph_self_edges, 3);
        assert_eq!(report.rewired_graph_edge_endpoints, 1);
        let edges = store.list_graph_edges().unwrap();
        assert!(edges.iter().any(|e| {
            e.id == "preexisting-self"
                && e.source_id == "canonical-node"
                && e.target_id == "canonical-node"
        }));
        assert!(
            !edges
                .iter()
                .any(|e| e.source_id == e.target_id && e.id != "preexisting-self")
        );
        assert!(edges.iter().any(|e| {
            e.id == "legacy-external"
                && e.source_id == "canonical-node"
                && e.target_id == "external-node"
        }));
    }

    #[test]
    fn cleanup_skips_candidate_that_owns_a_foreign_manifest_path() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-manifest.lin")).unwrap();
        let source = "/vault/owned.md";
        let body = "owned body";
        for doc in [
            document(
                "canonical",
                "file:///vault/owned.md",
                source,
                body,
                "raw",
                "active",
            ),
            document(
                "legacy",
                "project://owned.md",
                source,
                body,
                "raw",
                "active",
            ),
        ] {
            store.upsert_document(&doc).unwrap();
        }
        let hash = content_hash(body);
        store
            .upsert_source_manifest(SourceManifestWrite {
                canonical_path: "/vault/foreign.md",
                canonical_root: "/vault",
                size_bytes: body.len() as u64,
                mtime_ns: 1,
                content_hash: &hash,
                document_id: "legacy",
            })
            .unwrap();

        let report = store.cleanup_source_duplicates(false, true, 10).unwrap();
        assert_eq!(report.actionable_groups, 0);
        assert_eq!(report.skipped_groups, 1);
        assert!(
            report.groups[0]
                .skip_reasons
                .iter()
                .any(|reason| reason.contains("foreign source_manifest references"))
        );
        assert!(store.get_document("legacy").unwrap().is_some());
        let r = store.read().unwrap();
        let manifest_row = r
            .get_by_id("source_manifest", "/vault/foreign.md")
            .unwrap()
            .unwrap();
        assert_eq!(cell_str(&manifest_row, "document_id"), Some("legacy"));
    }

    #[test]
    fn cleanup_skips_manifest_path_owned_by_a_non_member_document() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-third-owner.lin")).unwrap();
        let source = "/vault/duplicate.md";
        let body = "duplicate body";
        for doc in [
            document(
                "canonical",
                "file:///vault/duplicate.md",
                source,
                body,
                "raw",
                "active",
            ),
            document(
                "legacy",
                "project://duplicate.md",
                source,
                body,
                "raw",
                "active",
            ),
            document(
                "third-owner",
                "file:///vault/third.md",
                "/vault/third.md",
                "different body",
                "raw",
                "active",
            ),
        ] {
            store.upsert_document(&doc).unwrap();
        }
        let hash = content_hash(body);
        store
            .upsert_source_manifest(SourceManifestWrite {
                canonical_path: source,
                canonical_root: "/vault",
                size_bytes: body.len() as u64,
                mtime_ns: 1,
                content_hash: &hash,
                document_id: "third-owner",
            })
            .unwrap();

        let report = store.cleanup_source_duplicates(false, true, 10).unwrap();
        assert_eq!(report.actionable_groups, 0);
        assert_eq!(report.skipped_groups, 1);
        assert!(!report.applied);
        assert!(
            report.groups[0]
                .skip_reasons
                .iter()
                .any(|reason| reason.contains("owned by non-member document"))
        );
        assert!(store.get_document("legacy").unwrap().is_some());
        let r = store.read().unwrap();
        let manifest_row = r.get_by_id("source_manifest", source).unwrap().unwrap();
        assert_eq!(cell_str(&manifest_row, "document_id"), Some("third-owner"));
    }

    #[test]
    fn cleanup_skips_candidate_with_revision_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-revisions.lin")).unwrap();
        let source = "/vault/history.md";
        let body = "same current body";
        let canonical = document(
            "canonical",
            "file:///vault/history.md",
            source,
            body,
            "raw",
            "active",
        );
        let mut legacy = document(
            "legacy",
            "project://history.md",
            source,
            body,
            "raw",
            "active",
        );
        store.upsert_document(&canonical).unwrap();
        store.upsert_document(&legacy).unwrap();
        legacy.title = "legacy updated title".into();
        legacy.updated_at = Utc::now();
        store.upsert_document(&legacy).unwrap();
        assert_eq!(
            store
                .list_document_revision_summaries("legacy", 10, 0)
                .unwrap()
                .total,
            1
        );

        let report = store.cleanup_source_duplicates(false, true, 10).unwrap();
        assert_eq!(report.actionable_groups, 0);
        assert_eq!(report.skipped_groups, 1);
        assert!(!report.applied);
        assert!(
            report.groups[0]
                .skip_reasons
                .iter()
                .any(|reason| reason.contains("document revisions"))
        );
        assert!(store.get_document("legacy").unwrap().is_some());
        assert_eq!(
            store
                .list_document_revision_summaries("legacy", 10, 0)
                .unwrap()
                .total,
            1
        );
    }

    #[test]
    fn cleanup_skips_when_only_legacy_candidate_has_search_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-chunks.lin")).unwrap();
        let source = "/vault/chunks.md";
        let body = "searchable duplicate body";
        for doc in [
            document(
                "canonical",
                "file:///vault/chunks.md",
                source,
                body,
                "raw",
                "active",
            ),
            document(
                "legacy",
                "project://chunks.md",
                source,
                body,
                "raw",
                "active",
            ),
        ] {
            store.upsert_document(&doc).unwrap();
        }
        store
            .insert_chunks(&[Chunk {
                id: "legacy-only-chunk".into(),
                document_id: "legacy".into(),
                chunk_index: 0,
                content: body.into(),
                embedding: vec![1.0, 0.0],
                char_start: 0,
                char_end: body.len() as i32,
                metadata_json: "{}".into(),
            }])
            .unwrap();

        let report = store.cleanup_source_duplicates(false, true, 10).unwrap();
        assert_eq!(report.actionable_groups, 0);
        assert_eq!(report.skipped_groups, 1);
        assert!(!report.applied);
        assert!(
            report.groups[0]
                .skip_reasons
                .iter()
                .any(|reason| reason.contains("canonical survivor has no chunks"))
        );
        assert!(store.get_document("legacy").unwrap().is_some());
        assert_eq!(store.list_chunks_for_document("legacy").unwrap().len(), 1);
    }

    #[test]
    fn cleanup_preview_bounds_group_and_member_details_but_keeps_totals() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-report-bound.lin")).unwrap();

        let dense_source = "/vault/000-dense.md";
        let dense_body = "dense duplicate body";
        store
            .upsert_document(&document(
                "dense-canonical",
                "file:///vault/000-dense.md",
                dense_source,
                dense_body,
                "raw",
                "active",
            ))
            .unwrap();
        for index in 0..(REPORT_MEMBER_LIMIT + 5) {
            store
                .upsert_document(&document(
                    &format!("dense-legacy-{index:04}"),
                    &format!("project://dense/{index}"),
                    dense_source,
                    dense_body,
                    "raw",
                    "active",
                ))
                .unwrap();
        }

        for index in 0..REPORT_GROUP_LIMIT {
            let source = format!("/vault/group-{index:04}.md");
            let body = format!("group body {index}");
            store
                .upsert_document(&document(
                    &format!("group-canonical-{index:04}"),
                    &format!("file://{source}"),
                    &source,
                    &body,
                    "raw",
                    "active",
                ))
                .unwrap();
            store
                .upsert_document(&document(
                    &format!("group-legacy-{index:04}"),
                    &format!("project://group/{index}"),
                    &source,
                    &body,
                    "raw",
                    "active",
                ))
                .unwrap();
        }

        let report = store.cleanup_source_duplicates(true, false, 1_000).unwrap();
        assert_eq!(report.matched_groups, REPORT_GROUP_LIMIT + 1);
        assert_eq!(
            report.candidate_count,
            (REPORT_MEMBER_LIMIT + 5 + REPORT_GROUP_LIMIT) as u64
        );
        assert_eq!(report.reported_groups, REPORT_GROUP_LIMIT);
        assert_eq!(report.groups.len(), REPORT_GROUP_LIMIT);
        assert!(report.groups_truncated);
        assert_eq!(report.reported_members, REPORT_MEMBER_LIMIT);
        assert!(report.members_truncated);
        assert_eq!(
            report.groups[0].document_count,
            (REPORT_MEMBER_LIMIT + 6) as u64
        );
        assert_eq!(report.groups[0].members.len(), REPORT_MEMBER_LIMIT);
        assert!(report.groups[0].members_truncated);
    }

    #[test]
    fn cleanup_report_prioritizes_the_applied_group_before_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-report-priority.lin")).unwrap();
        let body = "same body";
        for index in 0..REPORT_GROUP_LIMIT {
            let source = format!("/vault/aaa-skipped-{index:04}.md");
            for suffix in ["a", "b"] {
                store
                    .upsert_document(&document(
                        &format!("skipped-{index:04}-{suffix}"),
                        &format!("project://skipped/{index}/{suffix}"),
                        &source,
                        body,
                        "raw",
                        "active",
                    ))
                    .unwrap();
            }
        }
        let selected_source = "/vault/zzz-selected.md";
        for doc in [
            document(
                "selected-canonical",
                "file:///vault/zzz-selected.md",
                selected_source,
                body,
                "raw",
                "active",
            ),
            document(
                "selected-legacy",
                "project://selected/legacy",
                selected_source,
                body,
                "raw",
                "active",
            ),
        ] {
            store.upsert_document(&doc).unwrap();
        }

        let report = store.cleanup_source_duplicates(false, true, 1).unwrap();
        assert!(report.applied);
        assert_eq!(report.deleted_documents, 1);
        assert_eq!(report.matched_groups, REPORT_GROUP_LIMIT + 1);
        assert!(report.groups_truncated);
        assert!(report.groups.iter().any(|group| {
            group.source_file == selected_source && group.batch_candidate_count == 1
        }));
    }

    #[test]
    fn ambiguous_and_user_referenced_groups_are_reported_and_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-skips.lin")).unwrap();
        let body = "same";
        let no_canonical = "/vault/no-canonical.md";
        for doc in [
            document(
                "old-a",
                "project://old/a",
                no_canonical,
                body,
                "raw",
                "active",
            ),
            document(
                "old-b",
                "project://old/b",
                no_canonical,
                body,
                "raw",
                "active",
            ),
        ] {
            store.upsert_document(&doc).unwrap();
        }

        let protected = "/vault/protected.md";
        for doc in [
            document(
                "protected-canonical",
                "file:///vault/protected.md",
                protected,
                body,
                "raw",
                "active",
            ),
            document(
                "protected-legacy",
                "project://protected/legacy",
                protected,
                body,
                "raw",
                "active",
            ),
        ] {
            store.upsert_document(&doc).unwrap();
        }
        let now = Utc::now().to_rfc3339();
        store
            .create_collection(
                &Collection {
                    id: "c".into(),
                    name: "C".into(),
                    description: None,
                    metadata_json: "{}".into(),
                    created_at: now.clone(),
                    updated_at: now,
                },
                &[CollectionEntry {
                    document_id: "protected-legacy".into(),
                    position: 0,
                    parent_document_id: None,
                    depends_on: Vec::new(),
                }],
            )
            .unwrap();

        let report = store.cleanup_source_duplicates(false, true, 10).unwrap();
        assert_eq!(report.matched_groups, 2);
        assert_eq!(report.actionable_groups, 0);
        assert_eq!(report.skipped_groups, 2);
        assert_eq!(report.candidate_count, 0);
        assert!(!report.applied);
        assert!(store.get_document("old-a").unwrap().is_some());
        assert!(store.get_document("protected-legacy").unwrap().is_some());
        assert!(report.groups.iter().any(|group| {
            group
                .skip_reasons
                .iter()
                .any(|reason| reason.contains("canonical URI"))
        }));
        assert!(report.groups.iter().any(|group| {
            group
                .skip_reasons
                .iter()
                .any(|reason| reason.contains("collection references"))
        }));
    }
}
