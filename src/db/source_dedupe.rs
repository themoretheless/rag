//! Precise cleanup for legacy source-document duplicates.
//!
//! This is intentionally narrower than content-hash deduplication: a group is
//! actionable only when active raw documents share the same non-empty
//! `(source_file, content_hash)` and exactly one document has the canonical
//! `file://<source_file>` URI. All discovery, graph rewiring, manifest repair,
//! and deletion happens in one DuckDB transaction.

use super::rows;
use super::store::{delete_document_locked, Store, DOCUMENT_SELECT};
use crate::error::{AppError, Result};
use crate::models::Document;
use duckdb::{params, params_from_iter, Connection};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone)]
struct GroupKey {
    source_file: String,
    content_hash: String,
}

#[derive(Debug, Clone)]
struct BatchSelection {
    group_index: usize,
    candidate_ids: Vec<String>,
}

impl Store {
    /// Preview or atomically remove legacy raw documents that duplicate the
    /// canonical `file://<source_file>` document.
    ///
    /// Apply requires both `dry_run=false` and `confirm=true`. Each invocation
    /// selects at most `max_candidates` in stable `(source_file, content_hash,
    /// document_id)` order, so a large group can be drained safely over repeated
    /// calls. Groups with no unique canonical survivor, protected references,
    /// ambiguous manifest ownership, or candidate revision history are skipped.
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

        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let scanned_active_raw_documents = tx.query_row(
            r#"
            SELECT COUNT(*)::BIGINT
            FROM documents
            WHERE LOWER(COALESCE(NULLIF(TRIM(layer), ''), 'raw')) = 'raw'
              AND LOWER(COALESCE(NULLIF(TRIM(status), ''), 'active')) = 'active'
              AND source_file IS NOT NULL AND TRIM(source_file) <> ''
              AND content_hash IS NOT NULL AND TRIM(content_hash) <> ''
            "#,
            [],
            |row| row.get::<_, i64>(0),
        )?;
        let keys = duplicate_group_keys(&tx)?;
        let mut groups = Vec::with_capacity(keys.len());
        for key in keys {
            groups.push(inspect_group(&tx, &key)?);
        }

        let candidate_count = groups
            .iter()
            .filter(|group| group.actionable)
            .map(|group| group.candidate_count)
            .sum::<u64>();
        let actionable_groups = groups.iter().filter(|group| group.actionable).count();
        let skipped_groups = groups.len().saturating_sub(actionable_groups);
        let batch = select_batch(&groups, max_candidates);
        let batch_candidates = batch
            .iter()
            .map(|selection| selection.candidate_ids.len() as u64)
            .sum::<u64>();
        for selection in &batch {
            let group = &mut groups[selection.group_index];
            group.batch_candidate_count = selection.candidate_ids.len() as u64;
            group.remaining_candidate_count = group
                .candidate_count
                .saturating_sub(group.batch_candidate_count);
        }
        let remaining_candidates = candidate_count.saturating_sub(batch_candidates);
        let remaining_groups = groups
            .iter()
            .filter(|group| group.actionable && group.remaining_candidate_count > 0)
            .count();
        let cap_exceeded = remaining_candidates > 0;

        let mut report = SourceDuplicateCleanupReport {
            success: true,
            dry_run,
            confirmed: confirm,
            max_candidates,
            scanned_active_raw_documents: scanned_active_raw_documents.max(0) as u64,
            matched_groups: groups.len(),
            actionable_groups,
            skipped_groups,
            candidate_count,
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
            reported_groups: 0,
            groups_truncated: false,
            reported_members: 0,
            members_truncated: false,
            groups,
        };

