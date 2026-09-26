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
            let mut nodes = self.find_nodes_by_label(lab)?;
            if nodes.is_empty() {
                return Ok(None);
            }
            if let Some(pos) = nodes
                .iter()
                .position(|n| n.kind == "document" && n.resolved)
            {
                return Ok(Some(nodes.swap_remove(pos)));
            }
            if let Some(pos) = nodes.iter().position(|n| n.kind == "document") {
                return Ok(Some(nodes.swap_remove(pos)));
            }
            return Ok(Some(nodes.swap_remove(0)));
        }
        Ok(None)
    }

    /// Topology export for the optional inspector (`rag-mcp-ui --db`) and Mode C
    /// snapshot dumps.
    ///
    /// Applies PKB defaults (GRAPH_DESIGN §7.1 / EGUI_GRAPH_VIEW):
    /// - `rel_types = [wikilink, related]`
    /// - kinds: document, stub, entity (tags excluded unless `include_tags`)
    /// - `max_nodes` defaults to [`crate::models::UI_GRAPH_EXPORT_MAX_NODES`] (100,000)
    ///
    /// Returns pure topology (no positions). Prefer this over a bare
    /// [`Self::get_graph_view`] call when building a UI-oriented export so PKB
    /// defaults stay centralized (no egui dependency).
    pub fn export_graph_for_ui(
        &self,
        max_nodes: Option<u32>,
        include_tags: bool,
    ) -> Result<GraphView> {
        self.get_graph_view(GraphFilter::pkb_ui_export(max_nodes, include_tags))
    }

    /// Export a UI graph whose document nodes belong to one project (`documents.wing`).
    ///
    /// The project predicate is applied before the UI node cap. Unresolved stubs,
    /// entities, and optional tags directly connected to a project document are
    /// retained, while document nodes from other projects are not pulled through
    /// cross-project edges.
    pub fn export_project_graph_for_ui(
        &self,
        project: &str,
        max_nodes: Option<u32>,
        include_tags: bool,
    ) -> Result<GraphView> {
        let project = require_project(project)?;
        let max_nodes = max_nodes
            .unwrap_or(crate::models::UI_GRAPH_EXPORT_MAX_NODES)
            .clamp(1, crate::models::UI_GRAPH_EXPORT_MAX_NODES) as usize;
        let conn = self.lock()?;
        let mut nodes = load_bounded_project_nodes_locked(&conn, project, max_nodes)?;
        if nodes.len() < max_nodes {
            let project_node_ids = nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>();
            nodes.extend(load_bounded_project_companions_locked(
                &conn,
                &project_node_ids,
                max_nodes - nodes.len(),
                include_tags,
            )?);
        }
        enrich_document_placements_locked(&conn, &mut nodes)?;
        let node_ids = nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>();
        let edges = load_bounded_scoped_edges_locked(&conn, project, &node_ids, include_tags)?;
        Ok(GraphView { nodes, edges })
    }

    /// Resolve and expand a seed strictly inside one project's graph topology.
    pub fn export_project_neighbors_for_ui(
        &self,
        project: &str,
        seed: &str,
        depth: u32,
        max_nodes: u32,
        include_tags: bool,
    ) -> Result<GraphView> {
        let project = require_project(project)?;
        let seed = seed.trim();
        if seed.is_empty() {
            return Ok(GraphView::default());
        }
        let depth = depth.clamp(1, 3);
        let max_nodes = max_nodes.clamp(1, crate::models::UI_GRAPH_EXPORT_MAX_NODES) as usize;
        let conn = self.lock()?;
        let Some(seed_node) = resolve_project_seed_locked(&conn, project, seed, include_tags)?
        else {
            return Ok(GraphView::default());
        };

        let visited = expand_frontier(&seed_node.id, depth, max_nodes, |frontier, limit| {
            discover_scoped_neighbors_locked(&conn, project, frontier, limit, include_tags)
        })?;

        let mut nodes = load_nodes_by_ids_locked(&conn, &visited)?;
        enrich_document_placements_locked(&conn, &mut nodes)?;
        let edges = load_bounded_scoped_edges_locked(&conn, project, &visited, include_tags)?;
        Ok(GraphView { nodes, edges })
    }

    /// Resolve and expand a seed with the same PKB relation/kind policy as the
    /// global UI graph export.
    ///
    /// Filtering happens while discovering neighbors, before `max_nodes` is
    /// applied. This prevents hidden tag, tunnel, or dependency edges from
    /// consuming the focused-view budget.
    pub fn export_pkb_neighbors_for_ui(
        &self,
        seed: &str,
        depth: u32,
        max_nodes: u32,
        include_tags: bool,
    ) -> Result<GraphView> {
        let seed = seed.trim();
        if seed.is_empty() {
            return Ok(GraphView::default());
        }
        let depth = depth.clamp(1, 3);
        let max_nodes = max_nodes.clamp(1, crate::models::UI_GRAPH_EXPORT_MAX_NODES) as usize;
        let conn = self.lock()?;
        let Some(seed_node) = resolve_ui_seed_locked(&conn, seed, include_tags)? else {
            return Ok(GraphView::default());
        };

        let visited = expand_frontier(&seed_node.id, depth, max_nodes, |frontier, limit| {
            discover_ui_neighbors_locked(&conn, frontier, limit, include_tags)
        })?;

        let mut nodes = load_nodes_by_ids_locked(&conn, &visited)?;
        enrich_document_placements_locked(&conn, &mut nodes)?;
        let edges = load_bounded_ui_edges_locked(&conn, &visited, include_tags)?;
        Ok(GraphView { nodes, edges })
    }

    /// Resolve a UI seed string: exact node id, then `document_id`, then exact label.
    ///
    /// Label matches prefer resolved `document` nodes, then any document, else first hit.
    /// Empty / whitespace-only query → `Ok(None)`.
    pub fn find_seed_node(&self, query: &str) -> Result<Option<GraphNode>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(None);
        }
        if let Some(n) = self.find_node_by_id(q)? {
            return Ok(Some(n));
        }
        if let Some(n) = self.find_node_by_document_id(q)? {
            return Ok(Some(n));
        }
        let mut nodes = self.find_nodes_by_label(q)?;
        if nodes.is_empty() {
            return Ok(None);
        }
        if let Some(pos) = nodes
            .iter()
            .position(|n| n.kind == "document" && n.resolved)
        {
            return Ok(Some(nodes.swap_remove(pos)));
        }
        if let Some(pos) = nodes.iter().position(|n| n.kind == "document") {
            return Ok(Some(nodes.swap_remove(pos)));
        }
        Ok(Some(nodes.swap_remove(0)))
    }

    /// Local undirected neighborhood for UI: resolve seed, then BFS via
    /// [`Self::neighbors`]. Missing seed → empty view (not an error).
    ///
    /// `depth` / `max_nodes` follow [`Self::neighbors`] (`max_nodes == 0` → 100).
    /// Note: BFS currently follows all rel types; paint-side filters may still
    /// hide non-PKB edges. Prefer [`Self::export_graph_for_ui`] + client BFS when
    /// the full capped PKB export is already loaded.
    pub fn export_neighbors_for_ui(
        &self,
        seed: &str,
        depth: u32,
        max_nodes: u32,
    ) -> Result<GraphView> {
        let Some(node) = self.find_seed_node(seed)? else {
            return Ok(GraphView::default());
        };
        self.neighbors(&node.id, depth, max_nodes)
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
        let max_nodes = filter.max_nodes.unwrap_or(500) as usize;
        let conn = self.lock()?;

        let mut nodes: Vec<GraphNode> = Vec::new();
        {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, kind, label, document_id, uri, resolved, metadata_json
                FROM graph_nodes
                ORDER BY label ASC, id ASC
                "#,
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let node = row_to_node(row)?;
                if let Some(ref kinds) = filter.kinds {
                    if !kinds.iter().any(|k| k == &node.kind) {
                        continue;
                    }
                }
                if let Some(ref seeds) = filter.seed_ids {
                    if !seeds.iter().any(|s| s == &node.id) {
                        continue;
                    }
                }
                nodes.push(node);
                if nodes.len() >= max_nodes {
                    break;
                }
            }
        }

        let node_ids: HashSet<String> = nodes.iter().map(|n| n.id.clone()).collect();

        let mut edges: Vec<GraphEdge> = Vec::new();
        {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, source_id, target_id, rel_type, weight, context
                FROM graph_edges
                ORDER BY id ASC
                "#,
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let edge = row_to_edge(row)?;
                if let Some(ref rels) = filter.rel_types {
                    if !rels.iter().any(|r| r == &edge.rel_type) {
                        continue;
                    }
                }
                if !node_ids.contains(&edge.source_id) || !node_ids.contains(&edge.target_id) {
                    continue;
                }
                edges.push(edge);
            }
        }

        Ok(GraphView { nodes, edges })
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
        let mut rows = stmt.query([])?;
        let mut edges = Vec::new();
        while let Some(row) = rows.next()? {
            edges.push(row_to_edge(row)?);
        }
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
            let mut rows = stmt.query(params![node_id])?;
            while let Some(row) = rows.next()? {
                edges.push(row_to_edge(row)?);
            }
        }

        let mut nodes = Vec::new();
        let mut seen = HashSet::new();
        if let Some(n) = load_node_locked(&conn, node_id)? {
            seen.insert(n.id.clone());
            nodes.push(n);
        }
        for e in &edges {
            if seen.insert(e.source_id.clone()) {
                if let Some(n) = load_node_locked(&conn, &e.source_id)? {
                    nodes.push(n);
                }
            }
        }
        nodes.sort_by(|a, b| a.id.cmp(&b.id));

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
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let kind: String = row.get(0)?;
                let n: i64 = row.get(1)?;
                nodes_by_kind.insert(kind, n as u64);
            }
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
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let rel: String = row.get(0)?;
                let n: i64 = row.get(1)?;
                edges_by_rel_type.insert(rel, n as u64);
            }
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
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                edges_by_cut.push(EdgeCutRow {
                    rel_type: row.get(0)?,
                    origin: row.get(1)?,
                    context: row.get(2)?,
                    count: row.get::<_, i64>(3)? as u64,
                });
            }
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
            r#"
            DELETE FROM graph_nodes AS node
            WHERE node.kind = 'stub'
              AND NOT node.resolved
              AND NOT EXISTS (
                SELECT 1 FROM graph_edges AS edge
                WHERE edge.source_id = node.id OR edge.target_id = node.id
              )
            "#,
            [],
        )?;
        Ok(deleted)
    }

    /// Remove derived tag nodes that no rebuilt document references.
    pub fn prune_orphan_tags(&self) -> Result<usize> {
        let conn = self.lock()?;
        let deleted = conn.execute(
            r#"
            DELETE FROM graph_nodes AS node
            WHERE node.kind = 'tag'
              AND NOT EXISTS (
                SELECT 1 FROM graph_edges AS edge
                WHERE edge.source_id = node.id OR edge.target_id = node.id
              )
            "#,
            [],
        )?;
        Ok(deleted)
    }

    // --- Tunnels (`rel_type = tunnel`) ---

    /// Create an explicit tunnel edge between two existing nodes.
    ///
    /// Tunnels are treated as undirected for dedup: a second call with the same
    /// endpoints (either order) updates `weight` / `context` and returns the
    /// existing row. Self-loops are refused. Missing endpoints → [`AppError::NotFound`].
    pub fn create_tunnel(
        &self,
        source_id: &str,
        target_id: &str,
        weight: f64,
        context: Option<&str>,
    ) -> Result<GraphEdge> {
        let source_id = source_id.trim();
        let target_id = target_id.trim();
        if source_id.is_empty() || target_id.is_empty() {
            return Err(AppError::config(
                "create_tunnel requires non-empty source_id and target_id",
            ));
        }
        if source_id == target_id {
            return Err(AppError::config(
                "create_tunnel refuses self-loop (source_id == target_id)",
            ));
        }
        if !weight.is_finite() || weight <= 0.0 {
            return Err(AppError::config(
                "create_tunnel weight must be finite and > 0",
            ));
        }
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

        if let Some(existing) = self.find_tunnel_between(source_id, target_id)? {
            let conn = self.lock()?;
            let ctx = context.map(str::trim).filter(|s| !s.is_empty());
            conn.execute(
                r#"
                UPDATE graph_edges
                SET weight = ?, context = ?
                WHERE id = ? AND rel_type = ?
                "#,
                params![weight, ctx, existing.id, REL_TUNNEL],
            )?;
            return Ok(GraphEdge {
                id: existing.id,
                source_id: existing.source_id,
                target_id: existing.target_id,
                rel_type: REL_TUNNEL.into(),
                weight,
                context: ctx.map(str::to_string),
            });
        }

        let edge = GraphEdge {
            id: Uuid::new_v4().to_string(),
            source_id: source_id.to_string(),
            target_id: target_id.to_string(),
            rel_type: REL_TUNNEL.into(),
            weight,
            context: context
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        };
        self.insert_graph_edges(std::slice::from_ref(&edge))?;
        Ok(edge)
    }

    /// List tunnel edges, optionally incident to `node_id`.
    ///
    /// Ordered by `id` ascending. Empty graph → empty vec.
    pub fn list_tunnels(&self, node_id: Option<&str>) -> Result<Vec<GraphEdge>> {
        let conn = self.lock()?;
        let node_id = node_id.map(str::trim).filter(|s| !s.is_empty());
        let mut out = Vec::new();
        if let Some(nid) = node_id {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, source_id, target_id, rel_type, weight, context
                FROM graph_edges
                WHERE rel_type = ?
                  AND (source_id = ? OR target_id = ?)
                ORDER BY id ASC
                "#,
            )?;
            let mut rows = stmt.query(params![REL_TUNNEL, nid, nid])?;
            while let Some(row) = rows.next()? {
                out.push(row_to_edge(row)?);
            }
        } else {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, source_id, target_id, rel_type, weight, context
                FROM graph_edges
                WHERE rel_type = ?
                ORDER BY id ASC
                "#,
            )?;
            let mut rows = stmt.query(params![REL_TUNNEL])?;
            while let Some(row) = rows.next()? {
                out.push(row_to_edge(row)?);
            }
        }
        Ok(out)
    }

    /// Delete a tunnel edge by primary key. Returns `true` if a tunnel row was removed.
    ///
    /// Non-tunnel edges with the same id are left alone (returns `false`).
    pub fn delete_tunnel(&self, tunnel_id: &str) -> Result<bool> {
        let tunnel_id = tunnel_id.trim();
        if tunnel_id.is_empty() {
            return Err(AppError::config(
                "delete_tunnel requires non-empty tunnel_id",
            ));
        }
        let conn = self.lock()?;
        let n = conn.execute(
            "DELETE FROM graph_edges WHERE id = ? AND rel_type = ?",
            params![tunnel_id, REL_TUNNEL],
        )?;
        Ok(n > 0)
    }

    /// Multi-hop undirected BFS following **only** `rel_type=tunnel` edges.
    ///
    /// Missing seed → empty view. `depth` default semantics: caller supplies;
    /// `max_nodes == 0` is treated as 100. Returned edges are tunnel-only with
    /// both endpoints in the visited set.
    pub fn follow_tunnels(&self, node_id: &str, depth: u32, max_nodes: u32) -> Result<GraphView> {
        let max_nodes = if max_nodes == 0 { 100 } else { max_nodes };
        let depth = depth.clamp(1, MAX_LOCAL_GRAPH_DEPTH);
        let node_id = node_id.trim();
        if node_id.is_empty() {
            return Err(AppError::config(
                "follow_tunnels requires non-empty node_id",
            ));
        }

        let conn = self.lock()?;
        if load_node_locked(&conn, node_id)?.is_none() {
            return Ok(GraphView::default());
        }

        let visited = expand_frontier(node_id, depth, max_nodes as usize, |frontier, limit| {
            discover_frontier_neighbors_locked(&conn, frontier, limit, Some(&[REL_TUNNEL]))
        })?;

        let nodes = load_nodes_by_ids_locked(&conn, &visited)?;
        let edges = load_edges_among_locked(&conn, &visited, Some(&[REL_TUNNEL]))?;
        Ok(GraphView { nodes, edges })
    }

    /// Find tunnel edges matching optional filters.
    ///
    /// - `node_id`: incident to this node
    /// - `other_node_id`: must also be incident (pair / bridge filter; undirected)
    /// - `wing`: either endpoint's document has this `wing` (via `graph_nodes.document_id`)
    ///
    /// When no filters are set, returns all tunnels (same as [`Self::list_tunnels`] with
    /// `None`). Cap with `limit` when `Some` (0 means unlimited).
    pub fn find_tunnels(
        &self,
        node_id: Option<&str>,
        other_node_id: Option<&str>,
        wing: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<GraphEdge>> {
        let node_id = node_id.map(str::trim).filter(|s| !s.is_empty());
        let other_node_id = other_node_id.map(str::trim).filter(|s| !s.is_empty());
        let wing = wing.map(str::trim).filter(|s| !s.is_empty());

        if other_node_id.is_some() && node_id.is_none() {
            return Err(AppError::config(
                "find_tunnels: other_node_id requires node_id",
            ));
        }

        let mut edges = if let (Some(a), Some(b)) = (node_id, other_node_id) {
            // Pair: tunnels between a and b (either direction).
            let conn = self.lock()?;
            let mut stmt = conn.prepare(
                r#"
                SELECT id, source_id, target_id, rel_type, weight, context
                FROM graph_edges
                WHERE rel_type = ?
                  AND (
                    (source_id = ? AND target_id = ?)
                    OR (source_id = ? AND target_id = ?)
                  )
                ORDER BY id ASC
                "#,
            )?;
            let mut rows = stmt.query(params![REL_TUNNEL, a, b, b, a])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(row_to_edge(row)?);
            }
            out
        } else {
            self.list_tunnels(node_id)?
        };

        if let Some(w) = wing {
            let mut kept = Vec::new();
            for e in edges {
                if self.tunnel_touches_wing(&e, w)? {
                    kept.push(e);
                }
            }
            edges = kept;
        }

        if let Some(lim) = limit {
            if lim > 0 && edges.len() > lim as usize {
                edges.truncate(lim as usize);
            }
        }
        Ok(edges)
    }

    /// True when either endpoint node is linked to a document in `wing`.
    fn tunnel_touches_wing(&self, edge: &GraphEdge, wing: &str) -> Result<bool> {
        for nid in [&edge.source_id, &edge.target_id] {
            if let Some(node) = self.find_node_by_id(nid)? {
                if let Some(ref doc_id) = node.document_id {
                    if let Some(doc) = self.get_document(doc_id)? {
                        if doc.wing.as_deref() == Some(wing) {
                            return Ok(true);
                        }
                    }
                }
            }
        }
        Ok(false)
    }

    /// Existing tunnel edge between two nodes (either direction), if any.
    fn find_tunnel_between(&self, a: &str, b: &str) -> Result<Option<GraphEdge>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, source_id, target_id, rel_type, weight, context
            FROM graph_edges
            WHERE rel_type = ?
              AND (
                (source_id = ? AND target_id = ?)
                OR (source_id = ? AND target_id = ?)
              )
            ORDER BY id ASC
            LIMIT 1
            "#,
        )?;
        let mut rows = stmt.query(params![REL_TUNNEL, a, b, b, a])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_edge(row)?)),
            None => Ok(None),
        }
    }
}

