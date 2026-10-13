//! Bounded graph exports for the UI: whole-graph, project-scoped and
//! neighborhood views with their frontier and edge queries.

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
        let edges = load_bounded_edges_locked(&conn, Some(project), &node_ids, include_tags)?;
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
            discover_neighbors_locked(&conn, Some(project), frontier, limit, include_tags)
        })?;

        let mut nodes = load_nodes_by_ids_locked(&conn, &visited)?;
        enrich_document_placements_locked(&conn, &mut nodes)?;
        let edges = load_bounded_edges_locked(&conn, Some(project), &visited, include_tags)?;
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
            discover_neighbors_locked(&conn, None, frontier, limit, include_tags)
        })?;

        let mut nodes = load_nodes_by_ids_locked(&conn, &visited)?;
        enrich_document_placements_locked(&conn, &mut nodes)?;
        let edges = load_bounded_edges_locked(&conn, None, &visited, include_tags)?;
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

/// §7.1 default relation set for project-scoped UI queries, from the one source
/// ([`crate::models::pkb_rel_types`]) the MCP tools and the UI export use too.
pub(super) fn scoped_relation_types(include_tags: bool) -> String {
    sql_string_list(
        crate::models::pkb_rel_types(include_tags)
            .iter()
            .map(String::as_str),
    )
}

/// Companion (non-document) node kinds reachable from a project export.
pub(super) fn scoped_companion_kinds(include_tags: bool) -> String {
    sql_string_list(
        crate::models::pkb_node_kinds(include_tags)
            .iter()
            .map(String::as_str)
            .filter(|kind| !NodeKind::Document.is(kind)),
    )
}

/// Node kinds a UI export may contain (§7.1).
pub(super) fn ui_node_kinds(include_tags: bool) -> String {
    sql_string_list(
        crate::models::pkb_node_kinds(include_tags)
            .iter()
            .map(String::as_str),
    )
}

pub(super) fn values_clause(len: usize) -> String {
    (0..len).map(|_| "(?)").collect::<Vec<_>>().join(", ")
}

pub(super) fn placeholder_list(len: usize) -> String {
    vec!["?"; len].join(", ")
}

/// Make project-scoped graph exports self-describing for UI-side room/layer filters.
///
/// Graph extraction intentionally keeps node metadata independent from document
/// metadata, so placement is joined from the authoritative document row only at
/// the project export boundary. Document placement wins over stale graph metadata;
/// unrelated graph metadata is preserved.
pub(super) fn enrich_document_placements_locked(
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
    crate::db::rows::visit_rows(&mut rows, |row| {
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
        Ok(())
    })?;

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
    let nodes = crate::db::rows::query_vec(&mut stmt, params![project], row_to_node)?;
    Ok(nodes)
}

pub(super) fn load_bounded_project_companions_locked(
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
    let nodes = crate::db::rows::query_vec(
        &mut stmt,
        params_from_iter(project_node_ids.iter()),
        row_to_node,
    )?;
    Ok(nodes)
}

