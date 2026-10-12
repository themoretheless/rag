//! Corpus analytics read model used by `maintain::analyze` and `compress`.
//!
//! Counts, size statistics, exact duplicates, orphans, stubs and archive
//! candidates are plain SQL over `documents`, `chunks` and the graph tables;
//! the maintenance layer turns them into issues and plans.

use chrono::{DateTime, Duration, Utc};
use duckdb::params;
use serde::{Deserialize, Serialize};

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

pub(crate) fn collect_counts(
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

pub(crate) fn group_count_locked(conn: &duckdb::Connection, sql: &str) -> Result<Vec<CountBucket>> {
    let mut stmt = conn.prepare(sql)?;
    let out = crate::db::rows::query_vec(&mut stmt, [], |row| {
        let key: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        Ok(CountBucket {
            key,
            count: count as u64,
        })
    })?;
    Ok(out)
}

pub(crate) fn collect_size_stats(store: &Store, db_size_bytes: Option<u64>) -> Result<SizeStats> {
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

/// Groups of documents sharing a `content_hash` (largest first, capped).
///
/// `active_only` drops archived/tombstoned rows: compression merges only live
/// copies, while corpus analysis reports every stored duplicate.
pub(crate) fn find_exact_duplicates(
    store: &Store,
    active_only: bool,
) -> Result<Vec<DuplicateGroup>> {
    let status_filter = if active_only {
        "AND COALESCE(status, 'active') NOT IN ('archived', 'tombstone')"
    } else {
        ""
    };
    let conn = store.lock()?;
    // Top groups and their members in one query (was one members query per group).
    let mut stmt = conn.prepare(&format!(
        r#"
        WITH top_groups AS (
          SELECT content_hash,
                 row_number() OVER (ORDER BY COUNT(*) DESC, content_hash ASC) AS group_rank
          FROM documents
          WHERE content_hash IS NOT NULL AND TRIM(content_hash) <> ''
            {status_filter}
          GROUP BY content_hash
          HAVING COUNT(*) > 1
          ORDER BY group_rank
          LIMIT 50
        )
        SELECT top_groups.content_hash, documents.id, documents.title
        FROM top_groups
        JOIN documents ON documents.content_hash = top_groups.content_hash
        WHERE TRUE {status_filter}
        ORDER BY top_groups.group_rank ASC, documents.created_at ASC
        "#
    ))?;
    let mut rows = stmt.query([])?;
    let mut groups: Vec<DuplicateGroup> = Vec::new();
    crate::db::rows::visit_rows(&mut rows, |row| {
        let hash: String = row.get(0)?;
        let id: String = row.get(1)?;
        let title: String = row.get(2)?;
        match groups.last_mut() {
            Some(group) if group.content_hash == hash => {
                group.document_ids.push(id);
                group.titles.push(title);
                group.count += 1;
            }
            _ => groups.push(DuplicateGroup {
                content_hash: hash,
                document_ids: vec![id],
                titles: vec![title],
                count: 1,
            }),
        }
        Ok(())
    })?;
    groups.retain(|group| group.count > 1);
    groups.truncate(REPORT_LIST_CAP);
    Ok(groups)
}

pub(crate) fn find_orphans(store: &Store) -> Result<(Vec<OrphanNode>, Vec<OrphanNode>)> {
    let conn = store.lock()?;

    // Nodes with no incident edges.
    let mut stmt = conn.prepare(&format!(
        r#"
        SELECT n.id, n.document_id, n.kind, n.label
        FROM graph_nodes n
        WHERE NOT EXISTS (
            SELECT 1 FROM ({incident}) AS incident WHERE incident.id = n.id
        )
        ORDER BY n.kind ASC, n.label ASC
        LIMIT 200
        "#,
        incident = crate::db::graph::INCIDENT_NODE_IDS,
    ))?;
    let mut rows = stmt.query([])?;
    let mut orphan_nodes = Vec::new();
    crate::db::rows::try_visit_rows(&mut rows, |row| {
        orphan_nodes.push(OrphanNode {
            id: row.get(0)?,
            document_id: row.get(1)?,
            kind: row.get(2)?,
            label: row.get(3)?,
            layer: None,
        });
        if orphan_nodes.len() >= REPORT_LIST_CAP {
            return Ok(std::ops::ControlFlow::Break(()));
        }
        Ok(std::ops::ControlFlow::Continue(()))
    })?;
    drop(rows);
    drop(stmt);

    // Wiki documents whose graph node (if any) has degree 0, or no node at all.
    let mut stmt = conn.prepare(&format!(
        r#"
        SELECT d.id, d.title, d.layer, n.id
        FROM documents d
        LEFT JOIN graph_nodes n ON n.document_id = d.id
        WHERE d.layer = 'wiki'
          AND (
            n.id IS NULL
            OR NOT EXISTS (
              SELECT 1 FROM ({incident}) AS incident WHERE incident.id = n.id
            )
          )
        ORDER BY d.title ASC
        LIMIT 200
        "#,
        incident = crate::db::graph::INCIDENT_NODE_IDS,
    ))?;
    let mut rows = stmt.query([])?;
    let mut orphan_wiki = Vec::new();
    crate::db::rows::try_visit_rows(&mut rows, |row| {
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
            return Ok(std::ops::ControlFlow::Break(()));
        }
        Ok(std::ops::ControlFlow::Continue(()))
    })?;
    Ok((orphan_nodes, orphan_wiki))
}

pub(crate) fn find_stubs(
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

    crate::db::rows::try_visit_rows(&mut rows, |row| {
        let node_id: String = row.get(0)?;
        let label: String = row.get(1)?;
        let created_raw: Option<String> = row.get(2)?;
        let created_at = created_raw.as_deref().and_then(parse_ts);
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
            return Ok(std::ops::ControlFlow::Break(()));
        }
        Ok(std::ops::ControlFlow::Continue(()))
    })?;
    Ok((unresolved, aging))
}