fn require_project(project: &str) -> Result<&str> {
    let project = project.trim();
    if project.is_empty() {
        return Err(AppError::config("project must be non-empty"));
    }
    Ok(project)
}

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

/// §7.1 default relation set for project-scoped UI queries, from the one source
/// ([`crate::models::pkb_rel_types`]) the MCP tools and the UI export use too.
fn scoped_relation_types(include_tags: bool) -> String {
    sql_string_list(
        crate::models::pkb_rel_types(include_tags)
            .iter()
            .map(String::as_str),
    )
}

/// Companion (non-document) node kinds reachable from a project export.
fn scoped_companion_kinds(include_tags: bool) -> String {
    sql_string_list(
        crate::models::pkb_node_kinds(include_tags)
            .iter()
            .map(String::as_str)
            .filter(|kind| *kind != "document"),
    )
}

/// Node kinds a UI export may contain (§7.1).
fn ui_node_kinds(include_tags: bool) -> String {
    sql_string_list(
        crate::models::pkb_node_kinds(include_tags)
            .iter()
            .map(String::as_str),
    )
}

fn values_clause(len: usize) -> String {
    (0..len).map(|_| "(?)").collect::<Vec<_>>().join(", ")
}

fn placeholder_list(len: usize) -> String {
    vec!["?"; len].join(", ")
}

