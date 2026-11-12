//! Corpus analytics read model used by `maintain::analyze` and `compress` over `lin`.
//!
//! Counts, size statistics, exact duplicates, orphans, stubs and archive
//! candidates are computed over `documents`, `chunks` and the graph collections;
//! the maintenance layer turns them into issues and plans.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::lin_engine::{
    cell_bool, cell_f64, cell_opt_string, cell_str, cell_string, cell_ts_or_now, LinReader,
};
use crate::db::Store;
use crate::error::Result;
use crate::util::parse_db_timestamp;

/// Cap reported groups / lists to keep MCP payloads bounded.
pub(crate) const REPORT_LIST_CAP: usize = 100;

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
pub struct DuplicateGroup {
    pub content_hash: String,
    pub document_ids: Vec<String>,
    pub titles: Vec<String>,
    pub count: usize,
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
pub struct ArchiveCandidate {
    pub document_id: String,
    pub title: String,
    pub layer: String,
    pub boost: f64,
    pub updated_at: DateTime<Utc>,
    pub reason: String,
}

fn sorted_buckets(map: HashMap<String, u64>) -> Vec<CountBucket> {
    let mut out: Vec<CountBucket> = map
        .into_iter()
        .map(|(key, count)| CountBucket { key, count })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
    out
}

pub(crate) fn collect_counts(
    store: &Store,
    documents: u64,
    chunks: u64,
    nodes: u64,
    edges: u64,
) -> Result<CorpusCounts> {
    let r = store.read()?;
    let mut by_layer: HashMap<String, u64> = HashMap::new();
    let mut by_kind: HashMap<String, u64> = HashMap::new();
    let mut by_status: HashMap<String, u64> = HashMap::new();
    let mut by_wing: HashMap<String, u64> = HashMap::new();

    r.scan_each_projected(
        "documents",
        &["layer", "kind", "status", "wing"],
        |row| {
            let layer = cell_str(row, "layer").unwrap_or("raw");
            *by_layer.entry(layer.to_string()).or_insert(0) += 1;

            let kind = cell_str(row, "kind").unwrap_or("document");
            *by_kind.entry(kind.to_string()).or_insert(0) += 1;

            let status = cell_str(row, "status").unwrap_or("active");
            *by_status.entry(status.to_string()).or_insert(0) += 1;

            let wing = match cell_str(row, "wing") {
                Some(w) if !w.trim().is_empty() => w.to_string(),
                _ => "(unscoped)".to_string(),
            };
            *by_wing.entry(wing).or_insert(0) += 1;
            Ok(true)
        },
    )?;

    Ok(CorpusCounts {
        documents,
        chunks,
        nodes,
        edges,
        by_layer: sorted_buckets(by_layer),
        by_kind: sorted_buckets(by_kind),
        by_status: sorted_buckets(by_status),
        by_wing: sorted_buckets(by_wing),
    })
}

pub(crate) fn collect_size_stats(store: &Store, db_size_bytes: Option<u64>) -> Result<SizeStats> {
    let r = store.read()?;
    let mut chunk_n = 0u64;
    let mut chunk_chars = 0u64;
    r.scan_each_projected("chunks", &["content"], |row| {
        chunk_n += 1;
        if let Some(c) = cell_str(row, "content") {
            chunk_chars += c.chars().count() as u64;
        }
        Ok(true)
    })?;

    let mut doc_n = 0u64;
    let mut doc_chars = 0u64;
    r.scan_each_projected("documents", &["content"], |row| {
        doc_n += 1;
        if let Some(c) = cell_str(row, "content") {
            doc_chars += c.chars().count() as u64;
        }
        Ok(true)
    })?;

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
        total_chunk_chars: chunk_chars,
        total_document_chars: doc_chars,
    })
}

