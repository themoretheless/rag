//! Graph node/edge CRUD, filtered export, undirected BFS neighbors, and backlinks.

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::Utc;
use duckdb::params;
use uuid::Uuid;

use super::store::Store;
use crate::error::{AppError, Result};
use crate::graph::REL_TUNNEL;
use crate::models::{GraphEdge, GraphFilter, GraphNode, GraphStats, GraphView};

impl Store {
    /// Insert or replace a graph node by primary key `id`.
    ///
    /// Preserves original `created_at` when the id already exists.
    pub fn upsert_graph_node(&self, node: &GraphNode) -> Result<()> {
        let conn = self.lock()?;
        let now = format_ts_now();
        let metadata = if node.metadata_json.is_empty() {
            "{}"
        } else {
            node.metadata_json.as_str()
        };
        conn.execute(
            r#"
            INSERT OR REPLACE INTO graph_nodes
              (id, kind, label, document_id, uri, resolved, metadata_json, created_at, updated_at)
            VALUES
              (?, ?, ?, ?, ?, ?, ?,
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

    /// Insert edge rows (empty slice is a no-op).
    pub fn insert_graph_edges(&self, edges: &[GraphEdge]) -> Result<()> {
        if edges.is_empty() {
            return Ok(());
        }
        let conn = self.lock()?;
        let now = format_ts_now();
        let mut stmt = conn.prepare(
            r#"
            INSERT INTO graph_edges
              (id, source_id, target_id, rel_type, weight, context, created_at)
            VALUES
              (?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))
            "#,
        )?;
        for e in edges {
            stmt.execute(params![
                e.id,
                e.source_id,
                e.target_id,
                e.rel_type,
                e.weight,
                e.context,
                now.as_str(),
            ])?;
        }
        Ok(())
    }

    /// Delete all edges with the given source node (re-ingest rebuild).
    pub fn delete_edges_from(&self, source_id: &str) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "DELETE FROM graph_edges WHERE source_id = ?",
            params![source_id],
        )?;
        Ok(())
    }

