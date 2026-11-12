//! Graph node/edge CRUD, filtered export, undirected BFS neighbors, and backlinks over `lin`.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::Utc;
use uuid::Uuid;

use super::lin_engine::{
    cell_bool, cell_f64, cell_nonempty_string, cell_opt_string, cell_str, cell_string, LinReader,
    LinWriter, RecBuilder,
};
use super::rows;
use super::store::Store;
use crate::error::{AppError, Result};
use crate::graph::REL_TUNNEL;
use crate::models::{
    EdgeCutRow, EdgeOrigin, GraphEdge, GraphFilter, GraphNode, GraphStats, GraphView, NodeKind,
    RelType,
};

/// Node cap for a filtered graph export when the filter sets none.
const DEFAULT_EXPORT_MAX_NODES: u32 = 500;

/// GRAPH_DESIGN §9 hard cap for a local-graph walk. Hop-by-hop frontier queries
/// make depth affordable, so this bounds pathological requests rather than cost.
const MAX_LOCAL_GRAPH_DEPTH: u32 = 5;

#[cfg(test)]
static FAIL_DERIVED_EDGE_DUPLICATE_KEYS: std::sync::LazyLock<
    std::sync::Mutex<HashSet<usize>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashSet::new()));

#[cfg(test)]
pub(crate) fn set_fail_derived_edge_duplicate_key(key: usize, fail: bool) {
    let mut guard = FAIL_DERIVED_EDGE_DUPLICATE_KEYS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if fail {
        guard.insert(key);
    } else {
        guard.remove(&key);
    }
}

#[cfg(test)]
fn should_fail_derived_edge_duplicate(w: &impl LinWriter) -> bool {
    let key = std::sync::Arc::as_ptr(w.bridge_embedder()) as usize;
    FAIL_DERIVED_EDGE_DUPLICATE_KEYS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&key)
}

impl Store {
    /// Insert or replace a graph node by primary key `id`.
    ///
    /// Preserves original `created_at` when the id already exists.
    pub fn upsert_graph_node(&self, node: &GraphNode) -> Result<()> {
        let mut w = self.write()?;
        upsert_graph_node_locked(&mut w, node)
    }

