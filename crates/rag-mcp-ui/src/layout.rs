//! RadialLocal layout (MVP only). Positions keyed by graph node id string.
//!
//! Algorithm (EGUI_GRAPH_VIEW §5.1 + task contract):
//! 1. Seed at origin `(0, 0)`.
//! 2. Undirected BFS depth rings from seed using UiGraph edges.
//! 3. Radius `r = depth * RING_GAP`.
//! 4. On each ring: sort by **node id** (deterministic), equal angles.
//! 5. Write positions once and freeze (no continuous force).

use crate::adapter::{UiGraph, UiNode};
use egui::{Pos2, Rect, Vec2};
use std::collections::{HashMap, HashSet, VecDeque};
use std::f32::consts::TAU;

/// Positions keyed by stable graph node id, never petgraph::NodeIndex.
pub type PosCache = HashMap<String, Pos2>;

/// Ring gap in screen units (pre-zoom).
pub const RING_GAP: f32 = 140.0;

/// Deterministic radial placement from seed. Writes into `cache`, freezes after place.
///
/// - Seed at center.
/// - Neighbors on concentric rings by undirected BFS depth.
/// - Angular order on each ring sorted by node id (stable, deterministic).
/// - Unreachable nodes land on one extra outer ring, also id-sorted.
pub fn radial_place(graph: &UiGraph, seed_id: Option<&str>, cache: &mut PosCache) {
    cache.clear();
    if graph.nodes.is_empty() {
        return;
    }

    let node_ids: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    let by_id: HashMap<&str, &UiNode> = graph
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n))
        .collect();

    let seed = resolve_seed(graph, seed_id, &by_id);

    // Undirected adjacency; neighbor lists sorted by id for deterministic BFS.
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in &graph.edges {
        let s = e.source_id.as_str();
        let t = e.target_id.as_str();
        if !node_ids.contains(s) || !node_ids.contains(t) {
            continue;
        }
        if s == t {
            continue;
        }
        adj.entry(s).or_default().push(t);
        adj.entry(t).or_default().push(s);
    }
    for neis in adj.values_mut() {
        neis.sort_unstable();
        neis.dedup();
    }

    // BFS: depth rings. Frontier expands in id-sorted neighbor order.
    let mut depth_of: HashMap<&str, u32> = HashMap::new();
    let mut rings: HashMap<u32, Vec<&str>> = HashMap::new();
    let mut q = VecDeque::new();

    depth_of.insert(seed, 0);
    rings.entry(0).or_default().push(seed);
    q.push_back(seed);

    while let Some(id) = q.pop_front() {
        let d = depth_of[&id];
        let Some(neis) = adj.get(id) else {
            continue;
        };
        for &n in neis {
            if depth_of.contains_key(n) {
                continue;
            }
            depth_of.insert(n, d + 1);
            rings.entry(d + 1).or_default().push(n);
            q.push_back(n);
        }
    }

    // Unreachable: one outer ring beyond max BFS depth.
    let max_depth = rings.keys().copied().max().unwrap_or(0);
    let mut unreachable: Vec<&str> = graph
        .nodes
        .iter()
        .map(|n| n.id.as_str())
        .filter(|id| !depth_of.contains_key(id))
        .collect();
    if !unreachable.is_empty() {
        unreachable.sort_unstable();
        rings.insert(max_depth + 1, unreachable);
    }

    // Place each ring: depth 0 seed at center; others equal-angle by sorted id.
    let mut depths: Vec<u32> = rings.keys().copied().collect();
    depths.sort_unstable();

    for depth in depths {
        let mut ring = rings.remove(&depth).unwrap_or_default();
        ring.sort_unstable();

        if depth == 0 {
            // Seed (and any other depth-0 anomaly) at / near center.
            for (i, id) in ring.iter().enumerate() {
                let pos = if *id == seed {
                    Pos2::new(0.0, 0.0)
                } else {
                    // Should not happen for a single seed BFS; keep deterministic offset.
                    Pos2::new(20.0 * (i as f32), 0.0)
                };
                cache.insert((*id).to_string(), pos);
            }
            continue;
        }

        let count = ring.len().max(1) as f32;
        let r = depth as f32 * RING_GAP;
        for (i, id) in ring.iter().enumerate() {
            let angle = TAU * (i as f32) / count;
            cache.insert(
                (*id).to_string(),
                Pos2::new(r * angle.cos(), r * angle.sin()),
            );
        }
    }

    // Guarantee every node has a frozen position.
    for n in &graph.nodes {
        cache.entry(n.id.clone()).or_insert(Pos2::ZERO);
    }
}

