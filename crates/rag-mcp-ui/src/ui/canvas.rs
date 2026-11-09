//! Graph canvas with plain egui Painter (no egui_graphs dependency).
//!
//! MVP interaction (EGUI_GRAPH_VIEW §6.1 / §7.2):
//! - edges as lines (style by rel_type); nodes as filled circles + labels
//! - pan: drag background; zoom: scroll (pointer-anchored)
//! - click node: select; colors by kind (document / tag / stub / entity)
//! - label LOD: seed + selected always; hover any; all if n ≤ 40 or zoom high

use std::f32::consts::TAU;

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use rag_mcp_core::{NodeKind, RelType};

use crate::adapter::{UiGraph, UiNode};
use crate::layout::fit_bounds;
use crate::ui::scene::GraphScene;
use crate::ui::theme;

pub struct CanvasOut {
    /// Node id under a click.
    pub clicked_id: Option<String>,
    /// True when the pointer clicked the canvas but no node hit (deselect).
    pub clicked_empty: bool,
}

struct ScreenNode<'a> {
    node: &'a UiNode,
    /// Position in `UiGraph::nodes` (and in every per-node scene table).
    index: usize,
    center: Pos2,
    radius: f32,
}

/// Draw edges then nodes; pan via drag on background; zoom via scroll.
///
/// `scene` must have been built from this `graph` (same node order).
pub fn draw_canvas(
    ui: &mut Ui,
    graph: &UiGraph,
    scene: &GraphScene,
    selected: Option<&str>,
    pan: &mut Vec2,
    zoom: &mut f32,
    need_fit: &mut bool,
) -> CanvasOut {
    let (response, painter) =
        ui.allocate_painter(ui.available_size_before_wrap(), Sense::click_and_drag());
    let rect = response.rect;
    let canvas_pointer = ui
        .input(|input| input.pointer.hover_pos())
        .filter(|pointer| rect.contains(*pointer));

    // Subtle canvas background so empty vs graph is obvious.
    painter.rect_filled(rect, 0.0, Color32::from_rgb(10, 14, 24));

    if *need_fit && scene.bounds.is_some() {
        let (offset, scale) = fit_bounds(scene.bounds, rect);
        *pan = offset;
        *zoom = overview_camera_zoom(scale, graph.nodes.len());
        *need_fit = false;
    }

    // Pan: any drag on the canvas (read-only MVP: no node drag).
    if response.dragged() {
        *pan += response.drag_delta();
    }

    // Zoom toward pointer (scroll / trackpad).
    if let Some(pivot) = canvas_pointer {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            let factor = (1.0 + scroll * 0.001).clamp(0.9, 1.1);
            zoom_at(zoom, pan, pivot, factor);
        }
    }

    let z = *zoom;
    let p = *pan;
    let to_screen = |world: Pos2| -> Pos2 { Pos2::new(world.x * z + p.x, world.y * z + p.y) };

    // --- edges (under nodes) ---
    // Collected in paint order and handed to the painter in one call.
    let mut edge_shapes: Vec<egui::Shape> = Vec::with_capacity(graph.edges.len());
    for e in &graph.edges {
        let (Some(a), Some(b)) = (scene.positions[e.source_idx], scene.positions[e.target_idx])
        else {
            continue;
        };
        let sa = to_screen(a);
        let sb = to_screen(b);
        // Clip cheaply: skip if segment AABB misses expanded rect.
        if !segment_near_rect(sa, sb, rect) {
            continue;
        }
        let base_w = e
            .rel
            .map_or(RelType::DEFAULT_BASE_WIDTH, RelType::base_width);
        let width =
            (base_w + (e.weight as f32).ln_1p() * 0.45 + (e.multi_count as f32 - 1.0) * 0.25)
                .clamp(0.6, 5.0);
        let color = theme::edge_color(e.rel);
        let stroke = Stroke::new(width, color);
        if e.rel.is_some_and(RelType::is_dashed) {
            dashed_line(&mut edge_shapes, sa, sb, stroke, 4.0, 4.0, rect);
        } else {
            edge_shapes.push(egui::Shape::line_segment([sa, sb], stroke));
            // Direction arrow for directed rel types (EGUI_GRAPH_VIEW §7.2).
            if e.rel.is_some_and(RelType::is_directed) {
                push_arrowhead(&mut edge_shapes, sa, sb, stroke);
            }
        }
    }
    painter.extend(edge_shapes);

    let n_nodes = graph.nodes.len();
    let overview_lod = n_nodes > 5_000 && z < 2.0;
    let show_all_labels = n_nodes <= 40 || z > 1.8;
    let pointer = canvas_pointer;
    let click_pos = if response.clicked() {
        response.interact_pointer_pos()
    } else {
        None
    };

    // Precompute screen centers + radii for hit test and paint.
    let mut screen_nodes: Vec<ScreenNode<'_>> = Vec::with_capacity(graph.nodes.len());
    for (index, (node, world)) in graph.nodes.iter().zip(&scene.positions).enumerate() {
        let Some(world) = *world else {
            continue;
        };
        let center = to_screen(world);
        // Size: base + k * log1p(degree), scaled mildly with zoom (EGUI_GRAPH_VIEW §7.2).
        let radius = if overview_lod {
            2.5
        } else {
            (8.0 + (node.degree as f32 + 1.0).ln_1p() * 3.0).clamp(6.0, 22.0)
                * z.sqrt().clamp(0.6, 1.6)
        };

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
            index,
            center,
            radius,
        });
    }

    // Every visible node is a real focusable egui widget. That supplies Tab and
    // spatial arrow navigation, Enter/Space activation, and an AccessKit node
    // with a meaningful label while preserving painter-based rendering.
    let mut interaction_clicked_id = None;
    let mut focused_id = None;
    let mut node_drag_delta = Vec2::ZERO;
    for sn in &screen_nodes {
        if overview_lod {
            continue;
        }
        let hit_size = (sn.radius * 2.3).max(24.0);
        let hit_rect = Rect::from_center_size(sn.center, Vec2::splat(hit_size));
        if !hit_rect.intersects(rect) {
            continue;
        }
        let hit_rect = hit_rect.intersect(rect);
        let node_response = ui.interact(
            hit_rect,
            response.id.with(("graph_node", sn.node.id.as_str())),
            Sense::click_and_drag(),
        );
        node_response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                true,
                selected == Some(sn.node.id.as_str()),
                node_accessibility_label(sn.node),
            )
        });
        if node_response.clicked() {
            interaction_clicked_id = Some(sn.node.id.clone());
        }
        if node_response.has_focus() {
            focused_id = Some(sn.node.id.clone());
        }
        if node_response.dragged() {
            node_drag_delta = node_response.drag_delta();
        }
    }
    if node_drag_delta != Vec2::ZERO && !response.dragged() {
        // Keep the canvas pannable even when the gesture starts on a focusable
        // node target rather than on empty background.
        *pan += node_drag_delta;
    }

    // Hit test: closest center among nodes whose hit radius contains the pointer.
    let hover = pointer.and_then(|pos| pick_node(&screen_nodes, pos));
    let clicked_id = interaction_clicked_id.or_else(|| {
        click_pos.and_then(|pos| {
            pick_node(&screen_nodes, pos).map(|index| graph.nodes[index].id.clone())
        })
    });
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

    if overview_lod {
        let mut mesh = egui::Mesh::default();
        for sn in others.iter().chain(&selected_draw) {
            mesh.add_colored_rect(
                Rect::from_center_size(sn.center, Vec2::splat(sn.radius * 2.0)),
                theme::node_color(sn.node.node_kind, sn.node.layer.as_deref()),
            );
        }
        painter.add(egui::Shape::mesh(mesh));
    }

    for sn in others
        .into_iter()
        .chain(selected_draw)
        .filter(|_| !overview_lod)
    {
        let node = sn.node;
        let sn_index = sn.index;
        let center = sn.center;
        let radius = sn.radius;
        let selected_here = selected == Some(node.id.as_str());
        let focused_here = focused_id.as_deref() == Some(node.id.as_str());
        let is_stub = node.node_kind == Some(NodeKind::Stub) || !node.resolved;

        // Fill color by kind, documents by layer (EGUI_GRAPH_VIEW §7.2).
        painter.circle_filled(
            center,
            radius,
            theme::node_color(node.node_kind, node.layer.as_deref()),
        );

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

        if focused_here {
            painter.circle_stroke(center, radius + 5.0, Stroke::new(2.0, theme::L0));
        }

        let show_label = selected_here
            || hover == Some(sn_index)
            || (node.depth == 0 && n_nodes <= 5_000)
            || show_all_labels;
        if show_label {
            let font = egui::FontId::proportional(13.0);
            let text_color = Color32::from_rgb(226, 232, 240);
            let galley = painter.layout_no_wrap(node.label.clone(), font, text_color);
            // Semi-transparent backing so labels stay readable over edges.
            let label_pos = center + Vec2::new(0.0, radius + 4.0);
            let label_rect = Rect::from_center_size(
                Pos2::new(label_pos.x, label_pos.y + galley.size().y * 0.5),
                galley.size(),
            )
            .expand(2.0);
            painter.rect_filled(
                label_rect,
                2.0,
                Color32::from_rgba_unmultiplied(10, 14, 24, 215),
            );
            painter.galley(label_rect.min + Vec2::splat(2.0), galley, text_color);
        }
    }

    // View controls are registered after node hit targets so they remain the
    // topmost interaction if a node sits beneath the overlay.
    let ctl_rect = Rect::from_min_size(
        rect.right_top() + Vec2::new(-146.0, 6.0),
        Vec2::new(140.0, 30.0),
    );
    let mut ctl = ui.new_child(egui::UiBuilder::new().max_rect(ctl_rect));
    ctl.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui
            .add_sized([30.0, 30.0], egui::Button::new("+"))
            .on_hover_text("Приблизить")
            .clicked()
        {
            zoom_at(zoom, pan, rect.center(), 1.25);
        }
        if ui
            .add_sized([30.0, 30.0], egui::Button::new("−"))
            .on_hover_text("Отдалить")
            .clicked()
        {
            zoom_at(zoom, pan, rect.center(), 0.8);
        }
        if ui
            .add_sized([58.0, 30.0], egui::Button::new("Обзор"))
            .on_hover_text("Центрировать обзор графа")
            .clicked()
        {
            *need_fit = true;
        }
    });

    draw_legend(&painter, rect, scene);

    if let Some(index) = hover {
        let node = &graph.nodes[index];
        let multi = scene.max_multi[index];
        let multi_note = if multi > 1 {
            format!("\nмакс. кратность ребра x{multi}")
        } else {
            String::new()
        };
        response.on_hover_text(format!(
            "{}\nтип={} · связей={}{}",
            node.label,
            localized_kind(&node.kind),
            node.degree,
            multi_note
        ));
    }

    CanvasOut {
        clicked_id,
        clicked_empty,
    }
}

