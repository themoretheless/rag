//! Graph canvas with plain egui Painter (no egui_graphs dependency).
//!
//! MVP interaction (EGUI_GRAPH_VIEW §6.1 / §7.2):
//! - edges as lines (style by rel_type); nodes as filled circles + labels
//! - pan: drag background; zoom: scroll (pointer-anchored)
//! - click node: select; colors by kind (document / tag / stub / entity)
//! - label LOD: seed + selected always; hover any; all if n ≤ 40 or zoom high

use std::f32::consts::TAU;

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::adapter::{edge_color, UiGraph, UiNode};
use crate::layout::{fit_transform, PosCache};

pub struct CanvasOut {
    /// Node id under a click.
    pub clicked_id: Option<String>,
    /// True when the pointer clicked the canvas but no node hit (deselect).
    pub clicked_empty: bool,
    #[allow(dead_code)]
    pub hover_id: Option<String>,
}

struct ScreenNode<'a> {
    node: &'a UiNode,
    center: Pos2,
    radius: f32,
}

/// Draw edges then nodes; pan via drag on background; zoom via scroll.
pub fn draw_canvas(
    ui: &mut Ui,
    graph: &UiGraph,
    positions: &PosCache,
    selected: Option<&str>,
    pan: &mut Vec2,
    zoom: &mut f32,
    need_fit: &mut bool,
) -> CanvasOut {
    let (response, painter) =
        ui.allocate_painter(ui.available_size_before_wrap(), Sense::click_and_drag());
    let rect = response.rect;

    // Subtle canvas background so empty vs graph is obvious.
    painter.rect_filled(rect, 0.0, Color32::from_rgb(18, 18, 24));

    if *need_fit && !positions.is_empty() {
        let (offset, scale) = fit_transform(positions, rect);
        *pan = offset;
        *zoom = scale;
        *need_fit = false;
    }

    // Pan: any drag on the canvas (read-only MVP: no node drag).
    if response.dragged() {
        *pan += response.drag_delta();
    }

    // Zoom toward pointer (scroll / trackpad).
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            let factor = (1.0 + scroll * 0.001).clamp(0.9, 1.1);
            let new_zoom = (*zoom * factor).clamp(0.05, 12.0);
            if let Some(pointer) = response.hover_pos() {
                let before = (pointer.to_vec2() - *pan) / *zoom;
                *zoom = new_zoom;
                *pan = pointer.to_vec2() - before * *zoom;
            } else {
                *zoom = new_zoom;
            }
        }
    }

    let z = *zoom;
    let p = *pan;
    let to_screen = |world: Pos2| -> Pos2 { Pos2::new(world.x * z + p.x, world.y * z + p.y) };

    // --- edges (under nodes) ---
    for e in &graph.edges {
        let Some(a) = positions.get(&e.source_id) else {
            continue;
        };
        let Some(b) = positions.get(&e.target_id) else {
            continue;
        };
        let sa = to_screen(*a);
        let sb = to_screen(*b);
        // Clip cheaply: skip if segment AABB misses expanded rect.
        if !segment_near_rect(sa, sb, rect) {
            continue;
        }
        let base_w = match e.rel_type.as_str() {
            "related" => 0.8,
            "tunnel" => 2.2,
            "tagged" | "mentions" => 1.0,
            _ => 1.2,
        };
        let width = (base_w
            + (e.weight as f32).ln_1p() * 0.45
            + (e.multi_count as f32 - 1.0) * 0.25)
            .clamp(0.6, 5.0);
        let color = edge_color(&e.rel_type);
        let stroke = Stroke::new(width, color);
        match e.rel_type.as_str() {
            "tagged" | "mentions" => dashed_line(&painter, sa, sb, stroke, 4.0, 4.0),
            _ => {
                painter.line_segment([sa, sb], stroke);
            }
        }
    }

    let n_nodes = graph.nodes.len();
    let show_all_labels = n_nodes <= 40 || z > 1.8;
    let pointer = response.hover_pos();
    let click_pos = if response.clicked() {
        response.interact_pointer_pos()
    } else {
        None
    };

    // Precompute screen centers + radii for hit test and paint.
    let mut screen_nodes: Vec<ScreenNode<'_>> = Vec::with_capacity(graph.nodes.len());
    for node in &graph.nodes {
        let Some(world) = positions.get(&node.id) else {
            continue;
        };
        let center = to_screen(*world);
        // Size: base + k * log1p(degree), scaled mildly with zoom (EGUI_GRAPH_VIEW §7.2).
        let radius = (8.0 + (node.degree as f32 + 1.0).ln_1p() * 3.0).clamp(6.0, 22.0)
            * z.sqrt().clamp(0.6, 1.6);

        // Skip fully off-screen nodes when neither pointer nor click is near.
        let expanded = rect.expand(radius + 48.0);
        if !expanded.contains(center) {
            let near_pointer = pointer
                .map(|pp| (pp - center).length() <= radius * 3.0)
                .unwrap_or(false);
            let near_click = click_pos
                .map(|pp| (pp - center).length() <= radius * 3.0)
                .unwrap_or(false);
            if !near_pointer && !near_click {
                continue;
            }
        }
        screen_nodes.push(ScreenNode {
            node,
            center,
            radius,
        });
    }

    // Hit test: closest center among nodes whose hit radius contains the pointer.
    let hover_id = pointer.and_then(|pos| pick_node(&screen_nodes, pos).map(|n| n.id.clone()));
    let clicked_id = click_pos.and_then(|pos| pick_node(&screen_nodes, pos).map(|n| n.id.clone()));
    let clicked_empty = response.clicked() && clicked_id.is_none();

    // Paint order: non-selected first, selected last (on top).
    let mut others: Vec<ScreenNode<'_>> = Vec::new();
    let mut selected_draw: Vec<ScreenNode<'_>> = Vec::new();
    for sn in screen_nodes {
        if selected == Some(sn.node.id.as_str()) {
            selected_draw.push(sn);
        } else {
            others.push(sn);
        }
    }

    for sn in others.into_iter().chain(selected_draw.into_iter()) {
        let node = sn.node;
        let center = sn.center;
        let radius = sn.radius;
        let selected_here = selected == Some(node.id.as_str());
        let is_stub = node.kind == "stub" || !node.resolved;

        // Fill color comes from adapter kind_color (document/tag/stub/entity).
        painter.circle_filled(center, radius, node.color);

        if selected_here {
            painter.circle_stroke(center, radius, Stroke::new(2.5_f32, Color32::WHITE));
            // Outer halo for selection.
            painter.circle_stroke(
                center,
                radius + 3.0,
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 90)),
            );
        } else if is_stub {
            // Stub / unresolved: gray dashed border (EGUI_GRAPH_VIEW §7.2).
            dashed_circle(
                &painter,
                center,
                radius,
                Stroke::new(1.5_f32, Color32::from_rgb(200, 200, 200)),
            );
        } else {
            painter.circle_stroke(
                center,
                radius,
                Stroke::new(1.0_f32, Color32::from_rgb(30, 30, 40)),
            );
        }

        let show_label = selected_here
            || hover_id.as_deref() == Some(node.id.as_str())
            || node.depth == 0
            || show_all_labels;
        if show_label {
            painter.text(
                center + Vec2::new(0.0, radius + 4.0),
                egui::Align2::CENTER_TOP,
                &node.label,
                egui::FontId::proportional(12.0),
                Color32::from_gray(230),
            );
        }
    }

    if let Some(hid) = hover_id.as_deref() {
        if let Some(node) = graph.nodes.iter().find(|n| n.id == hid) {
            let multi = graph
                .edges
                .iter()
                .filter(|e| e.source_id == hid || e.target_id == hid)
                .filter(|e| e.multi_count > 1)
                .map(|e| e.multi_count)
                .max()
                .unwrap_or(1);
            let multi_note = if multi > 1 {
                format!("\nmax multi-edge ×{multi}")
            } else {
                String::new()
            };
            response.on_hover_text(format!(
                "{}\nkind={} degree={}{}",
                node.label, node.kind, node.degree, multi_note
            ));
        }
    }

    CanvasOut {
        clicked_id,
        clicked_empty,
        hover_id,
    }
}

