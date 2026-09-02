//! Unified Library workspace: lean server-side catalog with deliberate filters.

use crate::load::DocumentBody;
use crate::product::{LibraryItem, LibraryPage, LibraryRequest};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LibraryAction {
    #[default]
    None,
    ApplyFilters,
    ResetFilters,
    Refresh,
    PreviousPage,
    NextPage(String),
    Select(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LibraryDetailAction {
    #[default]
    None,
    Close,
    Retry,
    History(String),
    OpenWiki(String),
    OpenGraph(String),
}

pub fn draw_library_workspace(
    ui: &mut egui::Ui,
    request: &mut LibraryRequest,
    page: Option<&LibraryPage>,
    selected_id: Option<&str>,
    error: Option<&str>,
    loading: bool,
    has_previous_page: bool,
) -> LibraryAction {
    let mut action = LibraryAction::None;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.heading("Unified Library");
            ui.weak("Every indexed document, without loading document bodies.");
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            if ui.button("Refresh").clicked() {
                action = LibraryAction::Refresh;
            }
            if loading {
                ui.spinner();
            }
        });
    });
    ui.add_space(12.0);

    egui::Frame::group(ui.style())
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let search = ui.add_sized(
                    [260.0, 30.0],
                    egui::TextEdit::singleline(&mut request.q)
                        .hint_text("Title, URI or source file…"),
                );
                if search.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    action = LibraryAction::ApplyFilters;
                }
                ui.add(
                    egui::TextEdit::singleline(&mut request.room)
                        .hint_text("Room")
                        .desired_width(100.0),
                );
                filter_combo(ui, "library_layer", "Layer", &mut request.layer, LAYERS);
                filter_combo(ui, "library_kind", "Kind", &mut request.kind, KINDS);
                filter_combo(
                    ui,
                    "library_status",
                    "Status",
                    &mut request.status,
                    STATUSES,
                );
                ui.checkbox(&mut request.include_archived, "Include archived");
                if ui.button("Apply").clicked() {
                    action = LibraryAction::ApplyFilters;
                }
                if ui.button("Clear").clicked() {
                    action = LibraryAction::ResetFilters;
                }
            });
        });

    ui.add_space(10.0);
    if let Some(error) = error {
        ui.colored_label(
            egui::Color32::from_rgb(215, 100, 85),
            format!("Could not load the library: {error}"),
        );
        ui.add_space(8.0);
    }

    let Some(page) = page else {
        if loading {
            ui.add_space(50.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.weak("Loading catalog…");
            });
        } else if error.is_none() {
            ui.weak("The catalog has not been loaded yet.");
        }
        return action;
    };

    ui.horizontal(|ui| {
        ui.strong(format!("{} documents", page.total));
        let scope = request.wing.trim();
        if !scope.is_empty() {
            ui.separator();
            ui.weak(format!("Project: {scope}"));
        }
        let active_filters = active_filter_count(request);
        if active_filters > 0 {
            ui.separator();
            ui.weak(format!("{active_filters} active filters"));
        }
    });
    ui.add_space(5.0);

    if page.items.is_empty() {
        egui::Frame::group(ui.style())
            .inner_margin(18.0)
            .show(ui, |ui| {
                ui.strong("No matching documents");
                ui.label("Try clearing a filter or switching the project scope.");
            });
    } else {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("unified_library_rows")
                    .num_columns(5)
                    .striped(true)
                    .min_col_width(92.0)
                    .spacing([14.0, 9.0])
                    .show(ui, |ui| {
                        ui.strong("Document");
                        ui.strong("Scope");
                        ui.strong("Layer");
                        ui.strong("Kind");
                        ui.strong("Updated");
                        ui.end_row();
                        for item in &page.items {
                            let selected = selected_id == Some(item.id.as_str());
                            let mut title = item.title.trim();
                            if title.is_empty() {
                                title = item.uri.as_str();
                            }
                            let title = if item.pinned {
                                format!("◆ {title}")
                            } else {
                                title.to_string()
                            };
                            if ui.selectable_label(selected, title).clicked() {
                                action = LibraryAction::Select(item.id.clone());
                            }
                            ui.weak(item.scope_label());
                            badge(ui, &item.layer, layer_color(&item.layer));
                            ui.label(&item.kind);
                            ui.weak(short_timestamp(&item.updated_at));
                            ui.end_row();
                        }
                    });
            });
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                has_previous_page && !loading,
                egui::Button::new("← Previous"),
            )
            .clicked()
        {
            action = LibraryAction::PreviousPage;
        }
        let has_next = page.next_cursor.is_some();
        if ui
            .add_enabled(has_next && !loading, egui::Button::new("Next →"))
            .clicked()
        {
            if let Some(cursor) = &page.next_cursor {
                action = LibraryAction::NextPage(cursor.clone());
            }
        }
        ui.separator();
        ui.weak(format!("Showing {} on this page", page.items.len()));
    });
    action
}

