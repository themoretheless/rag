//! Graph node/edge CRUD, filtered export, undirected BFS neighbors, and backlinks.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use duckdb::{params, params_from_iter};
use uuid::Uuid;

use super::store::Store;
use crate::error::{AppError, Result};
use crate::graph::REL_TUNNEL;
use crate::models::{
    EdgeCutRow, EdgeOrigin, GraphEdge, GraphFilter, GraphNode, GraphStats, GraphView, NodeKind,
    RelType,
};

/// GRAPH_DESIGN §9 hard cap for a local-graph walk. Hop-by-hop frontier queries
/// make depth affordable, so this bounds pathological requests rather than cost.
const MAX_LOCAL_GRAPH_DEPTH: u32 = 5;

impl Store {
    /// Insert or replace a graph node by primary key `id`.
    ///
    /// Preserves original `created_at` when the id already exists.
    pub fn upsert_graph_node(&self, node: &GraphNode) -> Result<()> {
        let conn = self.lock()?;
        upsert_graph_node_locked(&conn, node)
    }

    /// Insert edge rows (empty slice is a no-op).
    /// Link a document node to the canonical tag hubs for `labels` in one
    /// transaction; `replace` first drops its existing `tagged` edges. A
    /// failure part-way leaves the previous tag edges intact.
    pub fn set_document_tags(&self, node_id: &str, labels: &[String], replace: bool) -> Result<()> {
        self.with_tx(|tx| {
            if replace {
                tx.execute(
                    "DELETE FROM graph_edges WHERE source_id = ? AND rel_type = ?",
                    params![node_id, crate::graph::REL_TAGGED],
                )?;
            }
            for label in labels {
                let tag_id = resolve::upsert_tag_node(tx, label)?;
                let exists: i64 = tx.query_row(
                    "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE source_id = ? AND target_id = ? AND rel_type = ?",
                    params![node_id, tag_id.as_str(), crate::graph::REL_TAGGED],
                    |row| row.get(0),
                )?;
                if exists == 0 {
                    insert_graph_edges_locked(
                        tx,
                        &[GraphEdge {
                            id: uuid::Uuid::new_v4().to_string(),
                            source_id: node_id.to_string(),
                            target_id: tag_id,
                            rel_type: crate::graph::REL_TAGGED.into(),
                            weight: 1.0,
                            context: None,
                        }],
                    )?;
                }
            }
            Ok(())
        })
    }

    pub fn insert_graph_edges(&self, edges: &[GraphEdge]) -> Result<()> {
        let conn = self.lock()?;
        insert_graph_edges_locked(&conn, edges)
    }

