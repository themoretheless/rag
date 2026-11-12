//! In-memory graph algorithms over `GraphView`: seed resolution, bounded
//! neighbourhoods and view merging. No IO.

use rag_mcp_core::{pkb_rel_types, GraphEdge, GraphNode, GraphView, UI_GRAPH_EXPORT_MAX_NODES};
use std::collections::{HashMap, HashSet, VecDeque};

/// Hard layout caps (EGUI_GRAPH_VIEW §8.1).
pub const UI_HARD_MAX_NODES: usize = 300;
pub const UI_MAX_DRAW_EDGES: usize = 2000;
/// Unseeded overview lenses draw at most this many nodes (the most connected
/// ones): the canvas switches to its simplified mode above it, and painting,
/// hit-testing and the detail panel visit every drawn element each frame.
pub const UI_OVERVIEW_MAX_NODES: usize = 5000;
/// Draw-edge cap for overview lenses (same priority order as the local cap).
pub const UI_OVERVIEW_MAX_EDGES: usize = 20_000;
pub const UI_LOCAL_MAX_NODES: u32 = UI_GRAPH_EXPORT_MAX_NODES;
pub const UI_DEFAULT_DEPTH: u32 = 1;

/// PKB default edge types for live Store load (EGUI_GRAPH_VIEW §7.1 / GRAPH_DESIGN §7.1).
/// Calls the library function so the UI can no longer drift from the server.
pub fn pkb_default_rel_types() -> Vec<String> {
    pkb_rel_types(false)
}

/// Resolve exact seed by id, document_id, then case-insensitive label.
pub fn resolve_exact_seed(view: &GraphView, query: &str) -> Option<String> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    if let Some(node) = view.nodes.iter().find(|node| node.id == q) {
        return Some(node.id.clone());
    }
    if let Some(node) = view
        .nodes
        .iter()
        .find(|node| node.document_id.as_deref() == Some(q))
    {
        return Some(node.id.clone());
    }
    let lower = q.to_lowercase();
    view.nodes
        .iter()
        .find(|node| node.label.to_lowercase() == lower)
        .map(|node| node.id.clone())
}

/// Resolve seed by id, document_id, exact label, then substring label.
pub fn resolve_seed(view: &GraphView, query: &str) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("empty seed".into());
    }
    if let Some(id) = resolve_exact_seed(view, q) {
        return Ok(id);
    }
    let lower = q.to_lowercase();
    let matches: Vec<&GraphNode> = view
        .nodes
        .iter()
        .filter(|n| n.label.to_lowercase().contains(&lower))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => Err(format!("No node matches “{q}”")),
        many => {
            let shown: Vec<String> = many
                .iter()
                .take(5)
                .map(|n| format!("{} ({})", n.label, n.id))
                .collect();
            let more = if many.len() > 5 { ", …" } else { "" };
            Err(format!(
                "ambiguous seed “{q}” ({} matches): {}{}",
                many.len(),
                shown.join(", "),
                more
            ))
        }
    }
}

/// Result of client-side BFS, including an exact signal that the node budget
/// omitted at least one otherwise reachable node.
#[derive(Debug, Clone)]
pub struct LocalNeighbors {
    pub view: GraphView,
    pub capped: bool,
}

/// Client-side undirected BFS neighbors on a loaded snapshot (Mode C expand).
pub fn local_neighbors_bounded(
    full: &GraphView,
    seed_id: &str,
    depth: u32,
    max_nodes: usize,
) -> LocalNeighbors {
    let max_nodes = max_nodes.max(1);
    let available: HashSet<&str> = full.nodes.iter().map(|node| node.id.as_str()).collect();
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for e in &full.edges {
        adj.entry(e.source_id.clone())
            .or_default()
            .push(e.target_id.clone());
        adj.entry(e.target_id.clone())
            .or_default()
            .push(e.source_id.clone());
    }

    let mut keep: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, u32)> = VecDeque::new();
    if full.nodes.iter().any(|n| n.id == seed_id) {
        queue.push_back((seed_id.to_string(), 0));
        keep.insert(seed_id.to_string());
    }

    let mut capped = false;
    while let Some((id, d)) = queue.pop_front() {
        if d >= depth {
            continue;
        }
        if let Some(neis) = adj.get(&id) {
            for n in neis {
                if !available.contains(n.as_str()) {
                    continue;
                }
                if keep.contains(n) {
                    continue;
                }
                if keep.len() >= max_nodes {
                    capped = true;
                    continue;
                }
                keep.insert(n.clone());
                queue.push_back((n.clone(), d + 1));
            }
        }
    }

    LocalNeighbors {
        view: subgraph_from_keep(full, &keep),
        capped,
    }
}

