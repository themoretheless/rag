//! Search workspace for lexical, vector and hybrid retrieval.

use crate::search::{SearchHit, SearchRequest, SearchResults};

use super::{closing_selectable_value, theme};

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
        include_archived: bool,
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

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.heading("Поиск");
            ui.label(
                egui::RichText::new(
                    "Лаборатория извлечения: режим, происхождение и составляющие ранжирования видны рядом с каждым хитом.",
                )
                .color(theme::MUTED),
            );
        });
    });
    ui.add_space(12.0);

    theme::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            for (value, label) in [("lex", "lex"), ("vec", "vec"), ("hybrid", "hybrid")] {
                if ui.selectable_label(request.mode == value, label).clicked() {
                    request.mode = value.to_string();
                }
            }
            ui.separator();
            let field_width = (ui.available_width() - 132.0).max(220.0);
            let response = ui.add_sized(
                [field_width, 34.0],
                egui::TextEdit::singleline(&mut request.query)
                    .id(egui::Id::new("native_search_query"))
                    .hint_text("Спросите корпус — здесь видны ранги и причины…"),
            );
            let enter =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if ui
                .add_enabled(
                    !request.query.trim().is_empty() && !loading,
                    egui::Button::new("Выполнить"),
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
            ui.label(
                egui::RichText::new("ФИЛЬТРЫ")
                    .monospace()
                    .small()
                    .color(theme::FAINT),
            );
            let mut room = request.room.clone().unwrap_or_default();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut room)
                        .hint_text("комната · все")
                        .desired_width(110.0),
                )
                .changed()
            {
                request.room = clean(room);
            }
            let selected_layer = request.layer.as_deref().unwrap_or("слой · все");
            egui::ComboBox::from_id_salt("search_layer")
                .selected_text(selected_layer)
                .width(100.0)
                .show_ui(ui, |ui| {
                    closing_selectable_value(ui, &mut request.layer, None, "слой · все");
                    for layer in ["raw", "wiki", "diary"] {
                        closing_selectable_value(
                            ui,
                            &mut request.layer,
                            Some(layer.to_string()),
                            layer,
                        );
                    }
                });
            ui.weak("top_k");
            ui.add(egui::DragValue::new(&mut request.top_k).range(1..=100));
            ui.weak("timeout");
            ui.add(egui::DragValue::new(&mut request.timeout_ms).range(100..=60_000));
            ui.checkbox(&mut request.include_archived, "архив");
            if ui.button("Сбросить").clicked() {
                action = SearchAction::Clear;
            }
        });
    });

    if let Some(error) = error {
        ui.add_space(8.0);
        theme::inset()
            .fill(theme::rgba(theme::DANGER, 15))
            .stroke(egui::Stroke::new(1.0, theme::rgba(theme::DANGER, 90)))
            .show(ui, |ui| {
                ui.colored_label(theme::DANGER, format!("Поиск завершился ошибкой: {error}"));
            });
    }

    ui.add_space(10.0);
    let Some(results) = results else {
        if !loading && error.is_none() {
            theme::inset().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(28.0);
                    ui.strong("Введите запрос");
                    ui.label(
                        egui::RichText::new(
                            "Hybrid — хороший режим по умолчанию: он объединяет точные термины и смысл.",
                        )
                        .color(theme::MUTED),
                    );
                    ui.add_space(28.0);
                });
            });
        }
        return action;
    };

    let controls_dirty = request != &results.request;
    ui.horizontal(|ui| {
        ui.strong("Хиты");
        badge(ui, &results.items.len().to_string(), theme::L1);
        badge(ui, &results.request.mode, theme::L1);
        if let Some(project) = results.request.wing.as_deref() {
            badge(ui, project, theme::L0);
        }
        if controls_dirty {
            badge(ui, "параметры изменены", theme::WARN);
        }
    });
    ui.add_space(6.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if results.items.is_empty() {
                theme::card().show(ui, |ui| {
                    ui.strong("Совпадений нет");
                    ui.label("Расширьте формулировку, включите Hybrid или снимите фильтр слоя.");
                });
            }
            for (index, hit) in results.items.iter().enumerate() {
                result_card(ui, index, hit, results, &mut action);
                ui.add_space(8.0);
            }
            ui.add_space(16.0);
        });
    action
}

fn result_card(
    ui: &mut egui::Ui,
    index: usize,
    hit: &SearchHit,
    results: &SearchResults,
    action: &mut SearchAction,
) {
    theme::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(theme::rgba(theme::L1, 22))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(7, 4))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!("#{:02}", index + 1))
                            .monospace()
                            .strong()
                            .color(theme::L1),
                    );
                });
            ui.vertical(|ui| {
                ui.strong(if hit.document_title.trim().is_empty() {
                    &hit.document_uri
                } else {
                    &hit.document_title
                });
                let location = hit
                    .heading_path
                    .as_ref()
                    .filter(|path| !path.is_empty())
                    .map(|path| path.join(" › "))
                    .or_else(|| hit.section.clone())
                    .unwrap_or_else(|| hit.document_uri.clone());
                ui.label(egui::RichText::new(location).small().color(theme::FAINT));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                score_chip(ui, "score", Some(hit.score), theme::L1);
                score_chip(ui, "rrf", hit.score_rrf, theme::L1);
                score_chip(ui, "vec", hit.score_vec, theme::L3);
                score_chip(ui, "lex", hit.score_lex, theme::L0);
            });
        });
        ui.add_space(8.0);
        ui.label(trim_excerpt(hit.visible_excerpt(), 620));
        ui.add_space(9.0);
        ui.horizontal(|ui| {
            if ui.button("Открыть в корпусе").clicked() {
                *action = SearchAction::OpenLibrary {
                    document_id: hit.document_id.clone(),
                    title: hit.document_title.clone(),
                    uri: hit.document_uri.clone(),
                    include_archived: results.request.include_archived,
                };
            }
            if ui.button("Показать связи").clicked() {
                *action = SearchAction::OpenGraph {
                    document_id: hit.document_id.clone(),
                    title: hit.document_title.clone(),
                };
            }
            ui.label(
                egui::RichText::new(format!("chunk {}", hit.chunk_index + 1))
                    .monospace()
                    .small()
                    .color(theme::FAINT),
            );
        });
    });
}

fn badge(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(theme::rgba(color, 24))
        .stroke(egui::Stroke::new(1.0, theme::rgba(color, 70)))
        .corner_radius(5.0)
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).monospace().small().color(color));
        });
}

fn score_chip(ui: &mut egui::Ui, label: &str, value: Option<f32>, color: egui::Color32) {
    if let Some(value) = value {
        badge(ui, &format!("{label} {value:.3}"), color);
    }
}

fn clean(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
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
    fn result_provenance_is_independent_from_editable_controls() {
        let applied = SearchRequest {
            query: "needle".to_string(),
            mode: "lex".to_string(),
            include_archived: true,
            ..SearchRequest::default()
        };
        let mut draft = applied.clone();
        draft.mode = "vec".to_string();
        assert_ne!(draft, applied);
        assert_eq!(applied.mode, "lex");
        assert!(applied.include_archived);
    }
}