pub(crate) fn find_archive_candidates(
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
    crate::db::rows::try_visit_rows(&mut rows, |row| {
        let id: String = row.get(0)?;
        let title: String = row.get(1)?;
        let layer: String = row.get(2)?;
        let boost: f64 = row.get(3)?;
        let ts: String = row.get(4)?;
        let Some(updated_at) = parse_ts(&ts) else {
            return Ok(std::ops::ControlFlow::Continue(()));
        };
        if updated_at > cutoff && boost >= 0.5 {
            // Only flag recent docs when boost is clearly suppressed.
            return Ok(std::ops::ControlFlow::Continue(()));
        }
        let reason = if updated_at <= cutoff && boost < 1.0 {
            format!(
                "boost={boost:.2}, updated {}d ago",
                (now - updated_at).num_days()
            )
        } else if boost < 0.5 {
            format!("low boost={boost:.2}")
        } else {
            return Ok(std::ops::ControlFlow::Continue(()));
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
            return Ok(std::ops::ControlFlow::Break(()));
        }
        Ok(std::ops::ControlFlow::Continue(()))
    })?;
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
    let conn = store.lock()?;
    Ok(conn.query_row(
        r#"
        SELECT
          COUNT(*)::BIGINT,
          COUNT(d.id)::BIGINT
        FROM chunks c
        LEFT JOIN documents d ON d.id = c.document_id
        "#,
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

/// Oldest tombstoned documents first: `(id, layer)`, at most `limit`.
pub(crate) fn tombstone_candidates(store: &Store, limit: usize) -> Result<Vec<(String, String)>> {
    let conn = store.lock()?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, COALESCE(layer, 'raw') FROM documents
        WHERE COALESCE(status, 'active') = 'tombstone'
        ORDER BY updated_at ASC
        LIMIT ?
        "#,
    )?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = stmt
        .query_map(params![limit], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}