/// Make project-scoped graph exports self-describing for UI-side room/layer filters.
///
/// Graph extraction intentionally keeps node metadata independent from document
/// metadata, so placement is joined from the authoritative document row only at
/// the project export boundary. Document placement wins over stale graph metadata;
/// unrelated graph metadata is preserved.
fn enrich_document_placements_locked(
    conn: &duckdb::Connection,
    nodes: &mut [GraphNode],
) -> Result<()> {
    let mut document_ids = nodes
        .iter()
        .filter_map(|node| node.document_id.as_ref())
        .collect::<Vec<_>>();
    document_ids.sort_unstable();
    document_ids.dedup();
    if document_ids.is_empty() {
        return Ok(());
    }

    let selected = values_clause(document_ids.len());
    let sql = format!(
        r#"
        WITH selected_documents(id) AS (VALUES {selected})
        SELECT d.id, d.wing, d.room, d.layer
        FROM selected_documents selected
        JOIN documents d ON d.id = selected.id
        "#
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(document_ids.iter()))?;
    let mut placements = HashMap::with_capacity(document_ids.len());
    while let Some(row) = rows.next()? {
        placements.insert(
            row.get::<_, String>(0)?,
            (
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "raw".into()),
            ),
        );
    }

    for node in nodes {
        let Some(document_id) = node.document_id.as_ref() else {
            continue;
        };
        let Some((wing, room, layer)) = placements.get(document_id) else {
            continue;
        };
        let mut metadata = serde_json::from_str::<serde_json::Value>(&node.metadata_json)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        set_optional_metadata(&mut metadata, "wing", wing.as_deref());
        set_optional_metadata(&mut metadata, "room", room.as_deref());
        set_optional_metadata(&mut metadata, "layer", Some(layer));
        node.metadata_json = serde_json::Value::Object(metadata).to_string();
    }
    Ok(())
}

fn set_optional_metadata(
    metadata: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<&str>,
) {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => {
            metadata.insert(
                key.to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
        None => {
            metadata.remove(key);
        }
    }
}

fn load_bounded_project_nodes_locked(
    conn: &duckdb::Connection,
    project: &str,
    limit: usize,
) -> Result<Vec<GraphNode>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let sql = format!(
        r#"
        SELECT n.id, n.kind, n.label, n.document_id, n.uri, n.resolved, n.metadata_json
        FROM graph_nodes n
        JOIN documents d ON d.id = n.document_id
        WHERE d.wing = ?
          AND n.kind IN ('document', 'stub', 'entity')
        ORDER BY n.label, n.id
        LIMIT {limit}
        "#
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![project])?;
    let mut nodes = Vec::with_capacity(limit);
    while let Some(row) = rows.next()? {
        nodes.push(row_to_node(row)?);
    }
    Ok(nodes)
}

fn load_bounded_project_companions_locked(
    conn: &duckdb::Connection,
    project_node_ids: &[String],
    limit: usize,
    include_tags: bool,
) -> Result<Vec<GraphNode>> {
    if limit == 0 || project_node_ids.is_empty() {
        return Ok(Vec::new());
    }

    let selected = values_clause(project_node_ids.len());
    let relation_types = scoped_relation_types(include_tags);
    let companion_kinds = scoped_companion_kinds(include_tags);
    let sql = format!(
        r#"
        WITH selected_project_nodes(id) AS (VALUES {selected}),
        direct_companions(id) AS (
          SELECT e.target_id
          FROM selected_project_nodes selected
          JOIN graph_edges e ON e.source_id = selected.id
          WHERE e.rel_type IN ({relation_types})
          UNION ALL
          SELECT e.source_id
          FROM selected_project_nodes selected
          JOIN graph_edges e ON e.target_id = selected.id
          WHERE e.rel_type IN ({relation_types})
        )
        SELECT DISTINCT
          n.id, n.kind, n.label, n.document_id, n.uri, n.resolved, n.metadata_json
        FROM direct_companions direct
        JOIN graph_nodes n ON n.id = direct.id
        WHERE n.document_id IS NULL
          AND n.kind IN ({companion_kinds})
        ORDER BY n.label, n.id
        LIMIT {limit}
        "#
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(project_node_ids.iter()))?;
    let mut nodes = Vec::with_capacity(limit);
    while let Some(row) = rows.next()? {
        nodes.push(row_to_node(row)?);
    }
    Ok(nodes)
}

fn resolve_project_seed_locked(
    conn: &duckdb::Connection,
    project: &str,
    seed: &str,
    include_tags: bool,
) -> Result<Option<GraphNode>> {
    for column in ["id", "document_id", "label"] {
        if let Some(node) = find_scoped_node_locked(conn, project, column, seed, include_tags)? {
            return Ok(Some(node));
        }
    }
    Ok(None)
}

fn resolve_ui_seed_locked(
    conn: &duckdb::Connection,
    seed: &str,
    include_tags: bool,
) -> Result<Option<GraphNode>> {
    let node_kinds = ui_node_kinds(include_tags);
    for column in ["id", "document_id", "label"] {
        let sql = format!(
            r#"
            SELECT id, kind, label, document_id, uri, resolved, metadata_json
            FROM graph_nodes
            WHERE {column} = ?
              AND kind IN ({node_kinds})
            ORDER BY
              CASE
                WHEN kind = 'document' AND resolved THEN 0
                WHEN kind = 'document' THEN 1
                ELSE 2
              END,
              id
            LIMIT 1
            "#
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params![seed])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(row_to_node(row)?));
        }
    }
    Ok(None)
}

fn find_scoped_node_locked(
    conn: &duckdb::Connection,
    project: &str,
    column: &str,
    value: &str,
    include_tags: bool,
) -> Result<Option<GraphNode>> {
    let relation_types = scoped_relation_types(include_tags);
    let companion_kinds = scoped_companion_kinds(include_tags);
    let sql = format!(
        r#"
        SELECT n.id, n.kind, n.label, n.document_id, n.uri, n.resolved, n.metadata_json
        FROM graph_nodes n
        LEFT JOIN documents d ON d.id = n.document_id
        WHERE n.{column} = ?
          AND (
            (
              n.document_id IS NOT NULL
              AND d.wing = ?
              AND n.kind IN ('document', 'stub', 'entity')
            )
            OR (
              n.document_id IS NULL
              AND n.kind IN ({companion_kinds})
              AND EXISTS (
                SELECT 1
                FROM graph_edges e
                JOIN graph_nodes project_node
                  ON project_node.id = CASE
                    WHEN e.source_id = n.id THEN e.target_id
                    ELSE e.source_id
                  END
                JOIN documents project_document
                  ON project_document.id = project_node.document_id
                WHERE (e.source_id = n.id OR e.target_id = n.id)
                  AND project_document.wing = ?
                  AND project_node.kind IN ('document', 'stub', 'entity')
                  AND e.rel_type IN ({relation_types})
                LIMIT 1
              )
            )
          )
        ORDER BY
          CASE
            WHEN n.kind = 'document' AND n.resolved THEN 0
            WHEN n.kind = 'document' THEN 1
            ELSE 2
          END,
          n.id
        LIMIT 1
        "#
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![value, project, project])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_node(row)?)),
        None => Ok(None),
    }
}

/// §7.2 local-graph BFS: `discover` is called once per hop with only the current
/// frontier, so no variant ever materialises the whole edge table.
///
/// `candidate_limit` is widened by `seen` so already-visited ids that the query
/// returns first cannot starve the budget; the caller-side `seen` filter then
/// takes the rest. `max_nodes` must already be non-zero.
fn expand_frontier(
    seed_id: &str,
    depth: u32,
    max_nodes: usize,
    mut discover: impl FnMut(&[String], usize) -> Result<Vec<String>>,
) -> Result<Vec<String>> {
    let mut visited = vec![seed_id.to_string()];
    let mut seen = HashSet::from([seed_id.to_string()]);
    let mut frontier = vec![seed_id.to_string()];
    for _ in 0..depth {
        let remaining = max_nodes - visited.len();
        if remaining == 0 {
            break;
        }
        let candidates = discover(&frontier, remaining + seen.len())?;
        let mut next_frontier = Vec::new();
        for candidate in candidates {
            if seen.insert(candidate.clone()) {
                next_frontier.push(candidate);
                if next_frontier.len() == remaining {
                    break;
                }
            }
        }
        if next_frontier.is_empty() {
            break;
        }
        visited.extend(next_frontier.iter().cloned());
        frontier = next_frontier;
    }
    Ok(visited)
}

/// One undirected hop over `frontier` (§7.2), reporting only ids that still have
/// a node row. `rel_type = Some` restricts the hop to a single projection such as
/// tunnels; `None` follows every relation and node kind, which is the raw
/// [`Store::neighbors`] contract (tag hubs included).
fn discover_frontier_neighbors_locked(
    conn: &duckdb::Connection,
    frontier: &[String],
    limit: usize,
    rel_types: Option<&[&str]>,
) -> Result<Vec<String>> {
    if frontier.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let frontier_values = (0..frontier.len())
        .map(|ordinal| format!("(?, {ordinal})"))
        .collect::<Vec<_>>()
        .join(", ");
    let rel_filter = match rel_types {
        Some(rel_types) if !rel_types.is_empty() => format!(
            "AND incident.rel_type IN ({})",
            placeholder_list(rel_types.len())
        ),
        _ => String::new(),
    };
    let sql = format!(
        r#"
        WITH frontier(id, ordinal) AS (VALUES {frontier_values}),
        incident AS (
          SELECT frontier.ordinal AS frontier_ordinal, e.id AS edge_id,
                 e.target_id AS neighbor_id, e.rel_type
          FROM frontier JOIN graph_edges e ON e.source_id = frontier.id
          UNION ALL
          SELECT frontier.ordinal AS frontier_ordinal, e.id AS edge_id,
                 e.source_id AS neighbor_id, e.rel_type
          FROM frontier JOIN graph_edges e ON e.target_id = frontier.id
        )
        SELECT incident.neighbor_id
        FROM incident
        WHERE EXISTS (SELECT 1 FROM graph_nodes n WHERE n.id = incident.neighbor_id)
          {rel_filter}
        GROUP BY incident.neighbor_id
        ORDER BY MIN(incident.frontier_ordinal), MIN(incident.edge_id), incident.neighbor_id
        LIMIT {limit}
        "#
    );
    let mut binds = frontier.to_vec();
    if let Some(rel_types) = rel_types {
        binds.extend(rel_types.iter().map(|rel| (*rel).to_string()));
    }
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(binds.iter()))?;
    let mut neighbors = Vec::with_capacity(limit.min(1024));
    while let Some(row) = rows.next()? {
        neighbors.push(row.get(0)?);
    }
    Ok(neighbors)
}