/// Groups of documents sharing a `content_hash` (largest first, capped).
///
/// `active_only` drops archived/tombstoned rows: compression merges only live
/// copies, while corpus analysis reports every stored duplicate.
pub(crate) fn find_exact_duplicates(
    store: &Store,
    active_only: bool,
) -> Result<Vec<DuplicateGroup>> {
    let r = store.read()?;
    let mut by_hash: HashMap<String, Vec<(DateTime<Utc>, String, String)>> = HashMap::new();

    r.scan_each_projected(
        "documents",
        &["id", "title", "status", "content_hash", "created_at"],
        |row| {
            if active_only {
                let st = cell_str(row, "status").unwrap_or("active");
                if matches!(st, "archived" | "tombstone") {
                    return Ok(true);
                }
            }
            let Some(hash) = cell_str(row, "content_hash").filter(|s| !s.trim().is_empty()) else {
                return Ok(true);
            };
            let id = cell_string(row, "id");
            let title = cell_string(row, "title");
            let created_at = cell_ts_or_now(row, "created_at");
            by_hash
                .entry(hash.to_string())
                .or_default()
                .push((created_at, id, title));
            Ok(true)
        },
    )?;

    let mut groups: Vec<DuplicateGroup> = by_hash
        .into_iter()
        .filter(|(_, members)| members.len() > 1)
        .map(|(content_hash, mut members)| {
            members.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            let count = members.len();
            let mut document_ids = Vec::with_capacity(count);
            let mut titles = Vec::with_capacity(count);
            for (_, id, title) in members {
                document_ids.push(id);
                titles.push(title);
            }
            DuplicateGroup {
                content_hash,
                document_ids,
                titles,
                count,
            }
        })
        .collect();

    groups.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.content_hash.cmp(&b.content_hash))
    });
    groups.truncate(50.min(REPORT_LIST_CAP));
    Ok(groups)
}

pub(crate) fn find_orphans(store: &Store) -> Result<(Vec<OrphanNode>, Vec<OrphanNode>)> {
    let r = store.read()?;
    let mut incident: HashSet<String> = HashSet::new();
    r.scan_each_projected("graph_edges", &["source_id", "target_id"], |row| {
        if let Some(sid) = cell_str(row, "source_id") {
            incident.insert(sid.to_string());
        }
        if let Some(tid) = cell_str(row, "target_id") {
            incident.insert(tid.to_string());
        }
        Ok(true)
    })?;

    let mut orphan_nodes: Vec<OrphanNode> = Vec::new();
    let mut nodes_by_doc: HashMap<String, Vec<String>> = HashMap::new();
    r.scan_each_projected(
        "graph_nodes",
        &["id", "document_id", "kind", "label"],
        |row| {
            let id = cell_string(row, "id");
            let doc_id = cell_opt_string(row, "document_id");
            if let Some(ref d) = doc_id {
                if !d.is_empty() {
                    nodes_by_doc.entry(d.clone()).or_default().push(id.clone());
                }
            }
            if !incident.contains(&id) {
                orphan_nodes.push(OrphanNode {
                    id,
                    document_id: doc_id,
                    kind: cell_string(row, "kind"),
                    label: cell_string(row, "label"),
                    layer: None,
                });
            }
            Ok(true)
        },
    )?;
    orphan_nodes.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.id.cmp(&b.id))
    });
    orphan_nodes.truncate(REPORT_LIST_CAP);

    let mut orphan_wiki: Vec<OrphanNode> = Vec::new();
    r.for_each_by_eq_projected(
        "documents",
        "layer",
        "wiki",
        Some(&["id", "title", "layer"]),
        |doc_row| {
            let doc_id = cell_string(doc_row, "id");
            let title = cell_string(doc_row, "title");
            let layer = cell_string(doc_row, "layer");
            match nodes_by_doc.get(&doc_id) {
                None => {
                    orphan_wiki.push(OrphanNode {
                        id: doc_id.clone(),
                        document_id: Some(doc_id),
                        kind: "wiki".into(),
                        label: title,
                        layer: Some(layer),
                    });
                }
                Some(nids) => {
                    for nid in nids {
                        if !incident.contains(nid) {
                            orphan_wiki.push(OrphanNode {
                                id: nid.clone(),
                                document_id: Some(doc_id.clone()),
                                kind: "wiki".into(),
                                label: title.clone(),
                                layer: Some(layer.clone()),
                            });
                        }
                    }
                }
            }
            Ok(true)
        },
    )?;
    orphan_wiki.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.id.cmp(&b.id)));
    orphan_wiki.truncate(REPORT_LIST_CAP);

    Ok((orphan_nodes, orphan_wiki))
}