fn node_accessibility_label(node: &UiNode) -> String {
    let resolution = if node.resolved {
        "разрешён"
    } else {
        "не разрешён"
    };
    format!(
        "{}; узел типа {}; {resolution}; связей: {}; глубина: {}",
        node.label,
        localized_kind(&node.kind),
        node.degree,
        node.depth
    )
}

pub(crate) fn localized_kind(kind: &str) -> &str {
    match NodeKind::parse(kind) {
        Some(NodeKind::Document) => "документ",
        Some(NodeKind::Tag) => "тег",
        Some(NodeKind::Stub) => "заглушка",
        Some(NodeKind::Entity) => "сущность",
        None => kind,
    }
}

pub(crate) fn localized_layer(layer: &str) -> &str {
    match layer {
        "raw" => "сырой корпус",
        "wiki" => "вики",
        "document" => "документ",
        other => other,
    }
}

pub(crate) fn localized_relation(relation: &str) -> &str {
    match RelType::parse(relation) {
        Some(RelType::Wikilink) => "вики-ссылка",
        Some(RelType::Related) => "связано",
        Some(RelType::Similar) => "похоже на",
        Some(RelType::Contains) => "содержит",
        Some(RelType::Imports) => "импортирует",
        Some(RelType::Describes) => "описывает",
        Some(RelType::Tunnel) => "туннель",
        Some(RelType::Tagged) => "с тегом",
        Some(RelType::Mentions) => "упоминает",
        Some(RelType::DependsOn) => "зависит от",
        Some(RelType::DerivedFrom) => "производное от",
        Some(RelType::Cites) => "получено из",
        Some(RelType::Supports) => "подтверждает",
        Some(RelType::Contradicts) => "противоречит",
        Some(RelType::Supersedes) => "замещает",
        None => relation,
    }
}