    /// Remove a node's edges on purpose; document delete uses demote instead (§6.3).
    pub fn delete_edges_from(&self, source_id: &str) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "DELETE FROM graph_edges WHERE source_id = ?",
            params![source_id],
        )?;
        Ok(())
    }

    /// Delete only the edges body-text extraction owns for `source_id`.
    ///
    /// This is the single authoritative rebuild predicate of `GRAPH_DESIGN.md`
    /// §1.6: extraction-owned relation names are cleaned even when a row carries
    /// a mis-tagged owner, while `explicit` / `system` owners survive re-ingest.
    pub fn delete_derived_edges_from(&self, source_id: &str) -> Result<()> {
        let conn = self.lock()?;
        delete_derived_edges_from_locked(&conn, source_id)
    }

    /// Delete a graph node by id (does not cascade edges).
    pub fn delete_graph_node(&self, node_id: &str) -> Result<bool> {
        let conn = self.lock()?;
        let n = conn.execute("DELETE FROM graph_nodes WHERE id = ?", params![node_id])?;
        Ok(n > 0)
    }

    /// Find the graph node linked to a document id, if any.
    pub fn find_node_by_document_id(&self, doc_id: &str) -> Result<Option<GraphNode>> {
        let conn = self.lock()?;
        find_node_by_document_id_locked(&conn, doc_id)
    }

    /// Find nodes whose [`label_key`](crate::graph::normalize::label_key) equals the
    /// normalized form of `label` (case-, width- and whitespace-insensitive).
    pub fn find_nodes_by_label(&self, label: &str) -> Result<Vec<GraphNode>> {
        let conn = self.lock()?;
        find_nodes_by_label_locked(&conn, label)
    }

    /// Find a node by primary key.
    pub fn find_node_by_id(&self, node_id: &str) -> Result<Option<GraphNode>> {
        let conn = self.lock()?;
        load_node_locked(&conn, node_id)
    }

    /// Find a node by exact `uri` match.
    ///
    /// Several nodes can share a uri (§5.2 creates a second node rather than steal an
    /// owned one). The canonical copy is the resolved, earliest created node, with the id
    /// as the final tie-break, so repeated reads name the same node.
    pub fn find_node_by_uri(&self, uri: &str) -> Result<Option<GraphNode>> {
        let conn = self.lock()?;
        find_node_by_uri_locked(&conn, uri)
    }

    /// Resolve one free-form seed (UI / HTTP `seed=`): node id, then document
    /// id, then node URI, then exact label with the same document-first
    /// preference as [`Self::resolve_node`].
    pub fn resolve_seed(&self, seed: &str) -> Result<Option<GraphNode>> {
        let seed = seed.trim();
        if seed.is_empty() {
            return Ok(None);
        }
        if let Some(node) = self.find_node_by_id(seed)? {
            return Ok(Some(node));
        }
        if let Some(node) = self.find_node_by_document_id(seed)? {
            return Ok(Some(node));
        }
        if let Some(node) = self.find_node_by_uri(seed)? {
            return Ok(Some(node));
        }
        self.resolve_node(None, None, Some(seed))
    }

    /// Resolve a single node by id, document id, or exact label (priority order).
    ///
    /// Label matches prefer resolved `document` nodes, then any document, else first hit.
    pub fn resolve_node(
        &self,
        node_id: Option<&str>,
        document_id: Option<&str>,
        label: Option<&str>,
    ) -> Result<Option<GraphNode>> {
        if let Some(id) = node_id.map(str::trim).filter(|s| !s.is_empty()) {
            return self.find_node_by_id(id);
        }
        if let Some(doc_id) = document_id.map(str::trim).filter(|s| !s.is_empty()) {
            return self.find_node_by_document_id(doc_id);
        }
        if let Some(lab) = label.map(str::trim).filter(|s| !s.is_empty()) {
            return Ok(prefer_document_node(self.find_nodes_by_label(lab)?));
        }
        Ok(None)
    }

    /// Export nodes and edges with optional filters.
    ///
    /// - `kinds`: keep only nodes of those kinds
    /// - `seed_ids`: keep only those node ids
    /// - `rel_types`: keep only edges of those types
    /// - `max_nodes`: cap (default 500)
    ///
    /// Edges are restricted to the returned node set (both endpoints present).
    pub fn get_graph_view(&self, filter: GraphFilter) -> Result<GraphView> {
        // A cap of 0 historically still returned the first node.
        let max_nodes = i64::from(filter.max_nodes.unwrap_or(500).max(1));
        let conn = self.lock()?;

        // Kind / seed filters and the node cap run in SQL; edges are joined
        // against the same selected node set instead of scanning every edge.
        let mut node_where = Vec::new();
        let mut node_binds: Vec<String> = Vec::new();
        for (column, values) in [("kind", &filter.kinds), ("id", &filter.seed_ids)] {
            if let Some(values) = values {
                if values.is_empty() {
                    return Ok(GraphView::default());
                }
                node_where.push(format!("{column} IN ({})", vec!["?"; values.len()].join(",")));
                node_binds.extend(values.iter().cloned());
            }
        }
        let node_where = if node_where.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", node_where.join(" AND "))
        };
        let selected_nodes = format!(
            "SELECT id, kind, label, document_id, uri, resolved, metadata_json \
             FROM graph_nodes {node_where} ORDER BY label ASC, id ASC LIMIT {max_nodes}"
        );

        let nodes = {
            let mut stmt = conn.prepare(&selected_nodes)?;
            crate::db::rows::query_vec(&mut stmt, params_from_iter(node_binds.iter()), row_to_node)?
        };
        if nodes.is_empty() {
            return Ok(GraphView::default());
        }

        let mut edge_binds = node_binds.clone();
        let rel_where = match &filter.rel_types {
            Some(rels) if rels.is_empty() => return Ok(GraphView { nodes, edges: Vec::new() }),
            Some(rels) => {
                edge_binds.extend(rels.iter().cloned());
                format!("WHERE e.rel_type IN ({})", vec!["?"; rels.len()].join(","))
            }
            None => String::new(),
        };
        let edges = {
            let mut stmt = conn.prepare(&format!(
                r#"
                WITH selected AS ({selected_nodes})
                SELECT e.id, e.source_id, e.target_id, e.rel_type, e.weight, e.context
                FROM graph_edges e
                JOIN selected s ON s.id = e.source_id
                JOIN selected t ON t.id = e.target_id
                {rel_where}
                ORDER BY e.id ASC
                "#
            ))?;
            crate::db::rows::query_vec(&mut stmt, params_from_iter(edge_binds.iter()), row_to_edge)?
        };

        Ok(GraphView { nodes, edges })
    }

    /// Every graph node ordered by `(label, id)`, with `metadata_json` left as
    /// `{}`: for whole-graph diagnostics that never read node metadata.
    pub fn list_graph_nodes(&self) -> Result<Vec<GraphNode>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, label, document_id, uri, resolved, '{}' \
             FROM graph_nodes ORDER BY label ASC, id ASC",
        )?;
        crate::db::rows::query_vec(&mut stmt, [], row_to_node)
    }

    /// Every graph edge ordered by `id`, without its `context` text (`None`):
    /// for whole-graph diagnostics that only need endpoints and relation.
    pub fn list_graph_links(&self) -> Result<Vec<GraphEdge>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, source_id, target_id, rel_type, weight, NULL \
             FROM graph_edges ORDER BY id ASC",
        )?;
        crate::db::rows::query_vec(&mut stmt, [], row_to_edge)
    }

    /// Return every graph edge, including edges whose endpoint node is missing.
    ///
    /// Most graph views intentionally omit dangling edges because they cannot be
    /// rendered. Integrity diagnostics use this raw view to report broken links.
    pub fn list_graph_edges(&self) -> Result<Vec<GraphEdge>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, source_id, target_id, rel_type, weight, context
            FROM graph_edges
            ORDER BY id ASC
            "#,
        )?;
        let edges =
            crate::db::rows::query_vec(&mut stmt, [], row_to_edge)?;
        Ok(edges)
    }

    /// Undirected BFS neighborhood of `node_id` up to `depth`, capped at `max_nodes`.
    ///
    /// Edges are followed in both directions. Returns visited nodes and edges whose
    /// both endpoints are visited. Missing seed yields an empty view. `max_nodes == 0`
    /// is treated as 100.
    ///
    /// Unlike the UI projections, this raw local graph follows every relation type
    /// and keeps tag nodes, which is what wiki backlink walking and internal graph
    /// maintenance expect; MCP `get_neighbors` narrows it with [`Self::neighbors_filtered`].
    /// Each hop is one indexed frontier query (§7.2); `depth` is clamped to the §9
    /// hard cap because the walk is budget-bound, not time-bound.
    pub fn neighbors(&self, node_id: &str, depth: u32, max_nodes: u32) -> Result<GraphView> {
        self.neighbors_filtered(node_id, depth, max_nodes, None)
    }

    /// The same bounded walk restricted to a relation set (§7.1).
    ///
    /// This is what backs MCP `get_neighbors`, whose default is the PKB literary
    /// set rather than every relation. `None` (or an empty set) means the raw local
    /// graph of [`Self::neighbors`].
    pub fn neighbors_filtered(
        &self,
        node_id: &str,
        depth: u32,
        max_nodes: u32,
        rel_types: Option<&[String]>,
    ) -> Result<GraphView> {
        let max_nodes = if max_nodes == 0 { 100 } else { max_nodes };
        let depth = depth.clamp(1, MAX_LOCAL_GRAPH_DEPTH);

        let conn = self.lock()?;
        if load_node_locked(&conn, node_id)?.is_none() {
            return Ok(GraphView::default());
        }

        let selected: Vec<&str> = rel_types
            .unwrap_or_default()
            .iter()
            .map(String::as_str)
            .collect();
        let rel_filter = (!selected.is_empty()).then_some(selected.as_slice());

        let visited = expand_frontier(node_id, depth, max_nodes as usize, |frontier, limit| {
            discover_frontier_neighbors_locked(&conn, frontier, limit, rel_filter)
        })?;

        let nodes = load_nodes_by_ids_locked(&conn, &visited)?;
        let edges = load_edges_among_locked(&conn, &visited, rel_filter)?;
        Ok(GraphView { nodes, edges })
    }

    /// Incoming edges + source nodes for `node_id` (Obsidian-style backlinks).
    pub fn backlinks(&self, node_id: &str) -> Result<GraphView> {
        let conn = self.lock()?;
        let mut edges: Vec<GraphEdge> = Vec::new();
        {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, source_id, target_id, rel_type, weight, context
                FROM graph_edges
                WHERE target_id = ?
                ORDER BY id ASC
                "#,
            )?;
            crate::db::rows::for_each_row(&mut stmt, params![node_id], |row| {
                edges.push(row_to_edge(row)?);
                Ok(())
            })?;
        }

        // The target plus every distinct source, in one read (hubs and tag
        // nodes have thousands of incoming edges).
        let mut stmt = conn.prepare(
            r#"
            SELECT id, kind, label, document_id, uri, resolved, metadata_json
            FROM graph_nodes
            WHERE id = ?
               OR id IN (SELECT source_id FROM graph_edges WHERE target_id = ?)
            ORDER BY id ASC
            "#,
        )?;
        let nodes = crate::db::rows::query_vec(&mut stmt, params![node_id, node_id], row_to_node)?;

        Ok(GraphView { nodes, edges })
    }

    /// Create an explicit edge between two existing nodes.
    ///
    /// Empty `rel_type` defaults to `"related"`. Returns [`AppError::NotFound`] if
    /// either endpoint is missing. §1.1: the name must be a structural wire string
    /// or a hand-authored wiki relation, i.e. [`RelType::validate_wire`] gates this
    /// write; anything else is rejected before it can reach the §1.6 predicate.
    pub fn link_nodes(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: &str,
        weight: f64,
    ) -> Result<GraphEdge> {
        if self.find_node_by_id(source_id)?.is_none() {
            return Err(AppError::not_found(format!(
                "source node not found: {source_id}"
            )));
        }
        if self.find_node_by_id(target_id)?.is_none() {
            return Err(AppError::not_found(format!(
                "target node not found: {target_id}"
            )));
        }
        let edge = GraphEdge {
            id: Uuid::new_v4().to_string(),
            source_id: source_id.to_string(),
            target_id: target_id.to_string(),
            rel_type: if rel_type.is_empty() {
                "related".into()
            } else {
                rel_type.to_string()
            },
            weight,
            context: None,
        };
        self.insert_graph_edges(std::slice::from_ref(&edge))?;
        Ok(edge)
    }

    /// Document id pairs joined by a `rel_type` edge, as `(source, target)`.
    /// Both endpoints must be document-backed nodes.
    pub fn document_pairs_by_relation(&self, rel_type: &str) -> Result<Vec<(String, String)>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT s.document_id, t.document_id FROM graph_edges e \
             JOIN graph_nodes s ON s.id = e.source_id \
             JOIN graph_nodes t ON t.id = e.target_id \
             WHERE e.rel_type = ? AND s.document_id IS NOT NULL AND t.document_id IS NOT NULL",
        )?;
        crate::db::rows::query_vec(&mut stmt, params![rel_type], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
    }

    /// Aggregate node counts by kind and edge counts by `rel_type`.
    ///
    /// Empty graph yields zeros and empty maps. Includes all kinds/rel_types
    /// present in the store (including `tunnel`).
    pub fn graph_stats(&self) -> Result<GraphStats> {
        let conn = self.lock()?;

        let total_nodes: i64 =
            conn.query_row("SELECT COUNT(*) FROM graph_nodes", [], |r| r.get(0))?;
        let total_edges: i64 =
            conn.query_row("SELECT COUNT(*) FROM graph_edges", [], |r| r.get(0))?;

        let mut nodes_by_kind = std::collections::BTreeMap::new();
        {
            let mut stmt = conn.prepare(
                r#"
                SELECT kind, COUNT(*) AS n
                FROM graph_nodes
                GROUP BY kind
                ORDER BY kind ASC
                "#,
            )?;
            crate::db::rows::for_each_row(&mut stmt, [], |row| {
                let kind: String = row.get(0)?;
                let n: i64 = row.get(1)?;
                nodes_by_kind.insert(kind, n as u64);
                Ok(())
            })?;
        }

        let mut edges_by_rel_type = std::collections::BTreeMap::new();
        {
            let mut stmt = conn.prepare(
                r#"
                SELECT rel_type, COUNT(*) AS n
                FROM graph_edges
                GROUP BY rel_type
                ORDER BY rel_type ASC
                "#,
            )?;
            crate::db::rows::for_each_row(&mut stmt, [], |row| {
                let rel: String = row.get(0)?;
                let n: i64 = row.get(1)?;
                edges_by_rel_type.insert(rel, n as u64);
                Ok(())
            })?;
        }

        let mut edges_by_cut = Vec::new();
        {
            let mut stmt = conn.prepare(
                r#"
                SELECT 
                  COALESCE(rel_type, '(none)'),
                  COALESCE(origin, '(none)'),
                  COALESCE(context, ''),
                  COUNT(*) AS n
                FROM graph_edges
                GROUP BY 1, 2, 3
                ORDER BY 1, 2, 3
                "#,
            )?;
            crate::db::rows::for_each_row(&mut stmt, [], |row| {
                edges_by_cut.push(EdgeCutRow {
                    rel_type: row.get(0)?,
                    origin: row.get(1)?,
                    context: row.get(2)?,
                    count: row.get::<_, i64>(3)? as u64,
                });
                Ok(())
            })?;
        }

        Ok(GraphStats {
            total_nodes: total_nodes as u64,
            total_edges: total_edges as u64,
            nodes_by_kind,
            edges_by_rel_type,
            edges_by_cut,
        })
    }

    /// Remove unresolved stubs that no edge references after a complete graph rebuild.
    ///
    /// Stub nodes are derived placeholders, so an unreferenced stub carries no
    /// recoverable user information. Call this only after a successful full pass.
    pub fn prune_orphan_stubs(&self) -> Result<usize> {
        let conn = self.lock()?;
        let deleted = conn.execute(
            &format!(
                r#"
            DELETE FROM graph_nodes AS node
            WHERE node.kind = 'stub'
              AND NOT node.resolved
              AND NOT EXISTS (
                SELECT 1 FROM ({INCIDENT_NODE_IDS}) AS incident
                WHERE incident.id = node.id
              )
            "#
            ),
            [],
        )?;
        Ok(deleted)
    }

    /// Remove derived tag nodes that no rebuilt document references.
    pub fn prune_orphan_tags(&self) -> Result<usize> {
        let conn = self.lock()?;
        let deleted = conn.execute(
            &format!(
                r#"
            DELETE FROM graph_nodes AS node
            WHERE node.kind = 'tag'
              AND NOT EXISTS (
                SELECT 1 FROM ({INCIDENT_NODE_IDS}) AS incident
                WHERE incident.id = node.id
              )
            "#
            ),
            [],
        )?;
        Ok(deleted)
    }

    // --- Tunnels (`rel_type = tunnel`) ---

}