/// Compatibility helper for expansion paths that only need the subgraph.
pub fn local_neighbors(full: &GraphView, seed_id: &str, depth: u32, max_nodes: usize) -> GraphView {
    local_neighbors_bounded(full, seed_id, depth, max_nodes).view
}

/// Merge `extra` nodes into `base`, preferring existing nodes, capped at `max_nodes`.
///
/// Edges are taken from `edge_source` when provided (full loaded graph), otherwise
/// the union of base+extra edges restricted to the kept node set.
pub fn merge_graph_views(
    base: &GraphView,
    extra: &GraphView,
    edge_source: Option<&GraphView>,
    max_nodes: usize,
) -> GraphView {
    let max_nodes = max_nodes.max(1);
    let mut nodes = base.nodes.clone();
    let mut keep: HashSet<String> = nodes.iter().map(|n| n.id.clone()).collect();

    // Stable order for new nodes: by id.
    let mut newcomers: Vec<&GraphNode> = extra
        .nodes
        .iter()
        .filter(|n| !keep.contains(&n.id))
        .collect();
    newcomers.sort_by(|a, b| a.id.cmp(&b.id));
    for n in newcomers {
        if nodes.len() >= max_nodes {
            break;
        }
        keep.insert(n.id.clone());
        nodes.push(n.clone());
    }

    let edges = match edge_source {
        Some(full) => full
            .edges
            .iter()
            .filter(|e| keep.contains(&e.source_id) && keep.contains(&e.target_id))
            .cloned()
            .collect(),
        None => {
            let mut by_id: HashMap<String, GraphEdge> = HashMap::new();
            for e in base.edges.iter().chain(extra.edges.iter()) {
                if keep.contains(&e.source_id) && keep.contains(&e.target_id) {
                    by_id.entry(e.id.clone()).or_insert_with(|| e.clone());
                }
            }
            let mut edges: Vec<GraphEdge> = by_id.into_values().collect();
            edges.sort_by(|a, b| a.id.cmp(&b.id));
            edges
        }
    };

    GraphView { nodes, edges }
}

/// Expand neighbors of `selected_id` by one hop (depth + 1 from that node) on a
/// loaded topology, merge into `current`, cap at `max_nodes`.
///
/// Used for Mode C snapshot / vault json (and any path where `full` is the
/// complete in-memory graph).
pub fn expand_neighbors_local(
    full: &GraphView,
    current: &GraphView,
    selected_id: &str,
    max_nodes: usize,
) -> GraphView {
    let max_nodes = max_nodes.max(1);
    // One hop from selected; budget = full cap so merge can prefer existing.
    let extra = local_neighbors(full, selected_id, 1, max_nodes);
    merge_graph_views(current, &extra, Some(full), max_nodes)
}

fn subgraph_from_keep(full: &GraphView, keep: &HashSet<String>) -> GraphView {
    let nodes: Vec<GraphNode> = full
        .nodes
        .iter()
        .filter(|n| keep.contains(&n.id))
        .cloned()
        .collect();
    let edges: Vec<GraphEdge> = full
        .edges
        .iter()
        .filter(|e| keep.contains(&e.source_id) && keep.contains(&e.target_id))
        .cloned()
        .collect();
    GraphView { nodes, edges }
}

#[cfg(test)]
mod tests {
    use super::test_support::{edge, node};
    use super::*;

    #[test]
    fn pkb_ui_export_filter_defaults() {
        let f = rag_mcp_core::GraphFilter::pkb_ui_export(Some(UI_HARD_MAX_NODES as u32), false);
        let rels = f.rel_types.expect("rels");
        assert!(rels.iter().any(|r| r == "wikilink"));
        assert!(rels.iter().any(|r| r == "related"));
        assert_eq!(f.max_nodes, Some(UI_HARD_MAX_NODES as u32));
        let kinds = f.kinds.expect("kinds");
        assert!(kinds.iter().any(|k| k == "document"));
        assert!(!kinds.iter().any(|k| k == "tag"));

        let with_tags =
            rag_mcp_core::GraphFilter::pkb_ui_export(Some(UI_HARD_MAX_NODES as u32), true);
        assert!(with_tags
            .kinds
            .expect("kinds")
            .iter()
            .any(|kind| kind == "tag"));
        assert!(with_tags
            .rel_types
            .expect("rels")
            .iter()
            .any(|rel| rel == "tagged"));
    }