fn resolve_seed<'a>(
    graph: &'a UiGraph,
    seed_id: Option<&'a str>,
    by_id: &HashMap<&'a str, &'a UiNode>,
) -> &'a str {
    if let Some(s) = seed_id {
        if by_id.contains_key(s) {
            return s;
        }
    }
    // Prefer depth==0 from adapter if present; else lexicographically first id.
    if let Some(n) = graph.nodes.iter().find(|n| n.depth == 0) {
        return n.id.as_str();
    }
    let mut ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    ids.sort_unstable();
    ids[0]
}

/// Place only nodes missing from `cache` (Expand path). Existing positions stay put.
///
/// New nodes go at the centroid of already-placed undirected neighbors, with a small
/// deterministic offset from node id so they do not stack. If no placed neighbor
/// exists, fall back to an outer ring beyond the max existing radius (id-sorted).
///
/// Does **not** reseed or clear the cache (EGUI_GRAPH_VIEW §5.1: expand keeps layout).
pub fn place_missing_near_neighbors(graph: &UiGraph, cache: &mut PosCache) {
    if graph.nodes.is_empty() {
        return;
    }

    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in &graph.edges {
        let s = e.source_id.as_str();
        let t = e.target_id.as_str();
        if s == t {
            continue;
        }
        adj.entry(s).or_default().push(t);
        adj.entry(t).or_default().push(s);
    }
    for neis in adj.values_mut() {
        neis.sort_unstable();
        neis.dedup();
    }

    let mut missing: Vec<&str> = graph
        .nodes
        .iter()
        .map(|n| n.id.as_str())
        .filter(|id| !cache.contains_key(*id))
        .collect();
    missing.sort_unstable();

    if missing.is_empty() {
        return;
    }

    // Outer-ring fallback radius from current placed nodes.
    let mut max_r = RING_GAP;
    for p in cache.values() {
        let r = (p.x * p.x + p.y * p.y).sqrt();
        if r > max_r {
            max_r = r;
        }
    }
    let outer_r = max_r + RING_GAP;

    // Multi-pass: later missing nodes may use earlier missing placements as anchors.
    for _ in 0..3 {
        let mut progress = false;
        for id in &missing {
            if cache.contains_key(*id) {
                continue;
            }
            let mut sx = 0.0_f32;
            let mut sy = 0.0_f32;
            let mut n = 0_u32;
            if let Some(neis) = adj.get(id) {
                for &nb in neis {
                    if let Some(p) = cache.get(nb) {
                        sx += p.x;
                        sy += p.y;
                        n += 1;
                    }
                }
            }
            if n > 0 {
                let cx = sx / n as f32;
                let cy = sy / n as f32;
                // Deterministic micro-offset so multi-newcomers at same centroid separate.
                let h = stable_unit_angle(id);
                let offset = 28.0;
                cache.insert(
                    (*id).to_string(),
                    Pos2::new(cx + offset * h.cos(), cy + offset * h.sin()),
                );
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }

    // Still missing: outer ring, equal angle by id order among remaining.
    let mut still: Vec<&str> = missing
        .iter()
        .copied()
        .filter(|id| !cache.contains_key(*id))
        .collect();
    still.sort_unstable();
    let count = still.len().max(1) as f32;
    for (i, id) in still.iter().enumerate() {
        let angle = TAU * (i as f32) / count;
        cache.insert(
            (*id).to_string(),
            Pos2::new(outer_r * angle.cos(), outer_r * angle.sin()),
        );
    }

    // Drop positions for nodes no longer in the graph (filter shrink).
    let live: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    cache.retain(|id, _| live.contains(id.as_str()));
}

fn stable_unit_angle(id: &str) -> f32 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    id.hash(&mut h);
    let bits = h.finish();
    (bits as f32 / u64::MAX as f32) * TAU
}

/// One-shot fit: pan/zoom so graph bounds fill `rect` (anti-thrash: call once after place).
pub fn fit_transform(positions: &PosCache, rect: Rect) -> (Vec2, f32) {
    if positions.is_empty() {
        return (rect.center().to_vec2(), 1.0);
    }
    let mut min = Pos2::new(f32::MAX, f32::MAX);
    let mut max = Pos2::new(f32::MIN, f32::MIN);
    for p in positions.values() {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    let size = max - min;
    let pad = 48.0_f32;
    let avail = (rect.size() - Vec2::splat(pad * 2.0)).max(Vec2::splat(1.0));
    let sx = if size.x > 1.0 {
        avail.x / size.x
    } else {
        1.0
    };
    let sy = if size.y > 1.0 {
        avail.y / size.y
    } else {
        1.0
    };
    let zoom = sx.min(sy).clamp(0.05, 8.0);
    let center_graph = Pos2::new((min.x + max.x) * 0.5, (min.y + max.y) * 0.5);
    // screen = p * zoom + pan  =>  pan = rect.center - center_graph * zoom
    let pan = rect.center().to_vec2() - center_graph.to_vec2() * zoom;
    (pan, zoom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{UiEdge, UiGraph, UiNode};

    fn node(id: &str, depth: u32) -> UiNode {
        UiNode {
            id: id.into(),
            kind: "document".into(),
            label: id.into(),
            color: egui::Color32::from_rgb(70, 130, 200),
            document_id: None,
            uri: None,
            resolved: true,
            layer: None,
            wing: None,
            room: None,
            depth,
            degree: 0,
            pinned: false,
        }
    }

    fn edge(id: &str, s: &str, t: &str) -> UiEdge {
        UiEdge {
            id: id.into(),
            source_id: s.into(),
            target_id: t.into(),
            rel_type: "wikilink".into(),
            weight: 1.0,
            origin: "extract".into(),
            multi_count: 1,
            members: vec![],
        }
    }

    #[test]
    fn seed_at_center_neighbors_on_rings() {
        let g = UiGraph {
            nodes: vec![
                node("c", 1),
                node("b", 1),
                node("a", 0),
                node("d", 2),
            ],
            edges: vec![
                edge("e1", "a", "c"),
                edge("e2", "a", "b"),
                edge("e3", "b", "d"),
            ],
            truncated_nodes: false,
            truncated_edges: false,
            raw_nodes: 4,
            raw_edges: 3,
            note: None,
        };
        let mut cache = PosCache::default();
        radial_place(&g, Some("a"), &mut cache);

        assert_eq!(cache.get("a"), Some(&Pos2::new(0.0, 0.0)));
        // depth-1: b,c sorted by id → b then c
        let pb = *cache.get("b").unwrap();
        let pc = *cache.get("c").unwrap();
        let r1 = RING_GAP;
        let len = |p: Pos2| (p.x * p.x + p.y * p.y).sqrt();
        assert!((len(pb) - r1).abs() < 1e-3);
        assert!((len(pc) - r1).abs() < 1e-3);
        // b is index 0 → angle 0
        assert!((pb.x - r1).abs() < 1e-3 && pb.y.abs() < 1e-3);
        // d is depth 2 via b
        let pd = *cache.get("d").unwrap();
        assert!((len(pd) - 2.0 * RING_GAP).abs() < 1e-3);
    }

    #[test]
    fn deterministic_by_node_id() {
        let g = UiGraph {
            nodes: vec![node("z", 0), node("m", 1), node("a", 1)],
            edges: vec![edge("1", "z", "m"), edge("2", "z", "a")],
            truncated_nodes: false,
            truncated_edges: false,
            raw_nodes: 3,
            raw_edges: 2,
            note: None,
        };
        let mut c1 = PosCache::default();
        let mut c2 = PosCache::default();
        radial_place(&g, Some("z"), &mut c1);
        radial_place(&g, Some("z"), &mut c2);
        assert_eq!(c1, c2);
        // ring order a before m → a at angle 0
        let pa = *c1.get("a").unwrap();
        assert!((pa.x - RING_GAP).abs() < 1e-3 && pa.y.abs() < 1e-3);
    }

    #[test]
    fn place_missing_keeps_existing_and_anchors_new() {
        let g0 = UiGraph {
            nodes: vec![node("a", 0), node("b", 1)],
            edges: vec![edge("e1", "a", "b")],
            truncated_nodes: false,
            truncated_edges: false,
            raw_nodes: 2,
            raw_edges: 1,
            note: None,
        };
        let mut cache = PosCache::default();
        radial_place(&g0, Some("a"), &mut cache);
        let a0 = *cache.get("a").unwrap();
        let b0 = *cache.get("b").unwrap();

        let g1 = UiGraph {
            nodes: vec![node("a", 0), node("b", 1), node("c", 2)],
            edges: vec![edge("e1", "a", "b"), edge("e2", "b", "c")],
            truncated_nodes: false,
            truncated_edges: false,
            raw_nodes: 3,
            raw_edges: 2,
            note: None,
        };
        place_missing_near_neighbors(&g1, &mut cache);
        assert_eq!(cache.get("a"), Some(&a0));
        assert_eq!(cache.get("b"), Some(&b0));
        assert!(cache.contains_key("c"));
        // c near b (centroid of placed neighbor)
        let pc = *cache.get("c").unwrap();
        let dist_b = ((pc.x - b0.x).powi(2) + (pc.y - b0.y).powi(2)).sqrt();
        assert!(dist_b < 50.0, "c should sit near b, dist={dist_b}");
    }

    #[test]
    fn empty_graph_clears_cache() {
        let g = UiGraph {
            nodes: vec![],
            edges: vec![],
            truncated_nodes: false,
            truncated_edges: false,
            raw_nodes: 0,
            raw_edges: 0,
            note: None,
        };
        let mut cache = PosCache::default();
        cache.insert("stale".into(), Pos2::new(1.0, 1.0));
        radial_place(&g, Some("x"), &mut cache);
        assert!(cache.is_empty());
    }

    #[test]
    fn every_node_gets_position_including_unreachable() {
        let g = UiGraph {
            nodes: vec![node("seed", 0), node("near", 1), node("isle", 0)],
            edges: vec![edge("e1", "seed", "near")],
            truncated_nodes: false,
            truncated_edges: false,
            raw_nodes: 3,
            raw_edges: 1,
            note: None,
        };
        let mut cache = PosCache::default();
        radial_place(&g, Some("seed"), &mut cache);
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.get("seed"), Some(&Pos2::ZERO));
        let near = *cache.get("near").unwrap();
        let isle = *cache.get("isle").unwrap();
        let len = |p: Pos2| (p.x * p.x + p.y * p.y).sqrt();
        assert!((len(near) - RING_GAP).abs() < 1e-3);
        // Unreachable lands on outer ring past max BFS depth (depth 1 → outer at 2).
        assert!((len(isle) - 2.0 * RING_GAP).abs() < 1e-3);
    }

    #[test]
    fn invalid_seed_falls_back_to_depth_zero() {
        let g = UiGraph {
            nodes: vec![node("z", 1), node("a", 0), node("m", 1)],
            edges: vec![edge("1", "a", "z"), edge("2", "a", "m")],
            truncated_nodes: false,
            truncated_edges: false,
            raw_nodes: 3,
            raw_edges: 2,
            note: None,
        };
        let mut cache = PosCache::default();
        radial_place(&g, Some("missing"), &mut cache);
        assert_eq!(cache.get("a"), Some(&Pos2::ZERO));
    }

    #[test]
    fn self_loops_ignored_equal_angle_neighbors() {
        let g = UiGraph {
            nodes: vec![node("s", 0), node("a", 1), node("b", 1)],
            edges: vec![
                edge("loop", "s", "s"),
                edge("e1", "s", "a"),
                edge("e2", "s", "b"),
            ],
            truncated_nodes: false,
            truncated_edges: false,
            raw_nodes: 3,
            raw_edges: 3,
            note: None,
        };
        let mut cache = PosCache::default();
        radial_place(&g, Some("s"), &mut cache);
        assert_eq!(cache.get("s"), Some(&Pos2::ZERO));
        // id order a,b → equal angles on ring 1
        let pa = *cache.get("a").unwrap();
        let pb = *cache.get("b").unwrap();
        assert!((pa.x - RING_GAP).abs() < 1e-3 && pa.y.abs() < 1e-3);
        assert!((pb.x + RING_GAP).abs() < 1e-3 && pb.y.abs() < 1e-3);
    }

    #[test]
    fn fit_transform_centers_and_scales() {
        let mut positions = PosCache::default();
        positions.insert("a".into(), Pos2::new(-100.0, 0.0));
        positions.insert("b".into(), Pos2::new(100.0, 0.0));
        let rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(400.0, 400.0));
        let (pan, zoom) = fit_transform(&positions, rect);
        assert!(zoom > 0.0 && zoom <= 8.0);
        // Graph center (0,0): screen = p * zoom + pan → pan ≈ rect.center.
        let center = rect.center();
        assert!((pan.x - center.x).abs() < 1e-2);
        assert!((pan.y - center.y).abs() < 1e-2);
    }

    #[test]
    fn radial_place_is_idempotent_freeze() {
        let g = UiGraph {
            nodes: vec![node("s", 0), node("n", 1)],
            edges: vec![edge("e", "s", "n")],
            truncated_nodes: false,
            truncated_edges: false,
            raw_nodes: 2,
            raw_edges: 1,
            note: None,
        };
        let mut cache = PosCache::default();
        radial_place(&g, Some("s"), &mut cache);
        let first = cache.clone();
        radial_place(&g, Some("s"), &mut cache);
        assert_eq!(cache, first);
    }
}