/// Ids of every node with at least one incident edge.
///
/// Orphan checks anti-join against this instead of a correlated
/// `source_id = n.id OR target_id = n.id`, which DuckDB cannot hash-join:
/// measured 77 s vs 35 ms on 20k nodes / 50k edges (debug build).
pub(crate) const INCIDENT_NODE_IDS: &str =
    "SELECT source_id AS id FROM graph_edges UNION SELECT target_id FROM graph_edges";

/// Inline a server-owned name list as a SQL `IN (…)` literal list.
///
/// Callers pass §1.1/§7.1 vocabulary constants, never caller input; the quote
/// doubling only keeps this honest if a name ever carries an apostrophe. Shared
/// with [`crate::db::schema`], whose §1.6 origin backfill inlines the structural
/// membership contexts from the same server-owned vocabulary.
pub(crate) fn sql_string_list<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    names
        .into_iter()
        .map(|name| format!("'{}'", name.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn upsert_graph_node_locked(conn: &duckdb::Connection, node: &GraphNode) -> Result<()> {
    let now = format_ts_now();
    let metadata = if node.metadata_json.is_empty() {
        "{}"
    } else {
        node.metadata_json.as_str()
    };
    // §1.1: unknown kinds are rejected where the row is written, not silently
    // stored for a projection filter to miss later.
    crate::models::parse_node_kind(&node.kind)?;
    conn.execute(
        r#"
        INSERT OR REPLACE INTO graph_nodes
          (id, kind, label, label_key, document_id, uri, resolved, metadata_json, created_at, updated_at)
        VALUES
          (?, ?, ?, ?, ?, ?, ?, ?,
           COALESCE(
             (SELECT created_at FROM graph_nodes WHERE id = ?),
             CAST(? AS TIMESTAMP)
           ),
           CAST(? AS TIMESTAMP))
        "#,
        params![
            node.id,
            node.kind,
            node.label,
            crate::graph::normalize::label_key(&node.label),
            node.document_id,
            node.uri,
            node.resolved,
            metadata,
            node.id,
            now.as_str(),
            now.as_str(),
        ],
    )?;
    Ok(())
}

/// Transaction-aware form of [`Store::delete_derived_edges_from`], used by the
/// document write path that already holds a `duckdb::Transaction`.
///
/// Deviation from the literal `GRAPH_DESIGN.md` §1.6 predicate, which reads
/// `origin = 'extract' OR rel_type IN (<extraction-owned names>)`. Applied
/// verbatim that OR also deletes user-created `link_nodes(..., 'wikilink')`
/// edges, i.e. the provenance column stops protecting anything an agent wrote
/// through the explicit API. Scoping the relation-name branch to rows whose
/// owner was never recorded keeps the documented intent (extraction-owned noise
/// is always reclaimed, even when a row is mis-tagged) without destroying
/// explicit edges. §1.6 is updated to match.
pub(crate) fn delete_derived_edges_from_locked(
    conn: &duckdb::Connection,
    source_id: &str,
) -> Result<()> {
    conn.execute(delete_derived_edges_sql(), params![source_id])?;
    Ok(())
}

/// The §1.6 delete statement, with the extraction-owned names inlined from
/// [`crate::models::EXTRACT_OWNED_REL_TYPES`]. Built once: this runs for every
/// document rebuild, and the name list is server-owned vocabulary.
fn delete_derived_edges_sql() -> &'static str {
    static SQL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SQL.get_or_init(|| {
        format!(
            "DELETE FROM graph_edges \
             WHERE source_id = ? \
               AND (origin = 'extract' \
                    OR (origin IS NULL AND rel_type IN ({extract_owned})))",
            extract_owned = sql_string_list(crate::models::EXTRACT_OWNED_REL_TYPES.iter().copied())
        )
    })
}

/// §6.3 / §14: deleting a document **demotes** its graph node instead of wiping it.
///
/// Inbound edges from other notes stay, so backlinks keep telling the truth about
/// what referenced the deleted note (Obsidian keeps it as an unresolved target).
/// Only the node's extraction-owned outbound edges are reclaimed, through the same
/// §1.6 predicate the incremental rebuild uses. `uri` is deliberately kept: it is
/// the key [`crate::graph::resolve`] re-binds a re-ingested file to the same node id.
pub(crate) fn demote_graph_for_document_locked(
    conn: &duckdb::Connection,
    document_id: &str,
) -> Result<u64> {
    const NODE_IDS_FOR_DOCUMENT: &str = "SELECT id FROM graph_nodes WHERE document_id = ?";
    let node_ids = {
        let mut stmt = conn.prepare(NODE_IDS_FOR_DOCUMENT)?;
        
        crate::db::rows::query_vec(&mut stmt, params![document_id], |row| Ok(row.get::<_, String>(0)?))?
    };
    for node_id in &node_ids {
        delete_derived_edges_from_locked(conn, node_id)?;
    }
    conn.execute(
        r#"
        UPDATE graph_nodes
        SET kind = 'stub', resolved = false, document_id = NULL,
            updated_at = CAST(? AS TIMESTAMP)
        WHERE document_id = ?
        "#,
        params![format_ts_now().as_str(), document_id],
    )?;
    Ok(node_ids.len() as u64)
}

pub(crate) fn insert_graph_edges_locked(
    conn: &duckdb::Connection,
    edges: &[GraphEdge],
) -> Result<()> {
    if edges.is_empty() {
        return Ok(());
    }
    validate_edge_vocabulary(edges.iter(), EdgeOrigin::EXPLICIT)?;
    let now = format_ts_now();
    crate::db::rows::insert_rows(
        conn,
        "INSERT INTO graph_edges \
         (id, source_id, target_id, rel_type, weight, context, created_at, origin) VALUES",
        "(?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), ?)",
        edges,
        |edge, values| {
            values.extend([
                &edge.id as &dyn duckdb::ToSql,
                &edge.source_id,
                &edge.target_id,
                &edge.rel_type,
                &edge.weight,
                &edge.context,
                &now,
                &EdgeOrigin::EXPLICIT,
            ]);
        },
    )
}

/// Where one extracted edge came from (§4.1, §6.1 step 5): the document span,
/// the alias and heading the author wrote, which occurrence of the same target
/// this is, and the chunk holding it. Only `origin = 'extract'` rows carry it —
/// a hand-made `link_nodes` edge has no span in any document.
pub(crate) struct DerivedEdge {
    pub edge: GraphEdge,
    pub alias: Option<String>,
    pub heading: Option<String>,
    /// Unicode scalar span of the markup, `None` for a system edge with no source
    /// span (project/directory membership).
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub occurrence: i64,
    pub chunk_id: Option<String>,
}

pub(crate) fn insert_derived_graph_edges_locked(
    conn: &duckdb::Connection,
    edges: &[DerivedEdge],
) -> Result<()> {
    if edges.is_empty() {
        return Ok(());
    }
    validate_edge_vocabulary(edges.iter().map(|row| &row.edge), EdgeOrigin::EXTRACT)?;
    let now = format_ts_now();
    crate::db::rows::insert_rows(
        conn,
        "INSERT INTO graph_edges \
         (id, source_id, target_id, rel_type, weight, context, created_at, origin, \
          alias, heading, chunk_id, char_start, char_end, occurrence) VALUES",
        "(?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), ?, ?, ?, ?, ?, ?, ?)",
        edges,
        |row, values| {
            values.extend([
                &row.edge.id as &dyn duckdb::ToSql,
                &row.edge.source_id,
                &row.edge.target_id,
                &row.edge.rel_type,
                &row.edge.weight,
                &row.edge.context,
                &now,
                &EdgeOrigin::EXTRACT,
                &row.alias,
                &row.heading,
                &row.chunk_id,
                &row.char_start,
                &row.char_end,
                &row.occurrence,
            ]);
        },
    )
}

/// §1.1: the store boundary is where the vocabulary is enforced, so no row can
/// reach the §1.6 rebuild predicate with a rel_type or owner nothing maps back to.
fn validate_edge_vocabulary<'a>(
    edges: impl Iterator<Item = &'a GraphEdge>,
    origin: &str,
) -> Result<()> {
    EdgeOrigin::parse(origin)?;
    for edge in edges {
        RelType::validate_wire(&edge.rel_type)?;
    }
    Ok(())
}