fn pick_node<'a>(nodes: &'a [ScreenNode<'a>], pos: Pos2) -> Option<&'a UiNode> {
    let mut best: Option<(&UiNode, f32)> = None;
    for sn in nodes {
        let hit_r = sn.radius * 1.15;
        let d = (pos - sn.center).length();
        if d <= hit_r {
            match best {
                Some((_, bd)) if bd <= d => {}
                _ => best = Some((sn.node, d)),
            }
        }
    }
    best.map(|(n, _)| n)
}

/// True if segment AABB intersects expanded rect (cheap clip).
fn segment_near_rect(a: Pos2, b: Pos2, rect: Rect) -> bool {
    let r = rect.expand(8.0);
    let min_x = a.x.min(b.x);
    let max_x = a.x.max(b.x);
    let min_y = a.y.min(b.y);
    let max_y = a.y.max(b.y);
    !(max_x < r.min.x || min_x > r.max.x || max_y < r.min.y || min_y > r.max.y)
}

fn dashed_line(painter: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke, dash: f32, gap: f32) {
    let dir = b - a;
    let len = dir.length();
    if len < 0.5 {
        return;
    }
    let unit = dir / len;
    let step = (dash + gap).max(1.0);
    let mut t = 0.0;
    while t < len {
        let t1 = (t + dash).min(len);
        painter.line_segment([a + unit * t, a + unit * t1], stroke);
        t += step;
    }
}

fn dashed_circle(painter: &egui::Painter, center: Pos2, radius: f32, stroke: Stroke) {
    // 24 segments; draw every other chord as a dash.
    let n = 24i32;
    for i in 0..n {
        if i % 2 != 0 {
            continue;
        }
        let a0 = TAU * (i as f32) / (n as f32);
        let a1 = TAU * ((i + 1) as f32) / (n as f32);
        let p0 = center + Vec2::new(a0.cos(), a0.sin()) * radius;
        let p1 = center + Vec2::new(a1.cos(), a1.sin()) * radius;
        painter.line_segment([p0, p1], stroke);
    }
}
