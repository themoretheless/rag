//! Per-graph draw data derived once from a [`UiGraph`] and its layout.
//!
//! The canvas and the detail panel run every frame; everything here depends
//! only on the graph and the node positions, so it is built when either
//! changes and then read by index.

use egui::Pos2;

use crate::adapter::UiGraph;
use crate::layout::{bounds, PosCache};

/// Draw data for one (`UiGraph`, layout) pair.
#[derive(Debug, Clone, Default)]
pub struct GraphScene {
    /// World position per node, indexed like `UiGraph::nodes`; `None` while a
    /// node has not been placed.
    pub positions: Vec<Option<Pos2>>,
    /// World-space bounding box of every position the layout holds (the fit
    /// target), `None` for an empty layout.
    pub bounds: Option<(Pos2, Pos2)>,
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
        }
    }
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
    fn unplaced_nodes_stay_unpositioned() {
        let graph = adapt(&synthetic_view(5, 4), &AdaptOptions::default());
        let scene = GraphScene::build(&graph, &PosCache::default());
        assert!(scene.positions.iter().all(Option::is_none));
        assert!(scene.bounds.is_none());
    }
}
