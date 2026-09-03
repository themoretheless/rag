//! Detail side panel for the selected node (read-only).
//!
//! Shows topology fields from `UiNode` / collapsed `UiEdge` members:
//! id, kind, label, document_id, uri, plus incident edges with provenance.
//! Optional document body (wiki/raw) from HTTP `/v1/document` or exclusive DB.

use egui::Ui;

use crate::adapter::{UiEdge, UiGraph, UiNode};
use crate::load::DocumentBody;
use crate::ui::document::draw_document_reader;
use crate::ui::theme;

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
/// `body` — loaded wiki/raw content when available; `body_error` — last load failure;
/// `body_loading` — a Read content request is on the wire (previous body stays visible).
pub fn draw_detail(
    ui: &mut Ui,
    graph: &UiGraph,
    selected_id: &str,
    body: Option<&DocumentBody>,
    body_error: Option<&str>,
    body_loading: bool,
    content_available: bool,
) -> DetailAction {
    let mut action = DetailAction::None;
    let Some(node) = graph.nodes.iter().find(|n| n.id == selected_id) else {
        ui.label("Выбранного узла нет в текущем представлении.");
        return action;
    };

    ui.heading(&node.label);
    ui.separator();

    draw_node_fields(ui, node);

    ui.separator();
    ui.horizontal(|ui| {
        let has_document = node.document_id.is_some() || node.uri.is_some();
        let can_read = content_available && has_document;
        let read = ui
            .add_enabled(can_read, egui::Button::new("Читать документ"))
            .on_hover_text(if content_available {
                "Загрузить полный wiki/raw-текст через HTTP /v1/document или --db"
            } else {
                "Snapshot содержит только топологию; содержимое требует HTTP или DB"
            });
        if read.clicked() {
            action = DetailAction::ReadContent;
        }
        if body.is_some() && ui.button("Закрыть текст").clicked() {
            action = DetailAction::CloseContent;
        }
        if !content_available {
            ui.weak("только топология");
        } else if !has_document {
            ui.weak("нет document_id/uri (stub/tag)");
        }
        if body_loading {
            ui.spinner();
            ui.weak("загрузка…");
        }
    });

    if let Some(err) = body_error {
        ui.colored_label(theme::DANGER, err);
    }

    if let Some(doc) = body {
        ui.separator();
        theme::inset().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(&doc.title);
                ui.label(
                    egui::RichText::new(format!("{} · {}", doc.layer, doc.kind))
                        .monospace()
                        .small()
                        .color(theme::layer_color(&doc.layer)),
                );
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&doc.uri)
                        .monospace()
                        .color(theme::MUTED),
                )
                .selectable(true)
                .wrap(),
            );
            ui.weak(format!("id {}", doc.id));
            if let Some(h) = doc.content_hash.as_deref() {
                ui.weak(format!("hash {}", &h[..h.len().min(12)]));
            }
            if let Some(ts) = doc.updated_at.as_deref() {
                ui.weak(ts);
            }
        });
        ui.add_space(7.0);
        egui::ScrollArea::vertical()
            .id_salt("doc_content_scroll")
            .max_height(420.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                draw_document_reader(ui, &doc.content, &doc.kind, &doc.uri);
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

            ui.label("тип");
            ui.label(&node.kind);
            ui.end_row();

            ui.label("название");
            ui.label(&node.label);
            ui.end_row();

            ui.label("document_id");
            ui.monospace(node.document_id.as_deref().unwrap_or("-"));
            ui.end_row();

            ui.label("uri");
            ui.monospace(node.uri.as_deref().unwrap_or("-"));
            ui.end_row();

            ui.label("разрешён");
            ui.label(if node.resolved { "да" } else { "нет" });
            ui.end_row();

            ui.label("глубина");
            ui.label(node.depth.to_string());
            ui.end_row();

            ui.label("степень");
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
        ui.strong("Связи узла");
        ui.weak(format!("({})", incident.len()));
    });
    if incident.is_empty() {
        ui.weak("В текущем представлении связей нет.");
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
                let dir = if outgoing { "исх." } else { "вх." };

                ui.group(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(dir);
                        ui.label(format!("-> {other_label}"));
                        ui.weak(format!("[{}]", e.rel_type));
                        ui.monospace(format!("w={:.2}", e.weight));
                        if e.multi_count > 1 {
                            ui.strong(format!("x{}", e.multi_count));
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.weak("id соседа");
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
        if ui.button("Копировать id").clicked() {
            ui.ctx().copy_text(node.id.clone());
        }
        if let Some(doc) = &node.document_id {
            if ui.button("Копировать document_id").clicked() {
                ui.ctx().copy_text(doc.clone());
            }
        }
        if let Some(uri) = &node.uri {
            if ui.button("Копировать uri").clicked() {
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
