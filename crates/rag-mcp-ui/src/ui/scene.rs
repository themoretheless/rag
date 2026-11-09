//! Per-graph draw data derived once from a [`UiGraph`] and its layout.
//!
//! The canvas and the detail panel run every frame; everything here depends
//! only on the graph and the node positions, so it is built when either
//! changes and then read by index.

use std::collections::{BTreeMap, HashMap};

use egui::{Color32, Pos2};
use rag_mcp_core::{NodeKind, RelType};

use crate::adapter::{UiEdge, UiGraph};
use crate::layout::{bounds, PosCache};
use crate::ui::canvas::{localized_kind, localized_layer, localized_relation};
use crate::ui::theme;

/// Draw data for one (`UiGraph`, layout) pair.
#[derive(Debug, Clone, Default)]
pub struct GraphScene {
    /// World position per node, indexed like `UiGraph::nodes`; `None` while a
    /// node has not been placed.
    pub positions: Vec<Option<Pos2>>,
    /// World-space bounding box of every position the layout holds (the fit
    /// target), `None` for an empty layout.
    pub bounds: Option<(Pos2, Pos2)>,
    /// Legend rows for the node kinds / document layers present, by label.
    pub legend_kinds: Vec<(String, Color32)>,
    /// Legend rows for the relations present, by wire name.
    pub legend_rels: Vec<(String, Color32)>,
    /// Per node: the largest multi-edge count among its incident edges (1 when
    /// none is collapsed); shown in the hover tooltip.
    pub max_multi: Vec<u32>,
    /// Per node: indices into `UiGraph::edges` of its incident edges in
    /// detail-panel order (outgoing first, then by the other endpoint,
    /// relation and edge id).
    pub incident: Vec<Vec<usize>>,
    index_of: HashMap<String, usize>,
}

impl GraphScene {
    /// Index the layout by node position in `graph.nodes`.
    pub fn build(graph: &UiGraph, layout: &PosCache) -> Self {
        Self {
            positions: graph
                .nodes
                .iter()
                .map(|node| layout.get(&node.id).copied())
                .collect(),
            bounds: bounds(layout),
            legend_kinds: legend_kinds(graph),
            legend_rels: legend_rels(graph),
            max_multi: max_multi(graph),
            incident: incident_edges(graph),
            index_of: graph
                .nodes
                .iter()
                .enumerate()
                .map(|(index, node)| (node.id.clone(), index))
                .collect(),
        }
    }

    /// Position of the node with this id in `UiGraph::nodes`.
    pub fn node_index(&self, id: &str) -> Option<usize> {
        self.index_of.get(id).copied()
    }
}

fn legend_kinds(graph: &UiGraph) -> Vec<(String, Color32)> {
    // The first node carrying a label decides its colour.
    let mut kinds: BTreeMap<&str, Color32> = BTreeMap::new();
    for node in &graph.nodes {
        let label = if node.node_kind == Some(NodeKind::Document) {
            localized_layer(node.layer.as_deref().unwrap_or("document"))
        } else {
            localized_kind(&node.kind)
        };
        kinds
            .entry(label)
            .or_insert_with(|| theme::node_color(node.node_kind, node.layer.as_deref()));
    }
    kinds
        .into_iter()
        .map(|(label, color)| (label.to_string(), color))
        .collect()
}

fn legend_rels(graph: &UiGraph) -> Vec<(String, Color32)> {
    let rels: BTreeMap<&str, Option<RelType>> = graph
        .edges
        .iter()
        .map(|edge| (edge.rel_type.as_str(), edge.rel))
        .collect();
    rels.into_iter()
        .map(|(wire, rel)| (localized_relation(wire).to_string(), theme::edge_color(rel)))
        .collect()
}

fn max_multi(graph: &UiGraph) -> Vec<u32> {
    let mut max = vec![1_u32; graph.nodes.len()];
    for edge in &graph.edges {
        for index in [edge.source_idx, edge.target_idx] {
            max[index] = max[index].max(edge.multi_count);
        }
    }
    max
}

