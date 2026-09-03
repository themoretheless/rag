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
use crate::ui::theme;

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
    let canvas_pointer = ui
        .input(|input| input.pointer.hover_pos())
        .filter(|pointer| rect.contains(*pointer));

    // Subtle canvas background so empty vs graph is obvious.
    painter.rect_filled(rect, 0.0, Color32::from_rgb(10, 14, 24));

    if *need_fit && !positions.is_empty() {
        let (offset, scale) = fit_transform(positions, rect);
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
        let width =
            (base_w + (e.weight as f32).ln_1p() * 0.45 + (e.multi_count as f32 - 1.0) * 0.25)
                .clamp(0.6, 5.0);
        let color = edge_color(&e.rel_type);
        let stroke = Stroke::new(width, color);
        match e.rel_type.as_str() {
            "tagged" | "mentions" => dashed_line(&painter, sa, sb, stroke, 4.0, 4.0),
            _ => {
                painter.line_segment([sa, sb], stroke);
                // Direction arrow for directed rel types (EGUI_GRAPH_VIEW §7.2).
                if matches!(
                    e.rel_type.as_str(),
                    "wikilink" | "depends_on" | "derived_from" | "supersedes"
                ) {
                    draw_arrowhead(&painter, sa, sb, stroke);
                }
            }
        }
    }

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
    for node in &graph.nodes {
        let Some(world) = positions.get(&node.id) else {
            continue;
        };
        let center = to_screen(*world);
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
    let hover_id = pointer.and_then(|pos| pick_node(&screen_nodes, pos).map(|n| n.id.clone()));
    let clicked_id = interaction_clicked_id.or_else(|| {
        click_pos.and_then(|pos| pick_node(&screen_nodes, pos).map(|node| node.id.clone()))
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
                sn.node.color,
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
        let center = sn.center;
        let radius = sn.radius;
        let selected_here = selected == Some(node.id.as_str());
        let focused_here = focused_id.as_deref() == Some(node.id.as_str());
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

        if focused_here {
            painter.circle_stroke(center, radius + 5.0, Stroke::new(2.0, theme::L0));
        }

        let show_label = selected_here
            || hover_id.as_deref() == Some(node.id.as_str())
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

    draw_legend(&painter, rect, graph);

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
    }

    CanvasOut {
        clicked_id,
        clicked_empty,
        hover_id,
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

fn localized_kind(kind: &str) -> &str {
    match kind {
        "document" => "документ",
        "tag" => "тег",
        "stub" => "заглушка",
        "entity" => "сущность",
        other => other,
    }
}

fn localized_layer(layer: &str) -> &str {
    match layer {
        "raw" => "сырой корпус",
        "wiki" => "вики",
        "document" => "документ",
        other => other,
    }
}

fn localized_relation(relation: &str) -> &str {
    match relation {
        "wikilink" => "вики-ссылка",
        "related" => "связано",
        "tunnel" => "туннель",
        "tagged" => "с тегом",
        "mentions" => "упоминает",
        "depends_on" => "зависит от",
        "derived_from" => "производное от",
        "supersedes" => "замещает",
        other => other,
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
fn draw_arrowhead(painter: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke) {
    let d = b - a;
    let len = d.length();
    if len < 24.0 {
        return;
    }
    let dir = d / len;
    let tip = b - dir * 14.0;
    let side = Vec2::new(-dir.y, dir.x);
    painter.line_segment([tip, tip - dir * 8.0 + side * 3.5], stroke);
    painter.line_segment([tip, tip - dir * 8.0 - side * 3.5], stroke);
}

/// Compact legend of kind / rel_type colors present in the view (bottom-left).
fn draw_legend(painter: &egui::Painter, rect: Rect, graph: &UiGraph) {
    use std::collections::{BTreeMap, BTreeSet};
    let mut kinds = BTreeMap::new();
    for node in &graph.nodes {
        let label = if node.kind == "document" {
            localized_layer(node.layer.as_deref().unwrap_or("document"))
        } else {
            localized_kind(&node.kind)
        };
        kinds.entry(label).or_insert(node.color);
    }
    let rels: BTreeSet<&str> = graph.edges.iter().map(|e| e.rel_type.as_str()).collect();
    let rows = kinds.len() + rels.len();
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
    for (kind, color) in kinds {
        let cy = y + line_h * 0.5;
        painter.circle_filled(Pos2::new(x + 4.0, cy), 4.0, color);
        painter.text(
            Pos2::new(x + 14.0, y),
            egui::Align2::LEFT_TOP,
            kind,
            font.clone(),
            text_color,
        );
        y += line_h;
    }
    for r in rels {
        let cy = y + line_h * 0.5;
        painter.line_segment(
            [Pos2::new(x, cy), Pos2::new(x + 10.0, cy)],
            Stroke::new(1.5_f32, edge_color(r)),
        );
        painter.text(
            Pos2::new(x + 14.0, y),
            egui::Align2::LEFT_TOP,
            localized_relation(r),
            font.clone(),
            text_color,
        );
        y += line_h;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessible_node_label_exposes_identity_and_topology() {
        let node = UiNode {
            id: "node-1".into(),
            kind: "document".into(),
            label: "Architecture".into(),
            color: Color32::WHITE,
            document_id: Some("doc-1".into()),
            uri: Some("wiki://architecture".into()),
            resolved: true,
            layer: Some("wiki".into()),
            wing: Some("alpha".into()),
            room: None,
            depth: 2,
            degree: 7,
            pinned: false,
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
}