    /// Delete edges incident to `node_id` (as source or target).
    pub fn delete_edges_incident(&self, node_id: &str) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "DELETE FROM graph_edges WHERE source_id = ? OR target_id = ?",
            params![node_id, node_id],
        )?;
        Ok(())
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
        let mut stmt = conn.prepare(
            r#"
            SELECT id, kind, label, document_id, uri, resolved, metadata_json
            FROM graph_nodes
            WHERE document_id = ?
            LIMIT 1
            "#,
        )?;
        let mut rows = stmt.query(params![doc_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_node(row)?)),
            None => Ok(None),
        }
    }

    /// Find nodes whose label equals `label` (case-sensitive).
    pub fn find_nodes_by_label(&self, label: &str) -> Result<Vec<GraphNode>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, kind, label, document_id, uri, resolved, metadata_json
            FROM graph_nodes
            WHERE label = ?
            ORDER BY resolved DESC, kind ASC, id ASC
            "#,
        )?;
        let mut rows = stmt.query(params![label])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_node(row)?);
        }
        Ok(out)
    }

    /// Find a node by primary key.
    pub fn find_node_by_id(&self, node_id: &str) -> Result<Option<GraphNode>> {
        let conn = self.lock()?;
        load_node_locked(&conn, node_id)
    }

    /// Find a node by exact `uri` match.
    pub fn find_node_by_uri(&self, uri: &str) -> Result<Option<GraphNode>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, kind, label, document_id, uri, resolved, metadata_json
            FROM graph_nodes
            WHERE uri = ?
            LIMIT 1
            "#,
        )?;
        let mut rows = stmt.query(params![uri])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_node(row)?)),
            None => Ok(None),
        }
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
    /// - `max_nodes` defaults to [`crate::models::UI_GRAPH_EXPORT_MAX_NODES`] (300)
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
    pub fn neighbors(&self, node_id: &str, depth: u32, max_nodes: u32) -> Result<GraphView> {
        let max_nodes = if max_nodes == 0 { 100 } else { max_nodes };

        if self.find_node_by_id(node_id)?.is_none() {
            return Ok(GraphView::default());
        }

        let conn = self.lock()?;
        let mut adj: HashMap<String, Vec<(String, GraphEdge)>> = HashMap::new();
        let mut all_edges: Vec<GraphEdge> = Vec::new();
        {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, source_id, target_id, rel_type, weight, context
                FROM graph_edges
                "#,
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let e = row_to_edge(row)?;
                adj.entry(e.source_id.clone())
                    .or_default()
                    .push((e.target_id.clone(), e.clone()));
                adj.entry(e.target_id.clone())
                    .or_default()
                    .push((e.source_id.clone(), e.clone()));
                all_edges.push(e);
            }
        }

        let mut visited: HashSet<String> = HashSet::new();
        let mut q: VecDeque<(String, u32)> = VecDeque::new();
        visited.insert(node_id.to_string());
        q.push_back((node_id.to_string(), 0));

        while let Some((cur, d)) = q.pop_front() {
            if d >= depth {
                continue;
            }
            if let Some(neis) = adj.get(&cur) {
                for (next, _) in neis {
                    if visited.len() as u32 >= max_nodes {
                        break;
                    }
                    if visited.insert(next.clone()) {
                        q.push_back((next.clone(), d + 1));
                    }
                }
            }
            if visited.len() as u32 >= max_nodes {
                break;
            }
        }

        let mut nodes = Vec::new();
        for id in &visited {
            if let Some(n) = load_node_locked(&conn, id)? {
                nodes.push(n);
            }
        }
        nodes.sort_by(|a, b| a.id.cmp(&b.id));

        let mut edges: Vec<GraphEdge> = all_edges
            .into_iter()
            .filter(|e| visited.contains(&e.source_id) && visited.contains(&e.target_id))
            .collect();
        edges.sort_by(|a, b| a.id.cmp(&b.id));

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
    /// either endpoint is missing. `tunnel` and other rel types are allowed
    /// (no allowlist; stored verbatim).
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
        self.insert_graph_edges(&[edge.clone()])?;
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

        Ok(GraphStats {
            total_nodes: total_nodes as u64,
            total_edges: total_edges as u64,
            nodes_by_kind,
            edges_by_rel_type,
        })
    }

    /// Remove graph data for a document: incident edges and the document node.
    ///
    /// Tags and stubs are left in place (may become orphans until next lint).
    pub fn delete_graph_for_document(&self, document_id: &str) -> Result<()> {
        if let Some(node) = self.find_node_by_document_id(document_id)? {
            self.delete_edges_incident(&node.id)?;
            self.delete_graph_node(&node.id)?;
        }
        Ok(())
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
        self.insert_graph_edges(&[edge.clone()])?;
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
            return Err(AppError::config("delete_tunnel requires non-empty tunnel_id"));
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
    pub fn follow_tunnels(
        &self,
        node_id: &str,
        depth: u32,
        max_nodes: u32,
    ) -> Result<GraphView> {
        let max_nodes = if max_nodes == 0 { 100 } else { max_nodes };
        let node_id = node_id.trim();
        if node_id.is_empty() {
            return Err(AppError::config("follow_tunnels requires non-empty node_id"));
        }
        if self.find_node_by_id(node_id)?.is_none() {
            return Ok(GraphView::default());
        }

        let conn = self.lock()?;
        let mut adj: HashMap<String, Vec<(String, GraphEdge)>> = HashMap::new();
        let mut all_tunnels: Vec<GraphEdge> = Vec::new();
        {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, source_id, target_id, rel_type, weight, context
                FROM graph_edges
                WHERE rel_type = ?
                "#,
            )?;
            let mut rows = stmt.query(params![REL_TUNNEL])?;
            while let Some(row) = rows.next()? {
                let e = row_to_edge(row)?;
                adj.entry(e.source_id.clone())
                    .or_default()
                    .push((e.target_id.clone(), e.clone()));
                adj.entry(e.target_id.clone())
                    .or_default()
                    .push((e.source_id.clone(), e.clone()));
                all_tunnels.push(e);
            }
        }

        let mut visited: HashSet<String> = HashSet::new();
        let mut q: VecDeque<(String, u32)> = VecDeque::new();
        visited.insert(node_id.to_string());
        q.push_back((node_id.to_string(), 0));

        while let Some((cur, d)) = q.pop_front() {
            if d >= depth {
                continue;
            }
            if let Some(neis) = adj.get(&cur) {
                for (next, _) in neis {
                    if visited.len() as u32 >= max_nodes {
                        break;
                    }
                    if visited.insert(next.clone()) {
                        q.push_back((next.clone(), d + 1));
                    }
                }
            }
            if visited.len() as u32 >= max_nodes {
                break;
            }
        }

        let mut nodes = Vec::new();
        for id in &visited {
            if let Some(n) = load_node_locked(&conn, id)? {
                nodes.push(n);
            }
        }
        nodes.sort_by(|a, b| a.id.cmp(&b.id));

        let mut edges: Vec<GraphEdge> = all_tunnels
            .into_iter()
            .filter(|e| visited.contains(&e.source_id) && visited.contains(&e.target_id))
            .collect();
        edges.sort_by(|a, b| a.id.cmp(&b.id));

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

fn format_ts_now() -> String {
    Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.6f")
        .to_string()
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
        store.delete_graph_for_document("d1").unwrap();
        assert!(store.find_node_by_document_id("d1").unwrap().is_none());
        // Stub remains
        assert!(store.find_node_by_id("n2").unwrap().is_some());
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
        // tagged edges still excluded by PKB rel_types
        assert!(!with_tags.edges.iter().any(|e| e.rel_type == "tagged"));

        let by_label = store.find_seed_node("A").unwrap().expect("label seed");
        assert_eq!(by_label.id, "n1");
        let by_doc = store.find_seed_node("d1").unwrap().expect("doc seed");
        assert_eq!(by_doc.id, "n1");
        let local = store.export_neighbors_for_ui("A", 1, 100).unwrap();
        assert!(local.nodes.iter().any(|n| n.id == "n1"));
        assert!(local.nodes.iter().any(|n| n.id == "n2"));
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
        let tunnel = store.link_nodes("n1", "n2", "tunnel", 0.75).unwrap();
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

        // Empty graph
        let empty = open_temp();
        let es = empty.graph_stats().expect("empty stats");
        assert_eq!(es.total_nodes, 0);
        assert_eq!(es.total_edges, 0);
        assert!(es.nodes_by_kind.is_empty());
        assert!(es.edges_by_rel_type.is_empty());
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