/// Edges with both endpoints inside `node_ids` — the edge set the pre-§7.2
/// in-memory BFS reported, with no relation cap.
fn load_edges_among_locked(
    conn: &duckdb::Connection,
    node_ids: &[String],
    rel_types: Option<&[&str]>,
) -> Result<Vec<GraphEdge>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let selected = values_clause(node_ids.len());
    let rel_filter = match rel_types {
        Some(rel_types) if !rel_types.is_empty() => {
            format!(
                "WHERE e.rel_type IN ({})",
                placeholder_list(rel_types.len())
            )
        }
        _ => String::new(),
    };
    let sql = format!(
        r#"
        WITH selected(id) AS (VALUES {selected})
        SELECT e.id, e.source_id, e.target_id, e.rel_type, e.weight, e.context
        FROM graph_edges e
        JOIN selected source_selected ON source_selected.id = e.source_id
        JOIN selected target_selected ON target_selected.id = e.target_id
        {rel_filter}
        ORDER BY e.id
        "#
    );
    let mut binds = node_ids.to_vec();
    if let Some(rel_types) = rel_types {
        binds.extend(rel_types.iter().map(|rel| (*rel).to_string()));
    }
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(binds.iter()))?;
    let mut edges = Vec::new();
    while let Some(row) = rows.next()? {
        edges.push(row_to_edge(row)?);
    }
    Ok(edges)
}

fn discover_scoped_neighbors_locked(
    conn: &duckdb::Connection,
    project: &str,
    frontier: &[String],
    limit: usize,
    include_tags: bool,
) -> Result<Vec<String>> {
    if frontier.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let frontier_values = (0..frontier.len())
        .map(|ordinal| format!("(?, {ordinal})"))
        .collect::<Vec<_>>()
        .join(", ");
    let relation_types = scoped_relation_types(include_tags);
    let companion_kinds = scoped_companion_kinds(include_tags);
    let sql = format!(
        r#"
        WITH frontier(id, ordinal) AS (VALUES {frontier_values}),
        incident AS (
          SELECT
            frontier.ordinal AS frontier_ordinal,
            e.id AS edge_id,
            e.source_id,
            e.target_id,
            e.target_id AS neighbor_id,
            e.rel_type
          FROM frontier
          JOIN graph_edges e ON e.source_id = frontier.id
          UNION ALL
          SELECT
            frontier.ordinal AS frontier_ordinal,
            e.id AS edge_id,
            e.source_id,
            e.target_id,
            e.source_id AS neighbor_id,
            e.rel_type
          FROM frontier
          JOIN graph_edges e ON e.target_id = frontier.id
        )
        SELECT incident.neighbor_id
        FROM incident
        JOIN graph_nodes neighbor ON neighbor.id = incident.neighbor_id
        JOIN graph_nodes source_node ON source_node.id = incident.source_id
        JOIN graph_nodes target_node ON target_node.id = incident.target_id
        LEFT JOIN documents neighbor_document ON neighbor_document.id = neighbor.document_id
        LEFT JOIN documents source_document ON source_document.id = source_node.document_id
        LEFT JOIN documents target_document ON target_document.id = target_node.document_id
        WHERE incident.rel_type IN ({relation_types})
          AND (
            (
              neighbor.document_id IS NOT NULL
              AND neighbor_document.wing = ?
              AND neighbor.kind IN ('document', 'stub', 'entity')
            )
            OR (
              neighbor.document_id IS NULL
              AND neighbor.kind IN ({companion_kinds})
            )
          )
          AND (source_document.wing = ? OR target_document.wing = ?)
        GROUP BY incident.neighbor_id
        ORDER BY
          MIN(incident.frontier_ordinal),
          MIN(incident.edge_id),
          incident.neighbor_id
        LIMIT {limit}
        "#
    );
    let mut binds = frontier.to_vec();
    binds.extend([
        project.to_string(),
        project.to_string(),
        project.to_string(),
    ]);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(binds.iter()))?;
    let mut neighbors = Vec::with_capacity(limit);
    while let Some(row) = rows.next()? {
        neighbors.push(row.get(0)?);
    }
    Ok(neighbors)
}

fn discover_ui_neighbors_locked(
    conn: &duckdb::Connection,
    frontier: &[String],
    limit: usize,
    include_tags: bool,
) -> Result<Vec<String>> {
    if frontier.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let frontier_values = (0..frontier.len())
        .map(|ordinal| format!("(?, {ordinal})"))
        .collect::<Vec<_>>()
        .join(", ");
    let relation_types = scoped_relation_types(include_tags);
    let node_kinds = ui_node_kinds(include_tags);
    let sql = format!(
        r#"
        WITH frontier(id, ordinal) AS (VALUES {frontier_values}),
        incident AS (
          SELECT
            frontier.ordinal AS frontier_ordinal,
            e.id AS edge_id,
            e.target_id AS neighbor_id,
            e.rel_type
          FROM frontier
          JOIN graph_edges e ON e.source_id = frontier.id
          UNION ALL
          SELECT
            frontier.ordinal AS frontier_ordinal,
            e.id AS edge_id,
            e.source_id AS neighbor_id,
            e.rel_type
          FROM frontier
          JOIN graph_edges e ON e.target_id = frontier.id
        )
        SELECT incident.neighbor_id
        FROM incident
        JOIN graph_nodes neighbor ON neighbor.id = incident.neighbor_id
        WHERE incident.rel_type IN ({relation_types})
          AND neighbor.kind IN ({node_kinds})
        GROUP BY incident.neighbor_id
        ORDER BY
          MIN(incident.frontier_ordinal),
          MIN(incident.edge_id),
          incident.neighbor_id
        LIMIT {limit}
        "#
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(frontier.iter()))?;
    let mut neighbors = Vec::with_capacity(limit);
    while let Some(row) = rows.next()? {
        neighbors.push(row.get(0)?);
    }
    Ok(neighbors)
}

fn load_nodes_by_ids_locked(
    conn: &duckdb::Connection,
    node_ids: &[String],
) -> Result<Vec<GraphNode>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let selected = values_clause(node_ids.len());
    let limit = node_ids.len();
    let sql = format!(
        r#"
        WITH selected(id) AS (VALUES {selected})
        SELECT n.id, n.kind, n.label, n.document_id, n.uri, n.resolved, n.metadata_json
        FROM selected
        JOIN graph_nodes n ON n.id = selected.id
        ORDER BY n.id
        LIMIT {limit}
        "#
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(node_ids.iter()))?;
    let mut nodes = Vec::with_capacity(limit);
    while let Some(row) = rows.next()? {
        nodes.push(row_to_node(row)?);
    }
    Ok(nodes)
}

fn load_bounded_scoped_edges_locked(
    conn: &duckdb::Connection,
    project: &str,
    node_ids: &[String],
    include_tags: bool,
) -> Result<Vec<GraphEdge>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let selected = values_clause(node_ids.len());
    let relation_types = scoped_relation_types(include_tags);
    let relation_count = if include_tags { 14 } else { 13 };
    let max_edges = node_ids
        .len()
        .saturating_mul(node_ids.len())
        .saturating_mul(relation_count)
        .max(1);
    let sql = format!(
        r#"
        WITH selected(id) AS (VALUES {selected})
        SELECT e.id, e.source_id, e.target_id, e.rel_type, e.weight, e.context
        FROM graph_edges e
        JOIN selected source_selected ON source_selected.id = e.source_id
        JOIN selected target_selected ON target_selected.id = e.target_id
        JOIN graph_nodes source_node ON source_node.id = e.source_id
        JOIN graph_nodes target_node ON target_node.id = e.target_id
        LEFT JOIN documents source_document ON source_document.id = source_node.document_id
        LEFT JOIN documents target_document ON target_document.id = target_node.document_id
        WHERE e.rel_type IN ({relation_types})
          AND (source_document.wing = ? OR target_document.wing = ?)
        ORDER BY e.id
        LIMIT {max_edges}
        "#
    );
    let mut binds = node_ids.to_vec();
    binds.extend([project.to_string(), project.to_string()]);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(binds.iter()))?;
    let mut edges = Vec::with_capacity(max_edges.min(1024));
    while let Some(row) = rows.next()? {
        edges.push(row_to_edge(row)?);
    }
    Ok(edges)
}

