//! Bounded graph exports for the UI: whole-graph, project-scoped and
//! neighborhood views with their frontier and edge queries over `lin`.

use std::collections::BTreeSet;

use super::*;

impl Store {
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
        let r = self.read()?;
        let mut nodes = load_bounded_project_nodes_locked(&r, project, max_nodes)?;
        if nodes.len() < max_nodes {
            let project_node_ids = nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>();
            nodes.extend(load_bounded_project_companions_locked(
                &r,
                &project_node_ids,
                max_nodes - nodes.len(),
                include_tags,
            )?);
        }
        enrich_document_placements_locked(&r, &mut nodes)?;
        let node_ids = nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>();
        let edges = load_bounded_edges_locked(&r, Some(project), &node_ids, include_tags)?;
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
        let r = self.read()?;
        let Some(seed_node) = resolve_project_seed_locked(&r, project, seed, include_tags)? else {
            return Ok(GraphView::default());
        };

        let visited = expand_frontier(&seed_node.id, depth, max_nodes, |frontier, limit| {
            discover_neighbors_locked(&r, Some(project), frontier, limit, include_tags)
        })?;

        let mut nodes = load_nodes_by_ids_locked(&r, &visited)?;
        enrich_document_placements_locked(&r, &mut nodes)?;
        let edges = load_bounded_edges_locked(&r, Some(project), &visited, include_tags)?;
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
        let r = self.read()?;
        let Some(seed_node) = resolve_ui_seed_locked(&r, seed, include_tags)? else {
            return Ok(GraphView::default());
        };

        let visited = expand_frontier(&seed_node.id, depth, max_nodes, |frontier, limit| {
            discover_neighbors_locked(&r, None, frontier, limit, include_tags)
        })?;

        let mut nodes = load_nodes_by_ids_locked(&r, &visited)?;
        enrich_document_placements_locked(&r, &mut nodes)?;
        let edges = load_bounded_edges_locked(&r, None, &visited, include_tags)?;
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
        Ok(prefer_document_node(self.find_nodes_by_label(q)?))
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
}

pub(super) fn require_project(project: &str) -> Result<&str> {
    let project = project.trim();
    if project.is_empty() {
        return Err(AppError::config("project must be non-empty"));
    }
    Ok(project)
}

fn scoped_relation_set(include_tags: bool) -> HashSet<String> {
    crate::models::pkb_rel_types(include_tags)
        .into_iter()
        .collect()
}

fn scoped_companion_kind_set(include_tags: bool) -> HashSet<String> {
    crate::models::pkb_node_kinds(include_tags)
        .into_iter()
        .filter(|kind| !NodeKind::Document.is(kind))
        .collect()
}

fn ui_node_kind_set(include_tags: bool) -> HashSet<String> {
    crate::models::pkb_node_kinds(include_tags)
        .into_iter()
        .collect()
}

fn is_project_doc_node_kind(kind: &str) -> bool {
    matches!(kind, "document" | "stub" | "entity")
}

fn seed_rank(node: &GraphNode) -> u8 {
    if NodeKind::Document.is(&node.kind) && node.resolved {
        0
    } else if NodeKind::Document.is(&node.kind) {
        1
    } else {
        2
    }
}

fn doc_wing_cached<'a>(
    r: &impl LinReader,
    doc_id: &str,
    cache: &'a mut HashMap<String, Option<String>>,
) -> Result<Option<&'a str>> {
    if !cache.contains_key(doc_id) {
        let wing = r
            .get_projected_by_id("documents", doc_id, &["wing"])?
            .and_then(|row| cell_nonempty_string(&row, "wing"));
        cache.insert(doc_id.to_string(), wing);
    }
    Ok(cache.get(doc_id).and_then(|w| w.as_deref()))
}