pub(crate) fn find_node_by_document_id_locked(
    conn: &duckdb::Connection,
    document_id: &str,
) -> Result<Option<GraphNode>> {
    find_one_node_locked(
        conn,
        r#"
        SELECT id, kind, label, document_id, uri, resolved, metadata_json
        FROM graph_nodes
        WHERE document_id = ?
        LIMIT 1
        "#,
        document_id,
    )
}

pub(crate) fn find_node_by_uri_locked(
    conn: &duckdb::Connection,
    uri: &str,
) -> Result<Option<GraphNode>> {
    // A uri can be carried by more than one node (measured on the 2026-09-25 live
    // snapshot: 5 groups of two `document` copies, each pair holding the same
    // document_id and the same neighbour set). Without an order, `LIMIT 1` resolved to
    // storage layout, so reads - and the §5.2 reuse that decides where new edges go -
    // could name either copy. Resolved wins over a demoted stub, the first writer wins
    // among equals (§5.2 keeps the original node id so its edges stay attached), and the
    // unique id closes the order.
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT id, kind, label, document_id, uri, resolved, metadata_json
        FROM graph_nodes
        WHERE uri = ?
        ORDER BY COALESCE(resolved, FALSE) DESC, created_at, id
        LIMIT 2
        "#,
    )?;
    let mut rows = stmt.query(params![uri])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let node = row_to_node(row)?;
    if rows.next()?.is_some() {
        tracing::warn!(
            uri,
            canonical_node_id = %node.id,
            "several graph nodes share this uri; the read follows the deterministic order"
        );
    }
    Ok(Some(node))
}