    /// Link a document node to the canonical tag hubs for `labels` in one
    /// transaction; `replace` first drops its existing `tagged` edges. A
    /// failure part-way leaves the previous tag edges intact.
    pub fn set_document_tags(&self, node_id: &str, labels: &[String], replace: bool) -> Result<()> {
        self.with_tx(|tx| {
            if replace {
                let pred = lin::query::pred::and(
                    lin::query::pred::eq("source_id", node_id),
                    lin::query::pred::eq("rel_type", crate::graph::REL_TAGGED),
                );
                tx.delete_where("graph_edges", pred)?;
            }
            for label in labels {
                let tag_id = resolve::upsert_tag_node(tx, label)?;
                let existing = tx.find_by_eq("graph_edges", "source_id", node_id)?;
                let already = existing.iter().any(|row| {
                    cell_str(row, "target_id") == Some(tag_id.as_str())
                        && cell_str(row, "rel_type") == Some(crate::graph::REL_TAGGED)
                });
                if !already {
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

    /// Insert graph edges under the store write lock.
    pub fn insert_graph_edges(&self, edges: &[GraphEdge]) -> Result<()> {
        let mut w = self.write()?;
        insert_graph_edges_locked(&mut w, edges)
    }

    /// Remove a node's edges on purpose; document delete uses demote instead (§6.3).
    pub fn delete_edges_from(&self, source_id: &str) -> Result<()> {
        let mut w = self.write()?;
        w.delete_where("graph_edges", lin::query::pred::eq("source_id", source_id))?;
        Ok(())
    }

    /// Delete only the edges body-text extraction owns for `source_id`.
    pub fn delete_derived_edges_from(&self, source_id: &str) -> Result<()> {
        let mut w = self.write()?;
        delete_derived_edges_from_locked(&mut w, source_id)
    }

    /// Delete a graph node by id (does not cascade edges).
    pub fn delete_graph_node(&self, node_id: &str) -> Result<bool> {
        let mut w = self.write()?;
        w.delete_by_id("graph_nodes", node_id)
    }

    /// Find the graph node linked to a document id, if any.
    pub fn find_node_by_document_id(&self, doc_id: &str) -> Result<Option<GraphNode>> {
        let r = self.read()?;
        find_node_by_document_id_locked(&r, doc_id)
    }

    /// Find nodes whose [`label_key`](crate::graph::normalize::label_key) equals the
    /// normalized form of `label` (case-, width- and whitespace-insensitive).
    pub fn find_nodes_by_label(&self, label: &str) -> Result<Vec<GraphNode>> {
        let r = self.read()?;
        find_nodes_by_label_locked(&r, label)
    }

    /// Find a node by primary key.
    pub fn find_node_by_id(&self, node_id: &str) -> Result<Option<GraphNode>> {
        let r = self.read()?;
        load_node_locked(&r, node_id)
    }

    /// Find a node by exact `uri` match.
    pub fn find_node_by_uri(&self, uri: &str) -> Result<Option<GraphNode>> {
        let r = self.read()?;
        find_node_by_uri_locked(&r, uri)
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
    pub fn get_graph_view(&self, filter: GraphFilter) -> Result<GraphView> {
        let max_nodes = filter.max_nodes.unwrap_or(DEFAULT_EXPORT_MAX_NODES).max(1) as usize;
        if let Some(ref kinds) = filter.kinds
            && kinds.is_empty()
        {
            return Ok(GraphView::default());
        }
        if let Some(ref seeds) = filter.seed_ids
            && seeds.is_empty()
        {
            return Ok(GraphView::default());
        }

        let r = self.read()?;
        let kind_set: Option<HashSet<&str>> = filter
            .kinds
            .as_ref()
            .map(|k| k.iter().map(String::as_str).collect());
        let seed_set: Option<HashSet<&str>> = filter
            .seed_ids
            .as_ref()
            .map(|s| s.iter().map(String::as_str).collect());

        let mut nodes: Vec<GraphNode> = Vec::new();
        if let Some(ref seeds) = seed_set {
            for &sid in seeds {
                if let Some(row) = r.get_by_id("graph_nodes", sid)? {
                    if kind_set
                        .as_ref()
                        .is_none_or(|ks| cell_str(&row, "kind").is_some_and(|k| ks.contains(k)))
                    {
                        nodes.push(rows::graph_node(&row)?);
                    }
                }
            }
        } else {
            r.scan_each_projected(
                "graph_nodes",
                &[
                    "id",
                    "kind",
                    "label",
                    "document_id",
                    "uri",
                    "resolved",
                    "metadata_json",
                ],
                |row| {
                    if kind_set
                        .as_ref()
                        .is_none_or(|ks| cell_str(row, "kind").is_some_and(|k| ks.contains(k)))
                    {
                        nodes.push(rows::graph_node(row)?);
                    }
                    Ok(true)
                },
            )?;
        }

        nodes.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.id.cmp(&b.id)));
        nodes.truncate(max_nodes);
        if nodes.is_empty() {
            return Ok(GraphView::default());
        }

        if let Some(ref rels) = filter.rel_types
            && rels.is_empty()
        {
            return Ok(GraphView {
                nodes,
                edges: Vec::new(),
            });
        }
        let rel_set: Option<HashSet<&str>> = filter
            .rel_types
            .as_ref()
            .map(|rels| rels.iter().map(String::as_str).collect());
        let selected_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();

        let mut edges: Vec<GraphEdge> = Vec::new();
        const EDGE_COLS: &[&str] = &[
            "id",
            "source_id",
            "target_id",
            "rel_type",
            "weight",
            "context",
        ];
        if selected_ids.len() <= 512 {
            for &nid in &selected_ids {
                r.for_each_by_eq_projected(
                    "graph_edges",
                    "source_id",
                    nid,
                    Some(EDGE_COLS),
                    |row| {
                        let Some(tgt) = cell_str(row, "target_id") else {
                            return Ok(true);
                        };
                        if !selected_ids.contains(tgt) {
                            return Ok(true);
                        }
                        if rel_set.as_ref().is_some_and(|rs| {
                            !cell_str(row, "rel_type").is_some_and(|rt| rs.contains(rt))
                        }) {
                            return Ok(true);
                        }
                        edges.push(rows::graph_edge(row)?);
                        Ok(true)
                    },
                )?;
            }
        } else {
            r.scan_each_projected("graph_edges", EDGE_COLS, |row| {
                let (Some(src), Some(tgt)) = (cell_str(row, "source_id"), cell_str(row, "target_id"))
                else {
                    return Ok(true);
                };
                if selected_ids.contains(src)
                    && selected_ids.contains(tgt)
                    && rel_set
                        .as_ref()
                        .is_none_or(|rs| cell_str(row, "rel_type").is_some_and(|rt| rs.contains(rt)))
                {
                    edges.push(rows::graph_edge(row)?);
                }
                Ok(true)
            })?;
        }
        edges.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(GraphView { nodes, edges })
    }

    /// Every graph node ordered by `(label, id)`, with `metadata_json` left as `{}`.
    pub fn list_graph_nodes(&self) -> Result<Vec<GraphNode>> {
        let r = self.read()?;
        let mut nodes = Vec::new();
        r.scan_each_projected(
            "graph_nodes",
            &["id", "kind", "label", "document_id", "uri", "resolved"],
            |row| {
                nodes.push(GraphNode {
                    id: cell_string(row, "id"),
                    kind: cell_string(row, "kind"),
                    label: cell_string(row, "label"),
                    document_id: cell_opt_string(row, "document_id"),
                    uri: cell_opt_string(row, "uri"),
                    resolved: cell_bool(row, "resolved", true),
                    metadata_json: "{}".into(),
                });
                Ok(true)
            },
        )?;
        nodes.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.id.cmp(&b.id)));
        Ok(nodes)
    }

    /// Every graph edge ordered by `id`, without its `context` text (`None`).
    pub fn list_graph_links(&self) -> Result<Vec<GraphEdge>> {
        let r = self.read()?;
        let mut edges = Vec::new();
        r.scan_each_projected(
            "graph_edges",
            &["id", "source_id", "target_id", "rel_type", "weight"],
            |row| {
                edges.push(GraphEdge {
                    id: cell_string(row, "id"),
                    source_id: cell_string(row, "source_id"),
                    target_id: cell_string(row, "target_id"),
                    rel_type: cell_string(row, "rel_type"),
                    weight: cell_f64(row, "weight", 1.0),
                    context: None,
                });
                Ok(true)
            },
        )?;
        edges.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(edges)
    }

    /// Return every graph edge, including edges whose endpoint node is missing.
    pub fn list_graph_edges(&self) -> Result<Vec<GraphEdge>> {
        let r = self.read()?;
        let mut edges = Vec::new();
        r.scan_each_projected(
            "graph_edges",
            &["id", "source_id", "target_id", "rel_type", "weight", "context"],
            |row| {
                edges.push(rows::graph_edge(row)?);
                Ok(true)
            },
        )?;
        edges.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(edges)
    }

    /// Undirected BFS neighborhood of `node_id` up to `depth`, capped at `max_nodes`.
    pub fn neighbors(&self, node_id: &str, depth: u32, max_nodes: u32) -> Result<GraphView> {
        self.neighbors_filtered(node_id, depth, max_nodes, None)
    }

    /// The same bounded walk restricted to a relation set (§7.1).
    pub fn neighbors_filtered(
        &self,
        node_id: &str,
        depth: u32,
        max_nodes: u32,
        rel_types: Option<&[String]>,
    ) -> Result<GraphView> {
        let max_nodes = if max_nodes == 0 { 100 } else { max_nodes };
        let depth = depth.clamp(1, MAX_LOCAL_GRAPH_DEPTH);

        let r = self.read()?;
        if load_node_locked(&r, node_id)?.is_none() {
            return Ok(GraphView::default());
        }

        let selected: Vec<&str> = rel_types
            .unwrap_or_default()
            .iter()
            .map(String::as_str)
            .collect();
        let rel_filter = (!selected.is_empty()).then_some(selected.as_slice());

        let visited = expand_frontier(node_id, depth, max_nodes as usize, |frontier, limit| {
            discover_frontier_neighbors_locked(&r, frontier, limit, rel_filter)
        })?;

        let nodes = load_nodes_by_ids_locked(&r, &visited)?;
        let edges = load_edges_among_locked(&r, &visited, rel_filter)?;
        Ok(GraphView { nodes, edges })
    }

    /// Incoming edges + source nodes for `node_id` (Obsidian-style backlinks).
    pub fn backlinks(&self, node_id: &str) -> Result<GraphView> {
        let r = self.read()?;
        let edge_rows = r.find_by_eq("graph_edges", "target_id", node_id)?;
        let mut edges: Vec<GraphEdge> = edge_rows
            .iter()
            .map(rows::graph_edge)
            .collect::<Result<Vec<_>>>()?;
        edges.sort_by(|a, b| a.id.cmp(&b.id));

        let mut node_ids: HashSet<String> = edges.iter().map(|e| e.source_id.clone()).collect();
        node_ids.insert(node_id.to_string());
        let mut sorted_ids: Vec<String> = node_ids.into_iter().collect();
        sorted_ids.sort();
        let nodes = load_nodes_by_ids_locked(&r, &sorted_ids)?;

        Ok(GraphView { nodes, edges })
    }

    /// Create an explicit edge between two existing nodes.
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
        let r = self.read()?;
        let edge_rows = r.find_by_eq("graph_edges", "rel_type", rel_type)?;
        let mut pairs = Vec::new();
        for row in &edge_rows {
            let (Some(src), Some(tgt)) = (cell_str(row, "source_id"), cell_str(row, "target_id"))
            else {
                continue;
            };
            let src_doc = r
                .get_by_id("graph_nodes", src)?
                .and_then(|n| cell_nonempty_string(&n, "document_id"));
            let tgt_doc = r
                .get_by_id("graph_nodes", tgt)?
                .and_then(|n| cell_nonempty_string(&n, "document_id"));
            if let (Some(sd), Some(td)) = (src_doc, tgt_doc) {
                pairs.push((sd, td));
            }
        }
        Ok(pairs)
    }

    /// Aggregate node counts by kind and edge counts by `rel_type`.
    pub fn graph_stats(&self) -> Result<GraphStats> {
        let r = self.read()?;
        let (total_nodes, node_buckets) = r.group_count_str1("graph_nodes", "kind")?;
        let mut nodes_by_kind = BTreeMap::new();
        for (kind_opt, cnt) in node_buckets {
            let kind = kind_opt.unwrap_or_default();
            *nodes_by_kind.entry(kind).or_insert(0u64) += cnt;
        }

        let (total_edges, edge_buckets) =
            r.group_count_str3("graph_edges", "rel_type", "origin", "context")?;
        let mut edges_by_rel_type = BTreeMap::new();
        let mut cuts: BTreeMap<(String, String, String), u64> = BTreeMap::new();
        for ((rel_opt, orig_opt, ctx_opt), cnt) in edge_buckets {
            let rel = rel_opt
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "(none)".into());
            *edges_by_rel_type.entry(rel.clone()).or_insert(0u64) += cnt;
            let origin = orig_opt
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "(none)".into());
            let context = ctx_opt.unwrap_or_default();
            *cuts.entry((rel, origin, context)).or_insert(0u64) += cnt;
        }