pub(crate) fn find_stubs(
    store: &Store,
    stub_age_days: u32,
    now: DateTime<Utc>,
) -> Result<(Vec<StubInfo>, Vec<StubInfo>)> {
    let r = store.read()?;
    let stub_rows = r.find_by_eq("graph_nodes", "kind", "stub")?;
    let mut candidates: Vec<(Option<DateTime<Utc>>, String, String)> = Vec::new();

    for row in &stub_rows {
        if cell_bool(row, "resolved", false) {
            continue;
        }
        let created_at = cell_str(row, "created_at").and_then(parse_ts);
        candidates.push((
            created_at,
            cell_string(row, "id"),
            cell_string(row, "label"),
        ));
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    let age_cutoff = now - Duration::days(i64::from(stub_age_days));
    let mut unresolved = Vec::new();
    let mut aging = Vec::new();
    for (created_at, node_id, label) in candidates.into_iter().take(200) {
        let age_days = created_at.map(|t| (now - t).num_days());
        let info = StubInfo {
            node_id,
            label,
            created_at,
            age_days,
        };
        if created_at.is_some_and(|t| t <= age_cutoff) {
            aging.push(info.clone());
        }
        unresolved.push(info);
        if unresolved.len() >= REPORT_LIST_CAP {
            break;
        }
    }
    Ok((unresolved, aging))
}

pub(crate) fn find_archive_candidates(
    store: &Store,
    min_age_days: u32,
    now: DateTime<Utc>,
) -> Result<Vec<ArchiveCandidate>> {
    let cutoff = now - Duration::days(i64::from(min_age_days));
    let r = store.read()?;
    let mut pool: Vec<(f64, DateTime<Utc>, String, String, String)> = Vec::new();

    r.scan_each_projected(
        "documents",
        &[
            "id",
            "title",
            "status",
            "pinned",
            "boost",
            "layer",
            "updated_at",
        ],
        |row| {
            let status = cell_str(row, "status").unwrap_or("active");
            if status != "active" {
                return Ok(true);
            }
            if cell_bool(row, "pinned", false) {
                return Ok(true);
            }
            let boost = cell_f64(row, "boost", 1.0);
            if boost >= 1.0 {
                return Ok(true);
            }
            let layer = cell_str(row, "layer").unwrap_or("raw");
            if matches!(layer, "schema" | "index" | "log") {
                return Ok(true);
            }
            let Some(updated_at) = cell_str(row, "updated_at").and_then(parse_ts) else {
                return Ok(true);
            };
            pool.push((
                boost,
                updated_at,
                cell_string(row, "id"),
                cell_string(row, "title"),
                layer.to_string(),
            ));
            Ok(true)
        },
    )?;

    pool.sort_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });

    let mut out = Vec::new();
    for (boost, updated_at, id, title, layer) in pool.into_iter().take(200) {
        if updated_at > cutoff && boost >= 0.5 {
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

pub(crate) fn short_hash(h: &str) -> String {
    if h.chars().count() <= 12 {
        h.to_string()
    } else {
        let mut s: String = h.chars().take(12).collect();
        s.push('…');
        s
    }
}

pub(crate) fn parse_ts(raw: &str) -> Option<DateTime<Utc>> {
    parse_db_timestamp(raw)
}

/// `(all chunks, chunks whose document still exists)` from one snapshot.
pub(crate) fn chunk_attachment_counts(store: &Store) -> Result<(i64, i64)> {
    let r = store.read()?;
    let mut doc_exists_cache: HashMap<String, bool> = HashMap::new();
    let mut all_chunks = 0i64;
    let mut attached_chunks = 0i64;

    r.scan_each_projected("chunks", &["document_id"], |row| {
        all_chunks += 1;
        if let Some(doc_id) = cell_str(row, "document_id") {
            let exists = if let Some(&cached) = doc_exists_cache.get(doc_id) {
                cached
            } else {
                let found = if let Some(eng) = r.disk_v2() {
                    eng.exists_by_id("documents", doc_id)
                } else {
                    r.get_by_id("documents", doc_id)?.is_some()
                };
                doc_exists_cache.insert(doc_id.to_string(), found);
                found
            };
            if exists {
                attached_chunks += 1;
            }
        }
        Ok(true)
    })?;
    Ok((all_chunks, attached_chunks))
}

/// Oldest tombstoned documents first: `(id, layer)`, at most `limit`.
pub(crate) fn tombstone_candidates(store: &Store, limit: usize) -> Result<Vec<(String, String)>> {
    let r = store.read()?;
    let mut candidates: Vec<(DateTime<Utc>, String, String)> = Vec::new();
    r.for_each_by_eq_projected(
        "documents",
        "status",
        "tombstone",
        Some(&["id", "layer", "updated_at"]),
        |row| {
            candidates.push((
                cell_ts_or_now(row, "updated_at"),
                cell_string(row, "id"),
                cell_str(row, "layer").unwrap_or("raw").to_string(),
            ));
            Ok(true)
        },
    )?;
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    Ok(candidates
        .into_iter()
        .take(limit)
        .map(|(_, id, layer)| (id, layer))
        .collect())
}