/// Zoom around a pivot point (screen coords), clamped like scroll zoom.
fn zoom_at(zoom: &mut f32, pan: &mut Vec2, pivot: Pos2, factor: f32) {
    let before = (pivot.to_vec2() - *pan) / *zoom;
    *zoom = (*zoom * factor).clamp(0.05, 12.0);
    *pan = pivot.to_vec2() - before * *zoom;
}

/// Dense overviews live on a virtual canvas larger than the viewport. Keep a
/// useful amount of context, but never squeeze every node into one rectangle.
fn overview_camera_zoom(fit_zoom: f32, node_count: usize) -> f32 {
    if node_count > 5_000 {
        (fit_zoom * 8.0).clamp(0.75, 8.0)
    } else {
        fit_zoom
    }
}

/// Small arrowhead just before the target node (direction of the relation).
fn push_arrowhead(shapes: &mut Vec<egui::Shape>, a: Pos2, b: Pos2, stroke: Stroke) {
    let d = b - a;
    let len = d.length();
    if len < 24.0 {
        return;
    }
    let dir = d / len;
    let tip = b - dir * 14.0;
    let side = Vec2::new(-dir.y, dir.x);
    shapes.push(egui::Shape::line_segment(
        [tip, tip - dir * 8.0 + side * 3.5],
        stroke,
    ));
    shapes.push(egui::Shape::line_segment(
        [tip, tip - dir * 8.0 - side * 3.5],
        stroke,
    ));
}