/// Make project-scoped graph exports self-describing for UI-side room/layer filters.
///
/// Graph extraction intentionally keeps node metadata independent from document
/// metadata, so placement is joined from the authoritative document row only at
/// the project export boundary. Document placement wins over stale graph metadata;
/// unrelated graph metadata is preserved.
pub(super) fn enrich_document_placements_locked(
    r: &impl LinReader,
    nodes: &mut [GraphNode],
) -> Result<()> {
    let mut document_ids: Vec<String> = nodes
        .iter()
        .filter_map(|node| node.document_id.clone())
        .collect();
    document_ids.sort_unstable();
    document_ids.dedup();
    if document_ids.is_empty() {
        return Ok(());
    }

    let mut placements = HashMap::with_capacity(document_ids.len());
    for doc_id in &document_ids {
        if let Some(row) = r.get_projected_by_id("documents", doc_id, &["wing", "room", "layer"])? {
            let wing = cell_opt_string(&row, "wing");
            let room = cell_opt_string(&row, "room");
            let layer = cell_opt_string(&row, "layer")
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "raw".into());
            placements.insert(doc_id.clone(), (wing, room, layer));
        }
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

pub(super) fn set_optional_metadata(
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

pub(super) fn load_bounded_project_nodes_locked(
    r: &impl LinReader,
    project: &str,
    limit: usize,
) -> Result<Vec<GraphNode>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let doc_rows = r.find_by_eq("documents", "wing", project)?;
    let mut nodes = Vec::new();
    for doc_row in &doc_rows {
        let Some(doc_id) = cell_str(doc_row, "id") else {
            continue;
        };
        for node_row in r.find_by_eq("graph_nodes", "document_id", doc_id)? {
            let kind = cell_str(&node_row, "kind").unwrap_or_default();
            if is_project_doc_node_kind(kind) {
                nodes.push(rows::graph_node(&node_row)?);
            }
        }
    }
    nodes.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.id.cmp(&b.id)));
    if nodes.len() > limit {
        nodes.truncate(limit);
    }
    Ok(nodes)
}

pub(super) fn load_bounded_project_companions_locked(
    r: &impl LinReader,
    project_node_ids: &[String],
    limit: usize,
    include_tags: bool,
) -> Result<Vec<GraphNode>> {
    if limit == 0 || project_node_ids.is_empty() {
        return Ok(Vec::new());
    }

    let relation_types = scoped_relation_set(include_tags);
    let companion_kinds = scoped_companion_kind_set(include_tags);
    let mut candidate_ids = HashSet::new();

    for pid in project_node_ids {
        for edge_row in r.find_by_eq("graph_edges", "source_id", pid.as_str())? {
            if let Some(rel) = cell_str(&edge_row, "rel_type") {
                if relation_types.contains(rel) {
                    if let Some(tid) = cell_str(&edge_row, "target_id") {
                        candidate_ids.insert(tid.to_string());
                    }
                }
            }
        }
        for edge_row in r.find_by_eq("graph_edges", "target_id", pid.as_str())? {
            if let Some(rel) = cell_str(&edge_row, "rel_type") {
                if relation_types.contains(rel) {
                    if let Some(sid) = cell_str(&edge_row, "source_id") {
                        candidate_ids.insert(sid.to_string());
                    }
                }
            }
        }
    }

    let mut nodes = Vec::new();
    for cid in candidate_ids {
        if let Some(node_row) = r.get_by_id("graph_nodes", &cid)? {
            let has_doc = cell_nonempty_string(&node_row, "document_id").is_some();
            let kind = cell_str(&node_row, "kind").unwrap_or_default();
            if !has_doc && companion_kinds.contains(kind) {
                nodes.push(rows::graph_node(&node_row)?);
            }
        }
    }
    nodes.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.id.cmp(&b.id)));
    if nodes.len() > limit {
        nodes.truncate(limit);
    }
    Ok(nodes)
}

pub(super) fn resolve_project_seed_locked(
    r: &impl LinReader,
    project: &str,
    seed: &str,
    include_tags: bool,
) -> Result<Option<GraphNode>> {
    for column in ["id", "document_id", "label"] {
        if let Some(node) = find_scoped_node_locked(r, project, column, seed, include_tags)? {
            return Ok(Some(node));
        }
    }
    Ok(None)
}

