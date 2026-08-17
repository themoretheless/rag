//! Detail side panel for the selected node (read-only).
//!
//! Shows topology fields from `UiNode` / collapsed `UiEdge` members:
//! id, kind, label, document_id, uri, plus incident edges with provenance.
//! Optional document body (wiki/raw) from HTTP `/v1/document` or exclusive DB.

use egui::Ui;

use crate::adapter::{UiEdge, UiGraph, UiNode};
use crate::load::DocumentBody;

/// User action from the detail panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailAction {
    #[default]
    None,
    /// Load full document text for the selected node.
    ReadContent,
    /// Hide the content pane.
    CloseContent,
}

/// Draw the right-hand detail panel for `selected_id` in the current view.
///
/// `body` — loaded wiki/raw content when available; `body_error` — last load failure.
pub fn draw_detail(
    ui: &mut Ui,
    graph: &UiGraph,
    selected_id: &str,
    body: Option<&DocumentBody>,
    body_error: Option<&str>,
) -> DetailAction {
    let mut action = DetailAction::None;
    let Some(node) = graph.nodes.iter().find(|n| n.id == selected_id) else {
        ui.label("Selection not in current view.");
        return action;
    };

    ui.heading(&node.label);
    ui.separator();

    draw_node_fields(ui, node);

    ui.separator();
    ui.horizontal(|ui| {
        let can_read = node.document_id.is_some() || node.uri.is_some();
        if ui
            .add_enabled(can_read, egui::Button::new("Read content"))
            .on_hover_text("Load full wiki/raw text via HTTP /v1/document or --db")
            .clicked()
        {
            action = DetailAction::ReadContent;
        }
        if body.is_some() && ui.button("Close content").clicked() {
            action = DetailAction::CloseContent;
        }
        if !can_read {
            ui.weak("no document_id/uri (stub/tag)");
        }
    });

    if let Some(err) = body_error {
        ui.colored_label(egui::Color32::from_rgb(220, 100, 100), err);
    }

    if let Some(doc) = body {
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong(&doc.title);
            ui.weak(format!("{} · {}", doc.layer, doc.kind));
        });
        ui.monospace(&doc.uri);
        ui.weak(format!("id {}", doc.id));
        if let Some(h) = doc.content_hash.as_deref() {
            ui.weak(format!("hash {}", &h[..h.len().min(12)]));
        }
        if let Some(ts) = doc.updated_at.as_deref() {
            ui.weak(ts);
        }
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("doc_content_scroll")
            .max_height(420.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(&doc.content).monospace())
                        .wrap()
                        .selectable(true),
                );
            });
    }

    ui.separator();
    draw_incident_edges(ui, graph, selected_id);

    ui.separator();
    draw_copy_actions(ui, node);

    action
}

fn draw_node_fields(ui: &mut Ui, node: &UiNode) {
    egui::Grid::new("node_detail")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label("id");
            ui.monospace(&node.id);
            ui.end_row();

            ui.label("kind");
            ui.label(&node.kind);
            ui.end_row();

            ui.label("label");
            ui.label(&node.label);
            ui.end_row();

            ui.label("document_id");
            ui.monospace(node.document_id.as_deref().unwrap_or("-"));
            ui.end_row();

            ui.label("uri");
            ui.monospace(node.uri.as_deref().unwrap_or("-"));
            ui.end_row();

            ui.label("resolved");
            ui.label(node.resolved.to_string());
            ui.end_row();

            ui.label("depth");
            ui.label(node.depth.to_string());
            ui.end_row();

            ui.label("degree");
            ui.label(node.degree.to_string());
            ui.end_row();
        });
}

fn draw_incident_edges(ui: &mut Ui, graph: &UiGraph, selected_id: &str) {
    let mut incident: Vec<&UiEdge> = graph
        .edges
        .iter()
        .filter(|e| e.source_id == selected_id || e.target_id == selected_id)
        .collect();
    // Stable order: outs first, then ins; then by other id / rel_type.
    incident.sort_by(|a, b| {
        let a_out = a.source_id == selected_id;
        let b_out = b.source_id == selected_id;
        b_out
            .cmp(&a_out)
            .then_with(|| {
                let ao = if a_out {
                    a.target_id.as_str()
                } else {
                    a.source_id.as_str()
                };
                let bo = if b_out {
                    b.target_id.as_str()
                } else {
                    b.source_id.as_str()
                };
                ao.cmp(bo)
            })
            .then_with(|| a.rel_type.cmp(&b.rel_type))
            .then_with(|| a.id.cmp(&b.id))
    });

    ui.horizontal(|ui| {
        ui.strong("Incident edges");
        ui.weak(format!("({})", incident.len()));
    });
    if incident.is_empty() {
        ui.weak("No edges in current view.");
        return;
    }

    egui::ScrollArea::vertical()
        .max_height(320.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for e in incident {
                let outgoing = e.source_id == selected_id;
                let other_id = if outgoing {
                    e.target_id.as_str()
                } else {
                    e.source_id.as_str()
                };
                let other_label = graph
                    .nodes
                    .iter()
                    .find(|n| n.id == other_id)
                    .map(|n| n.label.as_str())
                    .unwrap_or(other_id);
                let dir = if outgoing { "out" } else { "in" };

                ui.group(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(dir);
                        ui.label(format!("→ {other_label}"));
                        ui.weak(format!("[{}]", e.rel_type));
                        ui.monospace(format!("w={:.2}", e.weight));
                        if e.multi_count > 1 {
                            ui.strong(format!("×{}", e.multi_count));
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.weak("other id");
                        ui.monospace(other_id);
                    });

                    // Full provenance from collapsed members (detail only, not canvas).
                    for m in &e.members {
                        ui.horizontal_wrapped(|ui| {
                            ui.weak("·");
                            ui.monospace(&m.id);
                            ui.monospace(format!("w={:.2}", m.weight));
                            if let Some(ctx) = &m.context {
                                ui.weak(truncate(ctx, 120));
                            }
                        });
                    }
                });
            }
        });
}

fn draw_copy_actions(ui: &mut Ui, node: &UiNode) {
    ui.horizontal(|ui| {
        if ui.button("Copy id").clicked() {
            ui.ctx().copy_text(node.id.clone());
        }
        if let Some(doc) = &node.document_id {
            if ui.button("Copy document_id").clicked() {
                ui.ctx().copy_text(doc.clone());
            }
        }
        if let Some(uri) = &node.uri {
            if ui.button("Copy uri").clicked() {
                ui.ctx().copy_text(uri.clone());
            }
        }
    });
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}