        let edges_by_cut = cuts
            .into_iter()
            .map(|((rel_type, origin, context), count)| EdgeCutRow {
                rel_type,
                origin,
                context,
                count,
            })
            .collect();

        Ok(GraphStats {
            total_nodes,
            total_edges,
            nodes_by_kind,
            edges_by_rel_type,
            edges_by_cut,
        })
    }

    /// Remove unresolved stubs that no edge references after a complete graph rebuild.
    pub fn prune_orphan_stubs(&self) -> Result<usize> {
        let mut w = self.write()?;
        let incident = collect_incident_node_ids(&w)?;
        let mut to_delete = Vec::new();
        w.scan_each("graph_nodes", |row| {
            let kind = cell_str(row, "kind").unwrap_or("");
            let resolved = cell_bool(row, "resolved", true);
            if kind == "stub"
                && !resolved
                && let Some(id) = cell_str(row, "id")
                && !incident.contains(id)
            {
                to_delete.push(id.to_string());
            }
            Ok(true)
        })?;
        w.delete_by_ids("graph_nodes", &to_delete)
    }

    /// Remove derived tag nodes that no rebuilt document references.
    pub fn prune_orphan_tags(&self) -> Result<usize> {
        let mut w = self.write()?;
        let incident = collect_incident_node_ids(&w)?;
        let mut to_delete = Vec::new();
        w.scan_each("graph_nodes", |row| {
            if cell_str(row, "kind") == Some("tag")
                && let Some(id) = cell_str(row, "id")
                && !incident.contains(id)
            {
                to_delete.push(id.to_string());
            }
            Ok(true)
        })?;
        w.delete_by_ids("graph_nodes", &to_delete)
    }
}