fn candidate_nodes_for_column(
    r: &impl LinReader,
    column: &str,
    value: &str,
) -> Result<Vec<GraphNode>> {
    let raw_rows = match column {
        "id" => r.get_by_id("graph_nodes", value)?.into_iter().collect(),
        "document_id" => r.find_by_eq("graph_nodes", "document_id", value)?,
        "label" => {
            let key = crate::graph::normalize::label_key(value);
            let mut by_key = r.find_by_eq("graph_nodes", "label_key", key.as_str())?;
            by_key.retain(|row| cell_str(row, "label") == Some(value));
            by_key
        }
        _ => Vec::new(),
    };
    raw_rows.iter().map(rows::graph_node).collect()
}

pub(super) fn resolve_ui_seed_locked(
    r: &impl LinReader,
    seed: &str,
    include_tags: bool,
) -> Result<Option<GraphNode>> {
    let node_kinds = ui_node_kind_set(include_tags);
    for column in ["id", "document_id", "label"] {
        let mut candidates: Vec<GraphNode> = candidate_nodes_for_column(r, column, seed)?
            .into_iter()
            .filter(|n| node_kinds.contains(&n.kind))
            .collect();
        if !candidates.is_empty() {
            candidates.sort_by(|a, b| {
                seed_rank(a)
                    .cmp(&seed_rank(b))
                    .then_with(|| a.id.cmp(&b.id))
            });
            return Ok(candidates.into_iter().next());
        }
    }
    Ok(None)
}

