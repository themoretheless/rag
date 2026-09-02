//! Search workspace for lexical, vector and hybrid retrieval.

use crate::search::{SearchRequest, SearchResults};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SearchAction {
    #[default]
    None,
    Run,
    Clear,
    OpenLibrary {
        document_id: String,
        title: String,
        uri: String,
    },
    OpenGraph {
        document_id: String,
        title: String,
    },
}

pub fn draw_search_workspace(
    ui: &mut egui::Ui,
    request: &mut SearchRequest,
    results: Option<&SearchResults>,
    error: Option<&str>,
    loading: bool,
) -> SearchAction {
    let mut action = SearchAction::None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.heading("Search");
            ui.weak("Find evidence across raw sources, wiki pages and diary entries.");
            ui.add_space(14.0);
            egui::Frame::group(ui.style())
                .inner_margin(14.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let response = ui.add_sized(
                            [ui.available_width().max(240.0) - 130.0, 34.0],
                            egui::TextEdit::singleline(&mut request.query)
                                .hint_text("What do you want to find?"),
                        );
                        let enter = response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if ui
                            .add_enabled(
                                !request.query.trim().is_empty() && !loading,
                                egui::Button::new("Search"),
                            )
                            .clicked()
                            || enter
                        {
                            action = SearchAction::Run;
                        }
                        if loading {
                            ui.spinner();
                        }
                    });
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.weak("Mode");
                        egui::ComboBox::from_id_salt("search_mode")
                            .selected_text(mode_label(&request.mode))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut request.mode,
                                    "hybrid".to_string(),
                                    "Hybrid",
                                );
                                ui.selectable_value(
                                    &mut request.mode,
                                    "lex".to_string(),
                                    "Lexical",
                                );
                                ui.selectable_value(
                                    &mut request.mode,
                                    "vec".to_string(),
                                    "Semantic",
                                );
                            });
                        ui.weak("Room");
                        let mut room = request.room.clone().unwrap_or_default();
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut room)
                                    .hint_text("Any")
                                    .desired_width(90.0),
                            )
                            .changed()
                        {
                            request.room = clean(room);
                        }
                        ui.weak("Layer");
                        let selected_layer = request.layer.as_deref().unwrap_or("Any");
                        egui::ComboBox::from_id_salt("search_layer")
                            .selected_text(selected_layer)
                            .width(88.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut request.layer, None, "Any");
                                for layer in ["raw", "wiki", "diary"] {
                                    ui.selectable_value(
                                        &mut request.layer,
                                        Some(layer.to_string()),
                                        layer,
                                    );
                                }
                            });
                        ui.weak("Results");
                        ui.add(egui::DragValue::new(&mut request.top_k).range(1..=100));
                        ui.checkbox(&mut request.include_archived, "Archived");
                        if ui.button("Clear").clicked() {
                            action = SearchAction::Clear;
                        }
                    });
                });

            if let Some(error) = error {
                ui.add_space(10.0);
                ui.colored_label(
                    egui::Color32::from_rgb(215, 100, 85),
                    format!("Search failed: {error}"),
                );
            }

            let Some(results) = results else {
                if !loading && error.is_none() {
                    ui.add_space(44.0);
                    ui.vertical_centered(|ui| {
                        ui.strong("Search your knowledge base");
                        ui.weak("Hybrid is a good default: it combines exact terms and meaning.");
                    });
                }
                return;
            };

            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.strong(format!("{} results", results.items.len()));
                if let Some(project) = request.wing.as_deref() {
                    ui.separator();
                    ui.weak(format!("Project: {project}"));
                }
                ui.separator();
                ui.weak(mode_label(&request.mode));
            });
            ui.add_space(6.0);

            if results.items.is_empty() {
                egui::Frame::group(ui.style())
                    .inner_margin(18.0)
                    .show(ui, |ui| {
                        ui.strong("No matching evidence");
                        ui.label("Try a broader phrase, Hybrid mode, or clear the layer filter.");
                    });
            }
            for hit in &results.items {
                egui::Frame::group(ui.style())
                    .inner_margin(14.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.strong(if hit.document_title.trim().is_empty() {
                                &hit.document_uri
                            } else {
                                &hit.document_title
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.monospace(score_label(hit.score));
                                },
                            );
                        });
                        let location = hit
                            .heading_path
                            .as_ref()
                            .filter(|path| !path.is_empty())
                            .map(|path| path.join(" › "))
                            .or_else(|| hit.section.clone());
                        if let Some(location) = location {
                            ui.weak(location);
                        }
                        ui.add_space(7.0);
                        ui.label(trim_excerpt(hit.visible_excerpt(), 520));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Open in library").clicked() {
                                action = SearchAction::OpenLibrary {
                                    document_id: hit.document_id.clone(),
                                    title: hit.document_title.clone(),
                                    uri: hit.document_uri.clone(),
                                };
                            }
                            if ui.button("Connections").clicked() {
                                action = SearchAction::OpenGraph {
                                    document_id: hit.document_id.clone(),
                                    title: hit.document_title.clone(),
                                };
                            }
                            ui.weak(format!("chunk {}", hit.chunk_index + 1));
                        });
                    });
                ui.add_space(8.0);
            }
            ui.add_space(20.0);
        });
    action
}

fn clean(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn mode_label(mode: &str) -> &'static str {
    match mode {
        "lex" => "Lexical",
        "vec" => "Semantic",
        _ => "Hybrid",
    }
}

fn score_label(score: f32) -> String {
    format!("score {score:.3}")
}

fn trim_excerpt(value: &str, max_chars: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut output: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    output.push('…');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_trimming_is_unicode_safe() {
        assert_eq!(trim_excerpt("привет мир", 7), "привет…");
    }

    #[test]
    fn search_modes_have_user_facing_names() {
        assert_eq!(mode_label("hybrid"), "Hybrid");
        assert_eq!(mode_label("lex"), "Lexical");
        assert_eq!(mode_label("vec"), "Semantic");
    }

    #[test]
    fn score_is_stable_and_compact() {
        assert_eq!(score_label(0.812_49), "score 0.812");
    }
}
