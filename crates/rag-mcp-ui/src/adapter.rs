//! Domain GraphView → UI graph: filter, multi-edge collapse, style helpers, hard caps.

use rag_mcp::{GraphEdge, GraphNode, GraphView};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::load::{UI_HARD_MAX_NODES, UI_MAX_DRAW_EDGES};

/// Display node after clamp + style derivation.
#[derive(Debug, Clone)]
#[allow(dead_code)] // layer/wing/room/pinned reserved for post-MVP chrome
pub struct UiNode {
    pub id: String,
    pub kind: String,
    pub label: String,
    /// Fill color derived from `kind` (EGUI_GRAPH_VIEW §7.2).
    pub color: egui::Color32,
    pub document_id: Option<String>,
    pub uri: Option<String>,
    pub resolved: bool,
    pub layer: Option<String>,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub depth: u32,
    pub degree: u32,
    pub pinned: bool,
}

/// Canvas edge after multi-edge collapse by (source, target, rel_type).
#[derive(Debug, Clone)]
#[allow(dead_code)] // origin used by detail styling later
pub struct UiEdge {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub rel_type: String,
    pub weight: f64,
    pub origin: String,
    pub multi_count: u32,
    /// Full provenance for detail panel only (not drawn).
    pub members: Vec<GraphEdge>,
}

/// UI-ready topology under layout caps.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // raw_* for status banners / future chrome
pub struct UiGraph {
    pub nodes: Vec<UiNode>,
    pub edges: Vec<UiEdge>,
    pub truncated_nodes: bool,
    pub truncated_edges: bool,
    pub raw_nodes: usize,
    pub raw_edges: usize,
    /// Human-readable note when nodes/edges were dropped for caps.
    pub note: Option<String>,
}

/// Filter / style options for adapter.
#[derive(Debug, Clone)]
pub struct AdaptOptions {
    pub seed_id: Option<String>,
    pub show_tags: bool,
    pub show_stubs: bool,
    /// PKB defaults: only wikilink + related when true.
    pub pkb_rels_only: bool,
    pub wing: Option<String>,
    pub room: Option<String>,
}

impl Default for AdaptOptions {
    fn default() -> Self {
        Self {
            seed_id: None,
            show_tags: false,
            show_stubs: true,
            pkb_rels_only: true,
            wing: None,
            room: None,
        }
    }
}

