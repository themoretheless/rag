//! Per-graph draw data derived once from a [`UiGraph`] and its layout.
//!
//! The canvas and the detail panel run every frame; everything here depends
//! only on the graph and the node positions, so it is built when either
//! changes and then read by index.

use std::collections::{BTreeMap, HashMap};

use egui::{Color32, Pos2, Rect};
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
    /// Placed nodes bucketed by world position, for culling and hit-testing.
    pub grid: NodeGrid,
    index_of: HashMap<String, usize>,
}

/// Uniform world-space grid over the placed nodes.
///
/// A query returns every node whose cell overlaps a rectangle — a superset of
/// the nodes inside it — so callers still apply their exact test, but only to
/// the nodes near the viewport or the pointer instead of to the whole graph.
#[derive(Debug, Clone, Default)]
pub struct NodeGrid {
    origin: Pos2,
    cell: f32,
    cols: usize,
    rows: usize,
    /// `starts[c]..starts[c + 1]` is cell `c`'s slice of `items`.
    starts: Vec<u32>,
    /// Node indices grouped by cell, ascending within a cell.
    items: Vec<u32>,
}

impl NodeGrid {
    /// Target number of nodes per cell.
    const NODES_PER_CELL: f32 = 4.0;

    /// Bucket every placed node (`None` positions are left out).
    pub fn build(positions: &[Option<Pos2>]) -> Self {
        let placed = positions.iter().flatten().count();
        if placed == 0 {
            return Self::default();
        }
        let mut min = Pos2::new(f32::MAX, f32::MAX);
        let mut max = Pos2::new(f32::MIN, f32::MIN);
        for p in positions.iter().flatten() {
            min = min.min(*p);
            max = max.max(*p);
        }
        let size = max - min;
        let area = (size.x.max(1.0) * size.y.max(1.0)).max(1.0);
        let cell = (area * Self::NODES_PER_CELL / placed as f32)
            .sqrt()
            .max(1.0);
        let cols = ((size.x / cell).floor() as usize + 1).min(4_096);
        let rows = ((size.y / cell).floor() as usize + 1).min(4_096);
        let mut grid = Self {
            origin: min,
            cell,
            cols,
            rows,
            starts: vec![0; cols * rows + 1],
            items: vec![0; placed],
        };
        // Counting sort by cell keeps node order inside each cell.
        for p in positions.iter().flatten() {
            let cell = grid.cell_of(*p);
            grid.starts[cell + 1] += 1;
        }
        for cell in 0..cols * rows {
            grid.starts[cell + 1] += grid.starts[cell];
        }
        let mut next = grid.starts.clone();
        for (index, p) in positions.iter().enumerate() {
            if let Some(p) = p {
                let cell = grid.cell_of(*p);
                grid.items[next[cell] as usize] = index as u32;
                next[cell] += 1;
            }
        }
        grid
    }

    fn column(&self, x: f32) -> usize {
        (((x - self.origin.x) / self.cell).floor().max(0.0) as usize).min(self.cols - 1)
    }

    fn row(&self, y: f32) -> usize {
        (((y - self.origin.y) / self.cell).floor().max(0.0) as usize).min(self.rows - 1)
    }

    fn cell_of(&self, p: Pos2) -> usize {
        self.row(p.y) * self.cols + self.column(p.x)
    }

    /// Append to `out`, in ascending order, the index of every node whose
    /// cell overlaps `world`. Returns false (and leaves `out` alone) when that
    /// is every placed node, so the caller can simply walk all nodes.
    pub fn query(&self, world: Rect, out: &mut Vec<usize>) -> bool {
        if self.items.is_empty() {
            return true;
        }
        let extent = Pos2::new(
            self.origin.x + self.cell * self.cols as f32,
            self.origin.y + self.cell * self.rows as f32,
        );
        if world.max.x < self.origin.x
            || world.max.y < self.origin.y
            || world.min.x > extent.x
            || world.min.y > extent.y
        {
            return true;
        }
        let (c0, c1) = (self.column(world.min.x), self.column(world.max.x));
        let (r0, r1) = (self.row(world.min.y), self.row(world.max.y));
        if c0 == 0 && r0 == 0 && c1 == self.cols - 1 && r1 == self.rows - 1 {
            return false;
        }
        let before = out.len();
        for row in r0..=r1 {
            let first = self.starts[row * self.cols + c0] as usize;
            let last = self.starts[row * self.cols + c1 + 1] as usize;
            out.extend(self.items[first..last].iter().map(|&index| index as usize));
        }
        out[before..].sort_unstable();
        true
    }
}