fn incident_edges(graph: &UiGraph) -> Vec<Vec<usize>> {
    let mut incident = vec![Vec::new(); graph.nodes.len()];
    for (index, edge) in graph.edges.iter().enumerate() {
        incident[edge.source_idx].push(index);
        if edge.target_idx != edge.source_idx {
            incident[edge.target_idx].push(index);
        }
    }
    for (node, edges) in incident.iter_mut().enumerate() {
        // Stable order: outs first, then ins; then by other id / rel_type.
        let key = |edge: &UiEdge| {
            let outgoing = edge.source_idx == node;
            let other = if outgoing {
                edge.target_id.as_str()
            } else {
                edge.source_id.as_str()
            };
            (
                !outgoing,
                other.to_owned(),
                edge.rel_type.clone(),
                edge.id.clone(),
            )
        };
        edges.sort_by_cached_key(|&index| key(&graph.edges[index]));
    }
    incident
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{adapt, AdaptOptions};
    use crate::bench::synthetic_view;
    use crate::layout::overview_grid_place;

    #[test]
    fn positions_follow_node_order_and_edges_index_their_endpoints() {
        let graph = adapt(
            &synthetic_view(60, 200),
            &AdaptOptions {
                show_tags: true,
                pkb_rels_only: false,
                show_all_nodes: true,
                ..AdaptOptions::default()
            },
        );
        let mut layout = PosCache::default();
        overview_grid_place(&graph, &mut layout);
        let scene = GraphScene::build(&graph, &layout);

        assert_eq!(scene.positions.len(), graph.nodes.len());
        for (index, node) in graph.nodes.iter().enumerate() {
            assert_eq!(scene.positions[index], layout.get(&node.id).copied());
        }
        assert!(!graph.edges.is_empty());
        for edge in &graph.edges {
            assert_eq!(graph.nodes[edge.source_idx].id, edge.source_id);
            assert_eq!(graph.nodes[edge.target_idx].id, edge.target_id);
        }
        let (min, max) = scene.bounds.expect("placed graph has bounds");
        assert!(scene
            .positions
            .iter()
            .flatten()
            .all(|p| p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y));
    }

    #[test]
    fn derived_rows_match_a_direct_scan_of_the_graph() {
        let graph = adapt(
            &synthetic_view(80, 400),
            &AdaptOptions {
                show_tags: true,
                pkb_rels_only: false,
                show_all_nodes: true,
                ..AdaptOptions::default()
            },
        );
        let scene = GraphScene::build(&graph, &PosCache::default());

        for (index, node) in graph.nodes.iter().enumerate() {
            assert_eq!(scene.node_index(&node.id), Some(index));
            let touching = || {
                graph
                    .edges
                    .iter()
                    .filter(|edge| edge.source_id == node.id || edge.target_id == node.id)
            };
            // Hover tooltip: largest collapsed multiplicity, 1 when none.
            let expected_multi = touching()
                .filter(|edge| edge.multi_count > 1)
                .map(|edge| edge.multi_count)
                .max()
                .unwrap_or(1);
            assert_eq!(scene.max_multi[index], expected_multi);

            // Detail panel: outgoing first, then by other endpoint, relation, id.
            let mut expected: Vec<&UiEdge> = touching().collect();
            expected.sort_by(|a, b| {
                let (a_out, b_out) = (a.source_id == node.id, b.source_id == node.id);
                let other = |edge: &'_ UiEdge, outgoing: bool| {
                    if outgoing {
                        edge.target_id.clone()
                    } else {
                        edge.source_id.clone()
                    }
                };
                b_out
                    .cmp(&a_out)
                    .then_with(|| other(a, a_out).cmp(&other(b, b_out)))
                    .then_with(|| a.rel_type.cmp(&b.rel_type))
                    .then_with(|| a.id.cmp(&b.id))
            });
            let listed: Vec<&str> = scene.incident[index]
                .iter()
                .map(|&edge| graph.edges[edge].id.as_str())
                .collect();
            let wanted: Vec<&str> = expected.iter().map(|edge| edge.id.as_str()).collect();
            assert_eq!(listed, wanted);
        }

        let kinds: Vec<&str> = scene
            .legend_kinds
            .iter()
            .map(|row| row.0.as_str())
            .collect();
        assert_eq!(
            kinds,
            ["вики", "заглушка", "сущность", "сырой корпус", "тег"]
        );
        let mut rels: Vec<&str> = graph.edges.iter().map(|e| e.rel_type.as_str()).collect();
        rels.sort_unstable();
        rels.dedup();
        assert_eq!(scene.legend_rels.len(), rels.len());
        assert_eq!(scene.node_index("missing"), None);
    }

    #[test]
    fn unplaced_nodes_stay_unpositioned() {
        let graph = adapt(&synthetic_view(5, 4), &AdaptOptions::default());
        let scene = GraphScene::build(&graph, &PosCache::default());
        assert!(scene.positions.iter().all(Option::is_none));
        assert!(scene.bounds.is_none());
    }
}