/// Map domain topology → UiGraph (collapse, filter, depth, clamp).
///
/// Hard caps (code, not docs-only):
/// - `UI_HARD_MAX_NODES` (300): seed-BFS sample when seed present, else stable id truncate.
/// - `UI_MAX_DRAW_EDGES` (2000): after multi-edge collapse; drop tagged / lowest-weight first.
pub fn adapt(view: &GraphView, opts: &AdaptOptions) -> UiGraph {
    let raw_nodes = view.nodes.len();
    let raw_edges = view.edges.len();

    let kind_visible: Vec<&GraphNode> = view
        .nodes
        .iter()
        .filter(|n| match n.kind.as_str() {
            "tag" => opts.show_tags,
            "stub" => opts.show_stubs,
            _ => true,
        })
        .collect();

    // Project/room metadata belongs to placed documents. Companion graph nodes
    // (unresolved stubs, entities and tags) often have no placement of their
    // own, so keep them only when they are directly connected to a matching
    // document. Explicitly mismatched placement is never allowed through.
    let scope: HashMap<&str, ScopeDisposition> = kind_visible
        .iter()
        .map(|node| (node.id.as_str(), scope_disposition(node, opts)))
        .collect();
    let anchors: HashSet<&str> = kind_visible
        .iter()
        .filter(|node| {
            node.kind == "document" && scope.get(node.id.as_str()) == Some(&ScopeDisposition::Match)
        })
        .map(|node| node.id.as_str())
        .collect();
    let attached_companions: HashSet<&str> = view
        .edges
        .iter()
        .filter_map(|edge| {
            if anchors.contains(edge.source_id.as_str())
                && scope.get(edge.target_id.as_str()) == Some(&ScopeDisposition::Companion)
            {
                Some(edge.target_id.as_str())
            } else if anchors.contains(edge.target_id.as_str())
                && scope.get(edge.source_id.as_str()) == Some(&ScopeDisposition::Companion)
            {
                Some(edge.source_id.as_str())
            } else {
                None
            }
        })
        .collect();
    let filtered_nodes: Vec<&GraphNode> = kind_visible
        .into_iter()
        .filter(|node| match scope.get(node.id.as_str()) {
            Some(ScopeDisposition::Match) => true,
            Some(ScopeDisposition::Companion) => attached_companions.contains(node.id.as_str()),
            Some(ScopeDisposition::Excluded) | None => false,
        })
        .collect();

    let (kept_nodes, truncated_nodes) = clamp_nodes(&filtered_nodes, view, opts.seed_id.as_deref());

    let keep: HashSet<String> = kept_nodes.iter().map(|n| n.id.clone()).collect();

    let candidate_edges: Vec<&GraphEdge> = view
        .edges
        .iter()
        .filter(|e| keep.contains(&e.source_id) && keep.contains(&e.target_id))
        .filter(|e| {
            if !opts.pkb_rels_only {
                return true;
            }
            matches!(e.rel_type.as_str(), "wikilink" | "related")
                || (opts.show_tags && e.rel_type == "tagged")
        })
        .collect();

    let edge_owned: Vec<GraphEdge> = candidate_edges.iter().map(|e| (*e).clone()).collect();
    let mut ui_edges = collapse_edges(&edge_owned);
    let pre_cap_edge_count = ui_edges.len();
    let mut truncated_edges = false;
    if ui_edges.len() > UI_MAX_DRAW_EDGES {
        // Drop lowest-weight and tagged first; keep highest-weight wikilink/related.
        ui_edges.sort_by(|a, b| {
            let pa = edge_keep_priority(a);
            let pb = edge_keep_priority(b);
            pb.cmp(&pa)
                .then_with(|| {
                    b.weight
                        .partial_cmp(&a.weight)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.id.cmp(&b.id))
        });
        ui_edges.truncate(UI_MAX_DRAW_EDGES);
        truncated_edges = true;
    }

    // Undirected adjacency for depth / degree (post-collapse, post-cap).
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for e in &ui_edges {
        adj.entry(e.source_id.clone())
            .or_default()
            .push(e.target_id.clone());
        adj.entry(e.target_id.clone())
            .or_default()
            .push(e.source_id.clone());
    }

    let depth_map = bfs_depth(opts.seed_id.as_deref(), &keep, &adj);

    let mut degree: HashMap<String, u32> = HashMap::new();
    for e in &ui_edges {
        *degree.entry(e.source_id.clone()).or_default() += 1;
        *degree.entry(e.target_id.clone()).or_default() += 1;
    }

    let ui_nodes: Vec<UiNode> = kept_nodes
        .iter()
        .map(|n| {
            node_to_ui(
                n,
                depth_map.get(&n.id).copied().unwrap_or(0),
                degree.get(&n.id).copied().unwrap_or(0),
            )
        })
        .collect();

    let note = truncation_note(
        truncated_nodes,
        truncated_edges,
        raw_nodes,
        pre_cap_edge_count,
        ui_nodes.len(),
        ui_edges.len(),
    );

    UiGraph {
        nodes: ui_nodes,
        edges: ui_edges,
        truncated_nodes,
        truncated_edges,
        raw_nodes,
        raw_edges,
        note,
    }
}

/// Prefer higher priority when keeping edges under the draw cap.
/// tagged/tunnel/deps drop before wikilink/related of same weight.
fn edge_keep_priority(e: &UiEdge) -> i32 {
    match e.rel_type.as_str() {
        "wikilink" => 40,
        "related" => 30,
        "depends_on" | "derived_from" | "supersedes" => 20,
        "tunnel" => 15,
        "tagged" | "mentions" => 5,
        _ => 10,
    }
}

/// Clamp node set to UI_HARD_MAX_NODES.
/// With seed: undirected BFS from seed on full view edges (respecting keep-kind filter).
/// Without seed: stable sort by id, take first N.
fn clamp_nodes<'a>(
    filtered: &[&'a GraphNode],
    view: &GraphView,
    seed_id: Option<&str>,
) -> (Vec<&'a GraphNode>, bool) {
    if filtered.len() <= UI_HARD_MAX_NODES {
        return (filtered.to_vec(), false);
    }

    let filtered_ids: HashSet<&str> = filtered.iter().map(|n| n.id.as_str()).collect();
    let by_id: HashMap<&str, &GraphNode> = filtered.iter().map(|n| (n.id.as_str(), *n)).collect();

    if let Some(seed) = seed_id {
        if filtered_ids.contains(seed) {
            let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
            for e in &view.edges {
                let s = e.source_id.as_str();
                let t = e.target_id.as_str();
                if !filtered_ids.contains(s) || !filtered_ids.contains(t) {
                    continue;
                }
                adj.entry(s).or_default().push(t);
                adj.entry(t).or_default().push(s);
            }
            for neis in adj.values_mut() {
                neis.sort_unstable();
                neis.dedup();
            }

            let mut keep_order: Vec<&str> = Vec::new();
            let mut seen: HashSet<&str> = HashSet::new();
            let mut q = VecDeque::new();
            q.push_back(seed);
            seen.insert(seed);
            keep_order.push(seed);

            while let Some(id) = q.pop_front() {
                if keep_order.len() >= UI_HARD_MAX_NODES {
                    break;
                }
                let Some(neis) = adj.get(id) else {
                    continue;
                };
                for &n in neis {
                    if seen.contains(n) {
                        continue;
                    }
                    seen.insert(n);
                    keep_order.push(n);
                    q.push_back(n);
                    if keep_order.len() >= UI_HARD_MAX_NODES {
                        break;
                    }
                }
            }

            // Fill remaining slots from stable id order if BFS did not reach cap
            // (disconnected components).
            if keep_order.len() < UI_HARD_MAX_NODES {
                let mut rest: Vec<&str> = filtered_ids
                    .iter()
                    .copied()
                    .filter(|id| !seen.contains(id))
                    .collect();
                rest.sort_unstable();
                for id in rest {
                    if keep_order.len() >= UI_HARD_MAX_NODES {
                        break;
                    }
                    keep_order.push(id);
                }
            }

            let out: Vec<&GraphNode> = keep_order
                .into_iter()
                .filter_map(|id| by_id.get(id).copied())
                .collect();
            return (out, true);
        }
    }

    // No usable seed: stable id order, first N.
    let mut nodes = filtered.to_vec();
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    nodes.truncate(UI_HARD_MAX_NODES);
    (nodes, true)
}

fn bfs_depth(
    seed_id: Option<&str>,
    keep: &HashSet<String>,
    adj: &HashMap<String, Vec<String>>,
) -> HashMap<String, u32> {
    let mut depth_map: HashMap<String, u32> = HashMap::new();
    let Some(seed) = seed_id else {
        return depth_map;
    };
    if !keep.contains(seed) {
        return depth_map;
    }
    let mut q = VecDeque::new();
    q.push_back((seed.to_string(), 0u32));
    depth_map.insert(seed.to_string(), 0);
    while let Some((id, d)) = q.pop_front() {
        let Some(neis) = adj.get(&id) else {
            continue;
        };
        for n in neis {
            if depth_map.contains_key(n) {
                continue;
            }
            depth_map.insert(n.clone(), d + 1);
            q.push_back((n.clone(), d + 1));
        }
    }
    depth_map
}

fn truncation_note(
    truncated_nodes: bool,
    truncated_edges: bool,
    raw_nodes: usize,
    collapsed_edges: usize,
    kept_nodes: usize,
    kept_edges: usize,
) -> Option<String> {
    if !truncated_nodes && !truncated_edges {
        return None;
    }
    let mut parts = Vec::new();
    if truncated_nodes {
        parts.push(format!(
            "Showing {kept_nodes} of {raw_nodes} nodes (hard cap {UI_HARD_MAX_NODES})"
        ));
    }
    if truncated_edges {
        parts.push(format!(
            "Showing {kept_edges} of {collapsed_edges} draw edges (cap {UI_MAX_DRAW_EDGES})"
        ));
    }
    Some(parts.join("; "))
}

/// Collapse multi-edges for canvas by (source_id, target_id, rel_type).
/// One stroke; multi_count > 1; weight = max among members; full members kept for detail.
pub fn collapse_edges(edges: &[GraphEdge]) -> Vec<UiEdge> {
    let mut map: HashMap<(String, String, String), UiEdge> = HashMap::new();
    for e in edges {
        let key = (e.source_id.clone(), e.target_id.clone(), e.rel_type.clone());
        map.entry(key)
            .and_modify(|ui| {
                ui.multi_count += 1;
                if e.weight > ui.weight {
                    ui.weight = e.weight;
                    ui.id = e.id.clone();
                }
                ui.members.push(e.clone());
            })
            .or_insert_with(|| UiEdge {
                id: e.id.clone(),
                source_id: e.source_id.clone(),
                target_id: e.target_id.clone(),
                rel_type: e.rel_type.clone(),
                weight: e.weight,
                // Wire GraphEdge has no origin yet; default extract (GRAPH_DESIGN).
                origin: "extract".into(),
                multi_count: 1,
                members: vec![e.clone()],
            });
    }
    let mut out: Vec<UiEdge> = map.into_values().collect();
    out.sort_by(|a, b| {
        a.source_id
            .cmp(&b.source_id)
            .then_with(|| a.target_id.cmp(&b.target_id))
            .then_with(|| a.rel_type.cmp(&b.rel_type))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

fn node_to_ui(n: &GraphNode, depth: u32, degree: u32) -> UiNode {
    let (layer, wing, room) = parse_placement_meta(&n.metadata_json);
    UiNode {
        id: n.id.clone(),
        kind: n.kind.clone(),
        label: n.label.clone(),
        color: kind_color(&n.kind),
        document_id: n.document_id.clone(),
        uri: n.uri.clone(),
        resolved: n.resolved,
        layer,
        wing,
        room,
        depth,
        degree,
        pinned: false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeDisposition {
    Match,
    Companion,
    Excluded,
}

fn scope_disposition(node: &GraphNode, opts: &AdaptOptions) -> ScopeDisposition {
    let (_, wing, room) = parse_placement_meta(&node.metadata_json);
    let mut missing_placement = false;
    for (wanted, actual) in [
        (opts.wing.as_deref(), wing.as_deref()),
        (opts.room.as_deref(), room.as_deref()),
    ] {
        let Some(wanted) = wanted else {
            continue;
        };
        match actual {
            Some(actual) if actual == wanted => {}
            Some(_) => return ScopeDisposition::Excluded,
            None => missing_placement = true,
        }
    }

    if missing_placement {
        if node.kind == "document" {
            ScopeDisposition::Excluded
        } else {
            ScopeDisposition::Companion
        }
    } else {
        ScopeDisposition::Match
    }
}

fn parse_placement_meta(metadata_json: &str) -> (Option<String>, Option<String>, Option<String>) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(metadata_json) else {
        return (None, None, None);
    };
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|s| s.to_string());
    (s("layer"), s("wing"), s("room"))
}

/// Kind → fill color (EGUI_GRAPH_VIEW §7.2).
pub fn kind_color(kind: &str) -> egui::Color32 {
    match kind {
        "document" => egui::Color32::from_rgb(70, 130, 200),
        "tag" => egui::Color32::from_rgb(220, 170, 60),
        "stub" => egui::Color32::from_rgb(140, 140, 140),
        "entity" => egui::Color32::from_rgb(150, 100, 200),
        _ => egui::Color32::from_rgb(100, 100, 120),
    }
}

/// Rel type → edge stroke color.
pub fn edge_color(rel_type: &str) -> egui::Color32 {
    match rel_type {
        "wikilink" => egui::Color32::from_rgb(180, 180, 200),
        "related" => egui::Color32::from_rgb(120, 160, 180),
        "tagged" => egui::Color32::from_rgb(200, 160, 80),
        "tunnel" => egui::Color32::from_rgb(220, 100, 100),
        "depends_on" | "derived_from" | "supersedes" => egui::Color32::from_rgb(100, 180, 140),
        _ => egui::Color32::from_gray(150),
    }
}

/// Stable topology fingerprint for anti-thrash.
pub fn topology_generation(view: &GraphView) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut node_ids: Vec<&str> = view.nodes.iter().map(|n| n.id.as_str()).collect();
    let mut edge_ids: Vec<&str> = view.edges.iter().map(|e| e.id.as_str()).collect();
    node_ids.sort_unstable();
    edge_ids.sort_unstable();
    let mut h = DefaultHasher::new();
    node_ids.hash(&mut h);
    edge_ids.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, kind: &str, label: &str) -> GraphNode {
        GraphNode {
            id: id.into(),
            kind: kind.into(),
            label: label.into(),
            document_id: None,
            uri: None,
            resolved: true,
            metadata_json: "{}".into(),
        }
    }

    fn edge(id: &str, s: &str, t: &str, rel: &str, weight: f64) -> GraphEdge {
        GraphEdge {
            id: id.into(),
            source_id: s.into(),
            target_id: t.into(),
            rel_type: rel.into(),
            weight,
            context: None,
        }
    }

    #[test]
    fn multi_edge_collapse_by_source_target_rel() {
        let edges = vec![
            edge("e1", "a", "b", "wikilink", 1.0),
            edge("e2", "a", "b", "wikilink", 3.0),
            edge("e3", "a", "b", "related", 1.0),
        ];
        let collapsed = collapse_edges(&edges);
        assert_eq!(collapsed.len(), 2);
        let wiki = collapsed.iter().find(|e| e.rel_type == "wikilink").unwrap();
        assert_eq!(wiki.multi_count, 2);
        assert_eq!(wiki.weight, 3.0);
        assert_eq!(wiki.members.len(), 2);
        let rel = collapsed.iter().find(|e| e.rel_type == "related").unwrap();
        assert_eq!(rel.multi_count, 1);
    }

    #[test]
    fn nodes_carry_label_kind_color() {
        let view = GraphView {
            nodes: vec![node("d1", "document", "Doc"), node("t1", "tag", "Tag")],
            edges: vec![],
        };
        let g = adapt(
            &view,
            &AdaptOptions {
                show_tags: true,
                ..Default::default()
            },
        );
        assert_eq!(g.nodes.len(), 2);
        let doc = g.nodes.iter().find(|n| n.id == "d1").unwrap();
        assert_eq!(doc.label, "Doc");
        assert_eq!(doc.kind, "document");
        assert_eq!(doc.color, kind_color("document"));
        let tag = g.nodes.iter().find(|n| n.id == "t1").unwrap();
        assert_eq!(tag.color, kind_color("tag"));
    }

    #[test]
    fn hard_cap_nodes_with_note() {
        let mut nodes = Vec::new();
        for i in 0..350 {
            nodes.push(node(&format!("n{i:04}"), "document", &format!("N{i}")));
        }
        let view = GraphView {
            nodes,
            edges: vec![],
        };
        let g = adapt(&view, &AdaptOptions::default());
        assert_eq!(g.nodes.len(), UI_HARD_MAX_NODES);
        assert!(g.truncated_nodes);
        assert_eq!(g.raw_nodes, 350);
        let note = g.note.expect("truncation note");
        assert!(note.contains("300"));
        assert!(note.contains("350"));
    }

    #[test]
    fn seed_bfs_prefers_neighborhood_under_cap() {
        // seed + 10 neighbors + many disconnected nodes
        let mut nodes = vec![node("seed", "document", "Seed")];
        let mut edges = Vec::new();
        for i in 0..10 {
            let id = format!("near{i}");
            nodes.push(node(&id, "document", &id));
            edges.push(edge(&format!("e{i}"), "seed", &id, "wikilink", 1.0));
        }
        for i in 0..400 {
            nodes.push(node(&format!("far{i:04}"), "document", &format!("Far{i}")));
        }
        let view = GraphView { nodes, edges };
        let g = adapt(
            &view,
            &AdaptOptions {
                seed_id: Some("seed".into()),
                ..Default::default()
            },
        );
        assert_eq!(g.nodes.len(), UI_HARD_MAX_NODES);
        assert!(g.truncated_nodes);
        assert!(g.nodes.iter().any(|n| n.id == "seed"));
        for i in 0..10 {
            assert!(
                g.nodes.iter().any(|n| n.id == format!("near{i}")),
                "neighbor near{i} should survive seed-BFS cap"
            );
        }
    }

    #[test]
    fn hard_cap_draw_edges_with_note() {
        // Stay under node cap (300) but exceed draw-edge cap after collapse.
        // 80 nodes: directed complete digraph without self-loops = 80*79 = 6320 edges.
        let n = 80usize;
        let mut nodes = Vec::new();
        for i in 0..n {
            nodes.push(node(&format!("n{i}"), "document", &format!("N{i}")));
        }
        let mut edges = Vec::new();
        let mut ei = 0usize;
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                let rel = if ei.is_multiple_of(4) {
                    "tagged"
                } else {
                    "wikilink"
                };
                let w = if rel == "tagged" { 0.1 } else { 2.0 };
                edges.push(edge(
                    &format!("e{ei}"),
                    &format!("n{i}"),
                    &format!("n{j}"),
                    rel,
                    w,
                ));
                ei += 1;
            }
        }
        // Multi-edge duplicates that must collapse into one draw stroke.
        edges.push(edge("dup1", "n0", "n1", "wikilink", 1.0));
        edges.push(edge("dup2", "n0", "n1", "wikilink", 9.0));

        let view = GraphView { nodes, edges };
        let g = adapt(
            &view,
            &AdaptOptions {
                pkb_rels_only: false,
                ..Default::default()
            },
        );
        assert!(!g.truncated_nodes);
        assert_eq!(g.nodes.len(), n);
        assert_eq!(g.edges.len(), UI_MAX_DRAW_EDGES);
        assert!(g.truncated_edges);
        let note = g.note.expect("note");
        assert!(note.contains("draw edges"));
        assert!(note.contains(&UI_MAX_DRAW_EDGES.to_string()));
        // Prefer wikilink over tagged when dropping.
        let tagged = g.edges.iter().filter(|e| e.rel_type == "tagged").count();
        let wiki = g.edges.iter().filter(|e| e.rel_type == "wikilink").count();
        assert!(wiki > tagged);
        // Collapsed multi-edge keeps max weight for (n0,n1,wikilink) if retained.
        if let Some(e) = g
            .edges
            .iter()
            .find(|e| e.source_id == "n0" && e.target_id == "n1" && e.rel_type == "wikilink")
        {
            assert!(e.multi_count >= 2);
            assert_eq!(e.weight, 9.0);
        }
    }

    #[test]
    fn tags_hidden_by_default() {
        let view = GraphView {
            nodes: vec![node("d", "document", "D"), node("t", "tag", "T")],
            edges: vec![edge("e", "d", "t", "tagged", 1.0)],
        };
        let g = adapt(&view, &AdaptOptions::default());
        assert_eq!(g.nodes.len(), 1);
        assert_eq!(g.nodes[0].id, "d");
        // tagged rel filtered by pkb_rels_only even if both ends kept.
        assert!(g.edges.is_empty());
    }

    #[test]
    fn hard_cap_constants_match_mvp() {
        // EGUI_GRAPH_VIEW §7.1 / §8.1: UI hard layout caps (code, not docs-only).
        assert_eq!(UI_HARD_MAX_NODES, 300);
        assert_eq!(UI_MAX_DRAW_EDGES, 2000);
    }

    #[test]
    fn under_cap_no_truncation_flags() {
        let view = GraphView {
            nodes: vec![node("a", "document", "A"), node("b", "document", "B")],
            edges: vec![edge("e", "a", "b", "wikilink", 1.0)],
        };
        let g = adapt(
            &view,
            &AdaptOptions {
                seed_id: Some("a".into()),
                ..Default::default()
            },
        );
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1);
        assert!(!g.truncated_nodes);
        assert!(!g.truncated_edges);
        assert!(g.note.is_none());
        assert_eq!(g.raw_nodes, 2);
        assert_eq!(g.raw_edges, 1);
    }

    #[test]
    fn no_seed_truncates_stable_id_order() {
        let mut nodes = Vec::new();
        // Lexically later ids should be dropped first under id-sort truncate.
        for i in 0..310 {
            nodes.push(node(&format!("n{i:04}"), "document", &format!("N{i}")));
        }
        let view = GraphView {
            nodes,
            edges: vec![],
        };
        let g = adapt(&view, &AdaptOptions::default());
        assert_eq!(g.nodes.len(), UI_HARD_MAX_NODES);
        assert!(g.truncated_nodes);
        let ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "kept nodes must be stable id order");
        assert_eq!(g.nodes[0].id, "n0000");
        assert_eq!(g.nodes.last().unwrap().id, "n0299");
        assert!(!g.nodes.iter().any(|n| n.id == "n0300"));
    }

    #[test]
    fn pkb_rels_only_drops_depends_on_and_tunnel() {
        let view = GraphView {
            nodes: vec![
                node("a", "document", "A"),
                node("b", "document", "B"),
                node("c", "document", "C"),
            ],
            edges: vec![
                edge("w", "a", "b", "wikilink", 1.0),
                edge("r", "a", "c", "related", 1.0),
                edge("d", "b", "c", "depends_on", 1.0),
                edge("t", "a", "c", "tunnel", 1.0),
            ],
        };
        let g = adapt(&view, &AdaptOptions::default());
        assert_eq!(g.edges.len(), 2);
        assert!(g
            .edges
            .iter()
            .all(|e| matches!(e.rel_type.as_str(), "wikilink" | "related")));
        let g_all = adapt(
            &view,
            &AdaptOptions {
                pkb_rels_only: false,
                ..Default::default()
            },
        );
        assert_eq!(g_all.edges.len(), 4);
    }

    #[test]
    fn node_cap_never_exceeds_hard_max() {
        let mut nodes = Vec::new();
        for i in 0..1000 {
            nodes.push(node(&format!("x{i:04}"), "document", "x"));
        }
        let mut edges = Vec::new();
        // Chain so seed-BFS can walk past the cap.
        for i in 0..999 {
            edges.push(edge(
                &format!("e{i}"),
                &format!("x{i:04}"),
                &format!("x{:04}", i + 1),
                "wikilink",
                1.0,
            ));
        }
        let view = GraphView { nodes, edges };
        let g = adapt(
            &view,
            &AdaptOptions {
                seed_id: Some("x0000".into()),
                ..Default::default()
            },
        );
        assert!(g.nodes.len() <= UI_HARD_MAX_NODES);
        assert_eq!(g.nodes.len(), UI_HARD_MAX_NODES);
        assert!(g.truncated_nodes);
        assert!(g.nodes.iter().any(|n| n.id == "x0000"));
        // Seed-BFS keeps the near chain, not far tail.
        assert!(g.nodes.iter().any(|n| n.id == "x0001"));
        assert!(!g.nodes.iter().any(|n| n.id == "x0999"));
    }

    #[test]
    fn edge_cap_never_exceeds_draw_max() {
        // 50 nodes fully connected undirected as directed pairs → 50*49 = 2450 unique
        // (source,target,rel) after collapse, above 2000 when pkb allows all as wikilink.
        let n = 50usize;
        let mut nodes = Vec::new();
        for i in 0..n {
            nodes.push(node(&format!("n{i}"), "document", &format!("N{i}")));
        }
        let mut edges = Vec::new();
        let mut ei = 0usize;
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                edges.push(edge(
                    &format!("e{ei}"),
                    &format!("n{i}"),
                    &format!("n{j}"),
                    "wikilink",
                    1.0,
                ));
                ei += 1;
            }
        }
        let view = GraphView { nodes, edges };
        let g = adapt(&view, &AdaptOptions::default());
        assert!(g.edges.len() <= UI_MAX_DRAW_EDGES);
        assert_eq!(g.edges.len(), UI_MAX_DRAW_EDGES);
        assert!(g.truncated_edges);
    }

    #[test]
    fn stubs_visible_by_default_hidden_when_toggled() {
        let view = GraphView {
            nodes: vec![node("d", "document", "D"), node("s", "stub", "S")],
            edges: vec![edge("e", "d", "s", "wikilink", 1.0)],
        };
        let shown = adapt(&view, &AdaptOptions::default());
        assert_eq!(shown.nodes.len(), 2);
        let hidden = adapt(
            &view,
            &AdaptOptions {
                show_stubs: false,
                ..Default::default()
            },
        );
        assert_eq!(hidden.nodes.len(), 1);
        assert_eq!(hidden.nodes[0].id, "d");
        assert!(hidden.edges.is_empty());
    }

    #[test]
    fn project_and_room_filters_keep_matching_nodes_and_edges() {
        let mut a = node("a", "document", "A");
        a.metadata_json = r#"{"wing":"alpha","room":"docs"}"#.into();
        let mut b = node("b", "document", "B");
        b.metadata_json = r#"{"wing":"beta","room":"docs"}"#.into();
        let mut c = node("c", "document", "C");
        c.metadata_json = r#"{"wing":"alpha","room":"code"}"#.into();
        let view = GraphView {
            nodes: vec![a, b, c],
            edges: vec![
                edge("ab", "a", "b", "related", 1.0),
                edge("ac", "a", "c", "related", 1.0),
            ],
        };
        let filtered = adapt(
            &view,
            &AdaptOptions {
                wing: Some("alpha".into()),
                room: Some("docs".into()),
                ..Default::default()
            },
        );
        assert_eq!(filtered.nodes.len(), 1);
        assert_eq!(filtered.nodes[0].id, "a");
        assert!(filtered.edges.is_empty());
    }

    #[test]
    fn project_scope_keeps_only_companions_attached_to_matching_documents() {
        let mut alpha = node("alpha", "document", "Alpha");
        alpha.metadata_json = r#"{"wing":"alpha","room":"docs"}"#.into();
        let mut beta = node("beta", "document", "Beta");
        beta.metadata_json = r#"{"wing":"beta","room":"docs"}"#.into();
        let alpha_stub = node("alpha-stub", "stub", "Alpha stub");
        let alpha_entity = node("alpha-entity", "entity", "Alpha entity");
        let beta_stub = node("beta-stub", "stub", "Beta stub");
        let unrelated = node("orphan", "entity", "Orphan");
        let mut explicitly_beta = node("explicit-beta", "entity", "Explicit beta");
        explicitly_beta.metadata_json = r#"{"wing":"beta"}"#.into();
        let view = GraphView {
            nodes: vec![
                alpha,
                beta,
                alpha_stub,
                alpha_entity,
                beta_stub,
                unrelated,
                explicitly_beta,
            ],
            edges: vec![
                edge("as", "alpha", "alpha-stub", "wikilink", 1.0),
                edge("ae", "alpha-entity", "alpha", "related", 1.0),
                edge("bs", "beta", "beta-stub", "wikilink", 1.0),
                edge("ab", "alpha", "beta", "related", 1.0),
                edge("ax", "alpha", "explicit-beta", "related", 1.0),
            ],
        };

        let filtered = adapt(
            &view,
            &AdaptOptions {
                wing: Some("alpha".into()),
                room: Some("docs".into()),
                ..Default::default()
            },
        );
        let ids: HashSet<&str> = filtered.nodes.iter().map(|node| node.id.as_str()).collect();
        assert_eq!(ids, HashSet::from(["alpha", "alpha-stub", "alpha-entity"]));
        assert_eq!(filtered.edges.len(), 2);
        assert!(filtered
            .edges
            .iter()
            .all(|edge| edge.source_id == "alpha" || edge.target_id == "alpha"));
    }

    #[test]
    fn enabling_tags_keeps_tag_nodes_tagged_edges_and_tag_styling() {
        let view = GraphView {
            nodes: vec![node("doc", "document", "Doc"), node("tag", "tag", "Tag")],
            edges: vec![edge("tagged", "doc", "tag", "tagged", 1.0)],
        };
        let graph = adapt(
            &view,
            &AdaptOptions {
                show_tags: true,
                ..Default::default()
            },
        );

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].rel_type, "tagged");
        let tag = graph.nodes.iter().find(|node| node.id == "tag").unwrap();
        assert_eq!(tag.color, kind_color("tag"));
        assert_eq!(edge_color(&graph.edges[0].rel_type), edge_color("tagged"));
    }
}