pub(super) fn resolve_project_seed_locked(
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

pub(super) fn resolve_ui_seed_locked(
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

pub(super) fn find_scoped_node_locked(
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

/// One undirected hop over `frontier` (§7.2), reporting only ids that still have
/// a node row. `rel_type = Some` restricts the hop to a single projection such as
/// tunnels; `None` follows every relation and node kind, which is the raw
/// [`Store::neighbors`] contract (tag hubs included).
pub(super) fn discover_frontier_neighbors_locked(
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
    let neighbors = crate::db::rows::query_vec(&mut stmt, params_from_iter(binds.iter()), |row| {
        Ok(row.get(0)?)
    })?;
    Ok(neighbors)
}

/// Edges with both endpoints inside `node_ids` — the edge set the pre-§7.2
/// in-memory BFS reported, with no relation cap.
pub(super) fn load_edges_among_locked(
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
    let edges = crate::db::rows::query_vec(&mut stmt, params_from_iter(binds.iter()), row_to_edge)?;
    Ok(edges)
}

/// Neighbors of `frontier` over PKB relations, in frontier order. With a
/// `project`, only edges touching that wing and in-wing (or companion) nodes.
pub(super) fn discover_neighbors_locked(
    conn: &duckdb::Connection,
    project: Option<&str>,
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
    let (scope_joins, node_filter) = match project {
        Some(_) => (
            r#"
        JOIN graph_nodes source_node ON source_node.id = incident.source_id
        JOIN graph_nodes target_node ON target_node.id = incident.target_id
        LEFT JOIN documents neighbor_document ON neighbor_document.id = neighbor.document_id
        LEFT JOIN documents source_document ON source_document.id = source_node.document_id
        LEFT JOIN documents target_document ON target_document.id = target_node.document_id"#,
            format!(
                r#"(
            (
              neighbor.document_id IS NOT NULL
              AND neighbor_document.wing = ?
              AND neighbor.kind IN ('document', 'stub', 'entity')
            )
            OR (
              neighbor.document_id IS NULL
              AND neighbor.kind IN ({})
            )
          )
          AND (source_document.wing = ? OR target_document.wing = ?)"#,
                scoped_companion_kinds(include_tags)
            ),
        ),
        None => (
            "",
            format!("neighbor.kind IN ({})", ui_node_kinds(include_tags)),
        ),
    };
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
        JOIN graph_nodes neighbor ON neighbor.id = incident.neighbor_id{scope_joins}
        WHERE incident.rel_type IN ({relation_types})
          AND {node_filter}
        GROUP BY incident.neighbor_id
        ORDER BY
          MIN(incident.frontier_ordinal),
          MIN(incident.edge_id),
          incident.neighbor_id
        LIMIT {limit}
        "#
    );
    let mut binds = frontier.to_vec();
    if let Some(project) = project {
        binds.extend([
            project.to_string(),
            project.to_string(),
            project.to_string(),
        ]);
    }
    let mut stmt = conn.prepare(&sql)?;
    crate::db::rows::query_vec(&mut stmt, params_from_iter(binds.iter()), |row| {
        Ok(row.get(0)?)
    })
}

pub(super) fn load_nodes_by_ids_locked(
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
    let nodes = crate::db::rows::query_vec(&mut stmt, params_from_iter(node_ids.iter()), |row| {
        row_to_node(row)
    })?;
    Ok(nodes)
}

/// Edges among `node_ids` over PKB relations. With a `project`, only edges
/// with at least one endpoint document in that wing.
pub(super) fn load_bounded_edges_locked(
    conn: &duckdb::Connection,
    project: Option<&str>,
    node_ids: &[String],
    include_tags: bool,
) -> Result<Vec<GraphEdge>> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let selected = values_clause(node_ids.len());
    let relation_types = scoped_relation_types(include_tags);
    let max_edges = node_ids
        .len()
        .saturating_mul(node_ids.len())
        .saturating_mul(crate::models::pkb_rel_types(include_tags).len())
        .max(1);
    let (scope_joins, scope_filter) = match project {
        Some(_) => (
            r#"
        JOIN graph_nodes source_node ON source_node.id = e.source_id
        JOIN graph_nodes target_node ON target_node.id = e.target_id
        LEFT JOIN documents source_document ON source_document.id = source_node.document_id
        LEFT JOIN documents target_document ON target_document.id = target_node.document_id"#,
            "\n          AND (source_document.wing = ? OR target_document.wing = ?)",
        ),
        None => ("", ""),
    };
    let sql = format!(
        r#"
        WITH selected(id) AS (VALUES {selected})
        SELECT e.id, e.source_id, e.target_id, e.rel_type, e.weight, e.context
        FROM graph_edges e
        JOIN selected source_selected ON source_selected.id = e.source_id
        JOIN selected target_selected ON target_selected.id = e.target_id{scope_joins}
        WHERE e.rel_type IN ({relation_types}){scope_filter}
        ORDER BY e.id
        LIMIT {max_edges}
        "#
    );
    let mut binds = node_ids.to_vec();
    if let Some(project) = project {
        binds.extend([project.to_string(), project.to_string()]);
    }
    let mut stmt = conn.prepare(&sql)?;
    crate::db::rows::query_vec(&mut stmt, params_from_iter(binds.iter()), row_to_edge)
}
