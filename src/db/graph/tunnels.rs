//! Explicit cross-scope tunnel edges (create, list, follow, delete).

use super::*;

impl Store {
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
            crate::db::rows::for_each_row(&mut stmt, params![REL_TUNNEL, nid, nid], |row| {
                out.push(row_to_edge(row)?);
                Ok(())
            })?;
        } else {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, source_id, target_id, rel_type, weight, context
                FROM graph_edges
                WHERE rel_type = ?
                ORDER BY id ASC
                "#,
            )?;
            crate::db::rows::for_each_row(&mut stmt, params![REL_TUNNEL], |row| {
                out.push(row_to_edge(row)?);
                Ok(())
            })?;
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

            crate::db::rows::query_vec(&mut stmt, params![REL_TUNNEL, a, b, b, a], row_to_edge)?
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
    pub(super) fn tunnel_touches_wing(&self, edge: &GraphEdge, wing: &str) -> Result<bool> {
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
    pub(super) fn find_tunnel_between(&self, a: &str, b: &str) -> Result<Option<GraphEdge>> {
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