/// Compact legend of kind / rel_type colors present in the view (bottom-left).
fn draw_legend(painter: &egui::Painter, rect: Rect, scene: &GraphScene) {
    let rows = scene.legend_kinds.len() + scene.legend_rels.len();
    if rows == 0 {
        return;
    }
    let line_h = 16.0;
    let mut y = rect.max.y - 8.0 - line_h * rows as f32;
    let x = rect.min.x + 10.0;
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(rect.min.x + 4.0, y - 4.0),
            Vec2::new(130.0, line_h * rows as f32 + 8.0),
        ),
        4.0,
        Color32::from_rgba_unmultiplied(18, 18, 24, 160),
    );
    let font = egui::FontId::proportional(11.0);
    let text_color = Color32::from_gray(200);
    for (kind, color) in &scene.legend_kinds {
        let cy = y + line_h * 0.5;
        painter.circle_filled(Pos2::new(x + 4.0, cy), 4.0, *color);
        painter.text(
            Pos2::new(x + 14.0, y),
            egui::Align2::LEFT_TOP,
            kind,
            font.clone(),
            text_color,
        );
        y += line_h;
    }
    for (relation, color) in &scene.legend_rels {
        let cy = y + line_h * 0.5;
        painter.line_segment(
            [Pos2::new(x, cy), Pos2::new(x + 10.0, cy)],
            Stroke::new(1.5_f32, *color),
        );
        painter.text(
            Pos2::new(x + 14.0, y),
            egui::Align2::LEFT_TOP,
            relation,
            font.clone(),
            text_color,
        );
        y += line_h;
    }
}

