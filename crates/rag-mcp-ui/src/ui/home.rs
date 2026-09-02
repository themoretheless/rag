//! Project Home dashboard.

use crate::product::ProjectHome;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HomeAction {
    #[default]
    None,
    Refresh,
    OpenLibrary,
    OpenGraph,
}

pub fn draw_project_home(
    ui: &mut egui::Ui,
    selected_project: Option<&str>,
    home: Option<&ProjectHome>,
    error: Option<&str>,
    loading: bool,
) -> HomeAction {
    let mut action = HomeAction::None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("Project Home");
                ui.weak("A current inventory of one project, its knowledge layers and graph.");
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                if ui.button("Refresh").clicked() {
                    action = HomeAction::Refresh;
                }
                if loading {
                    ui.spinner();
                }
            });
        });
        ui.add_space(18.0);

        let Some(project) = selected_project.filter(|value| !value.trim().is_empty()) else {
            callout(
                ui,
                egui::Color32::from_rgb(65, 105, 150),
                "Choose a project",
                "Project Home is intentionally scoped. Select a project in the top bar to see its inventory and health.",
            );
            return;
        };

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(project).size(25.0).strong());
            ui.label(egui::RichText::new("PROJECT").small().weak());
        });

        if let Some(error) = error {
            ui.add_space(10.0);
            callout(
                ui,
                egui::Color32::from_rgb(155, 75, 65),
                "Could not load Project Home",
                error,
            );
        }

        let Some(home) = home else {
            if loading {
                ui.add_space(52.0);
                ui.vertical_centered(|ui| {
                    ui.spinner();
                    ui.weak("Loading project inventory…");
                });
            }
            return;
        };

        ui.add_space(14.0);
        ui.columns(3, |columns| {
            metric_card(
                &mut columns[0],
                "Documents",
                home.documents,
                "indexed records",
            );
            metric_card(
                &mut columns[1],
                "Chunks",
                home.chunks,
                "retrieval units",
            );
            metric_card(
                &mut columns[2],
                "Source files",
                home.source_files,
                "tracked inputs",
            );
        });
        ui.add_space(10.0);
        ui.columns(3, |columns| {
            metric_card(&mut columns[0], "Rooms", home.rooms, "project sections");
            metric_card(
                &mut columns[1],
                "Graph nodes",
                home.graph_nodes,
                "connected objects",
            );
            metric_card(
                &mut columns[2],
                "Graph edges",
                home.graph_edges,
                "relationships",
            );
        });

        ui.add_space(22.0);
        if !home.source_roots.is_empty() {
            ui.strong("Source roots & freshness");
            ui.add_space(6.0);
            egui::Frame::group(ui.style())
                .inner_margin(14.0)
                .show(ui, |ui| {
                    egui::Grid::new("project_source_roots")
                        .num_columns(3)
                        .striped(true)
                        .spacing([16.0, 7.0])
                        .show(ui, |ui| {
                            ui.strong("Root");
                            ui.strong("Files");
                            ui.strong("Last sync");
                            ui.end_row();
                            for root in home.source_roots.iter().take(8) {
                                ui.label(&root.canonical_root);
                                ui.label(root.file_count.to_string());
                                ui.weak(
                                    root.last_synced
                                        .as_deref()
                                        .or(root.last_seen.as_deref())
                                        .map(readable_timestamp)
                                        .unwrap_or_else(|| "Not synced yet".to_string()),
                                );
                                ui.end_row();
                            }
                        });
                    if home.source_roots.len() > 8 {
                        ui.weak(format!("+{} more roots", home.source_roots.len() - 8));
                    }
                });
            ui.add_space(18.0);
        }
        ui.columns(2, |columns| {
            egui::Frame::group(columns[0].style())
                .inner_margin(16.0)
                .show(&mut columns[0], |ui| {
                    ui.strong("Knowledge layers");
                    ui.add_space(10.0);
                    layer_row(ui, "Raw", home.raw_documents, home.documents);
                    layer_row(ui, "Wiki", home.wiki_documents, home.documents);
                    layer_row(ui, "Diary", home.diary_documents, home.documents);
                    layer_row(ui, "Archived", home.archived_documents, home.documents);
                });
            egui::Frame::group(columns[1].style())
                .inner_margin(16.0)
                .show(&mut columns[1], |ui| {
                    ui.strong("Graph quality");
                    ui.add_space(10.0);
                    if home.unresolved_links == 0 {
                        ui.colored_label(
                            egui::Color32::from_rgb(75, 165, 105),
                            "● All graph links resolve",
                        );
                    } else {
                        ui.colored_label(
                            egui::Color32::from_rgb(215, 145, 55),
                            format!("● {} unresolved links", home.unresolved_links),
                        );
                        ui.weak("Open Connections to inspect missing targets.");
                    }
                    ui.add_space(12.0);
                    ui.label(format!(
                        "{} nodes · {} edges",
                        compact_count(home.graph_nodes),
                        compact_count(home.graph_edges)
                    ));
                    if let Some(updated) = home.last_updated_at.as_deref() {
                        ui.add_space(8.0);
                        ui.weak(format!("Last document update: {}", readable_timestamp(updated)));
                    }
                });
        });

        ui.add_space(20.0);
        ui.horizontal(|ui| {
            if ui.button("Open unified library").clicked() {
                action = HomeAction::OpenLibrary;
            }
            if ui.button("Explore connections").clicked() {
                action = HomeAction::OpenGraph;
            }
        });
        ui.add_space(24.0);
    });
    action
}

fn metric_card(ui: &mut egui::Ui, label: &str, value: u64, detail: &str) {
    egui::Frame::group(ui.style())
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.set_min_height(86.0);
            ui.weak(label);
            ui.label(
                egui::RichText::new(compact_count(value))
                    .size(26.0)
                    .strong(),
            );
            ui.small(detail);
        });
}

fn layer_row(ui: &mut egui::Ui, label: &str, value: u64, total: u64) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.strong(value.to_string());
            let percent = if total == 0 {
                0.0
            } else {
                (value as f64 / total as f64) * 100.0
            };
            ui.weak(format!("{percent:.0}%"));
        });
    });
}

fn callout(ui: &mut egui::Ui, color: egui::Color32, title: &str, detail: &str) {
    egui::Frame::group(ui.style())
        .fill(color.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0_f32, color.gamma_multiply(0.65)))
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.strong(title);
            ui.label(detail);
        });
}

fn compact_count(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 10_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn readable_timestamp(value: &str) -> String {
    value.strip_suffix('Z').unwrap_or(value).replace('T', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_count_keeps_small_values_precise() {
        assert_eq!(compact_count(9999), "9999");
        assert_eq!(compact_count(12_450), "12.4K");
        assert_eq!(compact_count(2_100_000), "2.1M");
    }

    #[test]
    fn timestamp_is_readable_without_timezone_noise() {
        assert_eq!(
            readable_timestamp("2026-09-02T12:00:00Z"),
            "2026-09-02 12:00:00"
        );
    }
}