pub fn draw_library_detail(
    ui: &mut egui::Ui,
    item: &LibraryItem,
    body: Option<&DocumentBody>,
    error: Option<&str>,
    loading: bool,
) -> LibraryDetailAction {
    let mut action = LibraryDetailAction::None;
    ui.horizontal(|ui| {
        ui.strong("Document preview");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Close").clicked() {
                action = LibraryDetailAction::Close;
            }
        });
    });
    ui.separator();
    ui.heading(if item.title.trim().is_empty() {
        &item.uri
    } else {
        &item.title
    });
    ui.horizontal_wrapped(|ui| {
        badge(ui, &item.layer, layer_color(&item.layer));
        badge(ui, &item.kind, egui::Color32::from_rgb(100, 120, 155));
        if item.status != "active" {
            badge(ui, &item.status, egui::Color32::from_rgb(190, 125, 65));
        }
        if item.pinned {
            ui.strong("Pinned");
        }
    });
    ui.add_space(8.0);
    metadata_row(ui, "Scope", &item.scope_label());
    metadata_row(ui, "URI", &item.uri);
    if let Some(source_file) = item.source_file.as_deref() {
        metadata_row(ui, "Source", source_file);
    }
    metadata_row(ui, "Updated", &short_timestamp(&item.updated_at));
    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        if body.is_some() && ui.button("History").clicked() {
            action = LibraryDetailAction::History(item.id.clone());
        }
        if item.is_wiki() && ui.button("Open in Wiki").clicked() {
            action = LibraryDetailAction::OpenWiki(item.id.clone());
        }
        if ui.button("Show connections").clicked() {
            action = LibraryDetailAction::OpenGraph(item.id.clone());
        }
    });
    ui.separator();

    if loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("Loading full body…");
        });
    }
    if let Some(error) = error {
        ui.colored_label(egui::Color32::from_rgb(215, 100, 85), error);
        if ui.button("Retry preview").clicked() {
            action = LibraryDetailAction::Retry;
        }
    }
    if let Some(body) = body {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(egui::RichText::new(&body.content).monospace());
        });
    }
    action
}

const LAYERS: &[(&str, &str)] = &[("raw", "Raw"), ("wiki", "Wiki"), ("diary", "Diary")];
const KINDS: &[(&str, &str)] = &[
    ("document", "Document"),
    ("wiki", "Wiki"),
    ("diary", "Diary"),
    ("code", "Code"),
];
const STATUSES: &[(&str, &str)] = &[
    ("active", "Active"),
    ("archived", "Archived"),
    ("tombstone", "Tombstone"),
];

fn filter_combo(
    ui: &mut egui::Ui,
    id: &str,
    empty_label: &str,
    value: &mut String,
    choices: &[(&str, &str)],
) {
    let selected = choices
        .iter()
        .find(|(key, _)| *key == value)
        .map(|(_, label)| *label)
        .unwrap_or(empty_label);
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width(94.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, String::new(), empty_label);
            for (key, label) in choices {
                ui.selectable_value(value, (*key).to_string(), *label);
            }
        });
}

fn badge(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .stroke(egui::Stroke::new(1.0_f32, color.gamma_multiply(0.55)))
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).small().color(color));
        });
}

fn metadata_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.weak(format!("{label}:"));
        ui.label(value);
    });
}

fn layer_color(layer: &str) -> egui::Color32 {
    match layer {
        "wiki" => egui::Color32::from_rgb(125, 105, 205),
        "diary" => egui::Color32::from_rgb(70, 155, 145),
        "raw" => egui::Color32::from_rgb(80, 130, 190),
        _ => egui::Color32::from_rgb(120, 125, 135),
    }
}

fn short_timestamp(value: &str) -> String {
    value.strip_suffix('Z').unwrap_or(value).replace('T', " ")
}

fn active_filter_count(request: &LibraryRequest) -> usize {
    [
        request.q.as_str(),
        request.room.as_str(),
        request.layer.as_str(),
        request.kind.as_str(),
        request.status.as_str(),
    ]
    .into_iter()
    .filter(|value| !value.trim().is_empty())
    .count()
        + usize::from(request.include_archived)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_scope_is_not_counted_as_a_user_filter() {
        let request = LibraryRequest {
            wing: "alpha".to_string(),
            q: "design".to_string(),
            layer: "wiki".to_string(),
            include_archived: true,
            ..LibraryRequest::default()
        };
        assert_eq!(active_filter_count(&request), 3);
    }

    #[test]
    fn timestamps_are_compact_for_table_rows() {
        assert_eq!(
            short_timestamp("2026-09-02T12:34:56Z"),
            "2026-09-02 12:34:56"
        );
    }

    #[test]
    fn unknown_layer_gets_neutral_color() {
        assert_eq!(
            layer_color("custom"),
            egui::Color32::from_rgb(120, 125, 135)
        );
    }
}