        for selection in if dry_run { &[][..] } else { batch.as_slice() } {
            let group = &report.groups[selection.group_index];
            let survivor_id = group
                .survivor_document_id
                .as_deref()
                .expect("actionable group has one survivor");
            let survivor = load_document_locked(&tx, survivor_id)?.ok_or_else(|| {
                AppError::conflict(format!(
                    "canonical source document disappeared during cleanup: {survivor_id}"
                ))
            })?;
            let candidate_ids = selection
                .candidate_ids
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();

            let graph = rewire_graph_to_survivor(&tx, &survivor, &candidate_ids)?;
            report.rewired_graph_edge_endpoints += graph.rewired_endpoints;
            report.deduplicated_graph_edges += graph.deduplicated_edges;
            report.removed_graph_self_edges += graph.removed_self_edges;

            for candidate_id in &candidate_ids {
                report.rewired_kg_facts += tx.execute(
                    "UPDATE kg_facts SET source_document_id = ?, updated_at = now() WHERE source_document_id = ?",
                    params![survivor_id, candidate_id],
                )? as u64;
            }
            report.rebound_source_manifests += tx.execute(
                "UPDATE source_manifest SET document_id = ? WHERE canonical_path = ? AND document_id <> ?",
                params![survivor_id, group.source_file, survivor_id],
            )? as u64;

            for candidate_id in candidate_ids {
                if delete_document_locked(&tx, candidate_id)? {
                    report.deleted_documents += 1;
                } else {
                    return Err(AppError::conflict(format!(
                        "duplicate candidate disappeared during cleanup: {candidate_id}"
                    )));
                }
            }
        }

        report.applied = !dry_run && report.deleted_documents > 0;
        let detail = truncate_report_details(&mut report.groups);
        report.reported_groups = detail.reported_groups;
        report.groups_truncated = detail.groups_truncated;
        report.reported_members = detail.reported_members;
        report.members_truncated = detail.members_truncated;
        tx.commit()?;
        Ok(report)
    }
}

fn select_batch(groups: &[SourceDuplicateGroup], max_candidates: usize) -> Vec<BatchSelection> {
    let mut remaining = max_candidates;
    let mut batch = Vec::new();
    for (group_index, group) in groups.iter().enumerate() {
        if remaining == 0 {
            break;
        }
        if !group.actionable {
            continue;
        }
        let candidate_ids = group
            .members
            .iter()
            .filter(|member| !member.canonical_uri_match)
            .take(remaining)
            .map(|member| member.document_id.clone())
            .collect::<Vec<_>>();
        if candidate_ids.is_empty() {
            continue;
        }
        remaining = remaining.saturating_sub(candidate_ids.len());
        batch.push(BatchSelection {
            group_index,
            candidate_ids,
        });
    }
    batch
}

#[derive(Debug, Clone, Copy, Default)]
struct ReportDetailCounts {
    reported_groups: usize,
    groups_truncated: bool,
    reported_members: usize,
    members_truncated: bool,
}

fn truncate_report_details(groups: &mut Vec<SourceDuplicateGroup>) -> ReportDetailCounts {
    let matched_groups = groups.len();
    let matched_members = groups
        .iter()
        .map(|group| group.members.len())
        .sum::<usize>();
    // Applied/planned groups are the most actionable evidence. Stable
    // partitioning keeps their source/hash order, then preserves the original
    // order of all remaining diagnostic groups.
    let (mut selected, unselected): (Vec<_>, Vec<_>) = groups
        .drain(..)
        .partition(|group| group.batch_candidate_count > 0);
    selected.extend(unselected);
    *groups = selected;
    groups.truncate(REPORT_GROUP_LIMIT);

    let mut member_budget = REPORT_MEMBER_LIMIT;
    for group in groups.iter_mut() {
        let before = group.members.len();
        let keep = before.min(member_budget);
        group.members.truncate(keep);
        group.members_truncated = keep < before;
        member_budget = member_budget.saturating_sub(keep);
    }
    let reported_members = groups.iter().map(|group| group.members.len()).sum();
    ReportDetailCounts {
        reported_groups: groups.len(),
        groups_truncated: groups.len() < matched_groups,
        reported_members,
        members_truncated: reported_members < matched_members,
    }
}