fn load_bounded_ui_edges_locked(
    conn: &duckdb::Connection,
    node_ids: &[String],
    include_tags: bool,
) -> Result<Vec<GraphEdge>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let selected = values_clause(node_ids.len());
    let relation_types = scoped_relation_types(include_tags);
    let relation_count = if include_tags { 14 } else { 13 };
    let max_edges = node_ids
        .len()
        .saturating_mul(node_ids.len())
        .saturating_mul(relation_count)
        .max(1);
    let sql = format!(
        r#"
        WITH selected(id) AS (VALUES {selected})
        SELECT e.id, e.source_id, e.target_id, e.rel_type, e.weight, e.context
        FROM graph_edges e
        JOIN selected source_selected ON source_selected.id = e.source_id
        JOIN selected target_selected ON target_selected.id = e.target_id
        WHERE e.rel_type IN ({relation_types})
        ORDER BY e.id
        LIMIT {max_edges}
        "#
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(node_ids.iter()))?;
    let mut edges = Vec::with_capacity(max_edges.min(1024));
    while let Some(row) = rows.next()? {
        edges.push(row_to_edge(row)?);
    }
    Ok(edges)
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
    NodeKind::parse(&node.kind)?;
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
        let mut rows = stmt.query(params![document_id])?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next()? {
            ids.push(row.get::<_, String>(0)?);
        }
        ids
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
    let mut stmt = conn.prepare(
        r#"
        INSERT INTO graph_edges
          (id, source_id, target_id, rel_type, weight, context, created_at, origin)
        VALUES
          (?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), ?)
        "#,
    )?;
    for edge in edges {
        stmt.execute(params![
            edge.id,
            edge.source_id,
            edge.target_id,
            edge.rel_type,
            edge.weight,
            edge.context,
            now.as_str(),
            EdgeOrigin::EXPLICIT,
        ])?;
    }
    Ok(())
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
    let mut stmt = conn.prepare(
        r#"
        INSERT INTO graph_edges
          (id, source_id, target_id, rel_type, weight, context, created_at, origin,
           alias, heading, chunk_id, char_start, char_end, occurrence)
        VALUES
          (?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), ?, ?, ?, ?, ?, ?, ?)
        "#,
    )?;
    for row in edges {
        stmt.execute(params![
            row.edge.id,
            row.edge.source_id,
            row.edge.target_id,
            row.edge.rel_type,
            row.edge.weight,
            row.edge.context,
            now.as_str(),
            EdgeOrigin::EXTRACT,
            row.alias,
            row.heading,
            row.chunk_id,
            row.char_start,
            row.char_end,
            row.occurrence,
        ])?;
    }
    Ok(())
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
    let mut stmt = conn.prepare(
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
    let mut stmt = conn.prepare(sql)?;
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
    let mut stmt = conn.prepare(
        r#"
        SELECT id, kind, label, document_id, uri, resolved, metadata_json
        FROM graph_nodes
        WHERE label_key = ?
        ORDER BY resolved DESC, kind ASC, id ASC
        "#,
    )?;
    let mut rows = stmt.query(params![key])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row_to_node(row)?);
    }
    Ok(out)
}