pub(crate) fn collect_incident_node_ids(r: &impl LinReader) -> Result<HashSet<String>> {
    let mut ids = HashSet::new();
    r.scan_each("graph_edges", |row| {
        if let Some(s) = cell_str(row, "source_id") {
            ids.insert(s.to_string());
        }
        if let Some(t) = cell_str(row, "target_id") {
            ids.insert(t.to_string());
        }
        Ok(true)
    })?;
    Ok(ids)
}

/// Inline a server-owned name list as a SQL `IN (…)` literal list (kept for compat).
#[cfg(test)]
pub(crate) fn sql_string_list<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    names
        .into_iter()
        .map(|name| format!("'{}'", name.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn upsert_graph_node_locked(w: &mut impl LinWriter, node: &GraphNode) -> Result<()> {
    let now = format_ts_now();
    let metadata = if node.metadata_json.is_empty() {
        "{}"
    } else {
        node.metadata_json.as_str()
    };
    crate::models::parse_node_kind(&node.kind)?;
    let existing = w.get_by_id("graph_nodes", &node.id)?;
    let created_at = existing
        .as_ref()
        .and_then(|row| cell_nonempty_string(row, "created_at"))
        .unwrap_or_else(|| now.clone());

    w.upsert_by_id(
        "graph_nodes",
        &node.id,
        RecBuilder::new()
            .str("id", &node.id)
            .str("kind", &node.kind)
            .str("label", &node.label)
            .str("label_key", crate::graph::normalize::label_key(&node.label))
            .opt_string("document_id", &node.document_id)
            .opt_string("uri", &node.uri)
            .bool("resolved", node.resolved)
            .str("metadata_json", metadata)
            .str("created_at", created_at)
            .str("updated_at", now)
            .build(),
    )
}

/// Transaction-aware form of [`Store::delete_derived_edges_from`].
pub(crate) fn delete_derived_edges_from_locked(
    w: &mut impl LinWriter,
    source_id: &str,
) -> Result<()> {
    let mut to_delete = Vec::new();
    w.for_each_by_eq_str3(
        "graph_edges",
        "source_id",
        source_id,
        "id",
        "origin",
        Some("rel_type"),
        |id, origin, rel_type| {
            let Some(id) = id else {
                return Ok(true);
            };
            let origin_clean = origin.map(str::trim).filter(|s| !s.is_empty());
            let rel_type = rel_type.unwrap_or("");
            let is_derived = origin_clean == Some("extract")
                || (origin_clean.is_none()
                    && crate::models::EXTRACT_OWNED_REL_TYPES.contains(&rel_type));
            if is_derived {
                to_delete.push(id.to_string());
            }
            Ok(true)
        },
    )?;
    w.delete_by_ids("graph_edges", &to_delete)?;
    Ok(())
}

/// §6.3 / §14: deleting a document **demotes** its graph node instead of wiping it.
pub(crate) fn demote_graph_for_document_locked(
    w: &mut impl LinWriter,
    document_id: &str,
) -> Result<u64> {
    let mut node_ids = Vec::new();
    w.for_each_by_eq_str3(
        "graph_nodes",
        "document_id",
        document_id,
        "id",
        "kind",
        None,
        |id, _kind, _| {
            if let Some(id) = id.filter(|s| !s.is_empty()) {
                node_ids.push(id.to_string());
            }
            Ok(true)
        },
    )?;
    for node_id in &node_ids {
        delete_derived_edges_from_locked(w, node_id)?;
    }
    let now = format_ts_now();
    for node_id in &node_ids {
        w.update_where(
            "graph_nodes",
            lin::query::pred::eq("id", node_id.as_str()),
            RecBuilder::new()
                .str("kind", "stub")
                .bool("resolved", false)
                .opt_str("document_id", None)
                .str("updated_at", &now)
                .build(),
        )?;
    }
    Ok(node_ids.len() as u64)
}

pub(crate) fn insert_graph_edges_locked(
    w: &mut impl LinWriter,
    edges: &[GraphEdge],
) -> Result<()> {
    if edges.is_empty() {
        return Ok(());
    }
    validate_edge_vocabulary(edges.iter(), EdgeOrigin::EXPLICIT)?;
    let now = format_ts_now();
    let mut records = Vec::with_capacity(edges.len());
    for edge in edges {
        records.push(
            RecBuilder::new()
                .str("id", &edge.id)
                .str("source_id", &edge.source_id)
                .str("target_id", &edge.target_id)
                .str("rel_type", &edge.rel_type)
                .f64("weight", edge.weight)
                .opt_string("context", &edge.context)
                .str("created_at", &now)
                .str("origin", EdgeOrigin::EXPLICIT)
                .build(),
        );
    }
    w.insert_records("graph_edges", records)?;
    Ok(())
}

/// Where one extracted edge came from (§4.1, §6.1 step 5).
pub(crate) struct DerivedEdge {
    pub edge: GraphEdge,
    pub alias: Option<String>,
    pub heading: Option<String>,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub occurrence: i64,
    pub chunk_id: Option<String>,
}

pub(crate) fn insert_derived_graph_edges_locked(
    w: &mut impl LinWriter,
    edges: &[DerivedEdge],
) -> Result<()> {
    if edges.is_empty() {
        return Ok(());
    }
    validate_edge_vocabulary(edges.iter().map(|row| &row.edge), EdgeOrigin::EXTRACT)?;
    #[cfg(test)]
    if should_fail_derived_edge_duplicate(w) {
        let mut seen = HashSet::new();
        for row in edges {
            let key = (
                row.edge.source_id.as_str(),
                row.edge.target_id.as_str(),
                row.edge.rel_type.as_str(),
            );
            if !seen.insert(key) {
                return Err(AppError::db(
                    "unique constraint failed: duplicate derived graph edge",
                ));
            }
        }
    }
    let now = format_ts_now();
    let mut records = Vec::with_capacity(edges.len());
    for row in edges {
        records.push(
            RecBuilder::new()
                .str("id", &row.edge.id)
                .str("source_id", &row.edge.source_id)
                .str("target_id", &row.edge.target_id)
                .str("rel_type", &row.edge.rel_type)
                .f64("weight", row.edge.weight)
                .opt_string("context", &row.edge.context)
                .str("created_at", &now)
                .str("origin", EdgeOrigin::EXTRACT)
                .opt_string("alias", &row.alias)
                .opt_string("heading", &row.heading)
                .opt_string("chunk_id", &row.chunk_id)
                .opt_i64("char_start", row.char_start)
                .opt_i64("char_end", row.char_end)
                .i64("occurrence", row.occurrence)
                .build(),
        );
    }
    w.insert_records("graph_edges", records)?;
    Ok(())
}

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
    r: &impl LinReader,
    document_id: &str,
) -> Result<Option<GraphNode>> {
    let matches = r.find_by_eq("graph_nodes", "document_id", document_id)?;
    matches.first().map(rows::graph_node).transpose()
}

pub(crate) fn find_node_by_uri_locked(
    r: &impl LinReader,
    uri: &str,
) -> Result<Option<GraphNode>> {
    let mut matches = r.find_by_eq("graph_nodes", "uri", uri)?;
    if matches.is_empty() {
        return Ok(None);
    }
    // Deterministic order: COALESCE(resolved, FALSE) DESC, created_at ASC, id ASC
    matches.sort_by(|a, b| {
        let res_a = cell_bool(a, "resolved", false);
        let res_b = cell_bool(b, "resolved", false);
        res_b
            .cmp(&res_a)
            .then_with(|| {
                let ca = super::lin_engine::cell_ts_or_now(a, "created_at");
                let cb = super::lin_engine::cell_ts_or_now(b, "created_at");
                ca.cmp(&cb)
            })
            .then_with(|| cell_str(a, "id").cmp(&cell_str(b, "id")))
    });
    let node = rows::graph_node(&matches[0])?;
    if matches.len() > 1 {
        tracing::warn!(
            uri,
            canonical_node_id = %node.id,
            "several graph nodes share this uri; the read follows the deterministic order"
        );
    }
    Ok(Some(node))
}

pub(crate) fn find_nodes_by_label_locked(
    r: &impl LinReader,
    label: &str,
) -> Result<Vec<GraphNode>> {
    let key = crate::graph::normalize::label_key(label);
    let matches = r.find_by_eq("graph_nodes", "label_key", key.as_str())?;
    let mut nodes: Vec<GraphNode> = matches
        .iter()
        .map(rows::graph_node)
        .collect::<Result<Vec<_>>>()?;
    // ORDER BY resolved DESC, kind ASC, id ASC
    nodes.sort_by(|a, b| {
        b.resolved
            .cmp(&a.resolved)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(nodes)
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

fn load_node_locked(r: &impl LinReader, id: &str) -> Result<Option<GraphNode>> {
    r.get_by_id("graph_nodes", id)?
        .as_ref()
        .map(rows::graph_node)
        .transpose()
}

mod export;
pub mod resolve;
mod tunnels;

use self::export::*;

#[cfg(test)]
mod tests;