pub(super) fn find_scoped_node_locked(
    r: &impl LinReader,
    project: &str,
    column: &str,
    value: &str,
    include_tags: bool,
) -> Result<Option<GraphNode>> {
    let relation_types = scoped_relation_set(include_tags);
    let companion_kinds = scoped_companion_kind_set(include_tags);
    let mut wing_cache: HashMap<String, Option<String>> = HashMap::new();

    let candidates = candidate_nodes_for_column(r, column, value)?;
    let mut matching = Vec::new();
    for node in candidates {
        let qualifies = if let Some(doc_id) = node.document_id.as_deref() {
            is_project_doc_node_kind(&node.kind)
                && doc_wing_cached(r, doc_id, &mut wing_cache)? == Some(project)
        } else if companion_kinds.contains(&node.kind) {
            has_project_incident_edge(r, &node.id, project, &relation_types, &mut wing_cache)?
        } else {
            false
        };
        if qualifies {
            matching.push(node);
        }
    }

    if matching.is_empty() {
        return Ok(None);
    }
    matching.sort_by(|a, b| {
        seed_rank(a)
            .cmp(&seed_rank(b))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(matching.into_iter().next())
}

fn has_project_incident_edge(
    r: &impl LinReader,
    node_id: &str,
    project: &str,
    relation_types: &HashSet<String>,
    wing_cache: &mut HashMap<String, Option<String>>,
) -> Result<bool> {
    let check_other =
        |other_id: &str, wing_cache: &mut HashMap<String, Option<String>>| -> Result<bool> {
            let Some(other_row) = r.get_by_id("graph_nodes", other_id)? else {
                return Ok(false);
            };
            let other_kind = cell_str(&other_row, "kind").unwrap_or_default();
            if !is_project_doc_node_kind(other_kind) {
                return Ok(false);
            }
            let Some(doc_id) = cell_nonempty_string(&other_row, "document_id") else {
                return Ok(false);
            };
            Ok(doc_wing_cached(r, &doc_id, wing_cache)? == Some(project))
        };

    for edge_row in r.find_by_eq("graph_edges", "source_id", node_id)? {
        let Some(rel) = cell_str(&edge_row, "rel_type") else {
            continue;
        };
        if !relation_types.contains(rel) {
            continue;
        }
        if let Some(tid) = cell_str(&edge_row, "target_id") {
            if check_other(tid, wing_cache)? {
                return Ok(true);
            }
        }
    }
    for edge_row in r.find_by_eq("graph_edges", "target_id", node_id)? {
        let Some(rel) = cell_str(&edge_row, "rel_type") else {
            continue;
        };
        if !relation_types.contains(rel) {
            continue;
        }
        if let Some(sid) = cell_str(&edge_row, "source_id") {
            if check_other(sid, wing_cache)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// §7.2 local-graph BFS: `discover` is called once per hop with only the current
/// frontier, so no variant ever materialises the whole edge table.
///
/// `candidate_limit` is widened by `seen` so already-visited ids that the query
/// returns first cannot starve the budget; the caller-side `seen` filter then
/// takes the rest. `max_nodes` must already be non-zero.
pub(super) fn expand_frontier(
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

fn record_neighbor_hit(
    grouped: &mut HashMap<String, (usize, String)>,
    neighbor_id: &str,
    frontier_ordinal: usize,
    edge_id: &str,
) {
    match grouped.get_mut(neighbor_id) {
        Some((min_ord, min_edge)) => {
            if frontier_ordinal < *min_ord {
                *min_ord = frontier_ordinal;
            }
            if edge_id < min_edge.as_str() {
                *min_edge = edge_id.to_string();
            }
        }
        None => {
            grouped.insert(
                neighbor_id.to_string(),
                (frontier_ordinal, edge_id.to_string()),
            );
        }
    }
}

/// One undirected hop over `frontier` (§7.2), reporting only ids that still have
/// a node row. `rel_type = Some` restricts the hop to a single projection such as
/// tunnels; `None` follows every relation and node kind, which is the raw
/// [`Store::neighbors`] contract (tag hubs included).
pub(super) fn discover_frontier_neighbors_locked(
    r: &impl LinReader,
    frontier: &[String],
    limit: usize,
    rel_types: Option<&[&str]>,
) -> Result<Vec<String>> {
    if frontier.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let rel_set: Option<HashSet<&str>> = rel_types
        .filter(|rels| !rels.is_empty())
        .map(|rels| rels.iter().copied().collect());
    let mut node_exists_cache: HashMap<String, bool> = HashMap::new();
    // `grouped` stores ONLY verified (`exists_by_id == true`) nodes: `nid -> (min_ordinal, min_edge_id)`.
    let mut grouped: HashMap<String, (usize, String)> = HashMap::with_capacity(limit * 2);

    let recompute_worst = |m: &HashMap<String, String>| -> Option<(String, String)> {
        m.iter()
            .max_by(|(na, ea), (nb, eb)| {
                ea.as_str()
                    .cmp(eb.as_str())
                    .then_with(|| na.as_str().cmp(nb.as_str()))
            })
            .map(|(n, e)| (e.clone(), n.clone()))
    };

    let rel_col = rel_set.as_ref().map(|_| "rel_type");
    let mut cur_best: HashMap<String, String> = HashMap::with_capacity(limit + 1);

    for (ordinal, fid) in frontier.iter().enumerate() {
        cur_best.clear();
        let mut worst: Option<(String, String)> = None;

        let mut process_edge = |edge_id: &str, neighbor_id: &str| -> Result<()> {
            if !grouped.is_empty()
                && let Some((min_ord, min_edge)) = grouped.get_mut(neighbor_id)
            {
                if ordinal < *min_ord {
                    *min_ord = ordinal;
                }
                if edge_id < min_edge.as_str() {
                    min_edge.clear();
                    min_edge.push_str(edge_id);
                }
                return Ok(());
            }

            // Exact Top-`limit` pre-filter: once `cur_best` holds `limit` verified nodes at `ordinal`,
            // any edge `>= worst` can neither improve an existing entry in `cur_best` nor enter Top-`limit`!
            if let Some((ref w_eid, ref w_nid)) = worst
                && (edge_id, neighbor_id) >= (w_eid.as_str(), w_nid.as_str())
            {
                return Ok(());
            }

            if let Some(cur_e) = cur_best.get_mut(neighbor_id) {
                if edge_id < cur_e.as_str() {
                    let was_worst = worst
                        .as_ref()
                        .is_some_and(|(_, wn)| wn.as_str() == neighbor_id);
                    cur_e.clear();
                    cur_e.push_str(edge_id);
                    if was_worst {
                        worst = recompute_worst(&cur_best);
                    }
                }
                return Ok(());
            }

            let exists = if let Some(&cached) = node_exists_cache.get(neighbor_id) {
                cached
            } else {
                let ex = r.exists_by_id("graph_nodes", neighbor_id)?;
                node_exists_cache.insert(neighbor_id.to_string(), ex);
                ex
            };
            if !exists {
                return Ok(());
            }

            if cur_best.len() == limit
                && let Some((_, old_worst_nid)) = worst.take()
            {
                cur_best.remove(&old_worst_nid);
            }
            cur_best.insert(neighbor_id.to_string(), edge_id.to_string());
            if cur_best.len() == limit {
                worst = recompute_worst(&cur_best);
            }
            Ok(())
        };

        r.for_each_by_eq_str3(
            "graph_edges",
            "source_id",
            fid.as_str(),
            "id",
            "target_id",
            rel_col,
            |eid_opt, tid_opt, rel_opt| {
                if let Some(ref allowed) = rel_set {
                    let rel = rel_opt.unwrap_or_default();
                    if !allowed.contains(rel) {
                        return Ok(true);
                    }
                }
                if let Some(neighbor_id) = tid_opt {
                    process_edge(eid_opt.unwrap_or_default(), neighbor_id)?;
                }
                Ok(true)
            },
        )?;
        r.for_each_by_eq_str3(
            "graph_edges",
            "target_id",
            fid.as_str(),
            "id",
            "source_id",
            rel_col,
            |eid_opt, sid_opt, rel_opt| {
                if let Some(ref allowed) = rel_set {
                    let rel = rel_opt.unwrap_or_default();
                    if !allowed.contains(rel) {
                        return Ok(true);
                    }
                }
                if let Some(neighbor_id) = sid_opt {
                    process_edge(eid_opt.unwrap_or_default(), neighbor_id)?;
                }
                Ok(true)
            },
        )?;

        let valid_for_fid = cur_best.len();
        for (nid, eid) in cur_best.drain() {
            grouped.insert(nid, (ordinal, eid));
        }
        if valid_for_fid >= limit && grouped.len() >= limit {
            break;
        }
    }

    if grouped.is_empty() {
        return Ok(Vec::new());
    }

    let mut ordered: Vec<(usize, String, String)> = grouped
        .into_iter()
        .map(|(nid, (ord, eid))| (ord, eid, nid))
        .collect();
    ordered.sort_unstable_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    ordered.truncate(limit);
    Ok(ordered.into_iter().map(|(_, _, nid)| nid).collect())
}

/// Edges with both endpoints inside `node_ids` — the edge set the pre-§7.2
/// in-memory BFS reported, with no relation cap.
pub(super) fn load_edges_among_locked(
    r: &impl LinReader,
    node_ids: &[String],
    rel_types: Option<&[&str]>,
) -> Result<Vec<GraphEdge>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let selected: HashSet<&str> = node_ids.iter().map(String::as_str).collect();
    let rel_set: Option<HashSet<&str>> = rel_types
        .filter(|rels| !rels.is_empty())
        .map(|rels| rels.iter().copied().collect());
    let rel_col = rel_set.as_ref().map(|_| "rel_type");

    // Step 1: Zero-`Row` filter over `(id, target_id, rel_type)` so 20k-edge superhubs
    // never materialize `Row`s or Zstd `context` blocks for edges outside `selected`!
    let mut matching_edge_ids: Vec<String> = Vec::new();
    let mut seen_edges: HashSet<String> = HashSet::new();
    for &sid in &selected {
        r.for_each_by_eq_str3(
            "graph_edges",
            "source_id",
            sid,
            "id",
            "target_id",
            rel_col,
            |eid_opt, tid_opt, rel_opt| {
                let Some(tid) = tid_opt else {
                    return Ok(true);
                };
                if !selected.contains(tid) {
                    return Ok(true);
                }
                if let Some(ref allowed) = rel_set {
                    let rel = rel_opt.unwrap_or_default();
                    if !allowed.contains(rel) {
                        return Ok(true);
                    }
                }
                if let Some(eid) = eid_opt.filter(|s| !s.is_empty())
                    && seen_edges.insert(eid.to_string())
                {
                    matching_edge_ids.push(eid.to_string());
                }
                Ok(true)
            },
        )?;
    }

    // Step 2: Hydrate ONLY the matching induced-subgraph edges (`<= ~50-200` edges) by PK!
    let mut edges = Vec::with_capacity(matching_edge_ids.len());
    for eid in matching_edge_ids {
        if let Some(edge) = r.with_by_id("graph_edges", &eid, rows::graph_edge)? {
            edges.push(edge);
        }
    }
    edges.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(edges)
}

/// Neighbors of `frontier` over PKB relations, in frontier order. With a
/// `project`, only edges touching that wing and in-wing (or companion) nodes.
pub(super) fn discover_neighbors_locked(
    r: &impl LinReader,
    project: Option<&str>,
    frontier: &[String],
    limit: usize,
    include_tags: bool,
) -> Result<Vec<String>> {
    if frontier.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let relation_types = scoped_relation_set(include_tags);
    let companion_kinds = scoped_companion_kind_set(include_tags);
    let ui_kinds = ui_node_kind_set(include_tags);

    let mut node_cache: HashMap<String, Option<(String, Option<String>)>> = HashMap::new();
    let mut wing_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut grouped: HashMap<String, (usize, String)> = HashMap::new();

    let load_node_meta = |nid: &str,
                          cache: &mut HashMap<String, Option<(String, Option<String>)>>|
     -> Result<Option<(String, Option<String>)>> {
        if let Some(cached) = cache.get(nid) {
            return Ok(cached.clone());
        }
        let val = r
            .get_projected_by_id("graph_nodes", nid, &["kind", "document_id"])?
            .map(|row| {
                (
                    cell_string(&row, "kind"),
                    cell_nonempty_string(&row, "document_id"),
                )
            });
        cache.insert(nid.to_string(), val.clone());
        Ok(val)
    };

    let mut consider_incident = |ordinal: usize,
                                 edge_id: &str,
                                 source_id: &str,
                                 target_id: &str,
                                 rel: &str,
                                 neighbor_id: &str|
     -> Result<()> {
        if !relation_types.contains(rel) {
            return Ok(());
        }
        let Some((neighbor_kind, neighbor_doc_id)) = load_node_meta(neighbor_id, &mut node_cache)?
        else {
            return Ok(());
        };

        let qualifies = match project {
            None => ui_kinds.contains(&neighbor_kind),
            Some(proj) => {
                let Some((_, source_doc_id)) = load_node_meta(source_id, &mut node_cache)? else {
                    return Ok(());
                };
                let Some((_, target_doc_id)) = load_node_meta(target_id, &mut node_cache)? else {
                    return Ok(());
                };

                let neighbor_ok = if let Some(ref ndoc) = neighbor_doc_id {
                    is_project_doc_node_kind(&neighbor_kind)
                        && doc_wing_cached(r, ndoc, &mut wing_cache)? == Some(proj)
                } else {
                    companion_kinds.contains(&neighbor_kind)
                };
                if !neighbor_ok {
                    false
                } else {
                    let src_in_proj = match source_doc_id.as_deref() {
                        Some(sdoc) => doc_wing_cached(r, sdoc, &mut wing_cache)? == Some(proj),
                        None => false,
                    };
                    let tgt_in_proj = match target_doc_id.as_deref() {
                        Some(tdoc) => doc_wing_cached(r, tdoc, &mut wing_cache)? == Some(proj),
                        None => false,
                    };
                    src_in_proj || tgt_in_proj
                }
            }
        };

        if qualifies {
            record_neighbor_hit(&mut grouped, neighbor_id, ordinal, edge_id);
        }
        Ok(())
    };

    for (ordinal, fid) in frontier.iter().enumerate() {
        r.for_each_by_eq_str3(
            "graph_edges",
            "source_id",
            fid.as_str(),
            "id",
            "target_id",
            Some("rel_type"),
            |eid_opt, tid_opt, rel_opt| {
                if let (Some(tid), Some(rel)) = (tid_opt, rel_opt) {
                    consider_incident(
                        ordinal,
                        eid_opt.unwrap_or_default(),
                        fid.as_str(),
                        tid,
                        rel,
                        tid,
                    )?;
                }
                Ok(true)
            },
        )?;
        r.for_each_by_eq_str3(
            "graph_edges",
            "target_id",
            fid.as_str(),
            "id",
            "source_id",
            Some("rel_type"),
            |eid_opt, sid_opt, rel_opt| {
                if let (Some(sid), Some(rel)) = (sid_opt, rel_opt) {
                    consider_incident(
                        ordinal,
                        eid_opt.unwrap_or_default(),
                        sid,
                        fid.as_str(),
                        rel,
                        sid,
                    )?;
                }
                Ok(true)
            },
        )?;
    }

    let mut ordered: Vec<(usize, String, String)> = grouped
        .into_iter()
        .map(|(nid, (ord, eid))| (ord, eid, nid))
        .collect();
    ordered.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    if ordered.len() > limit {
        ordered.truncate(limit);
    }
    Ok(ordered.into_iter().map(|(_, _, nid)| nid).collect())
}

pub(super) fn load_nodes_by_ids_locked(
    r: &impl LinReader,
    node_ids: &[String],
) -> Result<Vec<GraphNode>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let limit = node_ids.len();
    let unique_sorted: BTreeSet<&str> = node_ids.iter().map(String::as_str).collect();
    let mut nodes = Vec::with_capacity(unique_sorted.len());
    for id in unique_sorted {
        if let Some(node) = r.with_by_id("graph_nodes", id, rows::graph_node)? {
            nodes.push(node);
            if nodes.len() == limit {
                break;
            }
        }
    }
    Ok(nodes)
}

/// Edges among `node_ids` over PKB relations. With a `project`, only edges
/// with at least one endpoint document in that wing.
pub(super) fn load_bounded_edges_locked(
    r: &impl LinReader,
    project: Option<&str>,
    node_ids: &[String],
    include_tags: bool,
) -> Result<Vec<GraphEdge>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let selected: HashSet<&str> = node_ids.iter().map(String::as_str).collect();
    let relation_types = scoped_relation_set(include_tags);
    let max_edges = node_ids
        .len()
        .saturating_mul(node_ids.len())
        .saturating_mul(crate::models::pkb_rel_types(include_tags).len())
        .max(1);

    let mut node_doc_cache: HashMap<String, Option<Option<String>>> = HashMap::new();
    let mut wing_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut seen_edges: HashSet<String> = HashSet::new();
    let mut edges = Vec::new();

    let load_node_doc = |nid: &str,
                         cache: &mut HashMap<String, Option<Option<String>>>|
     -> Result<Option<Option<String>>> {
        if let Some(cached) = cache.get(nid) {
            return Ok(cached.clone());
        }
        let val = r
            .get_by_id("graph_nodes", nid)?
            .map(|row| cell_nonempty_string(&row, "document_id"));
        cache.insert(nid.to_string(), val.clone());
        Ok(val)
    };

    for &sid in &selected {
        for edge_row in r.find_by_eq("graph_edges", "source_id", sid)? {
            let Some(tid) = cell_str(&edge_row, "target_id") else {
                continue;
            };
            if !selected.contains(tid) {
                continue;
            }
            let Some(rel) = cell_str(&edge_row, "rel_type") else {
                continue;
            };
            if !relation_types.contains(rel) {
                continue;
            }

            if let Some(proj) = project {
                let Some(src_doc) = load_node_doc(sid, &mut node_doc_cache)? else {
                    continue;
                };
                let Some(tgt_doc) = load_node_doc(tid, &mut node_doc_cache)? else {
                    continue;
                };
                let src_ok = match src_doc.as_deref() {
                    Some(d) => doc_wing_cached(r, d, &mut wing_cache)? == Some(proj),
                    None => false,
                };
                let tgt_ok = match tgt_doc.as_deref() {
                    Some(d) => doc_wing_cached(r, d, &mut wing_cache)? == Some(proj),
                    None => false,
                };
                if !src_ok && !tgt_ok {
                    continue;
                }
            }

            let edge = rows::graph_edge(&edge_row)?;
            if seen_edges.insert(edge.id.clone()) {
                edges.push(edge);
            }
        }
    }

    edges.sort_by(|a, b| a.id.cmp(&b.id));
    if edges.len() > max_edges {
        edges.truncate(max_edges);
    }
    Ok(edges)
}