fn format_ts_now() -> String {
    Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string()
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
    let mut stmt = conn.prepare(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    use chrono::Utc;

    /// The inlined SQL literals are a projection of §7.1, never a second copy.
    #[test]
    fn scoped_sql_name_lists_derive_from_the_models_sources() {
        assert_eq!(
            scoped_relation_types(false),
            sql_string_list(
                crate::models::pkb_rel_types(false)
                    .iter()
                    .map(String::as_str)
            )
        );
        assert_eq!(
            scoped_relation_types(true),
            sql_string_list(
                crate::models::pkb_rel_types(true)
                    .iter()
                    .map(String::as_str)
            )
        );
        assert!(crate::models::pkb_rel_types(true)
            .iter()
            .any(|rel| rel == "реализует"));
        assert!(!scoped_companion_kinds(true).contains("document"));
        assert_eq!(
            ui_node_kinds(false),
            sql_string_list(
                crate::models::pkb_node_kinds(false)
                    .iter()
                    .map(String::as_str)
            )
        );
    }

    /// §1.6: the rebuild delete predicate must inline the extraction-owned names
    /// from the models list, never a second copy of them.
    #[test]
    fn delete_predicate_inlines_the_models_extract_owned_names() {
        let sql = delete_derived_edges_sql();
        assert!(sql.contains(&sql_string_list(
            crate::models::EXTRACT_OWNED_REL_TYPES.iter().copied()
        )));
        assert!(sql.contains("origin = 'extract'"), "{sql}");
        assert!(sql.contains("origin IS NULL"), "{sql}");
    }

    #[test]
    fn sql_string_list_doubles_quotes() {
        assert_eq!(sql_string_list(["a", "b'c"]), "'a', 'b''c'");
    }

    #[test]
    fn hand_authored_wiki_relation_survives_the_boundary_and_the_pkb_walk() {
        let store = open_temp();
        for (id, label) in [("wiki-a", "A"), ("wiki-b", "B")] {
            store
                .upsert_graph_node(&node(id, "document", label, None))
                .expect("insert node");
        }

        let edge = store
            .link_nodes("wiki-a", "wiki-b", "реализует", 1.0)
            .expect("link_nodes with a wiki semantic rel_type");
        assert_eq!(edge.rel_type, "реализует");

        let pkb_rels = crate::models::pkb_rel_types(false);
        let view = store
            .neighbors_filtered("wiki-a", 1, 100, Some(&pkb_rels))
            .expect("PKB default walk");
        assert!(
            view.edges.iter().any(|e| e.rel_type == "реализует"),
            "the §7.1 default must keep hand-authored relations: {:?}",
            view.edges
        );

        let rejected = store.link_nodes("wiki-a", "wiki-b", "зависимость", 1.0);
        assert!(
            rejected
                .err()
                .is_some_and(|error| error.to_string().contains("invalid rel_type")),
            "an unknown rel_type must stay rejected"
        );
    }

    fn open_temp() -> Store {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("graph.duckdb");
        std::mem::forget(dir);
        Store::open(&path).expect("open")
    }

    fn node(id: &str, kind: &str, label: &str, doc_id: Option<&str>) -> GraphNode {
        GraphNode {
            id: id.into(),
            kind: kind.into(),
            label: label.into(),
            document_id: doc_id.map(str::to_string),
            uri: None,
            resolved: kind != "stub",
            metadata_json: "{}".into(),
        }
    }

    fn edge(id: &str, source_id: &str, target_id: &str, rel_type: &str) -> GraphEdge {
        GraphEdge {
            id: id.into(),
            source_id: source_id.into(),
            target_id: target_id.into(),
            rel_type: rel_type.into(),
            weight: 1.0,
            context: None,
        }
    }

    #[test]
    fn upsert_link_neighbors_backlinks() {
        let store = open_temp();
        let now = Utc::now();
        let doc = Document {
            id: "d1".into(),
            uri: "doc://a".into(),
            title: "A".into(),
            content: "x".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        store.upsert_document(&doc).unwrap();

        store
            .upsert_graph_node(&node("n1", "document", "A", Some("d1")))
            .unwrap();
        store
            .upsert_graph_node(&node("n2", "stub", "B", None))
            .unwrap();
        store
            .upsert_graph_node(&node("t1", "tag", "idea", None))
            .unwrap();

        store.link_nodes("n1", "n2", "wikilink", 1.0).unwrap();
        store.link_nodes("n1", "t1", "tagged", 1.0).unwrap();
        store.link_nodes("n1", "t1", "tunnel", 0.5).unwrap();

        let view = store.neighbors("n1", 1, 100).unwrap();
        assert_eq!(view.nodes.len(), 3);
        assert_eq!(view.edges.len(), 3);

        // Undirected: from stub n2 see n1
        let from_stub = store.neighbors("n2", 1, 100).unwrap();
        assert!(from_stub.nodes.iter().any(|n| n.id == "n1"));

        let bl = store.backlinks("n2").unwrap();
        assert_eq!(bl.edges.len(), 1);
        assert!(bl.nodes.iter().any(|n| n.id == "n1"));

        let by_doc = store
            .find_node_by_document_id("d1")
            .unwrap()
            .expect("doc node");
        assert_eq!(by_doc.id, "n1");

        let by_label = store.find_nodes_by_label("idea").unwrap();
        assert_eq!(by_label.len(), 1);

        let resolved = store
            .resolve_node(None, Some("d1"), None)
            .unwrap()
            .expect("resolve");
        assert_eq!(resolved.id, "n1");

        store.delete_edges_from("n1").unwrap();
        let bl_after = store.backlinks("n2").unwrap();
        assert!(bl_after.edges.is_empty());

        store.link_nodes("n1", "n2", "related", 1.0).unwrap();

        // §6.3: deleting the document demotes its node; the explicit edge survives
        // and n2 keeps a backlink to what used to be a note.
        assert!(store.delete_document("d1").unwrap());
        assert!(store.find_node_by_document_id("d1").unwrap().is_none());
        let demoted = store.find_node_by_id("n1").unwrap().expect("stub remains");
        assert_eq!(demoted.kind, "stub");
        assert!(!demoted.resolved);
        assert_eq!(demoted.document_id, None);
        assert_eq!(demoted.uri.as_deref(), None);
        assert!(store.find_node_by_id("n2").unwrap().is_some());
        let bl_after_delete = store.backlinks("n2").unwrap();
        assert_eq!(bl_after_delete.edges.len(), 1);
        assert_eq!(bl_after_delete.edges[0].rel_type, "related");
    }

    /// §5.2 allows two nodes to carry one uri, so the lookup must name one of them the
    /// same way on every read: a resolved node beats the stub that was created for the
    /// uri before the document arrived, the first writer beats a newer copy (§5.2
    /// rejects resolve-by-newest), and the id breaks an exact tie.
    #[test]
    fn find_node_by_uri_names_one_canonical_copy() {
        let store = open_temp();
        {
            let conn = store.lock().unwrap();
            conn.execute(
                "INSERT INTO graph_nodes (id, kind, label, document_id, uri, resolved, created_at, updated_at) VALUES
                 ('stub-from-a-wikilink', 'stub', 'dup.cs', NULL, 'file:///dup.cs', false, '2026-08-01 10:00:00', '2026-08-01 10:00:00'),
                 ('older-copy',    'document', 'dup.cs',      'doc-a', 'file:///dup.cs', true,  '2026-09-02 10:00:00', '2026-09-04 10:00:00'),
                 ('newer-copy',    'document', 'rag: dup.cs', 'doc-a', 'file:///dup.cs', true,  '2026-09-20 10:00:00', '2026-09-20 10:00:00'),
                 ('zz-tie',        'document', 'tie.cs',   'doc-b', 'file:///tie.cs', true,  '2026-09-01 10:00:00', '2026-09-20 10:00:00'),
                 ('aa-tie',        'document', 'tie.cs',   'doc-b', 'file:///tie.cs', true,  '2026-09-01 10:00:00', '2026-09-20 10:00:00')",
                [],
            )
            .unwrap();
        }

        for _ in 0..3 {
            let found = store
                .find_node_by_uri("file:///dup.cs")
                .unwrap()
                .expect("canonical copy");
            assert_eq!(found.id, "older-copy");
            assert_eq!(found.label, "dup.cs");
        }
        let tie = store
            .find_node_by_uri("file:///tie.cs")
            .unwrap()
            .expect("either copy is canonical");
        assert_eq!(tie.id, "aa-tie");
        assert!(store
            .find_node_by_uri("file:///absent.cs")
            .unwrap()
            .is_none());
    }

    /// §6.3: extraction noise is reclaimed on delete, user-authored inbound links
    /// from surviving notes are not.
    #[test]
    fn delete_document_keeps_inbound_and_drops_outbound_extract_edges() {
        let store = open_temp();
        let now = Utc::now();
        for (id, uri, title) in [("d1", "doc://a", "A"), ("d2", "doc://b", "B")] {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: uri.into(),
                    title: title.into(),
                    content: "x".into(),
                    metadata_json: "{}".into(),
                    created_at: now,
                    updated_at: now,
                    ..Default::default()
                })
                .unwrap();
        }
        store
            .upsert_graph_node(&node("n1", "document", "A", Some("d1")))
            .unwrap();
        store
            .upsert_graph_node(&node("n2", "document", "B", Some("d2")))
            .unwrap();
        // A -> B: one edge body extraction owns, one the user created.
        {
            let conn = store.lock().unwrap();
            let extracted = DerivedEdge {
                edge: edge("e-out-extract", "n1", "n2", "wikilink"),
                alias: None,
                heading: None,
                char_start: Some(0),
                char_end: Some(10),
                occurrence: 0,
                chunk_id: None,
            };
            insert_derived_graph_edges_locked(&conn, &[extracted]).unwrap();
        }
        let out_explicit = store.link_nodes("n1", "n2", "tunnel", 1.0).unwrap().id;
        // B -> A: inbound to the doomed node.
        let in_explicit = store.link_nodes("n2", "n1", "related", 1.0).unwrap().id;

        assert!(store.delete_document("d1").unwrap());

        let mut kept: Vec<String> = store
            .list_graph_edges()
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        kept.sort();
        let mut expected = vec![in_explicit, out_explicit];
        expected.sort();
        assert_eq!(kept, expected);
        let stub = store.find_node_by_id("n1").unwrap().expect("demoted");
        assert_eq!(stub.kind, "stub");
        assert!(!stub.resolved);
        assert_eq!(stub.document_id, None);
        // The demoted stub stays reachable from B, so the backlink keeps its meaning.
        let bl = store.backlinks("n1").unwrap();
        assert_eq!(bl.edges.len(), 1);
        assert_eq!(bl.edges[0].source_id, "n2");
    }

    fn ids<T, F: Fn(&T) -> &str>(rows: &[T], key: F) -> Vec<String> {
        let mut out: Vec<String> = rows.iter().map(|r| key(r).to_string()).collect();
        out.sort();
        out
    }

    /// §7.2: the walk is per-hop frontier SQL, so the budget has to be applied
    /// while discovering. Tags stay reachable here because `neighbors` is the raw
    /// local graph, not the UI's PKB projection.
    #[test]
    fn neighbors_bounds_multihop_walks_and_repeats_identically() {
        let store = open_temp();
        for (id, kind, label) in [
            ("c1", "document", "C1"),
            ("c2", "document", "C2"),
            ("c3", "tag", "inbox"),
        ] {
            store
                .upsert_graph_node(&node(id, kind, label, None))
                .unwrap();
        }
        store
            .insert_graph_edges(&[
                edge("e1", "c1", "c2", "wikilink"),
                edge("e2", "c2", "c3", "tagged"),
            ])
            .unwrap();

        let one_hop = store.neighbors("c1", 1, 100).unwrap();
        assert_eq!(ids(&one_hop.nodes, |n| n.id.as_str()), ["c1", "c2"]);
        assert_eq!(one_hop.edges.len(), 1);

        let two_hop = store.neighbors("c1", 2, 100).unwrap();
        assert_eq!(
            ids(&two_hop.nodes, |n| n.id.as_str()),
            ["c1", "c2", "c3"],
            "hop two reaches the tag hub"
        );
        assert_eq!(ids(&two_hop.edges, |e| e.id.as_str()), ["e1", "e2"]);

        let capped = store.neighbors("c1", 2, 2).unwrap();
        assert_eq!(capped.nodes.len(), 2, "max_nodes bounds the walk");
        assert_eq!(
            capped.edges.len(),
            1,
            "an edge to an unbudgeted node is not reported"
        );

        let again = store.neighbors("c1", 2, 100).unwrap();
        assert_eq!(
            ids(&again.nodes, |n| n.id.as_str()),
            ids(&two_hop.nodes, |n| n.id.as_str()),
            "the frontier order is stable across calls"
        );
    }

    /// §7.1: the walk can be narrowed to a named relation set, which is how MCP
    /// `get_neighbors` applies its PKB default. The raw contract stays untouched.
    #[test]
    fn neighbors_filtered_walks_only_the_named_relations() {
        let store = open_temp();
        for (id, kind, label) in [
            ("a", "document", "A"),
            ("b", "document", "B"),
            ("t", "tag", "inbox"),
        ] {
            store
                .upsert_graph_node(&node(id, kind, label, None))
                .unwrap();
        }
        store.link_nodes("a", "b", "wikilink", 1.0).unwrap();
        store.link_nodes("a", "t", "tagged", 1.0).unwrap();
        store.link_nodes("b", "t", "tunnel", 1.0).unwrap();

        let literary = vec!["wikilink".to_string(), "related".to_string()];
        let pkb = store
            .neighbors_filtered("a", 3, 100, Some(&literary))
            .unwrap();
        assert_eq!(ids(&pkb.nodes, |n| n.id.as_str()), vec!["a", "b"]);
        assert!(pkb
            .edges
            .iter()
            .all(|edge| edge.rel_type == "wikilink" || edge.rel_type == "related"));

        let with_tags = vec![
            "wikilink".to_string(),
            "related".to_string(),
            "tagged".to_string(),
        ];
        let tagged = store
            .neighbors_filtered("a", 3, 100, Some(&with_tags))
            .unwrap();
        assert_eq!(ids(&tagged.nodes, |n| n.id.as_str()), vec!["a", "b", "t"]);

        // Raw walk: the tunnel is followed, so the tag hub is reached through b.
        let raw = store.neighbors("a", 3, 100).unwrap();
        assert_eq!(ids(&raw.nodes, |n| n.id.as_str()), vec!["a", "b", "t"]);
        assert!(raw.edges.iter().any(|edge| edge.rel_type == "tunnel"));
    }

    #[test]
    fn neighbors_clamps_depth_to_the_local_graph_cap() {
        let store = open_temp();
        let chain: Vec<String> = (0..10).map(|i| format!("n{i}")).collect();
        for id in &chain {
            store
                .upsert_graph_node(&node(id, "document", id, None))
                .unwrap();
        }
        let links: Vec<GraphEdge> = chain
            .windows(2)
            .map(|pair| {
                let id = format!("e-{}-{}", pair[0], pair[1]);
                edge(&id, &pair[0], &pair[1], "wikilink")
            })
            .collect();
        store.insert_graph_edges(&links).unwrap();

        let view = store.neighbors("n0", u32::MAX, 100).unwrap();
        assert_eq!(
            view.nodes.len(),
            MAX_LOCAL_GRAPH_DEPTH as usize + 1,
            "depth beyond the §9 cap is not walked"
        );
    }

    /// An edge whose endpoint has no node row cannot carry the walk onward; the
    /// old adjacency-map BFS happily traversed through such ghosts.
    #[test]
    fn neighbors_does_not_route_through_missing_nodes() {
        let store = open_temp();
        for id in ["here", "beyond"] {
            store
                .upsert_graph_node(&node(id, "document", id, None))
                .unwrap();
        }
        store
            .insert_graph_edges(&[
                edge("e1", "here", "ghost", "wikilink"),
                edge("e2", "ghost", "beyond", "wikilink"),
            ])
            .unwrap();

        let view = store.neighbors("here", 3, 100).unwrap();
        assert_eq!(ids(&view.nodes, |n| n.id.as_str()), ["here"]);
        assert!(view.edges.is_empty());
    }

    #[test]
    fn prune_orphan_stubs_keeps_referenced_placeholders() {
        let store = open_temp();
        store
            .upsert_graph_node(&node("doc", "document", "Source", Some("d1")))
            .unwrap();
        store
            .upsert_graph_node(&node("used", "stub", "Expected", None))
            .unwrap();
        store
            .upsert_graph_node(&node("orphan", "stub", "Obsolete", None))
            .unwrap();
        store.link_nodes("doc", "used", "wikilink", 1.0).unwrap();

        assert_eq!(store.prune_orphan_stubs().unwrap(), 1);
        assert!(store.find_node_by_id("used").unwrap().is_some());
        assert!(store.find_node_by_id("orphan").unwrap().is_none());
    }

    #[test]
    fn prune_orphan_tags_keeps_referenced_tags() {
        let store = open_temp();
        store
            .upsert_graph_node(&node("doc", "document", "Source", Some("d1")))
            .unwrap();
        store
            .upsert_graph_node(&node("used-tag", "tag", "rust", None))
            .unwrap();
        store
            .upsert_graph_node(&node("orphan-tag", "tag", "000000", None))
            .unwrap();
        store.link_nodes("doc", "used-tag", "tagged", 1.0).unwrap();

        assert_eq!(store.prune_orphan_tags().unwrap(), 1);
        assert!(store.find_node_by_id("used-tag").unwrap().is_some());
        assert!(store.find_node_by_id("orphan-tag").unwrap().is_none());
    }

    #[test]
    fn get_graph_view_filters() {
        let store = open_temp();
        store
            .upsert_graph_node(&node("n1", "document", "A", Some("d1")))
            .unwrap();
        store
            .upsert_graph_node(&node("n2", "document", "B", Some("d2")))
            .unwrap();
        store
            .upsert_graph_node(&node("t1", "tag", "idea", None))
            .unwrap();
        store.link_nodes("n1", "n2", "wikilink", 1.0).unwrap();
        store.link_nodes("n1", "t1", "tagged", 1.0).unwrap();

        let all = store.get_graph_view(GraphFilter::default()).unwrap();
        assert_eq!(all.nodes.len(), 3);
        assert_eq!(all.edges.len(), 2);

        let docs = store
            .get_graph_view(GraphFilter {
                kinds: Some(vec!["document".into()]),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(docs.nodes.len(), 2);
        assert_eq!(docs.edges.len(), 1);

        let seeded = store
            .get_graph_view(GraphFilter {
                seed_ids: Some(vec!["n1".into(), "t1".into()]),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(seeded.nodes.len(), 2);
        assert_eq!(seeded.edges.len(), 1);
        assert_eq!(seeded.edges[0].rel_type, "tagged");

        let missing = store.link_nodes("n1", "nope", "related", 1.0);
        assert!(matches!(missing, Err(AppError::NotFound(_))));
    }

    #[test]
    fn export_graph_for_ui_pkb_defaults() {
        let store = open_temp();
        store
            .upsert_graph_node(&node("n1", "document", "A", Some("d1")))
            .unwrap();
        store
            .upsert_graph_node(&node("n2", "document", "B", Some("d2")))
            .unwrap();
        store
            .upsert_graph_node(&node("s1", "stub", "Missing", None))
            .unwrap();
        store
            .upsert_graph_node(&node("t1", "tag", "idea", None))
            .unwrap();
        store.link_nodes("n1", "n2", "wikilink", 1.0).unwrap();
        store.link_nodes("n1", "s1", "related", 1.0).unwrap();
        store.link_nodes("n1", "t1", "tagged", 1.0).unwrap();
        store.link_nodes("n1", "n2", "tunnel", 0.5).unwrap();

        let view = store.export_graph_for_ui(None, false).unwrap();
        let kinds: HashSet<_> = view.nodes.iter().map(|n| n.kind.as_str()).collect();
        assert!(kinds.contains("document"));
        assert!(kinds.contains("stub"));
        assert!(!kinds.contains("tag"));
        assert_eq!(view.nodes.len(), 3);
        for e in &view.edges {
            assert!(e.rel_type == "wikilink" || e.rel_type == "related");
        }
        assert_eq!(view.edges.len(), 2);

        let with_tags = store.export_graph_for_ui(Some(300), true).unwrap();
        assert!(with_tags.nodes.iter().any(|n| n.kind == "tag"));
        assert!(with_tags.edges.iter().any(|e| e.rel_type == "tagged"));

        let by_label = store.find_seed_node("A").unwrap().expect("label seed");
        assert_eq!(by_label.id, "n1");
        let by_doc = store.find_seed_node("d1").unwrap().expect("doc seed");
        assert_eq!(by_doc.id, "n1");
        let local = store.export_neighbors_for_ui("A", 1, 100).unwrap();
        assert!(local.nodes.iter().any(|n| n.id == "n1"));
        assert!(local.nodes.iter().any(|n| n.id == "n2"));
    }

    #[test]
    fn pkb_neighbors_filter_hidden_relations_before_budget_and_honor_tags() {
        let store = open_temp();
        for (id, kind, label) in [
            ("seed", "document", "Seed"),
            ("wanted", "document", "Wanted"),
            ("hidden", "document", "Hidden"),
            ("tag", "tag", "Tag"),
        ] {
            store
                .upsert_graph_node(&node(id, kind, label, None))
                .unwrap();
        }
        store
            .insert_graph_edges(&[
                edge("00-tunnel", "seed", "hidden", "tunnel"),
                edge("01-wikilink", "seed", "wanted", "wikilink"),
                edge("02-tagged", "seed", "tag", "tagged"),
                edge("03-dependency", "seed", "hidden", "depends_on"),
            ])
            .unwrap();

        let without_tags = store
            .export_pkb_neighbors_for_ui("Seed", 1, 2, false)
            .unwrap();
        let without_ids = without_tags
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(without_ids, HashSet::from(["seed", "wanted"]));
        assert_eq!(without_tags.edges.len(), 1);
        assert_eq!(without_tags.edges[0].rel_type, "wikilink");

        let with_tags = store
            .export_pkb_neighbors_for_ui("Seed", 1, 3, true)
            .unwrap();
        let with_ids = with_tags
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(with_ids, HashSet::from(["seed", "tag", "wanted"]));
        assert!(with_tags.edges.iter().any(|edge| edge.rel_type == "tagged"));
        assert!(!with_tags.nodes.iter().any(|node| node.id == "hidden"));
        assert!(with_tags
            .edges
            .iter()
            .all(|edge| matches!(edge.rel_type.as_str(), "wikilink" | "tagged")));
    }

    #[test]
    fn project_graph_scopes_before_cap_and_lazy_expansion_cannot_cross_projects() {
        let store = open_temp();
        for (id, wing, title) in [
            ("alpha-doc", "alpha", "Zed project note"),
            ("beta-doc", "beta", "Aardvark other note"),
        ] {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("file:///{wing}/{id}.md"),
                    title: title.into(),
                    content: title.into(),
                    wing: Some(wing.into()),
                    room: (wing == "alpha").then(|| "docs".into()),
                    layer: "raw".into(),
                    ..Default::default()
                })
                .unwrap();
        }
        // Additive migrations historically allowed NULL here. Project exports
        // must expose the same effective `raw` layer as the document codec.
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE documents SET layer = NULL WHERE id = ?",
                duckdb::params!["alpha-doc"],
            )
            .unwrap();
        let mut alpha_node = node(
            "alpha-node",
            "document",
            "Zed project note",
            Some("alpha-doc"),
        );
        alpha_node.metadata_json =
            r#"{"wing":"stale","room":"old","layer":"index","custom":true}"#.into();
        store.upsert_graph_node(&alpha_node).unwrap();
        let mut beta_node = node(
            "beta-node",
            "document",
            "Aardvark other note",
            Some("beta-doc"),
        );
        beta_node.metadata_json =
            r#"{"wing":"stale","room":"old","layer":"index","custom":"beta"}"#.into();
        store.upsert_graph_node(&beta_node).unwrap();
        store
            .upsert_graph_node(&node("alpha-stub", "stub", "Missing alpha", None))
            .unwrap();
        store
            .upsert_graph_node(&node("alpha-entity", "entity", "Entity alpha", None))
            .unwrap();
        store
            .upsert_graph_node(&node("alpha-tag", "tag", "Tag alpha", None))
            .unwrap();
        store
            .link_nodes("alpha-node", "alpha-stub", "wikilink", 1.0)
            .unwrap();
        store
            .link_nodes("alpha-node", "alpha-entity", "related", 1.0)
            .unwrap();
        store
            .link_nodes("alpha-node", "alpha-tag", "tagged", 1.0)
            .unwrap();
        store
            .link_nodes("alpha-node", "beta-node", "related", 1.0)
            .unwrap();

        let capped = store
            .export_project_graph_for_ui("alpha", Some(1), false)
            .unwrap();
        assert_eq!(capped.nodes.len(), 1);
        assert_eq!(capped.nodes[0].id, "alpha-node");

        let graph = store
            .export_project_graph_for_ui("alpha", Some(10), false)
            .unwrap();
        assert!(graph.nodes.iter().any(|node| node.id == "alpha-node"));
        assert!(graph.nodes.iter().any(|node| node.id == "alpha-stub"));
        assert!(graph.nodes.iter().any(|node| node.id == "alpha-entity"));
        assert!(!graph.nodes.iter().any(|node| node.id == "alpha-tag"));
        assert!(!graph.nodes.iter().any(|node| node.id == "beta-node"));
        assert_eq!(graph.edges.len(), 2);
        let alpha = graph
            .nodes
            .iter()
            .find(|node| node.id == "alpha-node")
            .expect("project document node");
        let alpha_meta: serde_json::Value =
            serde_json::from_str(&alpha.metadata_json).expect("placement metadata");
        assert_eq!(alpha_meta["wing"], "alpha");
        assert_eq!(alpha_meta["room"], "docs");
        assert_eq!(alpha_meta["layer"], "raw");
        assert_eq!(alpha_meta["custom"], true);

        let beta_graph = store
            .export_project_graph_for_ui("beta", Some(10), false)
            .unwrap();
        let beta = beta_graph
            .nodes
            .iter()
            .find(|node| node.id == "beta-node")
            .expect("other project document node");
        let beta_meta: serde_json::Value =
            serde_json::from_str(&beta.metadata_json).expect("placement metadata");
        assert_eq!(beta_meta["wing"], "beta");
        assert!(beta_meta.get("room").is_none());
        assert_eq!(beta_meta["layer"], "raw");
        assert_eq!(beta_meta["custom"], "beta");

        let graph_with_tags = store
            .export_project_graph_for_ui("alpha", Some(10), true)
            .unwrap();
        assert!(graph_with_tags
            .nodes
            .iter()
            .any(|node| node.id == "alpha-tag"));
        assert!(graph_with_tags
            .edges
            .iter()
            .any(|edge| edge.rel_type == "tagged"));
        assert!(!graph_with_tags
            .nodes
            .iter()
            .any(|node| node.id == "beta-node"));

        let lazy = store
            .export_project_neighbors_for_ui("alpha", "Zed project note", 1, 10, false)
            .unwrap();
        assert!(lazy.nodes.iter().any(|node| node.id == "alpha-stub"));
        assert!(lazy.nodes.iter().any(|node| node.id == "alpha-entity"));
        assert!(!lazy.nodes.iter().any(|node| node.id == "alpha-tag"));
        assert!(!lazy.nodes.iter().any(|node| node.id == "beta-node"));
        let lazy_alpha = lazy
            .nodes
            .iter()
            .find(|node| node.id == "alpha-node")
            .expect("lazy project document node");
        let lazy_meta: serde_json::Value =
            serde_json::from_str(&lazy_alpha.metadata_json).expect("placement metadata");
        assert_eq!(lazy_meta["wing"], "alpha");
        assert_eq!(lazy_meta["room"], "docs");
        assert!(store
            .export_project_neighbors_for_ui("alpha", "Aardvark other note", 1, 10, false)
            .unwrap()
            .nodes
            .is_empty());
    }

    #[test]
    fn project_neighbors_are_deterministic_bounded_and_depth_clamped() {
        let store = open_temp();
        for (id, wing) in [
            ("seed-doc", "alpha"),
            ("a-doc", "alpha"),
            ("b-doc", "alpha"),
            ("c-doc", "alpha"),
            ("d-doc", "alpha"),
            ("e-doc", "alpha"),
            ("beta-doc", "beta"),
        ] {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("file:///{wing}/{id}.md"),
                    title: id.into(),
                    content: id.into(),
                    wing: Some(wing.into()),
                    ..Default::default()
                })
                .unwrap();
        }
        for (id, label, document_id) in [
            ("seed", "Seed", "seed-doc"),
            ("a", "A", "a-doc"),
            ("b", "B", "b-doc"),
            ("c", "C", "c-doc"),
            ("d", "D", "d-doc"),
            ("e", "E", "e-doc"),
            ("beta", "Beta", "beta-doc"),
        ] {
            store
                .upsert_graph_node(&node(id, "document", label, Some(document_id)))
                .unwrap();
        }
        store
            .insert_graph_edges(&[
                edge("00-cross-project", "seed", "beta", "related"),
                edge("01-first", "seed", "b", "related"),
                edge("02-second", "seed", "a", "related"),
                edge("03-depth-two", "b", "c", "related"),
                edge("04-depth-three", "c", "d", "related"),
                edge("05-depth-four", "d", "e", "related"),
            ])
            .unwrap();

        let first = store
            .export_project_neighbors_for_ui("alpha", "Seed", 1, 2, false)
            .unwrap();
        let second = store
            .export_project_neighbors_for_ui("alpha", "Seed", 1, 2, false)
            .unwrap();
        let first_ids = first
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>();
        let second_ids = second
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(first_ids, vec!["b", "seed"]);
        assert_eq!(second_ids, first_ids);
        assert_eq!(
            first
                .edges
                .iter()
                .map(|edge| edge.id.as_str())
                .collect::<Vec<_>>(),
            vec!["01-first"]
        );

        let clamped = store
            .export_project_neighbors_for_ui("alpha", "seed-doc", u32::MAX, u32::MAX, false)
            .unwrap();
        let clamped_ids = clamped
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<HashSet<_>>();
        assert!(clamped.nodes.len() <= crate::models::UI_GRAPH_EXPORT_MAX_NODES as usize);
        assert!(clamped_ids.contains("a"));
        assert!(clamped_ids.contains("b"));
        assert!(clamped_ids.contains("c"));
        assert!(clamped_ids.contains("d"));
        assert!(!clamped_ids.contains("e"));
        assert!(!clamped_ids.contains("beta"));
    }

    #[test]
    fn direct_companion_seed_stays_inside_project_scope() {
        let store = open_temp();
        for (id, wing) in [("alpha-doc", "alpha"), ("beta-doc", "beta")] {
            store
                .upsert_document(&Document {
                    id: id.into(),
                    uri: format!("file:///{wing}/{id}.md"),
                    title: id.into(),
                    content: id.into(),
                    wing: Some(wing.into()),
                    ..Default::default()
                })
                .unwrap();
        }
        for (id, label, document_id) in [
            ("alpha-node", "Alpha", Some("alpha-doc")),
            ("beta-node", "Beta", Some("beta-doc")),
            ("shared-stub", "Shared", None),
            ("orphan-stub", "Orphan", None),
            ("alpha-tag", "Alpha tag", None),
        ] {
            let kind = if id == "alpha-tag" {
                "tag"
            } else if document_id.is_none() {
                "stub"
            } else {
                "document"
            };
            store
                .upsert_graph_node(&node(id, kind, label, document_id))
                .unwrap();
        }
        store
            .insert_graph_edges(&[
                edge("01-alpha-shared", "alpha-node", "shared-stub", "wikilink"),
                edge("02-beta-shared", "beta-node", "shared-stub", "wikilink"),
                edge("03-alpha-tag", "alpha-node", "alpha-tag", "tagged"),
            ])
            .unwrap();

        let shared = store
            .export_project_neighbors_for_ui("alpha", "shared-stub", 1, 10, false)
            .unwrap();
        assert!(shared.nodes.iter().any(|node| node.id == "alpha-node"));
        assert!(shared.nodes.iter().any(|node| node.id == "shared-stub"));
        assert!(!shared.nodes.iter().any(|node| node.id == "beta-node"));
        assert_eq!(shared.edges.len(), 1);

        assert!(store
            .export_project_neighbors_for_ui("alpha", "orphan-stub", 1, 10, false)
            .unwrap()
            .nodes
            .is_empty());
        assert!(store
            .export_project_neighbors_for_ui("alpha", "alpha-tag", 1, 10, false)
            .unwrap()
            .nodes
            .is_empty());

        let tag = store
            .export_project_neighbors_for_ui("alpha", "alpha-tag", 1, 10, true)
            .unwrap();
        assert!(tag.nodes.iter().any(|node| node.id == "alpha-node"));
        assert!(tag.nodes.iter().any(|node| node.id == "alpha-tag"));
        assert!(tag.edges.iter().any(|edge| edge.rel_type == "tagged"));
    }

    #[test]
    fn graph_stats_by_kind_and_rel_type_includes_tunnel() {
        let store = open_temp();
        store
            .upsert_graph_node(&node("n1", "document", "A", Some("d1")))
            .unwrap();
        store
            .upsert_graph_node(&node("n2", "document", "B", Some("d2")))
            .unwrap();
        store
            .upsert_graph_node(&node("t1", "tag", "idea", None))
            .unwrap();
        store
            .upsert_graph_node(&node("s1", "stub", "Missing", None))
            .unwrap();

        store.link_nodes("n1", "n2", "wikilink", 1.0).unwrap();
        store.link_nodes("n1", "t1", "tagged", 1.0).unwrap();
        let tunnel = store
            .create_tunnel("n1", "n2", 0.75, Some("bridge a-b"))
            .unwrap();
        assert_eq!(tunnel.rel_type, "tunnel");
        assert!((tunnel.weight - 0.75).abs() < 1e-9);

        let s = store.graph_stats().expect("graph_stats");
        assert_eq!(s.total_nodes, 4);
        assert_eq!(s.total_edges, 3);
        assert_eq!(s.nodes_by_kind.get("document"), Some(&2));
        assert_eq!(s.nodes_by_kind.get("tag"), Some(&1));
        assert_eq!(s.nodes_by_kind.get("stub"), Some(&1));
        assert_eq!(s.edges_by_rel_type.get("wikilink"), Some(&1));
        assert_eq!(s.edges_by_rel_type.get("tagged"), Some(&1));
        assert_eq!(s.edges_by_rel_type.get("tunnel"), Some(&1));

        // The cut separates the tunnel's context from the context-less links, so
        // a hand-authored edge is distinguishable from a membership one.
        assert_eq!(
            s.edges_by_cut,
            vec![
                EdgeCutRow {
                    rel_type: "tagged".into(),
                    origin: "explicit".into(),
                    context: String::new(),
                    count: 1,
                },
                EdgeCutRow {
                    rel_type: "tunnel".into(),
                    origin: "explicit".into(),
                    context: "bridge a-b".into(),
                    count: 1,
                },
                EdgeCutRow {
                    rel_type: "wikilink".into(),
                    origin: "explicit".into(),
                    context: String::new(),
                    count: 1,
                },
            ]
        );
        // `graph_stats` is a JSON tool result: the cut must serialize as an array.
        let rendered = serde_json::to_value(&s).expect("graph_stats serializes");
        assert_eq!(rendered["edges_by_cut"][1]["context"], "bridge a-b");

        // Empty graph
        let empty = open_temp();
        let es = empty.graph_stats().expect("empty stats");
        assert_eq!(es.total_nodes, 0);
        assert_eq!(es.total_edges, 0);
        assert!(es.nodes_by_kind.is_empty());
        assert!(es.edges_by_rel_type.is_empty());
        assert!(es.edges_by_cut.is_empty());
    }

    #[test]
    fn tunnel_create_list_delete_follow_find() {
        let store = open_temp();
        store
            .upsert_graph_node(&node("n1", "document", "A", Some("d1")))
            .unwrap();
        store
            .upsert_graph_node(&node("n2", "document", "B", Some("d2")))
            .unwrap();
        store
            .upsert_graph_node(&node("n3", "document", "C", Some("d3")))
            .unwrap();

        // Non-tunnel noise must not appear in follow/list
        store.link_nodes("n1", "n2", "wikilink", 1.0).unwrap();

        let t12 = store
            .create_tunnel("n1", "n2", 1.0, Some("bridge a-b"))
            .unwrap();
        assert_eq!(t12.rel_type, REL_TUNNEL);
        assert_eq!(t12.context.as_deref(), Some("bridge a-b"));

        // Idempotent undirected: reverse order updates same edge
        let again = store
            .create_tunnel("n2", "n1", 0.5, Some("updated"))
            .unwrap();
        assert_eq!(again.id, t12.id);
        assert!((again.weight - 0.5).abs() < 1e-9);
        assert_eq!(again.context.as_deref(), Some("updated"));

        let t23 = store.create_tunnel("n2", "n3", 1.0, None).unwrap();
        assert_ne!(t23.id, t12.id);

        let all = store.list_tunnels(None).unwrap();
        assert_eq!(all.len(), 2);

        let at_n2 = store.list_tunnels(Some("n2")).unwrap();
        assert_eq!(at_n2.len(), 2);

        let at_n1 = store.list_tunnels(Some("n1")).unwrap();
        assert_eq!(at_n1.len(), 1);

        // Multi-hop follow: n1 → n2 → n3 along tunnels only
        let hop1 = store.follow_tunnels("n1", 1, 100).unwrap();
        assert_eq!(hop1.nodes.len(), 2); // n1, n2
        assert!(hop1.edges.iter().all(|e| e.rel_type == REL_TUNNEL));
        assert!(!hop1.nodes.iter().any(|n| n.id == "n3"));

        let hop2 = store.follow_tunnels("n1", 2, 100).unwrap();
        assert_eq!(hop2.nodes.len(), 3);
        assert!(hop2.nodes.iter().any(|n| n.id == "n3"));
        // Only tunnel edges (not wikilink)
        assert_eq!(hop2.edges.len(), 2);
        assert!(hop2.edges.iter().all(|e| e.rel_type == REL_TUNNEL));

        let pair = store
            .find_tunnels(Some("n1"), Some("n2"), None, None)
            .unwrap();
        assert_eq!(pair.len(), 1);
        assert_eq!(pair[0].id, t12.id);

        assert!(store.delete_tunnel(&t12.id).unwrap());
        assert!(!store.delete_tunnel(&t12.id).unwrap());
        assert_eq!(store.list_tunnels(None).unwrap().len(), 1);

        // Self-loop refused
        let loop_err = store.create_tunnel("n1", "n1", 1.0, None);
        assert!(matches!(loop_err, Err(AppError::Config(_))));
    }
}