/// Index (in `UiGraph::nodes`) of the node under `pos`: the closest centre
/// among nodes whose hit radius contains it.
fn pick_node(nodes: &[ScreenNode<'_>], pos: Pos2) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for sn in nodes {
        let hit_r = sn.radius * 1.15;
        let d = (pos - sn.center).length();
        if d <= hit_r {
            match best {
                Some((_, bd)) if bd <= d => {}
                _ => best = Some((sn.index, d)),
            }
        }
    }
    best.map(|(index, _)| index)
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

/// Range of distances from `a` along `a → b` (unit direction `unit`, length
/// `len`) that lies inside `rect`, or `None` when the segment misses it.
fn segment_span_in_rect(a: Pos2, unit: Vec2, len: f32, rect: Rect) -> Option<(f32, f32)> {
    let (mut t0, mut t1) = (0.0_f32, len);
    for (origin, direction, low, high) in [
        (a.x, unit.x, rect.min.x, rect.max.x),
        (a.y, unit.y, rect.min.y, rect.max.y),
    ] {
        if direction.abs() < f32::EPSILON {
            if origin < low || origin > high {
                return None;
            }
            continue;
        }
        let (near, far) = ((low - origin) / direction, (high - origin) / direction);
        t0 = t0.max(near.min(far));
        t1 = t1.min(near.max(far));
        if t0 > t1 {
            return None;
        }
    }
    Some((t0, t1))
}

/// Dashes of length `dash` separated by `gap`, phased from `a`. Only dashes
/// that can touch `clip` are emitted: a long edge that leaves the viewport
/// costs what is visible, not its full on-screen length.
fn dashed_line(
    shapes: &mut Vec<egui::Shape>,
    a: Pos2,
    b: Pos2,
    stroke: Stroke,
    dash: f32,
    gap: f32,
    clip: Rect,
) {
    let dir = b - a;
    let len = dir.length();
    if len < 0.5 {
        return;
    }
    let unit = dir / len;
    let step = (dash + gap).max(1.0);
    let margin = stroke.width + 1.0;
    let Some((from, to)) = segment_span_in_rect(a, unit, len, clip.expand(margin)) else {
        return;
    };
    // Whole dashes only, on the same phase as an unclipped line.
    let first = ((from - dash) / step).floor().max(0.0) as u32;
    let mut index = first;
    loop {
        let t = index as f32 * step;
        if t >= len || t > to {
            break;
        }
        let t1 = (t + dash).min(len);
        shapes.push(egui::Shape::line_segment(
            [a + unit * t, a + unit * t1],
            stroke,
        ));
        index += 1;
    }
}

/// Unit circle sampled every 15°, shared by every dashed stub outline.
fn unit_circle() -> &'static [Vec2; DASHED_CIRCLE_SEGMENTS + 1] {
    static TABLE: std::sync::OnceLock<[Vec2; DASHED_CIRCLE_SEGMENTS + 1]> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|i| {
            let angle = TAU * (i as f32) / (DASHED_CIRCLE_SEGMENTS as f32);
            Vec2::new(angle.cos(), angle.sin())
        })
    })
}

const DASHED_CIRCLE_SEGMENTS: usize = 24;