fn find_one_node_locked(
    conn: &duckdb::Connection,
    sql: &str,
    value: &str,
) -> Result<Option<GraphNode>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let mut rows = stmt.query(params![value])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_node(row)?)),
        None => Ok(None),
    }
}

pub(crate) fn find_nodes_by_label_locked(
    conn: &duckdb::Connection,
    label: &str,
) -> Result<Vec<GraphNode>> {
    let key = crate::graph::normalize::label_key(label);
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT id, kind, label, document_id, uri, resolved, metadata_json
        FROM graph_nodes
        WHERE label_key = ?
        ORDER BY resolved DESC, kind ASC, id ASC
        "#,
    )?;
    let out =
        crate::db::rows::query_vec(&mut stmt, params![key], row_to_node)?;
    Ok(out)
}

/// Among nodes sharing a label, prefer a resolved document node, then any
/// document node, then the first match.
fn prefer_document_node(mut nodes: Vec<GraphNode>) -> Option<GraphNode> {
    let pos = nodes
        .iter()
        .position(|n| NodeKind::Document.is(&n.kind) && n.resolved)
        .or_else(|| nodes.iter().position(|n| NodeKind::Document.is(&n.kind)))
        .or((!nodes.is_empty()).then_some(0))?;
    Some(nodes.swap_remove(pos))
}