fn duplicate_group_keys(conn: &Connection) -> Result<Vec<GroupKey>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT source_file, content_hash
        FROM documents
        WHERE LOWER(COALESCE(NULLIF(TRIM(layer), ''), 'raw')) = 'raw'
          AND LOWER(COALESCE(NULLIF(TRIM(status), ''), 'active')) = 'active'
          AND source_file IS NOT NULL AND TRIM(source_file) <> ''
          AND content_hash IS NOT NULL AND TRIM(content_hash) <> ''
        GROUP BY source_file, content_hash
        HAVING COUNT(*) > 1
        ORDER BY source_file ASC, content_hash ASC
        "#,
    )?;
    let mut rows = stmt.query([])?;
    let mut keys = Vec::new();
    while let Some(row) = rows.next()? {
        keys.push(GroupKey {
            source_file: row.get(0)?,
            content_hash: row.get(1)?,
        });
    }
    Ok(keys)
}

fn inspect_group(conn: &Connection, key: &GroupKey) -> Result<SourceDuplicateGroup> {
    let canonical_uri = format!("file://{}", key.source_file);
    let mut stmt = conn.prepare(
        r#"
        SELECT
          d.id,
          d.uri,
          d.title,
          d.uri = ? AS canonical_uri_match,
          (SELECT COUNT(*)::BIGINT FROM chunks c WHERE c.document_id = d.id),
          (SELECT COUNT(*)::BIGINT FROM graph_nodes n WHERE n.document_id = d.id),
          (SELECT COUNT(*)::BIGINT
             FROM graph_edges e
            WHERE e.source_id IN (SELECT id FROM graph_nodes n WHERE n.document_id = d.id)
               OR e.target_id IN (SELECT id FROM graph_nodes n WHERE n.document_id = d.id)),
          (SELECT COUNT(*)::BIGINT FROM wiki_index w
            WHERE w.document_id = d.id OR w.page_id = d.id),
          ((SELECT COUNT(*)::BIGINT FROM collection_entries ce
             WHERE ce.document_id = d.id OR ce.parent_document_id = d.id)
           + (SELECT COUNT(*)::BIGINT FROM collection_dependencies cd
             WHERE cd.document_id = d.id OR cd.depends_on_document_id = d.id)),
          (SELECT COUNT(*)::BIGINT FROM source_manifest sm
            WHERE sm.document_id = d.id AND sm.canonical_path <> d.source_file),
          (SELECT COUNT(*)::BIGINT FROM document_revisions r WHERE r.document_id = d.id)
        FROM documents d
        WHERE d.source_file = ? AND d.content_hash = ?
          AND LOWER(COALESCE(NULLIF(TRIM(d.layer), ''), 'raw')) = 'raw'
          AND LOWER(COALESCE(NULLIF(TRIM(d.status), ''), 'active')) = 'active'
        ORDER BY canonical_uri_match DESC, d.id ASC
        "#,
    )?;
    let mut rows = stmt.query(params![canonical_uri, key.source_file, key.content_hash])?;
    let mut members = Vec::new();
    while let Some(row) = rows.next()? {
        members.push(SourceDuplicateDocument {
            document_id: row.get(0)?,
            uri: row.get(1)?,
            title: row.get(2)?,
            canonical_uri_match: row.get(3)?,
            chunks: nonnegative(row.get(4)?),
            graph_nodes: nonnegative(row.get(5)?),
            incident_graph_edges: nonnegative(row.get(6)?),
            wiki_references: nonnegative(row.get(7)?),
            collection_references: nonnegative(row.get(8)?),
            foreign_source_manifest_references: nonnegative(row.get(9)?),
            revisions: nonnegative(row.get(10)?),
        });
    }

    let canonical = members
        .iter()
        .filter(|member| member.canonical_uri_match)
        .collect::<Vec<_>>();
    let mut skip_reasons = Vec::new();
    if canonical.len() != 1 {
        skip_reasons.push(format!(
            "expected exactly one canonical URI {canonical_uri}, found {}",
            canonical.len()
        ));
    }
    let candidates = if canonical.len() == 1 {
        members
            .iter()
            .filter(|member| !member.canonical_uri_match)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let canonical_chunks = canonical.first().map(|member| member.chunks).unwrap_or(0);
    let candidate_chunks = candidates.iter().map(|member| member.chunks).sum::<u64>();
    if canonical_chunks == 0 && candidate_chunks > 0 {
        skip_reasons.push(format!(
            "canonical survivor has no chunks while legacy candidates have {candidate_chunks}; reindex before cleanup"
        ));
    }
    let wiki_references = candidates
        .iter()
        .map(|member| member.wiki_references)
        .sum::<u64>();
    if wiki_references > 0 {
        skip_reasons.push(format!(
            "legacy candidates have {wiki_references} wiki_index references"
        ));
    }
    let collection_references = candidates
        .iter()
        .map(|member| member.collection_references)
        .sum::<u64>();
    if collection_references > 0 {
        skip_reasons.push(format!(
            "legacy candidates have {collection_references} collection references"
        ));
    }
    let foreign_source_manifest_references = candidates
        .iter()
        .map(|member| member.foreign_source_manifest_references)
        .sum::<u64>();
    if foreign_source_manifest_references > 0 {
        skip_reasons.push(format!(
            "legacy candidates own {foreign_source_manifest_references} foreign source_manifest references"
        ));
    }

    let candidate_revisions = candidates
        .iter()
        .map(|member| member.revisions)
        .sum::<u64>();
    if candidate_revisions > 0 {
        skip_reasons.push(format!(
            "legacy candidates have {candidate_revisions} document revisions requiring manual preservation"
        ));
    }

    let source_manifest_document_id = conn
        .query_row(
            "SELECT document_id FROM source_manifest WHERE canonical_path = ?",
            params![key.source_file],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(owner) = source_manifest_document_id.as_deref() {
        if !members.iter().any(|member| member.document_id == owner) {
            skip_reasons.push(format!(
                "canonical source_manifest path is owned by non-member document {owner}"
            ));
        }
    }
    let survivor_document_id = canonical.first().map(|member| member.document_id.clone());
    let actionable = skip_reasons.is_empty() && !candidates.is_empty();
    Ok(SourceDuplicateGroup {
        source_file: key.source_file.clone(),
        content_hash: key.content_hash.clone(),
        document_count: members.len() as u64,
        survivor_document_id,
        source_manifest_document_id,
        candidate_count: if actionable {
            candidates.len() as u64
        } else {
            0
        },
        batch_candidate_count: 0,
        remaining_candidate_count: if actionable {
            candidates.len() as u64
        } else {
            0
        },
        actionable,
        skip_reasons,
        members,
        members_truncated: false,
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct GraphRewireReport {
    rewired_endpoints: u64,
    deduplicated_edges: u64,
    removed_self_edges: u64,
}

fn rewire_graph_to_survivor(
    conn: &Connection,
    survivor: &Document,
    candidate_ids: &[&str],
) -> Result<GraphRewireReport> {
    let mut survivor_node_ids = graph_node_ids(conn, &survivor.id)?;
    let mut candidate_node_ids = Vec::new();
    for candidate_id in candidate_ids {
        candidate_node_ids.extend(graph_node_ids(conn, candidate_id)?);
    }
    candidate_node_ids.sort();
    candidate_node_ids.dedup();
    if survivor_node_ids.is_empty() && candidate_node_ids.is_empty() {
        return Ok(GraphRewireReport::default());
    }
    let canonical_node_id = if survivor_node_ids.is_empty() {
        let node_id = candidate_node_ids.remove(0);
        promote_node_to_survivor(conn, &node_id, survivor)?;
        node_id
    } else {
        let node_id = survivor_node_ids.remove(0);
        promote_node_to_survivor(conn, &node_id, survivor)?;
        node_id
    };

    let mut merged_node_ids = survivor_node_ids;
    merged_node_ids.extend(candidate_node_ids);
    merged_node_ids.sort();
    merged_node_ids.dedup();
    merged_node_ids.retain(|node_id| node_id != &canonical_node_id);

    let removed_self_edges =
        delete_edges_that_would_collapse_to_self(conn, &canonical_node_id, &merged_node_ids)?;
    let deduplicated_edges =
        deduplicate_prospective_edges(conn, &canonical_node_id, &merged_node_ids)?;

    let mut rewired_endpoints = 0u64;
    for old_node_id in &merged_node_ids {
        rewired_endpoints += conn.execute(
            "UPDATE graph_edges SET source_id = ? WHERE source_id = ?",
            params![canonical_node_id, old_node_id],
        )? as u64;
        rewired_endpoints += conn.execute(
            "UPDATE graph_edges SET target_id = ? WHERE target_id = ?",
            params![canonical_node_id, old_node_id],
        )? as u64;
        conn.execute("DELETE FROM graph_nodes WHERE id = ?", params![old_node_id])?;
    }

    Ok(GraphRewireReport {
        rewired_endpoints,
        deduplicated_edges,
        removed_self_edges,
    })
}

fn deduplicate_prospective_edges(
    conn: &Connection,
    canonical_node_id: &str,
    merged_node_ids: &[String],
) -> Result<u64> {
    if merged_node_ids.is_empty() {
        return Ok(0);
    }
    let mut collapse_ids = Vec::with_capacity(merged_node_ids.len() + 1);
    collapse_ids.push(canonical_node_id.to_string());
    collapse_ids.extend(merged_node_ids.iter().cloned());
    let merged_placeholders = std::iter::repeat_n("?", merged_node_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let collapse_placeholders = std::iter::repeat_n("?", collapse_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        r#"
        DELETE FROM graph_edges
        WHERE id IN (
          SELECT id FROM (
            SELECT id,
                   ROW_NUMBER() OVER (
                     PARTITION BY
                       CASE WHEN source_id IN ({merged_placeholders}) THEN ? ELSE source_id END,
                       CASE WHEN target_id IN ({merged_placeholders}) THEN ? ELSE target_id END,
                       rel_type, weight, context
                     ORDER BY
                       CASE WHEN source_id IN ({merged_placeholders})
                                  OR target_id IN ({merged_placeholders})
                            THEN 1 ELSE 0 END ASC,
                       created_at ASC, id ASC
                   ) AS duplicate_rank
            FROM graph_edges
            WHERE source_id IN ({collapse_placeholders})
               OR target_id IN ({collapse_placeholders})
          ) ranked
          WHERE duplicate_rank > 1
        )
        "#
    );
    let mut binds = Vec::with_capacity(merged_node_ids.len() * 4 + collapse_ids.len() * 2 + 2);
    binds.extend(merged_node_ids.iter().cloned());
    binds.push(canonical_node_id.to_string());
    binds.extend(merged_node_ids.iter().cloned());
    binds.push(canonical_node_id.to_string());
    binds.extend(merged_node_ids.iter().cloned());
    binds.extend(merged_node_ids.iter().cloned());
    binds.extend(collapse_ids.iter().cloned());
    binds.extend(collapse_ids);
    Ok(conn.execute(&sql, params_from_iter(binds.iter()))? as u64)
}

fn delete_edges_that_would_collapse_to_self(
    conn: &Connection,
    canonical_node_id: &str,
    merged_node_ids: &[String],
) -> Result<u64> {
    if merged_node_ids.is_empty() {
        return Ok(0);
    }
    let mut collapse_ids = Vec::with_capacity(merged_node_ids.len() + 1);
    collapse_ids.push(canonical_node_id.to_string());
    collapse_ids.extend(merged_node_ids.iter().cloned());
    let collapse_placeholders = std::iter::repeat_n("?", collapse_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let merged_placeholders = std::iter::repeat_n("?", merged_node_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        r#"
        DELETE FROM graph_edges
        WHERE source_id IN ({collapse_placeholders})
          AND target_id IN ({collapse_placeholders})
          AND (source_id IN ({merged_placeholders})
               OR target_id IN ({merged_placeholders}))
        "#
    );
    let mut binds = Vec::with_capacity(collapse_ids.len() * 2 + merged_node_ids.len() * 2);
    binds.extend(collapse_ids.iter().cloned());
    binds.extend(collapse_ids);
    binds.extend(merged_node_ids.iter().cloned());
    binds.extend(merged_node_ids.iter().cloned());
    Ok(conn.execute(&sql, params_from_iter(binds.iter()))? as u64)
}

fn promote_node_to_survivor(conn: &Connection, node_id: &str, survivor: &Document) -> Result<()> {
    conn.execute(
        r#"
        UPDATE graph_nodes
        SET kind = 'document', label = ?, document_id = ?, uri = ?,
            resolved = true, updated_at = now()
        WHERE id = ?
        "#,
        params![survivor.title, survivor.id, survivor.uri, node_id],
    )?;
    Ok(())
}

fn graph_node_ids(conn: &Connection, document_id: &str) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT id FROM graph_nodes WHERE document_id = ? ORDER BY id ASC")?;
    let mut rows = stmt.query(params![document_id])?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next()? {
        ids.push(row.get(0)?);
    }
    Ok(ids)
}

fn load_document_locked(conn: &Connection, document_id: &str) -> Result<Option<Document>> {
    let sql = format!("SELECT {DOCUMENT_SELECT} FROM documents WHERE id = ?");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![document_id])?;
    match rows.next()? {
        Some(row) => Ok(Some(rows::document(row)?)),
        None => Ok(None),
    }
}

fn nonnegative(value: i64) -> u64 {
    value.max(0) as u64
}

trait OptionalRow<T> {
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalRow<T> for std::result::Result<T, duckdb::Error> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(duckdb::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::db::SourceManifestWrite;
    use crate::models::{Chunk, GraphEdge, GraphNode};
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
        let store = Store::open(&dir.path().join("source-dedupe.duckdb")).unwrap();
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
        {
            let conn = store.lock().unwrap();
            conn.execute(
                r#"INSERT INTO kg_facts
                   (id, subject, predicate, object, status, source_document_id,
                    metadata_json, created_at, updated_at)
                   VALUES ('fact', 'a', 'rel', 'b', 'active', 'legacy', '{}', now(), now())"#,
                [],
            )
            .unwrap();
        }

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

        let conn = store.lock().unwrap();
        let manifest_document: String = conn
            .query_row(
                "SELECT document_id FROM source_manifest WHERE canonical_path = ?",
                params![source],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(manifest_document, "canonical");
        let fact_document: String = conn
            .query_row(
                "SELECT source_document_id FROM kg_facts WHERE id = 'fact'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fact_document, "canonical");
        let legacy_nodes: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_nodes WHERE document_id = 'legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_nodes, 0);
        let canonical_edges: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE source_id = 'canonical-node' OR target_id = 'canonical-node'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(canonical_edges, 2);
        let preserved_canonical_edge: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE id = 'z-canonical-out'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved_canonical_edge, 1);
        let removed_legacy_edge: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE id = 'a-legacy-out'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(removed_legacy_edge, 0);
        let dangling_edges: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE source_id = 'legacy-node' OR target_id = 'legacy-node'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dangling_edges, 0);
    }

    #[test]
    fn cleanup_requires_confirm_and_drains_an_oversized_group_in_bounded_batches() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-cap.duckdb")).unwrap();
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
        let store = Store::open(&dir.path().join("source-dedupe-promote.duckdb")).unwrap();
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
        let conn = store.lock().unwrap();
        let (document_id, uri, metadata): (String, String, String) = conn
            .query_row(
                "SELECT document_id, uri, metadata_json FROM graph_nodes WHERE id = 'legacy-node'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(document_id, "canonical");
        assert_eq!(uri, "file:///vault/promote.md");
        assert_eq!(metadata, r#"{"preserved":true}"#);
        let preserved_edge: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE id = 'incoming' AND target_id = 'legacy-node'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved_edge, 1);
    }

    #[test]
    fn cleanup_does_not_create_graph_state_when_duplicates_have_no_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-no-graph.duckdb")).unwrap();
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
        let graph_nodes: i64 = store
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*)::BIGINT FROM graph_nodes", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(graph_nodes, 0);
    }

    #[test]
    fn cleanup_removes_only_edges_that_its_node_collapse_would_make_self_loops() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-self-edges.duckdb")).unwrap();
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
        let conn = store.lock().unwrap();
        let preexisting_self: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE id = 'preexisting-self' AND source_id = 'canonical-node' AND target_id = 'canonical-node'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preexisting_self, 1);
        let new_self_edges: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE source_id = target_id AND id <> 'preexisting-self'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(new_self_edges, 0);
        let preserved_external: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE id = 'legacy-external' AND source_id = 'canonical-node' AND target_id = 'external-node'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved_external, 1);
    }

    #[test]
    fn cleanup_skips_candidate_that_owns_a_foreign_manifest_path() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-manifest.duckdb")).unwrap();
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
        assert!(report.groups[0]
            .skip_reasons
            .iter()
            .any(|reason| reason.contains("foreign source_manifest references")));
        assert!(store.get_document("legacy").unwrap().is_some());
        let conn = store.lock().unwrap();
        let manifest_document_id: String = conn
            .query_row(
                "SELECT document_id FROM source_manifest WHERE canonical_path = '/vault/foreign.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(manifest_document_id, "legacy");
    }

    #[test]
    fn cleanup_skips_manifest_path_owned_by_a_non_member_document() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-third-owner.duckdb")).unwrap();
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
        assert!(report.groups[0]
            .skip_reasons
            .iter()
            .any(|reason| reason.contains("owned by non-member document")));
        assert!(store.get_document("legacy").unwrap().is_some());
        let manifest_document_id: String = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT document_id FROM source_manifest WHERE canonical_path = ?",
                params![source],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(manifest_document_id, "third-owner");
    }

    #[test]
    fn cleanup_skips_candidate_with_revision_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-revisions.duckdb")).unwrap();
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
        assert!(report.groups[0]
            .skip_reasons
            .iter()
            .any(|reason| reason.contains("document revisions")));
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
        let store = Store::open(&dir.path().join("source-dedupe-chunks.duckdb")).unwrap();
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
        assert!(report.groups[0]
            .skip_reasons
            .iter()
            .any(|reason| reason.contains("canonical survivor has no chunks")));
        assert!(store.get_document("legacy").unwrap().is_some());
        assert_eq!(store.list_chunks_for_document("legacy").unwrap().len(), 1);
    }

    #[test]
    fn cleanup_preview_bounds_group_and_member_details_but_keeps_totals() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("source-dedupe-report-bound.duckdb")).unwrap();

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
        let store = Store::open(&dir.path().join("source-dedupe-report-priority.duckdb")).unwrap();
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
        let store = Store::open(&dir.path().join("source-dedupe-skips.duckdb")).unwrap();
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
        {
            let conn = store.lock().unwrap();
            conn.execute(
                "INSERT INTO collections (id, name, metadata_json, created_at, updated_at) VALUES ('c', 'C', '{}', now(), now())",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO collection_entries (collection_id, document_id, position) VALUES ('c', 'protected-legacy', 0)",
                [],
            )
            .unwrap();
        }

        let report = store.cleanup_source_duplicates(false, true, 10).unwrap();
        assert_eq!(report.matched_groups, 2);
        assert_eq!(report.actionable_groups, 0);
        assert_eq!(report.skipped_groups, 2);
        assert_eq!(report.candidate_count, 0);
        assert!(!report.applied);
        assert!(store.get_document("old-a").unwrap().is_some());
        assert!(store.get_document("protected-legacy").unwrap().is_some());
        assert!(report.groups.iter().any(|group| group
            .skip_reasons
            .iter()
            .any(|reason| reason.contains("canonical URI"))));
        assert!(report.groups.iter().any(|group| group
            .skip_reasons
            .iter()
            .any(|reason| reason.contains("collection references"))));
    }
}
