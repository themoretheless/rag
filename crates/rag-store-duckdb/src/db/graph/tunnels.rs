//! Explicit cross-scope tunnel edges (create, list, follow, delete) over `lin`.

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
            let mut w = self.write()?;
            let ctx = context.map(str::trim).filter(|s| !s.is_empty());
            let pred = lin::query::pred::and(
                lin::query::pred::eq("id", existing.id.as_str()),
                lin::query::pred::eq("rel_type", REL_TUNNEL),
            );
            w.update_where(
                "graph_edges",
                pred,
                RecBuilder::new()
                    .f64("weight", weight)
                    .opt_str("context", ctx)
                    .build(),
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
        let r = self.read()?;
        let node_id = node_id.map(str::trim).filter(|s| !s.is_empty());
        let tunnel_rows = r.find_by_eq("graph_edges", "rel_type", REL_TUNNEL)?;
        let mut out = Vec::new();
        for row in &tunnel_rows {
            if let Some(nid) = node_id {
                let matches_endpoint = cell_str(row, "source_id") == Some(nid)
                    || cell_str(row, "target_id") == Some(nid);
                if !matches_endpoint {
                    continue;
                }
            }
            out.push(rows::graph_edge(row)?);
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
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
        let mut w = self.write()?;
        let pred = lin::query::pred::and(
            lin::query::pred::eq("id", tunnel_id),
            lin::query::pred::eq("rel_type", REL_TUNNEL),
        );
        let n = w.delete_where("graph_edges", pred)?;
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

        let r = self.read()?;
        if load_node_locked(&r, node_id)?.is_none() {
            return Ok(GraphView::default());
        }

        let visited = expand_frontier(node_id, depth, max_nodes as usize, |frontier, limit| {
            discover_frontier_neighbors_locked(&r, frontier, limit, Some(&[REL_TUNNEL]))
        })?;

        let nodes = load_nodes_by_ids_locked(&r, &visited)?;
        let edges = load_edges_among_locked(&r, &visited, Some(&[REL_TUNNEL]))?;
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
            let r = self.read()?;
            let mut pair_edges = Vec::new();
            for row in r.find_by_eq("graph_edges", "rel_type", REL_TUNNEL)? {
                let s = cell_str(&row, "source_id");
                let t = cell_str(&row, "target_id");
                if (s == Some(a) && t == Some(b)) || (s == Some(b) && t == Some(a)) {
                    pair_edges.push(rows::graph_edge(&row)?);
                }
            }
            pair_edges.sort_by(|x, y| x.id.cmp(&y.id));
            pair_edges
        } else {
            self.list_tunnels(node_id)?
        };

        if let Some(w) = wing {
            let endpoints: Vec<&str> = edges
                .iter()
                .flat_map(|e| [e.source_id.as_str(), e.target_id.as_str()])
                .collect();
            let in_wing = self.nodes_in_wing(&endpoints, w)?;
            edges.retain(|e| {
                in_wing.contains(e.source_id.as_str()) || in_wing.contains(e.target_id.as_str())
            });
        }

        if let Some(lim) = limit {
            if lim > 0 && edges.len() > lim as usize {
                edges.truncate(lim as usize);
            }
        }
        Ok(edges)
    }

    /// Which of `node_ids` are linked to a document in `wing`, in one query.
    pub(super) fn nodes_in_wing(&self, node_ids: &[&str], wing: &str) -> Result<HashSet<String>> {
        if node_ids.is_empty() {
            return Ok(HashSet::new());
        }
        let r = self.read()?;
        let mut out = HashSet::new();
        for &nid in node_ids {
            let Some(node_row) = r.get_by_id("graph_nodes", nid)? else {
                continue;
            };
            let Some(doc_id) = cell_nonempty_string(&node_row, "document_id") else {
                continue;
            };
            if let Some(doc_row) = r.get_by_id("documents", &doc_id)? {
                if cell_str(&doc_row, "wing") == Some(wing) {
                    out.insert(nid.to_string());
                }
            }
        }
        Ok(out)
    }

    /// Existing tunnel edge between two nodes (either direction), if any.
    pub(super) fn find_tunnel_between(&self, a: &str, b: &str) -> Result<Option<GraphEdge>> {
        let r = self.read()?;
        let mut matches = Vec::new();
        for row in r.find_by_eq("graph_edges", "rel_type", REL_TUNNEL)? {
            let s = cell_str(&row, "source_id");
            let t = cell_str(&row, "target_id");
            if (s == Some(a) && t == Some(b)) || (s == Some(b) && t == Some(a)) {
                matches.push(rows::graph_edge(&row)?);
            }
        }
        matches.sort_by(|x, y| x.id.cmp(&y.id));
        Ok(matches.into_iter().next())
    }
}