fn format_ts_now() -> String {
    crate::util::format_db_timestamp(Utc::now())
}

fn row_to_node(row: &duckdb::Row<'_>) -> Result<GraphNode> {
    let resolved: bool = match row.get::<_, bool>(5) {
        Ok(b) => b,
        Err(_) => {
            let v: i64 = row.get(5)?;
            v != 0
        }
    };
    Ok(GraphNode {
        id: row.get(0)?,
        kind: row.get(1)?,
        label: row.get(2)?,
        document_id: row.get(3)?,
        uri: row.get(4)?,
        resolved,
        metadata_json: row
            .get::<_, Option<String>>(6)?
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "{}".into()),
    })
}

fn row_to_edge(row: &duckdb::Row<'_>) -> Result<GraphEdge> {
    Ok(GraphEdge {
        id: row.get(0)?,
        source_id: row.get(1)?,
        target_id: row.get(2)?,
        rel_type: row.get(3)?,
        weight: row.get(4)?,
        context: row.get(5)?,
    })
}

fn load_node_locked(conn: &duckdb::Connection, id: &str) -> Result<Option<GraphNode>> {
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT id, kind, label, document_id, uri, resolved, metadata_json
        FROM graph_nodes
        WHERE id = ?
        LIMIT 1
        "#,
    )?;
    let mut rows = stmt.query(params![id])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_node(row)?)),
        None => Ok(None),
    }
}

mod export;
pub mod resolve;
mod tunnels;

use self::export::*;

#[cfg(test)]
mod tests;