    #[test]
    fn local_neighbors_depth1() {
        let view = GraphView {
            nodes: vec![node("a", "A"), node("b", "B"), node("c", "C")],
            edges: vec![edge("e1", "a", "b"), edge("e2", "b", "c")],
        };
        let local = local_neighbors(&view, "a", 1, 100);
        let ids: HashSet<_> = local.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains("a"));
        assert!(ids.contains("b"));
        assert!(!ids.contains("c"));
    }

    #[test]
    fn local_neighbors_reports_only_actual_budget_omissions() {
        let complete = GraphView {
            nodes: vec![node("seed", "Seed"), node("only", "Only")],
            edges: vec![edge("e1", "seed", "only")],
        };
        let exact = local_neighbors_bounded(&complete, "seed", 1, 2);
        assert_eq!(exact.view.nodes.len(), 2);
        assert!(
            !exact.capped,
            "reaching the budget is not itself truncation"
        );

        let omitted = GraphView {
            nodes: vec![node("seed", "Seed"), node("one", "One"), node("two", "Two")],
            edges: vec![edge("e1", "seed", "one"), edge("e2", "seed", "two")],
        };
        let capped = local_neighbors_bounded(&omitted, "seed", 1, 2);
        assert_eq!(capped.view.nodes.len(), 2);
        assert!(capped.capped);
    }

    #[test]
    fn expand_neighbors_local_merges_one_hop() {
        // seed a depth-1: {a,b}; expand b → pulls c
        let full = GraphView {
            nodes: vec![
                node("a", "A"),
                node("b", "B"),
                node("c", "C"),
                node("d", "D"),
            ],
            edges: vec![
                edge("e1", "a", "b"),
                edge("e2", "b", "c"),
                edge("e3", "c", "d"),
            ],
        };
        let current = local_neighbors(&full, "a", 1, 100);
        assert_eq!(current.nodes.len(), 2);
        let expanded = expand_neighbors_local(&full, &current, "b", 100);
        let ids: HashSet<_> = expanded.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains("a"));
        assert!(ids.contains("b"));
        assert!(ids.contains("c"));
        assert!(!ids.contains("d"));
        assert!(expanded
            .edges
            .iter()
            .any(|e| e.source_id == "b" && e.target_id == "c"));
    }

    #[test]
    fn expand_neighbors_local_respects_max_nodes() {
        let mut nodes = vec![node("seed", "Seed"), node("hub", "Hub")];
        let mut edges = vec![edge("e0", "seed", "hub")];
        for i in 0..20 {
            let id = format!("n{i}");
            nodes.push(node(&id, &id));
            edges.push(edge(&format!("eh{i}"), "hub", &id));
        }
        let full = GraphView { nodes, edges };
        let current = local_neighbors(&full, "seed", 1, 100);
        // current = seed + hub
        assert_eq!(current.nodes.len(), 2);
        let expanded = expand_neighbors_local(&full, &current, "hub", 5);
        assert!(expanded.nodes.len() <= 5);
        assert!(expanded.nodes.iter().any(|n| n.id == "seed"));
        assert!(expanded.nodes.iter().any(|n| n.id == "hub"));
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use rag_mcp_core::{GraphEdge, GraphNode};

    pub(crate) fn node(id: &str, label: &str) -> GraphNode {
        GraphNode {
            id: id.into(),
            kind: "document".into(),
            label: label.into(),
            document_id: None,
            uri: None,
            resolved: true,
            metadata_json: "{}".into(),
        }
    }

    pub(crate) fn edge(id: &str, s: &str, t: &str) -> GraphEdge {
        GraphEdge {
            id: id.into(),
            source_id: s.into(),
            target_id: t.into(),
            rel_type: "wikilink".into(),
            weight: 1.0,
            context: None,
        }
    }
}