impl GraphScene {
    /// Index the layout by node position in `graph.nodes`.
    pub fn build(graph: &UiGraph, layout: &PosCache) -> Self {
        let positions: Vec<Option<Pos2>> = graph
            .nodes
            .iter()
            .map(|node| layout.get(&node.id).copied())
            .collect();
        Self {
            bounds: bounds(layout),
            legend_kinds: legend_kinds(graph),
            legend_rels: legend_rels(graph),
            max_multi: max_multi(graph),
            incident: incident_edges(graph),
            grid: NodeGrid::build(&positions),
            index_of: graph
                .nodes
                .iter()
                .enumerate()
                .map(|(index, node)| (node.id.clone(), index))
                .collect(),
            positions,
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

    /// Deterministic scatter with an unplaced node every 17th index.
    fn scattered(count: usize) -> Vec<Option<Pos2>> {
        let mut state = 0x9E37_79B9_u32;
        let mut next = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / (1 << 24) as f32
        };
        (0..count)
            .map(|index| {
                let p = Pos2::new(next() * 4_000.0 - 1_000.0, next() * 2_500.0 - 300.0);
                (index % 17 != 3).then_some(p)
            })
            .collect()
    }

    fn query(grid: &NodeGrid, world: Rect, total: usize) -> Vec<usize> {
        let mut out = Vec::new();
        if grid.query(world, &mut out) {
            out
        } else {
            (0..total).collect()
        }
    }

    #[test]
    fn grid_query_returns_every_node_inside_the_rectangle_in_order() {
        let positions = scattered(2_000);
        let grid = NodeGrid::build(&positions);
        let rects = [
            Rect::from_min_max(Pos2::new(-200.0, 100.0), Pos2::new(350.0, 420.0)),
            Rect::from_min_max(Pos2::new(1_500.0, 900.0), Pos2::new(1_520.0, 910.0)),
            Rect::from_min_max(Pos2::new(-5_000.0, -5_000.0), Pos2::new(-900.0, 0.0)),
            Rect::from_min_max(Pos2::new(2_900.0, 2_100.0), Pos2::new(9_000.0, 9_000.0)),
            Rect::from_min_max(Pos2::new(-9_000.0, -9_000.0), Pos2::new(9_000.0, 9_000.0)),
            Rect::from_min_max(Pos2::new(8_000.0, 8_000.0), Pos2::new(9_000.0, 9_000.0)),
        ];
        let mut narrowed = false;
        for rect in rects {
            let found = query(&grid, rect, positions.len());
            assert!(
                found.windows(2).all(|pair| pair[0] < pair[1]),
                "ascending, unique"
            );
            for (index, p) in positions.iter().enumerate() {
                if p.is_some_and(|p| rect.contains(p)) {
                    assert!(
                        found.contains(&index),
                        "node {index} inside {rect:?} was dropped"
                    );
                }
            }
            // Unplaced nodes are never reported by a narrowing query.
            if found.len() < positions.len() {
                narrowed = true;
                assert!(found.iter().all(|&index| positions[index].is_some()));
            }
        }
        assert!(narrowed, "at least one query must cull");
        // A small viewport touches a small share of the graph.
        let small = query(&grid, rects[1], positions.len());
        assert!(small.len() < 60, "{} candidates", small.len());
    }

    #[test]
    fn grid_handles_empty_and_degenerate_layouts() {
        let empty = NodeGrid::build(&[None, None]);
        assert!(query(&empty, Rect::EVERYTHING, 2).is_empty());

        // Every node on one point: one cell, still found.
        let stacked = vec![Some(Pos2::new(5.0, 5.0)); 9];
        let grid = NodeGrid::build(&stacked);
        let around = Rect::from_center_size(Pos2::new(5.0, 5.0), egui::Vec2::splat(2.0));
        assert_eq!(query(&grid, around, 9).len(), 9);
        let away = Rect::from_center_size(Pos2::new(500.0, 5.0), egui::Vec2::splat(2.0));
        assert!(query(&grid, away, 9).is_empty());

        // A horizontal line of nodes (zero height).
        let line: Vec<_> = (0..100)
            .map(|i| Some(Pos2::new(i as f32 * 10.0, 0.0)))
            .collect();
        let grid = NodeGrid::build(&line);
        let window = Rect::from_min_max(Pos2::new(95.0, -1.0), Pos2::new(205.0, 1.0));
        let found = query(&grid, window, 100);
        for index in 10..=20 {
            assert!(found.contains(&index));
        }
        assert!(found.len() < 40);
    }

    #[test]
    fn unplaced_nodes_stay_unpositioned() {
        let graph = adapt(&synthetic_view(5, 4), &AdaptOptions::default());
        let scene = GraphScene::build(&graph, &PosCache::default());
        assert!(scene.positions.iter().all(Option::is_none));
        assert!(scene.bounds.is_none());
    }
}