fn dashed_circle(painter: &egui::Painter, center: Pos2, radius: f32, stroke: Stroke) {
    // 24 segments; draw every other chord as a dash.
    let table = unit_circle();
    for i in (0..DASHED_CIRCLE_SEGMENTS).step_by(2) {
        let p0 = center + table[i] * radius;
        let p1 = center + table[i + 1] * radius;
        painter.line_segment([p0, p1], stroke);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessible_node_label_exposes_identity_and_topology() {
        let node = UiNode {
            id: "node-1".into(),
            kind: "document".into(),
            node_kind: Some(NodeKind::Document),
            label: "Architecture".into(),
            document_id: Some("doc-1".into()),
            uri: Some("wiki://architecture".into()),
            resolved: true,
            layer: Some("wiki".into()),
            wing: Some("alpha".into()),
            room: None,
            depth: 2,
            degree: 7,
        };
        let label = node_accessibility_label(&node);
        assert!(label.contains("Architecture"));
        assert!(label.contains("узел типа документ"));
        assert!(label.contains("связей: 7"));
        assert!(label.contains("глубина: 2"));
    }

    #[test]
    fn dense_overview_uses_a_virtual_canvas_camera() {
        assert!((overview_camera_zoom(0.1, 5_001) - 0.8).abs() < f32::EPSILON);
        assert!((overview_camera_zoom(0.01, 5_001) - 0.75).abs() < f32::EPSILON);
        assert!((overview_camera_zoom(0.1, 5_000) - 0.1).abs() < f32::EPSILON);
    }

    fn dash_segments(a: Pos2, b: Pos2, clip: Rect) -> Vec<[Pos2; 2]> {
        let mut shapes = Vec::new();
        dashed_line(
            &mut shapes,
            a,
            b,
            Stroke::new(1.0, Color32::WHITE),
            4.0,
            4.0,
            clip,
        );
        shapes
            .into_iter()
            .map(|shape| match shape {
                egui::Shape::LineSegment { points, .. } => points,
                other => panic!("unexpected shape {other:?}"),
            })
            .collect()
    }

    #[test]
    fn clipped_dashes_are_the_visible_subset_of_the_full_line() {
        let everything = Rect::from_min_max(Pos2::new(-1e6, -1e6), Pos2::new(1e6, 1e6));
        let view = Rect::from_min_max(Pos2::new(100.0, 100.0), Pos2::new(300.0, 250.0));
        for (a, b) in [
            (Pos2::new(-500.0, -400.0), Pos2::new(900.0, 700.0)),
            (Pos2::new(900.0, 120.0), Pos2::new(-300.0, 240.0)),
            (Pos2::new(150.0, -2_000.0), Pos2::new(150.0, 2_000.0)),
            (Pos2::new(120.0, 120.0), Pos2::new(280.0, 230.0)),
        ] {
            let full = dash_segments(a, b, everything);
            let clipped = dash_segments(a, b, view);
            assert!(clipped.len() < full.len() || full.len() <= 40);
            // Every clipped dash is one of the unclipped dashes (same phase)...
            for dash in &clipped {
                assert!(full.iter().any(|other| {
                    (other[0] - dash[0]).length() < 0.01 && (other[1] - dash[1]).length() < 0.01
                }));
            }
            // ...and no dash that reaches the viewport is dropped.
            let visible = view.expand(1.0);
            for dash in &full {
                let touches = (0..=8).any(|step| {
                    visible.contains(dash[0] + (dash[1] - dash[0]) * (step as f32 / 8.0))
                });
                if touches {
                    assert!(clipped
                        .iter()
                        .any(|kept| (kept[0] - dash[0]).length() < 0.01));
                }
            }
        }
        // A line that misses the viewport costs nothing.
        assert!(dash_segments(Pos2::new(0.0, 0.0), Pos2::new(50.0, 900.0), view).is_empty());
    }

    #[test]
    fn unit_circle_table_closes_the_loop() {
        let table = unit_circle();
        assert!((table[0] - Vec2::new(1.0, 0.0)).length() < 1e-6);
        assert!((table[DASHED_CIRCLE_SEGMENTS] - table[0]).length() < 1e-5);
        assert!(table.iter().all(|p| (p.length() - 1.0).abs() < 1e-6));
    }
}
